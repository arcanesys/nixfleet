//! Shared state + configuration types for the long-running server.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nixfleet_proto::agent_wire::{CheckinRequest, ReportRequest};
use nixfleet_proto::FleetResolved;
use tokio::sync::RwLock;

pub(super) const REPORT_RING_CAP: usize = 32;

pub(super) const NEXT_CHECKIN_SECS: u32 = 60;

pub(super) const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// Must exceed agent poll budget (~300s) plus slack to avoid magic-rollback / agent-poll races.
pub const DEFAULT_CONFIRM_DEADLINE_SECS: i64 = 360;

/// `Default` defaults are bogus on purpose; prod paths fail at first IO if
/// clap parsing is skipped.
#[derive(Debug, Clone)]
pub struct ServeArgs {
    pub listen: SocketAddr,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub client_ca: Option<PathBuf>,
    /// Often the same path as `client_ca`.
    pub fleet_ca_cert: Option<PathBuf>,
    /// File-backed CA signer's private key PEM. TPM (pubkey + wrapper) wins.
    pub fleet_ca_key: Option<PathBuf>,
    /// TPM-backed CA signer: keyslot scope's `pubkey.raw` (64 raw P-256 X||Y).
    pub tpm_ca_pubkey_raw: Option<PathBuf>,
    /// TPM-backed CA signer: keyslot scope's `tpm-sign-<keyname>` wrapper.
    pub tpm_ca_sign_wrapper: Option<PathBuf>,
    pub audit_log_path: Option<PathBuf>,
    pub artifact_path: PathBuf,
    pub signature_path: PathBuf,
    pub trust_path: PathBuf,
    /// File-backed fallback used only when no agents checked in AND `channel_refs` is None.
    pub observed_path: PathBuf,
    pub freshness_window: Duration,
    pub confirm_deadline_secs: i64,
    /// `None` -> file-backed `--artifact` only.
    pub channel_refs: Option<crate::polling::channel_refs_poll::ChannelRefsSource>,
    pub revocations: Option<crate::polling::revocations_poll::RevocationsSource>,
    /// `None` -> in-memory state only.
    pub db_path: Option<PathBuf>,
    /// `None` -> `/v1/agent/closure/<hash>` returns 501.
    pub closure_upstream: Option<String>,
    /// Pre-signed `<rolloutId>.{json,sig}` pairs; falls back to `rollouts_source`, then 503.
    pub rollouts_dir: Option<PathBuf>,
    /// HTTP-fetched manifests; required when `nixfleet-release` writes manifests post-build.
    pub rollouts_source: Option<crate::rollouts_source::RolloutsSource>,
    /// Refuse to start when any security-fallback flag is unset.
    pub strict: bool,
    /// `agent-<machineId>.<suffix>` for issued cert CNs. Must match the
    /// issuance CA's `dNSName` name constraint (D14).
    pub agent_cn_suffix: String,
    /// Test-only: skip the readiness gate so endpoint tests don't have to
    /// drive a real channel-refs poll. Production paths MUST leave `false`;
    /// the CLI never sets it.
    pub mark_ready_at_startup: bool,
}

