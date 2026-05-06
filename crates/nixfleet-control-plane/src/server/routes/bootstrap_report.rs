//! `POST /v1/agent/bootstrap-report` — anonymous, bootstrap-token-authed
//! event channel for failure modes the agent hits before it has a client
//! cert (parse_trust_file failure, enroll failure). Validates the same
//! orgRootKey signature as `/v1/enroll` but does NOT consume the nonce —
//! the agent must still be able to enroll on the next attempt.
//!
//! Allowlist on event variants: only `TrustError` and `EnrollmentFailed`
//! make sense pre-cert. Everything else is rejected with 422 so this
//! endpoint can't be repurposed to backdoor regular reports without mTLS.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use nixfleet_proto::agent_wire::{ReportEvent, ReportRequest};
use nixfleet_proto::enroll_wire::BootstrapEventRequest;

use super::super::state::{AppState, ReportRecord, REPORT_RING_CAP};

pub(in crate::server) async fn bootstrap_report(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BootstrapEventRequest>,
) -> Result<StatusCode, StatusCode> {
    let now = Utc::now();

    // Validity window — same shape as /v1/enroll.
    if now < req.token.claims.issued_at || now >= req.token.claims.expires_at {
        tracing::warn!(
            hostname = %req.token.claims.hostname,
            "bootstrap-report: token outside validity window"
        );
        return Err(StatusCode::UNAUTHORIZED);
    }

    let trust_path = state.issuance_paths.read().await.trust_path.clone();
    crate::auth::issuance::verify_bootstrap_token_against_trust(&trust_path, &req.token).map_err(
        |err| match err {
            crate::auth::issuance::TrustVerifyError::SignatureMismatch => {
                tracing::warn!(
                    hostname = %req.token.claims.hostname,
                    "bootstrap-report: {err}",
                );
                StatusCode::UNAUTHORIZED
            }
            other => {
                tracing::error!(error = %other, "bootstrap-report: trust verification failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        },
    )?;

    // Decode + allowlist the event variant. Only pre-cert failure modes
    // are accepted via this anonymous path; everything else routes
    // through mTLS-authed /v1/agent/report.
    let event: ReportEvent = match serde_json::from_value(req.event.clone()) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(
                hostname = %req.token.claims.hostname,
                error = %err,
                "bootstrap-report: event payload not a recognised ReportEvent"
            );
            return Err(StatusCode::BAD_REQUEST);
        }
    };
    if !matches!(
        event,
        ReportEvent::TrustError { .. } | ReportEvent::EnrollmentFailed { .. }
    ) {
        tracing::warn!(
            hostname = %req.token.claims.hostname,
            event = %event.discriminator(),
            "bootstrap-report: event variant not on the pre-cert allowlist"
        );
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let event_id = super::new_event_id();
    let received_at = Utc::now();
    let event_str = event.discriminator();

    tracing::warn!(
        target: "bootstrap-report",
        hostname = %req.token.claims.hostname,
        event = %event_str,
        agent_version = %req.agent_version,
        event_id = %event_id,
        "bootstrap-report received (pre-cert failure)"
    );

    let report_request = ReportRequest {
        hostname: req.token.claims.hostname.clone(),
        agent_version: req.agent_version.clone(),
        occurred_at: req.occurred_at,
        rollout: None,
        event,
    };
    let record = ReportRecord {
        event_id: event_id.clone(),
        received_at,
        report: report_request.clone(),
        // Bootstrap-token auth carries no per-event signature; mark None
        // so the wave-promotion gate's `counts_for_gate()` filter treats
        // this consistently with mTLS-authed unsigned events.
        signature_status: None,
    };

    if let Some(db) = state.db.as_ref() {
        if let Ok(report_json) = serde_json::to_string(&report_request) {
            if let Err(err) = db
                .reports()
                .record_host_report(&crate::db::HostReportInsert {
                    hostname: &req.token.claims.hostname,
                    event_id: &event_id,
                    received_at,
                    event_kind: event_str,
                    rollout: None,
                    signature_status: None,
                    report_json: &report_json,
                })
            {
                tracing::warn!(
                    target: "bootstrap-report",
                    error = %err,
                    "bootstrap-report SQLite write failed; in-memory ring buffer still updated",
                );
            }
        }
    }

    let mut reports = state.host_reports.write().await;
    let buf = reports
        .entry(req.token.claims.hostname.clone())
        .or_default();
    if buf.len() >= REPORT_RING_CAP {
        buf.pop_front();
    }
    buf.push_back(record);

    Ok(StatusCode::NO_CONTENT)
}

