//! `/v1/agent/checkin` and `/v1/agent/confirm` handlers.

mod dispatch_target;
mod recovery;
mod rollback_signal;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use chrono::Utc;
use nixfleet_proto::agent_wire::{CheckinRequest, CheckinResponse, ConfirmRequest};

use super::middleware::AuthenticatedCn;
use super::state::{AppState, HostCheckinRecord, NEXT_CHECKIN_SECS};

/// `POST /v1/agent/checkin`.
pub(super) async fn checkin(
    State(state): State<Arc<AppState>>,
    Extension(cn): Extension<AuthenticatedCn>,
    Json(req): Json<CheckinRequest>,
) -> Result<Json<CheckinResponse>, StatusCode> {
    let cn = cn.into_string();
    if cn != req.hostname {
        tracing::warn!(
            cert_cn = %cn,
            body_hostname = %req.hostname,
            "checkin rejected: cert CN does not match body hostname"
        );
        return Err(StatusCode::FORBIDDEN);
    }

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

    let now = Utc::now();
    let record = HostCheckinRecord {
        last_checkin: now,
        checkin: req.clone(),
    };
    state
        .host_checkins
        .write()
        .await
        .insert(req.hostname.clone(), record);

    rollback_signal::clear_left_healthy_for_checkin(&state, &req).await;
    recovery::recover_soak_state_from_attestation(&state, &req, now).await;
    // Revives confirmed when deadline fired before confirm arrived (split-brain race).
    let _ = recovery::try_recover_pending_from_checkin(&state, &req).await;

    let target = dispatch_target::dispatch_target_for_checkin(&state, &req, now).await;
    let rollback = rollback_signal::rollback_signal_for_checkin(&state, &req).await;

    Ok(Json(CheckinResponse {
        target,
        rollback,
        next_checkin_secs: NEXT_CHECKIN_SECS,
    }))
}