impl Default for ServeArgs {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:0".parse().expect("static loopback addr"),
            tls_cert: PathBuf::new(),
            tls_key: PathBuf::new(),
            client_ca: None,
            fleet_ca_cert: None,
            fleet_ca_key: None,
            tpm_ca_pubkey_raw: None,
            tpm_ca_sign_wrapper: None,
            audit_log_path: None,
            artifact_path: PathBuf::new(),
            signature_path: PathBuf::new(),
            trust_path: PathBuf::new(),
            observed_path: PathBuf::new(),
            freshness_window: Duration::from_secs(2_592_000),
            confirm_deadline_secs: DEFAULT_CONFIRM_DEADLINE_SECS,
            channel_refs: None,
            revocations: None,
            db_path: None,
            closure_upstream: None,
            rollouts_dir: None,
            rollouts_source: None,
            strict: false,
            agent_cn_suffix: crate::auth::issuance::DEFAULT_AGENT_CN_SUFFIX.to_string(),
            mark_ready_at_startup: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HostCheckinRecord {
    pub last_checkin: DateTime<Utc>,
    pub checkin: CheckinRequest,
}

/// `signature_status` `None` for variants that don't sign or events predating the field.
#[derive(Debug, Clone)]
pub struct ReportRecord {
    pub event_id: String,
    pub received_at: DateTime<Utc>,
    pub report: ReportRequest,
    pub signature_status: Option<nixfleet_reconciler::evidence::SignatureStatus>,
}

/// `(fleet, hash)` pair under one lock prevents readers seeing fresh fleet with stale hash.
#[derive(Clone, Debug)]
pub struct VerifiedFleetSnapshot {
    pub fleet: Arc<FleetResolved>,
    pub fleet_resolved_hash: String,
}

#[derive(Clone, Debug)]
pub struct ClosureUpstream {
    pub base_url: String,
    pub client: reqwest::Client,
}

#[derive(Debug, Clone, Default)]
pub struct IssuancePaths {
    pub fleet_ca_cert: Option<PathBuf>,
    pub fleet_ca_key: Option<PathBuf>,
    pub audit_log: Option<PathBuf>,
    /// Path to the daemon-configured trust.json (the `--trust-file`
    /// flag). Both `/v1/enroll` (orgRootKey signature verify) and
    /// `/v1/agent/bootstrap-report` (same) read this. The polling
    /// loops have their own copy via `ChannelRefsSource.trust_path`
    /// - they all point at the same file in production. ONE source
    /// of truth for the daemon's trust roots, not derived from
    /// fleet_ca_cert (which broke when operators placed
    /// fleet-ca.pem outside `/etc/nixfleet/cp/`).
    pub trust_path: PathBuf,
}

pub struct AppState {
    pub last_tick_at: RwLock<Option<DateTime<Utc>>>,
    pub host_checkins: RwLock<HashMap<String, HostCheckinRecord>>,
    pub host_reports: RwLock<HashMap<String, VecDeque<ReportRecord>>>,
    pub channel_refs_cache: Arc<RwLock<crate::polling::channel_refs_poll::ChannelRefsCache>>,
    pub issuance_paths: RwLock<IssuancePaths>,
    /// Built once at server start from `ServeArgs` - `TpmCaSigner` if
    /// the TPM flags are set, `FileCaSigner` otherwise, `None` if no
    /// CA flags supplied (enroll/renew return 500). `dyn` lets enroll
    /// + renew handlers stay agnostic to signing backend.
    pub ca_signer: RwLock<Option<Arc<dyn crate::auth::issuance::CaSigner>>>,
    pub db: Option<Arc<crate::db::Db>>,
    pub closure_upstream: Option<ClosureUpstream>,
    pub verified_fleet: Arc<RwLock<Option<VerifiedFleetSnapshot>>>,
    /// Most recent successfully-journalled `RolloutDeferred` per channel.
    /// Fed into the reconciler's `Observed.last_deferrals` so a still-blocked
    /// channel doesn't re-emit the same line every reconcile tick. In-memory
    /// only - losing this on restart just means one duplicate line on the
    /// first post-restart tick, which is correct behavior.
    pub last_deferrals: Arc<RwLock<HashMap<String, nixfleet_reconciler::observed::DeferralRecord>>>,
    /// Event-driven kick for the channel-refs poll. The reconciler sends `()`
    /// after state transitions that might release a channelEdges-blocked
    /// successor (`ConvergeRollout` stamping `terminal_at`, `SoakHost`
    /// transitioning a host to Soaked). The polling loop selects on its
    /// 60 s ticker AND this watch - whichever fires first triggers a
    /// `poll_once`. Closes the timing window between channelEdges semantically
    /// releasing and the table reflecting the new successor rollout.
    ///
    /// `watch` semantics (latest value, no backlog) collapse a burst of
    /// kicks into one wake - the poller doesn't need to drain a queue.
    /// The 60 s ticker stays as a safety net: if the kick is ever missed
    /// (subscriber starvation, reconciler crash mid-stamp), polling
    /// catches up within the cadence.
    pub channel_refs_kick: tokio::sync::watch::Sender<()>,
    pub confirm_deadline_secs: i64,
    pub rollouts_dir: Option<PathBuf>,
    pub rollouts_source: Option<crate::rollouts_source::RolloutsSource>,
    pub strict: bool,
    /// See `ServeArgs::agent_cn_suffix`. Captured into AppState so the
    /// enroll/renew handlers can canonicalise CNs without going
    /// through `issuance_paths`.
    pub agent_cn_suffix: String,
    /// Set to `true` once the channel-refs poll (or build-time prime)
    /// has populated `verified_fleet` with a freshly-verified snapshot.
    /// Stays `false` indefinitely when neither prime path produces a
    /// verifiable artifact (operator must provision `artifact_path` or
    /// configure `channel_refs.artifact_url`). Read by the
    /// `require_ready` middleware to gate `/v1/*` with 503 until set.
    pub artifact_primed: Arc<AtomicBool>,
    /// Set to `true` once the revocations poll has applied a verified
    /// list at least once. Only consulted when `revocations_required`
    /// is `true`; otherwise the readiness check ignores this flag.
    pub revocations_primed: Arc<AtomicBool>,
    /// `true` iff `--revocations-{artifact,signature}-url` were both set
    /// at startup. Captured into AppState so the readiness check stays
    /// pure (no need to thread `ServeArgs` into middleware).
    pub revocations_required: bool,
}

impl AppState {
    /// Composite readiness: artifact verified AND (when configured)
    /// revocations verified. Strict: full trust footprint loaded
    /// before serving agents. See #95.
    pub fn is_ready(&self) -> bool {
        if !self.artifact_primed.load(Ordering::Acquire) {
            return false;
        }
        if self.revocations_required && !self.revocations_primed.load(Ordering::Acquire) {
            return false;
        }
        true
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            last_tick_at: RwLock::new(None),
            host_checkins: RwLock::new(HashMap::new()),
            host_reports: RwLock::new(HashMap::new()),
            channel_refs_cache: Arc::new(RwLock::new(
                crate::polling::channel_refs_poll::ChannelRefsCache::default(),
            )),
            issuance_paths: RwLock::new(IssuancePaths::default()),
            ca_signer: RwLock::new(None),
            db: None,
            closure_upstream: None,
            verified_fleet: Arc::new(RwLock::new(None)),
            last_deferrals: Arc::new(RwLock::new(HashMap::new())),
            channel_refs_kick: tokio::sync::watch::channel(()).0,
            confirm_deadline_secs: DEFAULT_CONFIRM_DEADLINE_SECS,
            rollouts_dir: None,
            rollouts_source: None,
            strict: false,
            agent_cn_suffix: crate::auth::issuance::DEFAULT_AGENT_CN_SUFFIX.to_string(),
            artifact_primed: Arc::new(AtomicBool::new(false)),
            revocations_primed: Arc::new(AtomicBool::new(false)),
            revocations_required: false,
        }
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("db", &self.db.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod ready_tests {
    use super::*;

    /// Default fresh AppState - neither artifact nor revocations primed.
    /// `is_ready` must return false so the middleware can hold /v1/* with 503.
    #[test]
    fn fresh_state_is_not_ready() {
        let state = AppState::default();
        assert!(!state.is_ready(), "fresh state must not be ready");
    }

    /// `revocations_required = false` (no `--revocations-*-url` flags).
    /// Only the artifact prime gates readiness - revocations_primed is ignored.
    #[test]
    fn artifact_prime_alone_is_enough_when_revocations_not_required() {
        let state = AppState::default();
        state.artifact_primed.store(true, Ordering::Release);
        assert!(
            state.is_ready(),
            "artifact-only ready when revocations not required"
        );
    }

    /// `revocations_required = true` (operator configured the polling loop).
    /// Artifact prime alone must NOT flip ready - full trust footprint required.
    #[test]
    fn artifact_alone_is_not_enough_when_revocations_required() {
        let state = AppState {
            revocations_required: true,
            ..AppState::default()
        };
        state.artifact_primed.store(true, Ordering::Release);
        assert!(
            !state.is_ready(),
            "must not be ready until revocations also primed",
        );
    }

    /// Both flags set in the required configuration -> ready.
    #[test]
    fn both_primed_with_revocations_required_is_ready() {
        let state = AppState {
            revocations_required: true,
            ..AppState::default()
        };
        state.artifact_primed.store(true, Ordering::Release);
        state.revocations_primed.store(true, Ordering::Release);
        assert!(state.is_ready(), "both primed must flip ready");
    }

    /// Revocations primed but artifact not -> not ready (can't serve dispatch
    /// without a verified fleet snapshot, even if the revocation list loaded).
    #[test]
    fn revocations_alone_is_not_ready() {
        let state = AppState {
            revocations_required: true,
            ..AppState::default()
        };
        state.revocations_primed.store(true, Ordering::Release);
        assert!(
            !state.is_ready(),
            "revocations without artifact is not ready"
        );
    }
}
