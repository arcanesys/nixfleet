//! Long-running TLS server (Phase 3 PR-1).
//!
//! axum router + axum-server TLS listener + internal `tokio::time::
//! interval(30s)` reconcile loop. PR-1 ships exactly one real
//! endpoint (`GET /healthz`); subsequent PRs layer mTLS (PR-2),
//! `/v1/whoami` (PR-2), `/v1/agent/checkin` + `/v1/agent/report`
//! (PR-3), `/v1/enroll` + `/v1/agent/renew` (PR-5). The `tick`
//! function reused here is the same one the `tick` subcommand
//! invokes — verify-and-reconcile lives in one place across both
//! entry points.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::{
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use nixfleet_proto::agent_wire::{
    CheckinRequest, CheckinResponse, ReportRequest, ReportResponse,
};
use nixfleet_proto::enroll_wire::{EnrollRequest, EnrollResponse, RenewRequest, RenewResponse};
use rcgen::PublicKeyData;
use std::collections::HashSet;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::auth_cn::{MtlsAcceptor, PeerCertificates};
use crate::{render_plan, tick, TickInputs};

/// Per-host event ring buffer cap. Phase 3's `/v1/agent/report` is
/// in-memory only — Phase 4 adds SQLite persistence. 32 entries is
/// enough to spot a flapping host without unbounded memory growth.
const REPORT_RING_CAP: usize = 32;

/// Returned to the agent in CheckinResponse. Phase 3 never dispatches
/// rollouts (Phase 4 introduces that), so the agent is told to come
/// back in 60s with the next regular checkin.
const NEXT_CHECKIN_SECS: u32 = 60;

/// Reconcile loop cadence — D2 default. Operator-visible drift (host
/// failed to check in) shows up in the journal within one cycle;
/// slow enough not to spam.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// Inputs the `serve` subcommand receives from clap.
#[derive(Debug, Clone)]
pub struct ServeArgs {
    pub listen: SocketAddr,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub client_ca: Option<PathBuf>,
    /// Fleet CA cert path — used by issuance to read the CA cert
    /// for chaining new agent certs. Often the same path as
    /// `client_ca`. PR-5 onwards.
    pub fleet_ca_cert: Option<PathBuf>,
    /// Fleet CA private key path — issuance signs new agent certs
    /// with this. **Online on the CP per the deferred-trust-hardening
    /// design (issue #41).**
    pub fleet_ca_key: Option<PathBuf>,
    /// Path to the audit log JSON-lines file.
    pub audit_log_path: Option<PathBuf>,
    pub artifact_path: PathBuf,
    pub signature_path: PathBuf,
    pub trust_path: PathBuf,
    /// Phase 2/early-PR-1 fallback path. PR-4 prefers the live
    /// projection from check-ins; this path is used only when no
    /// agents have checked in yet AND `forgejo` is None (offline
    /// dev/test mode).
    pub observed_path: PathBuf,
    pub freshness_window: Duration,
    /// PR-4: Forgejo poll config. When `None`, the CP falls back to
    /// reading `--observed` for the channel-refs portion of Observed.
    pub forgejo: Option<crate::forgejo_poll::ForgejoConfig>,
}

/// In-memory record of the most recent checkin per host. Phase 4
/// promotes this to the source-of-truth for the projection that
/// feeds the reconcile loop (PR-4). For PR-3 it's just observability
/// state — operator can grep journal or, eventually, query an admin
/// endpoint.
#[derive(Debug, Clone)]
pub struct HostCheckinRecord {
    pub last_checkin: DateTime<Utc>,
    pub checkin: CheckinRequest,
}

/// In-memory record of an event report. Bounded ring buffer per
/// host (cap = `REPORT_RING_CAP`). Phase 4 adds SQLite persistence
/// + correlation with rollouts.
#[derive(Debug, Clone)]
pub struct ReportRecord {
    pub event_id: String,
    pub received_at: DateTime<Utc>,
    pub report: ReportRequest,
}