/// `POST /v1/agent/confirm`: 204 on flip or orphan-recovery; 410 on mismatch; 503 with no DB.
pub(super) async fn confirm(
    State(state): State<Arc<AppState>>,
    Extension(cn): Extension<AuthenticatedCn>,
    Json(req): Json<ConfirmRequest>,
) -> Result<Response, StatusCode> {
    let cn = cn.into_string();
    if cn != req.hostname {
        tracing::warn!(
            cert_cn = %cn,
            body_hostname = %req.hostname,
            "confirm rejected: cert CN does not match body hostname"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    let db = state.db.as_ref().ok_or_else(|| {
        tracing::warn!("confirm: no db configured — endpoint unusable");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let updated = db
        .host_dispatch_state()
        .confirm(&req.hostname, &req.rollout)
        .map_err(|err| {
            tracing::error!(error = %err, "confirm: db update failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if updated == 0 {
        if recovery::try_recover_orphan_confirm(&state, &req).await {
        } else {
            // Inline mark_rolled_back so agent's 410-driven local rollback and CP view converge in one round-trip.
            if let Err(err) = db.host_dispatch_state().mark_rolled_back(&[(
                req.hostname.clone(),
                req.rollout.clone(),
            )]) {
                tracing::warn!(
                    hostname = %req.hostname,
                    rollout = %req.rollout,
                    error = %err,
                    "confirm-410: inline mark_rolled_back failed; rollback_timer will retry",
                );
            }
            tracing::info!(
                hostname = %req.hostname,
                rollout = %req.rollout,
                "confirm: no matching pending row + no recoverable orphan — returning 410"
            );
            return Ok(Response::builder()
                .status(StatusCode::GONE)
                .body(Body::from(""))
                .expect("Response::builder with valid status + body is infallible"));
        }
    } else {
        // transition_host_state SELECTs prev under the same lock and
        // emits the actual prev→new transition counter from inside —
        // no synthetic "(any)" tag needed at this call site.
        if let Err(err) = db.rollout_state().transition_host_state(
            &req.hostname,
            &req.rollout,
            crate::state::HostRolloutState::Healthy,
            crate::state::HealthyMarker::Set(Utc::now()),
            None,
        ) {
            tracing::warn!(
                hostname = %req.hostname,
                rollout = %req.rollout,
                error = %err,
                "confirm: transition to Healthy failed; soak timer will skip this host",
            );
        }
    }

    tracing::info!(
        target: "confirm",
        hostname = %req.hostname,
        rollout = %req.rollout,
        wave = req.wave,
        closure_hash = %req.generation.closure_hash,
        "confirm received"
    );
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::from(""))
        .expect("Response::builder with valid status + body is infallible"))
}

#[cfg(test)]
pub(super) mod tests {
    use crate::db::Db;
    use chrono::{DateTime, Utc};
    use nixfleet_proto::agent_wire::{ConfirmRequest, GenerationRef};
    use nixfleet_proto::fleet_resolved::Meta;
    use nixfleet_proto::{Channel, Compliance, Host};
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::AppState;

    pub(super) fn fleet_with_host(
        hostname: &str,
        closure: Option<&str>,
    ) -> nixfleet_proto::FleetResolved {
        let mut hosts = HashMap::new();
        hosts.insert(
            hostname.to_string(),
            Host {
                system: "x86_64-linux".to_string(),
                tags: vec![],
                channel: "stable".to_string(),
                closure_hash: closure.map(String::from),
                pubkey: None,
            },
        );
        let mut channels = HashMap::new();
        channels.insert(
            "stable".to_string(),
            Channel {
                rollout_policy: "default".to_string(),
                reconcile_interval_minutes: 5,
                freshness_window: 60,
                signing_interval_minutes: 30,
                compliance: Compliance {
                    frameworks: vec![],
                    mode: "disabled".to_string(),
                },
            },
        );
        let mut rollout_policies = HashMap::new();
        rollout_policies.insert(
            "default".to_string(),
            nixfleet_proto::RolloutPolicy {
                strategy: "waves".to_string(),
                waves: vec![],
                health_gate: nixfleet_proto::HealthGate::default(),
                on_health_failure: nixfleet_proto::OnHealthFailure::Halt,
            },
        );
        nixfleet_proto::FleetResolved {
            schema_version: 1,
            hosts,
            channels,
            rollout_policies,
            waves: HashMap::new(),
            edges: vec![],
            channel_edges: vec![],
            disruption_budgets: vec![],
            meta: Meta {
                schema_version: 1,
                signed_at: Some(
                    DateTime::parse_from_rfc3339("2026-04-30T00:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
                ci_commit: Some("abc12345".to_string()),
                signature_algorithm: Some("ed25519".to_string()),
            },
        }
    }

    pub(super) const TEST_FLEET_HASH: &str =
        "0000000000000000000000000000000000000000000000000000000000000000";

    pub(super) fn expected_rollout_id_for(
        fleet: &nixfleet_proto::FleetResolved,
        channel: &str,
    ) -> String {
        nixfleet_reconciler::compute_rollout_id_for_channel(fleet, TEST_FLEET_HASH, channel)
            .expect("projection succeeds")
            .expect("non-empty channel")
    }

    pub(super) fn checkin_req_with_attestation(
        hostname: &str,
        closure: &str,
        attested: Option<DateTime<Utc>>,
    ) -> nixfleet_proto::agent_wire::CheckinRequest {
        nixfleet_proto::agent_wire::CheckinRequest {
            hostname: hostname.to_string(),
            agent_version: "test".into(),
            current_generation: GenerationRef {
                closure_hash: closure.to_string(),
                channel_ref: None,
                boot_id: "boot".to_string(),
            },
            pending_generation: None,
            last_evaluated_target: None,
            last_fetch_outcome: None,
            uptime_secs: None,
            last_confirmed_at: attested,
        }
    }

    pub(super) fn confirm_req(hostname: &str, rollout: &str, closure: &str) -> ConfirmRequest {
        ConfirmRequest {
            hostname: hostname.to_string(),
            rollout: rollout.to_string(),
            wave: 0,
            generation: GenerationRef {
                closure_hash: closure.to_string(),
                channel_ref: None,
                boot_id: "boot".to_string(),
            },
        }
    }

    pub(super) async fn state_with_fleet_and_db(
        fleet: nixfleet_proto::FleetResolved,
    ) -> (Arc<AppState>, Arc<Db>) {
        let db = Arc::new(Db::open_in_memory().unwrap());
        db.migrate().unwrap();
        let state = Arc::new(AppState {
            db: Some(Arc::clone(&db)),
            verified_fleet: Arc::new(tokio::sync::RwLock::new(Some(
                crate::server::VerifiedFleetSnapshot {
                    fleet: Arc::new(fleet),
                    fleet_resolved_hash: TEST_FLEET_HASH.to_string(),
                },
            ))),
            ..AppState::default()
        });
        (state, db)
    }
}
