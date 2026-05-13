//! Cert-issuance handlers for enroll and renew.

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use nixfleet_proto::enroll_wire::{EnrollRequest, EnrollResponse, RenewRequest, RenewResponse};
use rcgen::PublicKeyData;

use super::super::middleware::AuthenticatedCn;
use super::super::route_error::{bad_request, bad_request_error, internal};
use super::super::state::AppState;

/// `POST /v1/enroll` - bootstrap a new fleet host (no mTLS; auth via bootstrap-token signature).
pub(in crate::server) async fn enroll(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, StatusCode> {
    let now = chrono::Utc::now();

    let db = state.db.as_ref().ok_or_else(|| {
        tracing::warn!("enroll: no db configured - endpoint unusable");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    if db
        .tokens()
        .token_seen(&req.token.claims.nonce)
        .map_err(internal("enroll: db token_seen failed"))?
    {
        tracing::warn!(nonce = %req.token.claims.nonce, "enroll: token replay rejected");
        return Err(StatusCode::CONFLICT);
    }

    if now < req.token.claims.issued_at || now >= req.token.claims.expires_at {
        tracing::warn!(
            hostname = %req.token.claims.hostname,
            "enroll: token outside validity window"
        );
        return Err(StatusCode::UNAUTHORIZED);
    }

    // LOADBEARING: re-read trust.json per enroll so operator key rotations propagate without restart.
    // Single source of truth - the daemon's --trust-file arg, plumbed through IssuancePaths.
    let trust_path = state.issuance_paths.read().await.trust_path.clone();
    crate::auth::issuance::verify_bootstrap_token_against_trust(&trust_path, &req.token, now)
        .map_err(|err| match err {
            crate::auth::issuance::TrustVerifyError::SignatureMismatch => {
                tracing::warn!(
                    hostname = %req.token.claims.hostname,
                    nonce = %req.token.claims.nonce,
                    "enroll: {err}",
                );
                StatusCode::UNAUTHORIZED
            }
            other => {
                tracing::error!(error = %other, "enroll: trust verification failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    let csr_params = rcgen::CertificateSigningRequestParams::from_pem(&req.csr_pem)
        .map_err(bad_request("enroll: parse CSR PEM"))?;
    let csr_cn: Option<String> = csr_params.params.distinguished_name.iter().find_map(
        |(t, v): (&rcgen::DnType, &rcgen::DnValue)| {
            if matches!(t, rcgen::DnType::CommonName) {
                Some(match v {
                    rcgen::DnValue::PrintableString(s) => s.to_string(),
                    rcgen::DnValue::Utf8String(s) => s.to_string(),
                    _ => format!("{:?}", v),
                })
            } else {
                None
            }
        },
    );
    let csr_cn = csr_cn.ok_or_else(|| {
        tracing::warn!("enroll: CSR has no CN");
        StatusCode::BAD_REQUEST
    })?;
    let csr_pubkey_der = csr_params.public_key.der_bytes();
    let csr_fingerprint = crate::auth::issuance::fingerprint(csr_pubkey_der);

    if let Err(err) = crate::auth::issuance::validate_token_claims(
        &req.token.claims,
        &csr_cn,
        &csr_fingerprint,
        now,
    ) {
        tracing::warn!(error = %err, hostname = %req.token.claims.hostname, "enroll: claim validation");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // RFC-0003 §2 binding: CSR pubkey MUST equal the host's declared
    // SSH host pubkey from fleet.resolved. Closes #43 (cert <--> host key
    // bond) and #9 (declarative-enrollment fingerprint match) in one
    // call site. Fail-closed when no fleet snapshot is verified yet
    // (cold-start race) or when the host has no declared pubkey.
    let snap = state.verified_fleet.read().await.clone().ok_or_else(|| {
        tracing::warn!("enroll: no verified fleet snapshot - refusing");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    let host_decl = snap.fleet.hosts.get(&csr_cn).ok_or_else(|| {
        tracing::warn!(host = %csr_cn, "enroll: host not declared in fleet.nix");
        StatusCode::UNAUTHORIZED
    })?;
    // FOOTGUN: rcgen 0.13's `PublicKeyData::der_bytes()` returns the
    // raw 32-byte ed25519 pubkey for ed25519 CSRs (not a 44-byte SPKI
    // wrapper as RFC 5280 SubjectPublicKeyInfo would suggest). Existing
    // fingerprint computation already relies on this - pass the bytes
    // straight to the binding check.
    if csr_pubkey_der.len() != 32 {
        tracing::warn!(
            hostname = %csr_cn,
            len = csr_pubkey_der.len(),
            "enroll: CSR pubkey is not 32 raw bytes (non-ed25519 CSR rejected)",
        );
        return Err(StatusCode::BAD_REQUEST);
    }
    if let Err(err) = crate::auth::issuance::validate_csr_against_fleet_host(
        csr_pubkey_der,
        host_decl.pubkey.as_deref(),
    ) {
        tracing::warn!(host = %csr_cn, error = %err, "enroll: fleet-pubkey binding check failed");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // LOADBEARING: plain INSERT closes the TOCTOU between token_seen() and cert issuance via PK conflict.
    let outcome = db
        .tokens()
        .record_token_nonce(&req.token.claims.nonce, &req.token.claims.hostname)
        .map_err(internal(
            "enroll: db record_token_nonce failed; refusing enrollment",
        ))?;
    if matches!(outcome, crate::db::RecordTokenOutcome::AlreadyRecorded) {
        tracing::warn!(
            nonce = %req.token.claims.nonce,
            "enroll: token replay detected at record (concurrent enroll race or retry)",
        );
        return Err(StatusCode::CONFLICT);
    }

    let audit_log_path = state.issuance_paths.read().await.audit_log.clone();
    let signer = match state.ca_signer.read().await.as_ref() {
        Some(s) => Arc::clone(s),
        None => {
            tracing::error!("enroll: CA signer not configured");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let (cert_pem, not_after) = crate::auth::issuance::issue_cert(
        &req.csr_pem,
        signer.as_ref(),
        state.agent_cert_validity,
        now,
        &state.agent_cn_suffix,
    )
    .map_err(bad_request_error("enroll: issue_cert failed"))?;

    if let Some(path) = &audit_log_path {
        // `issued_cn` records the cert's actual CN (canonical
        // `agent-<machineId>.<suffix>`) - same form the renew path
        // records, so audit-log rows are uniform across enroll + renew.
        crate::auth::issuance::audit_log(
            path,
            now,
            "<enroll>",
            &crate::auth::issuance::canonical_agent_cn(
                &req.token.claims.hostname,
                &state.agent_cn_suffix,
            ),
            not_after,
            &crate::auth::issuance::AuditContext::Enroll {
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

    Ok(Json(EnrollResponse {
        cert_pem,
        not_after,
    }))
}

/// `POST /v1/agent/renew` - mTLS-required; verified CN is stamped onto the new cert.
pub(in crate::server) async fn renew(
    State(state): State<Arc<AppState>>,
    Extension(cn): Extension<AuthenticatedCn>,
    Json(req): Json<RenewRequest>,
) -> Result<Json<RenewResponse>, StatusCode> {
    let cn = cn.into_string();
    let now = chrono::Utc::now();

    // RFC-0003 §2 binding: renewal CSR's pubkey MUST equal the host's
    // declared SSH host pubkey, identical predicate to enroll. Without
    // this, renewal would silently let the agent rotate to a fresh
    // (non-host-bound) keypair - defeating the binding the operator
    // declared in fleet.nix.
    let renew_csr_params = rcgen::CertificateSigningRequestParams::from_pem(&req.csr_pem)
        .map_err(bad_request("renew: parse CSR PEM"))?;
    let csr_pubkey_der = renew_csr_params.public_key.der_bytes();
    if csr_pubkey_der.len() != 32 {
        tracing::warn!(
            hostname = %cn,
            len = csr_pubkey_der.len(),
            "renew: CSR pubkey is not 32 raw bytes (non-ed25519 CSR rejected)",
        );
        return Err(StatusCode::BAD_REQUEST);
    }
    let snap = state.verified_fleet.read().await.clone().ok_or_else(|| {
        tracing::warn!("renew: no verified fleet snapshot - refusing");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    // Verified mTLS CN may be canonical (`agent-<id>.<suffix>`, post-C.3)
    // or bare machineId (legacy). Strip to bare for the fleet.hosts lookup.
    let machine_id = crate::auth::issuance::extract_machine_id(&cn, &state.agent_cn_suffix);
    let host_decl = snap.fleet.hosts.get(&machine_id).ok_or_else(|| {
        tracing::warn!(host = %cn, machine_id, "renew: host not declared in fleet.nix");
        StatusCode::UNAUTHORIZED
    })?;
    if let Err(err) = crate::auth::issuance::validate_csr_against_fleet_host(
        csr_pubkey_der,
        host_decl.pubkey.as_deref(),
    ) {
        tracing::warn!(host = %cn, error = %err, "renew: fleet-pubkey binding check failed");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let audit_log_path = state.issuance_paths.read().await.audit_log.clone();
    let signer = match state.ca_signer.read().await.as_ref() {
        Some(s) => Arc::clone(s),
        None => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let (cert_pem, not_after) = crate::auth::issuance::issue_cert(
        &req.csr_pem,
        signer.as_ref(),
        state.agent_cert_validity,
        now,
        &state.agent_cn_suffix,
    )
    .map_err(bad_request_error("renew: issue_cert failed"))?;

    if let Some(path) = &audit_log_path {
        crate::auth::issuance::audit_log(
            path,
            now,
            &cn,
            &cn,
            not_after,
            &crate::auth::issuance::AuditContext::Renew {
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

    Ok(Json(RenewResponse {
        cert_pem,
        not_after,
    }))
}