/// Server-wide shared state. PR-3 adds `host_checkins` and
/// `host_reports`. PR-4 adds `channel_refs_cache`. PR-5 adds
/// `seen_token_nonces` (in-memory replay set for /v1/enroll;
/// Phase 4 promotes to SQLite) and `issuance_paths` (CA cert + key
/// + audit-log paths read on each issuance).
#[derive(Debug, Default)]
pub struct AppState {
    pub last_tick_at: RwLock<Option<DateTime<Utc>>>,
    pub host_checkins: RwLock<HashMap<String, HostCheckinRecord>>,
    pub host_reports: RwLock<HashMap<String, VecDeque<ReportRecord>>>,
    pub channel_refs_cache: RwLock<crate::forgejo_poll::ChannelRefsCache>,
    pub seen_token_nonces: RwLock<HashSet<String>>,
    pub issuance_paths: RwLock<IssuancePaths>,
}

#[derive(Debug, Clone, Default)]
pub struct IssuancePaths {
    pub fleet_ca_cert: Option<PathBuf>,
    pub fleet_ca_key: Option<PathBuf>,
    pub audit_log: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct HealthzResponse {
    ok: bool,
    version: &'static str,
    /// rfc3339-formatted UTC timestamp, or `null` if the reconcile
    /// loop has not yet ticked once. (Realistic only for the first
    /// ~30s after boot.)
    last_tick_at: Option<String>,
}

async fn healthz(state: axum::extract::State<Arc<AppState>>) -> Json<HealthzResponse> {
    let last = *state.last_tick_at.read().await;
    Json(HealthzResponse {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        last_tick_at: last.map(|t| t.to_rfc3339()),
    })
}

#[derive(Debug, Serialize)]
struct WhoamiResponse {
    cn: String,
    /// rfc3339-formatted timestamp the server received the request.
    /// `issuedAt` semantically refers to "the moment we observed
    /// this verified identity", not the cert's notBefore — that's
    /// available from the cert chain itself if a future endpoint
    /// needs it.
    #[serde(rename = "issuedAt")]
    issued_at: String,
}

/// `GET /v1/whoami` — returns the verified mTLS CN of the caller.
/// Useful for confirming the cert pipeline is wired correctly before
/// the agent body is real (PR-3). When mTLS is not configured (no
/// `--client-ca`), the handler returns 401 — `/v1/whoami` is
/// intentionally one of the gated routes since there's nothing to
/// say without a verified peer.
async fn whoami(
    Extension(peer_certs): Extension<PeerCertificates>,
) -> Result<Json<WhoamiResponse>, StatusCode> {
    if !peer_certs.is_present() {
        // mTLS not configured at the server level, or client did not
        // present a cert. Either way: nothing meaningful to report.
        return Err(StatusCode::UNAUTHORIZED);
    }
    let cn = peer_certs.leaf_cn().ok_or_else(|| {
        tracing::warn!("whoami: peer cert has no parseable CN");
        StatusCode::UNAUTHORIZED
    })?;
    Ok(Json(WhoamiResponse {
        cn,
        issued_at: Utc::now().to_rfc3339(),
    }))
}

/// Extract the verified CN from `PeerCertificates`, or return 401.
/// Centralised so each /v1/* handler reads the same way.
fn require_cn(peer_certs: &PeerCertificates) -> Result<String, StatusCode> {
    if !peer_certs.is_present() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    peer_certs.leaf_cn().ok_or(StatusCode::UNAUTHORIZED)
}

/// `POST /v1/agent/checkin` — record an agent checkin.
///
/// Validates the body's `hostname` matches the verified mTLS CN
/// (sanity check, not a security boundary — the CN was already
/// authenticated by WebPkiClientVerifier; this just catches
/// configuration drift like a host using the wrong cert).
///
/// Emits a journal line per checkin so operators can grep
/// `journalctl -u nixfleet-control-plane | grep checkin`.
async fn checkin(
    State(state): State<Arc<AppState>>,
    Extension(peer_certs): Extension<PeerCertificates>,
    Json(req): Json<CheckinRequest>,
) -> Result<Json<CheckinResponse>, StatusCode> {
    let cn = require_cn(&peer_certs)?;
    if cn != req.hostname {
        tracing::warn!(
            cert_cn = %cn,
            body_hostname = %req.hostname,
            "checkin rejected: cert CN does not match body hostname"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    // Surface the checkin in the journal in a grep-friendly shape.
    // `last_fetch` is the field operators care about most for spotting
    // stuck agents (verify-failed, fetch-failed) without parsing the
    // full body.
    let last_fetch = req
        .last_fetch_outcome
        .as_ref()
        .map(|o| format!("{:?}", o.result).to_lowercase())
        .unwrap_or_else(|| "none".to_string());
    let pending = req
        .pending_generation
        .as_ref()
        .map(|p| p.closure_hash.as_str())
        .unwrap_or("null");
    tracing::info!(
        target: "checkin",
        hostname = %req.hostname,
        closure_hash = %req.current_generation.closure_hash,
        pending = %pending,
        last_fetch = %last_fetch,
        "checkin received"
    );

    let record = HostCheckinRecord {
        last_checkin: Utc::now(),
        checkin: req.clone(),
    };
    state
        .host_checkins
        .write()
        .await
        .insert(req.hostname.clone(), record);

    Ok(Json(CheckinResponse {
        target: None,
        next_checkin_secs: NEXT_CHECKIN_SECS,
    }))
}

/// `POST /v1/agent/report` — record an out-of-band event report.
///
/// In-memory ring buffer per host, capped at `REPORT_RING_CAP`. New
/// reports push to the back; if the buffer is full, the oldest is
/// dropped. Phase 4 promotes this to SQLite + correlates with
/// rollouts.
async fn report(
    State(state): State<Arc<AppState>>,
    Extension(peer_certs): Extension<PeerCertificates>,
    Json(req): Json<ReportRequest>,
) -> Result<Json<ReportResponse>, StatusCode> {
    let cn = require_cn(&peer_certs)?;
    if cn != req.hostname {
        tracing::warn!(
            cert_cn = %cn,
            body_hostname = %req.hostname,
            "report rejected: cert CN does not match body hostname"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    // Generate an opaque event ID. Not cryptographically random —
    // it's a journal correlation handle, not a security boundary.
    let event_id = format!(
        "evt-{}-{}",
        Utc::now().timestamp_millis(),
        rand_suffix(8)
    );

    let received_at = Utc::now();
    let kind_str = format!("{:?}", req.kind).to_lowercase();
    let error_str = req.error.clone().unwrap_or_else(|| "<none>".to_string());
    tracing::info!(
        target: "report",
        hostname = %req.hostname,
        kind = %kind_str,
        error = %error_str,
        event_id = %event_id,
        "report received"
    );

    let record = ReportRecord {
        event_id: event_id.clone(),
        received_at,
        report: req.clone(),
    };
    let mut reports = state.host_reports.write().await;
    let buf = reports.entry(req.hostname).or_default();
    if buf.len() >= REPORT_RING_CAP {
        buf.pop_front();
    }
    buf.push_back(record);

    Ok(Json(ReportResponse { event_id }))
}

/// 8-char lowercase-alnum suffix for event IDs. Not crypto-grade —
/// just enough to make IDs visually distinct in journal output. Uses
/// system time microseconds + nanos as the entropy source so we
/// don't pull the `rand` crate just for this.
fn rand_suffix(n: usize) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let alphabet: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(n);
    let mut x = nanos.wrapping_mul(0x9e3779b97f4a7c15);
    for _ in 0..n {
        let idx = (x % alphabet.len() as u64) as usize;
        out.push(alphabet[idx] as char);
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    }
    out
}

/// `POST /v1/enroll` — bootstrap a new fleet host.
///
/// No mTLS required (this is the path before the host has a cert).
/// Authentication is via the bootstrap-token signature against the
/// org root key in trust.json. Order of checks matches the
/// security narrative in RFC-0003 §2:
/// 1. Replay: refuse already-seen nonces.
/// 2. Expiry: refuse tokens outside their issued/expires window.
/// 3. Signature: verify against `orgRootKey.current` (and `.previous`
///    during a rotation grace window) from trust.json.
/// 4. Hostname binding: claim's hostname must match CSR CN (validated
///    by `issuance::issue_cert` chain).
/// 5. Pubkey-fingerprint binding: SHA-256 of the CSR's pubkey DER
///    must match `claims.expected_pubkey_fingerprint`.
async fn enroll(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, StatusCode> {
    use base64::Engine;

    let now = chrono::Utc::now();

    // 1. Replay defense — drop the nonce on the floor early so a
    //    flood of replays doesn't pay for parsing + signature work.
    {
        let seen = state.seen_token_nonces.read().await;
        if seen.contains(&req.token.claims.nonce) {
            tracing::warn!(nonce = %req.token.claims.nonce, "enroll: token replay rejected");
            return Err(StatusCode::CONFLICT);
        }
        // Hold off inserting until after signature verification —
        // we don't want a forged token's nonce to lock out a real
        // operator-minted retry.
    }

    // 2. Expiry.
    if now < req.token.claims.issued_at || now >= req.token.claims.expires_at {
        tracing::warn!(
            hostname = %req.token.claims.hostname,
            "enroll: token outside validity window"
        );
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 3. Signature verification against trust.json's `orgRootKey`.
    //    Re-read on every enroll so operator key rotations propagate
    //    without restart. `orgRootKey.current` and `.previous` are
    //    both candidates during a rotation grace window per
    //    CONTRACTS.md §II #3.
    let trust_path = state
        .issuance_paths
        .read()
        .await
        .fleet_ca_cert
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("/etc/nixfleet/cp"))
        .join("trust.json");
    let trust_raw = std::fs::read_to_string(&trust_path).map_err(|err| {
        tracing::error!(error = %err, path = %trust_path.display(), "enroll: read trust.json");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let trust: nixfleet_proto::TrustConfig = serde_json::from_str(&trust_raw).map_err(|err| {
        tracing::error!(error = %err, "enroll: parse trust.json");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let org_root = trust.org_root_key.as_ref().ok_or_else(|| {
        tracing::error!(
            "enroll: trust.json has no orgRootKey — refusing to accept any token. \
             Set nixfleet.trust.orgRootKey.current in fleet.nix and rebuild."
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let candidates = org_root.active_keys();
    if candidates.is_empty() {
        tracing::error!("enroll: orgRootKey has no current/previous keys");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let mut sig_ok = false;
    for pubkey in &candidates {
        if pubkey.algorithm != "ed25519" {
            tracing::warn!(
                algorithm = %pubkey.algorithm,
                "enroll: skipping non-ed25519 orgRootKey candidate (only ed25519 supported)"
            );
            continue;
        }
        let pubkey_bytes = match base64::engine::general_purpose::STANDARD.decode(&pubkey.public) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(error = %err, "enroll: orgRootKey base64 decode");
                continue;
            }
        };
        if crate::issuance::verify_token_signature(&req.token, &pubkey_bytes).is_ok() {
            sig_ok = true;
            break;
        }
    }
    if !sig_ok {
        tracing::warn!(
            hostname = %req.token.claims.hostname,
            nonce = %req.token.claims.nonce,
            "enroll: token signature did not verify against any orgRootKey candidate"
        );
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 4. Hostname / 5. pubkey-fingerprint validation against CSR.
    //    Done by reading the CSR before issuance (issuance::issue_cert
    //    will populate the cert's CN from the CSR). We pre-validate
    //    here so we can refuse before doing any signing work.
    let csr_params =
        rcgen::CertificateSigningRequestParams::from_pem(&req.csr_pem).map_err(|err| {
            tracing::warn!(error = %err, "enroll: parse CSR PEM");
            StatusCode::BAD_REQUEST
        })?;
    let csr_cn: Option<String> = csr_params
        .params
        .distinguished_name
        .iter()
        .find_map(|(t, v): (&rcgen::DnType, &rcgen::DnValue)| {
            if matches!(t, rcgen::DnType::CommonName) {
                Some(match v {
                    rcgen::DnValue::PrintableString(s) => s.to_string(),
                    rcgen::DnValue::Utf8String(s) => s.to_string(),
                    _ => format!("{:?}", v),
                })
            } else {
                None
            }
        });
    let csr_cn = csr_cn.ok_or_else(|| {
        tracing::warn!("enroll: CSR has no CN");
        StatusCode::BAD_REQUEST
    })?;
    let csr_pubkey_der = csr_params.public_key.der_bytes();
    let csr_fingerprint = crate::issuance::fingerprint(csr_pubkey_der);

    if let Err(err) = crate::issuance::validate_token_claims(
        &req.token.claims,
        &csr_cn,
        &csr_fingerprint,
        now,
    ) {
        tracing::warn!(error = %err, hostname = %req.token.claims.hostname, "enroll: claim validation");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // All checks passed — commit the nonce as seen so a replay of
    // this exact (verified) token is rejected.
    state
        .seen_token_nonces
        .write()
        .await
        .insert(req.token.claims.nonce.clone());

    // Issue the cert.
    let paths = state.issuance_paths.read().await.clone();
    let (ca_cert, ca_key, audit_log_path) = match (&paths.fleet_ca_cert, &paths.fleet_ca_key) {
        (Some(c), Some(k)) => (c.clone(), k.clone(), paths.audit_log.clone()),
        _ => {
            tracing::error!("enroll: fleet CA cert/key paths not configured");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let (cert_pem, not_after) = crate::issuance::issue_cert(
        &req.csr_pem,
        &ca_cert,
        &ca_key,
        crate::issuance::AGENT_CERT_VALIDITY,
        now,
    )
    .map_err(|err| {
        tracing::error!(error = %err, "enroll: issue_cert failed");
        StatusCode::BAD_REQUEST
    })?;

    if let Some(path) = &audit_log_path {
        crate::issuance::audit_log(
            path,
            now,
            "<enroll>",
            &req.token.claims.hostname,
            not_after,
            &crate::issuance::AuditContext::Enroll {
                token_nonce: req.token.claims.nonce.clone(),
            },
        );
    }
    tracing::info!(
        target: "issuance",
        hostname = %req.token.claims.hostname,
        not_after = %not_after.to_rfc3339(),
        "enrolled"
    );

    Ok(Json(EnrollResponse { cert_pem, not_after }))
}

/// `POST /v1/agent/renew` — issue a fresh cert for an authenticated
/// agent. mTLS-required; the verified CN must match the CSR's CN.
async fn renew(
    State(state): State<Arc<AppState>>,
    Extension(peer_certs): Extension<PeerCertificates>,
    Json(req): Json<RenewRequest>,
) -> Result<Json<RenewResponse>, StatusCode> {
    let cn = require_cn(&peer_certs)?;
    let now = chrono::Utc::now();

    let paths = state.issuance_paths.read().await.clone();
    let (ca_cert, ca_key, audit_log_path) = match (&paths.fleet_ca_cert, &paths.fleet_ca_key) {
        (Some(c), Some(k)) => (c.clone(), k.clone(), paths.audit_log.clone()),
        _ => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let (cert_pem, not_after) = crate::issuance::issue_cert(
        &req.csr_pem,
        &ca_cert,
        &ca_key,
        crate::issuance::AGENT_CERT_VALIDITY,
        now,
    )
    .map_err(|err| {
        tracing::error!(error = %err, "renew: issue_cert failed");
        StatusCode::BAD_REQUEST
    })?;

    if let Some(path) = &audit_log_path {
        crate::issuance::audit_log(
            path,
            now,
            &cn,
            &cn,
            not_after,
            &crate::issuance::AuditContext::Renew {
                previous_cert_serial: "<unknown>".to_string(),
            },
        );
    }
    tracing::info!(
        target: "issuance",
        hostname = %cn,
        not_after = %not_after.to_rfc3339(),
        "renewed"
    );

    Ok(Json(RenewResponse { cert_pem, not_after }))
}

fn build_router(state: Arc<AppState>) -> Router {
    // /healthz remains unauthenticated per spec D7 — operational
    // debuggability outweighs the marginal sovereignty gain of
    // mTLS-gating a status endpoint.
    //
    // /v1/* requires verified mTLS — the MtlsAcceptor injects
    // PeerCertificates into request extensions; handlers extract via
    // the Extension extractor and 401 if absent/empty.
    //
    // PR-1: /healthz
    // PR-2: + /v1/whoami
    // PR-3: + /v1/agent/checkin, /v1/agent/report
    // PR-4: + /v1/admin/observed (proposed) — TBD
    // PR-5: + /v1/enroll, /v1/agent/renew
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/whoami", get(whoami))
        .route("/v1/agent/checkin", post(checkin))
        .route("/v1/agent/report", post(report))
        .route("/v1/enroll", post(enroll))
        .route("/v1/agent/renew", post(renew))
        .with_state(state)
}

/// Spawn the reconcile loop. Each tick:
/// 1. Reads the channel-refs cache (refreshed by the Forgejo poll
///    task; falls back to file-backed observed.json when empty).
/// 2. Builds an `Observed` from the in-memory checkin state +
///    cached channel-refs (PR-4 projection).
/// 3. Verifies the resolved artifact and reconciles against the
///    projected `Observed`.
/// 4. Emits the plan via tracing.
///
/// Errors at any step are logged and fall through; the loop never
/// crashes on transient failures.
fn spawn_reconcile_loop(state: Arc<AppState>, inputs: TickInputs) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + RECONCILE_INTERVAL,
            RECONCILE_INTERVAL,
        );
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            ticker.tick().await;
            let now = Utc::now();

            // Snapshot the cache + checkins under read locks. Drop
            // the locks before doing the (potentially slow) verify +
            // reconcile work.
            let channel_refs = {
                let cache = state.channel_refs_cache.read().await;
                cache.refs.clone()
            };
            let checkins = state.host_checkins.read().await.clone();

            // PR-4 projection: in-memory checkins + cached channel-refs.
            // When the Forgejo poll hasn't succeeded yet AND no agents
            // have checked in, fall back to the file-backed
            // observed.json so PR-1's deploy-without-agents path keeps
            // working.
            let inputs_now = TickInputs {
                now,
                ..inputs.clone()
            };
            let result = if checkins.is_empty() && channel_refs.is_empty() {
                tick(&inputs_now)
            } else {
                run_tick_with_projection(&inputs_now, &checkins, &channel_refs)
            };

            match result {
                Ok(out) => {
                    let plan = render_plan(&out);
                    tracing::info!(target: "reconcile", "{}", plan.trim_end());
                }
                Err(err) => {
                    tracing::warn!(error = %err, "reconcile tick failed");
                }
            }
            *state.last_tick_at.write().await = Some(now);
        }
    });
}

/// Run a tick using the in-memory projection rather than reading
/// `observed.json`. Mirrors `crate::tick` but takes the projected
/// `Observed` from the caller.
fn run_tick_with_projection(
    inputs: &TickInputs,
    checkins: &HashMap<String, HostCheckinRecord>,
    channel_refs: &HashMap<String, String>,
) -> anyhow::Result<crate::TickOutput> {
    use anyhow::Context;
    let artifact = std::fs::read(&inputs.artifact_path)
        .with_context(|| format!("read artifact {}", inputs.artifact_path.display()))?;
    let signature = std::fs::read(&inputs.signature_path)
        .with_context(|| format!("read signature {}", inputs.signature_path.display()))?;
    let trust_raw = std::fs::read_to_string(&inputs.trust_path)
        .with_context(|| format!("read trust {}", inputs.trust_path.display()))?;
    let trust: nixfleet_proto::TrustConfig =
        serde_json::from_str(&trust_raw).context("parse trust")?;

    let trusted_keys = trust.ci_release_key.active_keys();
    let reject_before = trust.ci_release_key.reject_before;

    let verify = match nixfleet_reconciler::verify_artifact(
        &artifact,
        &signature,
        &trusted_keys,
        inputs.now,
        inputs.freshness_window,
        reject_before,
    ) {
        Ok(fleet) => {
            let signed_at = fleet.meta.signed_at.expect("verified artifact carries meta.signedAt");
            let ci_commit = fleet.meta.ci_commit.clone();
            let observed = crate::observed_projection::project(checkins, channel_refs);
            let actions = nixfleet_reconciler::reconcile(&fleet, &observed, inputs.now);
            crate::VerifyOutcome::Ok {
                signed_at,
                ci_commit,
                observed,
                actions,
            }
        }
        Err(err) => crate::VerifyOutcome::Failed {
            reason: format!("{:?}", err),
        },
    };

    Ok(crate::TickOutput {
        now: inputs.now,
        verify,
    })
}

/// Serve until interrupted. Builds the TLS config, starts the
/// reconcile loop, binds the listener, runs forever.
pub async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let state = Arc::new(AppState::default());

    // Seed issuance config (PR-5). When fleet-ca-cert/key are unset
    // the /v1/enroll and /v1/agent/renew endpoints return 500 — they
    // need both to issue. PR-5's deploy expects them populated by
    // fleet/modules/secrets/nixos.nix.
    *state.issuance_paths.write().await = IssuancePaths {
        fleet_ca_cert: args.fleet_ca_cert.clone(),
        fleet_ca_key: args.fleet_ca_key.clone(),
        audit_log: args.audit_log_path.clone(),
    };

    // Reconcile loop runs concurrently with the listener — never gate
    // operator visibility on a TLS handshake completing.
    let tick_inputs = TickInputs {
        artifact_path: args.artifact_path.clone(),
        signature_path: args.signature_path.clone(),
        trust_path: args.trust_path.clone(),
        observed_path: args.observed_path.clone(),
        now: Utc::now(),
        freshness_window: args.freshness_window,
    };
    spawn_reconcile_loop(state.clone(), tick_inputs);

    // PR-4: Forgejo poll task. Updates a shared cache that's
    // mirrored into AppState's channel_refs_cache. When
    // `--forgejo-base-url` is unset, the cache stays empty and the
    // reconcile loop falls through to its file-backed observed.json
    // fallback.
    //
    // Bridge note: forgejo_poll::spawn takes its own
    // Arc<RwLock<ChannelRefsCache>>. AppState already owns one
    // inside `channel_refs_cache` (a RwLock, not an Arc). A short
    // mirror task copies poll-task → AppState every 30s. Cleaner
    // refactor (AppState holds an Arc) is a TODO follow-up to
    // keep this PR's diff focused on the live-projection switch.
    if let Some(forgejo_config) = args.forgejo.clone() {
        let cache_arc: Arc<RwLock<crate::forgejo_poll::ChannelRefsCache>> =
            Arc::new(RwLock::new(crate::forgejo_poll::ChannelRefsCache::default()));
        crate::forgejo_poll::spawn(cache_arc.clone(), forgejo_config);
        let state_for_mirror = state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(30));
            loop {
                ticker.tick().await;
                let snap = cache_arc.read().await.clone();
                *state_for_mirror.channel_refs_cache.write().await = snap;
            }
        });
    }

    let app = build_router(state);

    let tls_config = crate::tls::build_server_config(
        &args.tls_cert,
        &args.tls_key,
        args.client_ca.as_deref(),
    )?;
    let rustls_config =
        axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(tls_config));

    // Wrap RustlsAcceptor in MtlsAcceptor so peer certs are extracted
    // after the handshake and injected into request extensions. The
    // /v1/whoami handler reads the extension; PR-3+ middleware reads
    // it for CN-vs-path-id enforcement on agent routes.
    //
    // When --client-ca is unset (PR-1's TLS-only mode), the wrapper
    // still injects a PeerCertificates extension — just an empty one.
    // The /v1/whoami handler returns 401 in that case, which is
    // correct behaviour for the endpoint.
    let rustls_acceptor = axum_server::tls_rustls::RustlsAcceptor::new(rustls_config);
    let mtls_acceptor = MtlsAcceptor::new(rustls_acceptor);

    let mode = if args.client_ca.is_some() {
        "TLS+mTLS"
    } else {
        tracing::warn!(
            "control plane started without --client-ca: /v1/* endpoints will reject all clients with 401. \
             Pass --client-ca to enable mTLS — recommended for any non-PR-1 deployment."
        );
        "TLS-only"
    };
    tracing::info!(listen = %args.listen, %mode, "control plane listening");
    axum_server::bind(args.listen)
        .acceptor(mtls_acceptor)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}
