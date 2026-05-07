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
    // Bundle C: cert CN may be canonical (`agent-<machineId>.<suffix>`)
    // while the agent still sends bare `machineId` in the body. Strip
    // back to bare for the equality check; legacy bare CNs pass
    // through unchanged.
    let machine_id = crate::auth::issuance::extract_machine_id(&cn, &state.agent_cn_suffix);
    if machine_id != req.hostname {
        tracing::warn!(
            cert_cn = %cn,
            machine_id = %machine_id,
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
    let machine_id = crate::auth::issuance::extract_machine_id(&cn, &state.agent_cn_suffix);
    if machine_id != req.hostname {
        tracing::warn!(
            cert_cn = %cn,
            machine_id = %machine_id,
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
        .map_err(super::route_error::internal("confirm: db update failed"))?;

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
    use std::sync::Arc;

    use super::AppState;

    pub(super) fn fleet_with_host(
        hostname: &str,
        closure: Option<&str>,
    ) -> nixfleet_proto::FleetResolved {
        use nixfleet_proto::testing::FleetBuilder;
        // Pin policy name to "default" — `rollback_signal` tests look it up by that key.
        let mut b = FleetBuilder::new()
            .channel("stable", "default")
            .host(hostname, "stable")
            .policy_strategy("default", "waves")
            .policy_waves("default", vec![])
            .meta(Meta {
                schema_version: 1,
                signed_at: Some(
                    DateTime::parse_from_rfc3339("2026-04-30T00:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
                ci_commit: Some("abc12345".to_string()),
                signature_algorithm: Some("ed25519".to_string()),
            });
        b = match closure {
            Some(c) => b.host_closure(hostname, c),
            None => b.host_no_closure(hostname),
        };
        let mut f = b.build();
        // host() auto-created policy "p"; tests only consult "default".
        f.rollout_policies.remove("p");
        f
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
            attestation_signature: None,
        health_probes: vec![],
        health_check_mode: None,
        }
    }

    /// Signed-attestation fixture for soak-recovery tests.
    ///
    /// Returns `(fleet_with_pubkey, signing_key, rollout_id)`. The fleet
    /// has `hosts.<hostname>.pubkey` populated with the OpenSSH-format
    /// pubkey of `signing_key`. Callers can then build a CheckinRequest,
    /// sign `LastConfirmedAtSignedPayload` against the returned key, and
    /// pass through `recover_soak_state_from_attestation` — closing the
    /// real binding (not a `pubkey: None` test escape hatch).
    pub(super) fn signed_attestation_fixture(
        hostname: &str,
        closure: &str,
    ) -> (
        nixfleet_proto::FleetResolved,
        ed25519_dalek::SigningKey,
        String,
    ) {
        use base64::Engine;
        use rand::RngCore;

        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let pubkey_raw = sk.verifying_key().to_bytes();

        // Build the OpenSSH pubkey line: `ssh-ed25519 <base64> test@host`.
        let mut blob = Vec::new();
        blob.extend_from_slice(&(b"ssh-ed25519".len() as u32).to_be_bytes());
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&(pubkey_raw.len() as u32).to_be_bytes());
        blob.extend_from_slice(&pubkey_raw);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        let openssh = format!("ssh-ed25519 {b64} test@host");

        let mut fleet = fleet_with_host(hostname, Some(closure));
        fleet.hosts.get_mut(hostname).unwrap().pubkey = Some(openssh);

        let rollout_id = expected_rollout_id_for(&fleet, "stable");
        (fleet, sk, rollout_id)
    }

    /// Sign a `LastConfirmedAtSignedPayload` and return base64 string.
    pub(super) fn sign_attestation(
        sk: &ed25519_dalek::SigningKey,
        hostname: &str,
        rollout_id: &str,
        last_confirmed_at: DateTime<Utc>,
    ) -> String {
        use base64::Engine;
        use ed25519_dalek::Signer;
        let payload = nixfleet_proto::evidence_signing::LastConfirmedAtSignedPayload {
            hostname,
            rollout_id,
            last_confirmed_at,
        };
        let canonical = serde_jcs::to_vec(&payload).unwrap();
        let sig = sk.sign(&canonical);
        base64::engine::general_purpose::STANDARD.encode(sig.to_bytes())
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
