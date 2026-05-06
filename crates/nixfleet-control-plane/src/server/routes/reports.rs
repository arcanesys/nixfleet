//! `/v1/agent/report` handler plus signature-verification helpers.

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use nixfleet_proto::agent_wire::{ReportRequest, ReportResponse};

use super::super::middleware::AuthenticatedCn;
use super::super::state::{AppState, ReportRecord, REPORT_RING_CAP};

/// `POST /v1/agent/report` — persists to SQLite and mirrors into a per-host ring buffer.
pub(in crate::server) async fn report(
    State(state): State<Arc<AppState>>,
    Extension(cn): Extension<AuthenticatedCn>,
    Json(req): Json<ReportRequest>,
) -> Result<Json<ReportResponse>, StatusCode> {
    let cn = cn.into_string();
    if cn != req.hostname {
        tracing::warn!(
            cert_cn = %cn,
            body_hostname = %req.hostname,
            "report rejected: cert CN does not match body hostname"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    let event_id = super::new_event_id();
    let received_at = Utc::now();

    let event_str = req.event.discriminator();
    let rollout_str = req.rollout.clone().unwrap_or_else(|| "<none>".to_string());

    // Best-effort: we always store the record (mTLS already authenticated); verdict shapes gating.
    let signature_status = compute_signature_status(&state, &req).await;

    tracing::info!(
        target: "report",
        hostname = %req.hostname,
        event = %event_str,
        rollout = %rollout_str,
        agent_version = %req.agent_version,
        event_id = %event_id,
        signature_status = ?signature_status,
        "report received"
    );

    let record = ReportRecord {
        event_id: event_id.clone(),
        received_at,
        report: req.clone(),
        signature_status,
    };

    if let Some(db) = state.db.as_ref() {
        let signature_status_str = signature_status.as_ref().and_then(|s| {
            serde_json::to_value(s)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
        });
        // FOOTGUN: writing "" on serde failure produced phantom DB rows that hydration silently skipped.
        match serde_json::to_string(&req) {
            Ok(report_json) => {
                if let Err(err) = db
                    .reports()
                    .record_host_report(&crate::db::HostReportInsert {
                        hostname: &req.hostname,
                        event_id: &event_id,
                        received_at,
                        event_kind: event_str,
                        rollout: req.rollout.as_deref(),
                        signature_status: signature_status_str.as_deref(),
                        report_json: &report_json,
                    })
                {
                    tracing::warn!(
                        target: "report",
                        hostname = %req.hostname,
                        event_id = %event_id,
                        error = %err,
                        "report SQLite write failed; in-memory ring buffer still updated",
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    target: "report",
                    hostname = %req.hostname,
                    event_id = %event_id,
                    error = %err,
                    "report serialisation to JSON failed; skipping SQLite persistence (in-memory ring still updated)",
                );
            }
        }
    }

    // LOADBEARING: flips Failed → Reverted so compute_rollback_signal stops re-emitting forever.
    if let Some(db) = state.db.as_ref() {
        apply_rollback_state_transition(db, &req);
    }

    let mut reports = state.host_reports.write().await;
    let buf = reports.entry(req.hostname).or_default();
    if buf.len() >= REPORT_RING_CAP {
        buf.pop_front();
    }
    buf.push_back(record);

    Ok(Json(ReportResponse { event_id }))
}

/// `GET /v1/host-reports?limit=N` — fleet-wide recent host reports from
/// the durable `host_reports` table. Backs the dashboard's "recent reports"
/// panel — DB-sourced, so it stays accurate regardless of the journal
/// rotation window. Default limit 15, max 200.
pub(in crate::server) async fn list_recent(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<RecentReportsQuery>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse as _;
    let db = state.db.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let limit = params.limit.unwrap_or(15).clamp(1, 200);
    let rows = db.reports().recent_across_hosts(limit).map_err(|err| {
        tracing::warn!(error = %err, "list_recent host_reports failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let reports: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(host, r)| {
            // Try to surface the inner ReportEvent details for downstream
            // panels — render.sh wants control_id / status etc. without
            // re-parsing the whole envelope itself.
            let parsed: Option<serde_json::Value> = serde_json::from_str(&r.report_json).ok();
            let event_details = parsed.as_ref().and_then(|v| v.get("event").cloned());
            let details_block = parsed.as_ref().and_then(|v| v.get("details").cloned());
            serde_json::json!({
                "hostname": host,
                "eventId": r.event_id,
                "receivedAt": r.received_at.to_rfc3339(),
                "eventKind": r.event_kind,
                "rollout": r.rollout,
                "signatureStatus": r.signature_status,
                "event": event_details,
                "details": details_block,
            })
        })
        .collect();
    let body = serde_json::json!({ "reports": reports }).to_string();
    let mut response = (StatusCode::OK, body).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

#[derive(serde::Deserialize)]
pub struct RecentReportsQuery {
    pub limit: Option<usize>,
}

/// Flip `host_rollout_state` Failed → Reverted on `RollbackTriggered`; guard leaves non-Failed alone.
fn apply_rollback_state_transition(db: &crate::db::Db, req: &ReportRequest) {
    use nixfleet_proto::agent_wire::ReportEvent;
    if !matches!(req.event, ReportEvent::RollbackTriggered { .. }) {
        return;
    }
    let Some(rollout) = req.rollout.as_deref() else {
        return;
    };
    match db.rollout_state().transition_host_state(
        &req.hostname,
        rollout,
        crate::state::HostRolloutState::Reverted,
        crate::state::HealthyMarker::Untouched,
        Some(crate::state::HostRolloutState::Failed),
    ) {
        Ok(0) => {
            tracing::debug!(
                target: "report",
                hostname = %req.hostname,
                rollout = %rollout,
                "RollbackTriggered: no Failed row to transition (guard preserved non-Failed state)",
            );
        }
        Ok(_) => {
            tracing::info!(
                target: "report",
                hostname = %req.hostname,
                rollout = %rollout,
                "RollbackTriggered: host_rollout_state Failed → Reverted",
            );
            // GOTCHA: record_terminal scopes by rollout_id so a newer dispatch is not clobbered.
            let now = Utc::now();
            if let Err(err) = db.host_dispatch_state().record_terminal(
                &req.hostname,
                rollout,
                crate::state::TerminalState::RolledBack,
            ) {
                tracing::warn!(
                    target: "report",
                    hostname = %req.hostname,
                    rollout = %rollout,
                    error = %err,
                    "RollbackTriggered: operational terminal stamp failed",
                );
            }
            if let Err(err) = db.dispatch_history().mark_terminal_for_rollout_host(
                rollout,
                &req.hostname,
                crate::state::TerminalState::RolledBack,
                now,
            ) {
                tracing::warn!(
                    target: "report",
                    hostname = %req.hostname,
                    rollout = %rollout,
                    error = %err,
                    "RollbackTriggered: audit terminal stamp failed",
                );
            }
        }
        Err(err) => {
            tracing::warn!(
                target: "report",
                hostname = %req.hostname,
                rollout = %rollout,
                error = %err,
                "RollbackTriggered: Failed → Reverted transition failed; report still persisted",
            );
        }
    }
}

/// Verdict for incoming reports; absent pubkey → `NoPubkey`, unsigned variants return `None`.
async fn compute_signature_status(
    state: &Arc<AppState>,
    req: &ReportRequest,
) -> Option<nixfleet_reconciler::evidence::SignatureStatus> {
    use nixfleet_proto::agent_wire::ReportEvent;
    use nixfleet_proto::evidence_signing::{
        ActivationFailedSignedPayload, ClosureSignatureMismatchSignedPayload,
        ComplianceFailureSignedPayload, ManifestMismatchSignedPayload,
        ManifestMissingSignedPayload, ManifestVerifyFailedSignedPayload,
        RealiseFailedSignedPayload, RollbackTriggeredSignedPayload, RuntimeGateErrorSignedPayload,
        StaleTargetSignedPayload, VerifyMismatchSignedPayload,
    };
    use nixfleet_reconciler::evidence::verify_event;

    let pubkey: Option<String> = {
        let fleet_guard = state.verified_fleet.read().await;
        fleet_guard
            .as_ref()
            .and_then(|snap| snap.fleet.hosts.get(&req.hostname))
            .and_then(|h| h.pubkey.clone())
    };

    match &req.event {
        ReportEvent::ComplianceFailure {
            control_id,
            status,
            framework_articles,
            evidence_snippet,
            evidence_collected_at,
            signature,
        } => {
            // GOTCHA: snippet_sha must match the agent's JCS-canonical hash; abort to None on JCS failure.
            let snippet_sha = match evidence_snippet {
                Some(v) => nixfleet_canonicalize::sha256_jcs_hex(v).ok()?,
                None => String::new(),
            };
            let payload = ComplianceFailureSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                control_id,
                status,
                framework_articles,
                evidence_collected_at: *evidence_collected_at,
                evidence_snippet_sha256: snippet_sha,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::RuntimeGateError {
            reason,
            collector_exit_code,
            evidence_collected_at,
            activation_completed_at,
            signature,
        } => {
            let payload = RuntimeGateErrorSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                reason,
                collector_exit_code: *collector_exit_code,
                evidence_collected_at: *evidence_collected_at,
                activation_completed_at: *activation_completed_at,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::ActivationFailed {
            phase,
            exit_code,
            stderr_tail,
            signature,
        } => {
            let stderr_tail_sha256 = nixfleet_canonicalize::sha256_jcs_hex(&stderr_tail.as_deref().unwrap_or("")).ok()?;
            let payload = ActivationFailedSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                phase,
                exit_code: *exit_code,
                stderr_tail_sha256,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::RealiseFailed {
            closure_hash,
            reason,
            signature,
        } => {
            let payload = RealiseFailedSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                closure_hash,
                reason,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::VerifyMismatch {
            expected,
            actual,
            signature,
        } => {
            let payload = VerifyMismatchSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                expected,
                actual,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::RollbackTriggered { reason, signature } => {
            let payload = RollbackTriggeredSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                reason,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::ClosureSignatureMismatch {
            closure_hash,
            stderr_tail,
            signature,
        } => {
            let stderr_tail_sha256 = nixfleet_canonicalize::sha256_jcs_hex(&stderr_tail).ok()?;
            let payload = ClosureSignatureMismatchSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                closure_hash,
                stderr_tail_sha256,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::StaleTarget {
            closure_hash,
            channel_ref,
            signed_at,
            freshness_window_secs,
            age_secs,
            signature,
        } => {
            let payload = StaleTargetSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                closure_hash,
                channel_ref,
                signed_at: *signed_at,
                freshness_window_secs: *freshness_window_secs,
                age_secs: *age_secs,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::ManifestMissing {
            rollout_id,
            reason,
            signature,
        } => {
            let payload = ManifestMissingSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                rollout_id,
                reason,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::ManifestVerifyFailed {
            rollout_id,
            reason,
            signature,
        } => {
            let payload = ManifestVerifyFailedSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                rollout_id,
                reason,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }
        ReportEvent::ManifestMismatch {
            rollout_id,
            reason,
            signature,
        } => {
            let payload = ManifestMismatchSignedPayload {
                hostname: &req.hostname,
                rollout: req.rollout.as_deref(),
                rollout_id,
                reason,
            };
            Some(verify_event(
                signature.as_deref(),
                pubkey.as_deref(),
                &payload,
            ))
        }

        ReportEvent::ActivationStarted { .. }
        | ReportEvent::EnrollmentFailed { .. }
        | ReportEvent::RenewalFailed { .. }
        | ReportEvent::TrustError { .. }
        | ReportEvent::Other { .. } => None,
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::state::{HealthyMarker, HostRolloutState};
    use chrono::Utc;
    use nixfleet_proto::agent_wire::{ReportEvent, ReportRequest};

    fn fresh_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.migrate().unwrap();
        db
    }

    fn rollback_report(host: &str, rollout: Option<&str>) -> ReportRequest {
        ReportRequest {
            hostname: host.to_string(),
            agent_version: "test".into(),
            occurred_at: Utc::now(),
            rollout: rollout.map(String::from),
            event: ReportEvent::RollbackTriggered {
                reason: "test".into(),
                signature: None,
            },
        }
    }

    #[test]
    fn rollback_triggered_flips_failed_to_reverted_then_stamps_terminals() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&crate::db::DispatchInsert {
                hostname: "ohm",
                rollout_id: "stable@abc12345",
                channel: "stable",
                wave: 0,
                target_closure_hash: "system-r1",
                target_channel_ref: "stable@abc12345",
                confirm_deadline: deadline,
            })
            .unwrap();
        db.rollout_state()
            .transition_host_state(
                "ohm",
                "stable@abc12345",
                HostRolloutState::Failed,
                HealthyMarker::Untouched,
                None,
            )
            .unwrap();
        assert_eq!(
            db.rollout_state()
                .host_state("ohm", "stable@abc12345")
                .unwrap()
                .as_deref(),
            Some("Failed"),
        );

        let req = rollback_report("ohm", Some("stable@abc12345"));
        apply_rollback_state_transition(&db, &req);

        assert_eq!(
            db.rollout_state()
                .host_state("ohm", "stable@abc12345")
                .unwrap()
                .as_deref(),
            Some("Reverted"),
        );
        let op = db
            .host_dispatch_state()
            .host_state("ohm")
            .unwrap()
            .expect("operational row present");
        assert_eq!(op.state, "rolled-back");
        let history = db
            .dispatch_history()
            .recent_for_host("ohm", 10)
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].terminal_state.as_deref(), Some("rolled-back"));
        assert!(history[0].terminal_at.is_some());
    }

    #[test]
    fn rollback_triggered_leaves_non_failed_states_untouched() {
        let db = fresh_db();
        for state in [
            HostRolloutState::Healthy,
            HostRolloutState::Soaked,
            HostRolloutState::ConfirmWindow,
            HostRolloutState::Activating,
            HostRolloutState::Converged,
        ] {
            let rollout = format!("stable@{}", state.as_db_str().to_lowercase());
            db.rollout_state()
                .transition_host_state("ohm", &rollout, state, HealthyMarker::Untouched, None)
                .unwrap();
            let req = rollback_report("ohm", Some(&rollout));
            apply_rollback_state_transition(&db, &req);
            assert_eq!(
                db.rollout_state()
                    .host_state("ohm", &rollout)
                    .unwrap()
                    .as_deref(),
                Some(state.as_db_str()),
                "{} should not flip to Reverted",
                state.as_db_str(),
            );
        }
    }

    #[test]
    fn rollback_triggered_without_rollout_is_a_noop() {
        let db = fresh_db();
        db.rollout_state()
            .transition_host_state(
                "ohm",
                "stable@abc12345",
                HostRolloutState::Failed,
                HealthyMarker::Untouched,
                None,
            )
            .unwrap();
        let req = rollback_report("ohm", None);
        apply_rollback_state_transition(&db, &req);
        assert_eq!(
            db.rollout_state()
                .host_state("ohm", "stable@abc12345")
                .unwrap()
                .as_deref(),
            Some("Failed"),
        );
    }

    #[test]
    fn non_rollback_events_do_not_transition_state() {
        let db = fresh_db();
        db.rollout_state()
            .transition_host_state(
                "ohm",
                "stable@abc12345",
                HostRolloutState::Failed,
                HealthyMarker::Untouched,
                None,
            )
            .unwrap();
        let req = ReportRequest {
            hostname: "ohm".into(),
            agent_version: "test".into(),
            occurred_at: Utc::now(),
            rollout: Some("stable@abc12345".into()),
            event: ReportEvent::RealiseFailed {
                closure_hash: "abc".into(),
                reason: "substituter 503".into(),
                signature: None,
            },
        };
        apply_rollback_state_transition(&db, &req);
        assert_eq!(
            db.rollout_state()
                .host_state("ohm", "stable@abc12345")
                .unwrap()
                .as_deref(),
            Some("Failed"),
            "non-RollbackTriggered events must not trigger Failed → Reverted",
        );
    }
}
