//! Orphan-confirm + soak-state recovery for CP rebuild mid-flight.
//!
//! LOADBEARING: only synthesises state when the agent's claim matches the
//! verified fleet (closure AND rollout id). Closure mismatch / missing
//! snapshot / missing host declaration → fall through (caller decides 410).
//! Failures here are non-fatal — agent's local rollback still fires on 410.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use nixfleet_proto::agent_wire::{CheckinRequest, ConfirmRequest};

use super::super::state::AppState;

/// `true` absorbs the confirm; `false` falls through to 410.
pub(super) async fn try_recover_orphan_confirm(
    state: &Arc<AppState>,
    req: &ConfirmRequest,
) -> bool {
    let Some(db) = state.db.as_ref() else {
        return false;
    };
    let Some((target_closure, channel)) = validate_orphan_recovery(state, req).await else {
        return false;
    };
    synthesise_orphan_confirm_rows(db, req, &target_closure, &channel)
}

/// Returns `(target_closure, channel)` only when both closure AND rollout id match.
async fn validate_orphan_recovery(
    state: &AppState,
    req: &ConfirmRequest,
) -> Option<(String, String)> {
    let snap = state.verified_fleet.read().await.clone().or_else(|| {
        tracing::debug!(
            hostname = %req.hostname,
            "orphan-confirm recovery: no verified fleet snapshot yet",
        );
        None
    })?;
    let fleet = snap.fleet;
    let fleet_resolved_hash = snap.fleet_resolved_hash;
    let host_decl = fleet.hosts.get(&req.hostname).or_else(|| {
        tracing::debug!(
            hostname = %req.hostname,
            "orphan-confirm recovery: host not in verified fleet",
        );
        None
    })?;
    let target_closure = host_decl.closure_hash.as_ref().or_else(|| {
        tracing::debug!(
            hostname = %req.hostname,
            "orphan-confirm recovery: host has no declared closureHash",
        );
        None
    })?;
    if target_closure != &req.generation.closure_hash {
        tracing::info!(
            hostname = %req.hostname,
            rollout = %req.rollout,
            agent_closure = %req.generation.closure_hash,
            target_closure = %target_closure,
            "orphan-confirm recovery: closure_hash mismatch — genuine 410",
        );
        return None;
    }

    // FOOTGUN: closure match alone doesn't prove `req.rollout` is THIS
    // snapshot's id — content-addressed manifests mean a CI re-sign with the
    // same closure but different host_set/wave_layout produces a new rolloutId.
    let expected_rollout_id = match nixfleet_reconciler::compute_rollout_id_for_channel(
        &fleet,
        &fleet_resolved_hash,
        &host_decl.channel,
    ) {
        Ok(Some(id)) => id,
        Ok(None) | Err(_) => {
            tracing::info!(
                hostname = %req.hostname,
                "orphan-confirm recovery: rolloutId could not be projected — genuine 410",
            );
            return None;
        }
    };
    if expected_rollout_id != req.rollout {
        tracing::info!(
            hostname = %req.hostname,
            agent_rollout = %req.rollout,
            expected_rollout = %expected_rollout_id,
            "orphan-confirm recovery: rollout id mismatch — agent is on a stale rollout, genuine 410",
        );
        return None;
    }

    Some((target_closure.clone(), host_decl.channel.clone()))
}

/// Returns true iff operational + Healthy-marker rows landed atomically.
///
/// Pre-fix (split into two transactions): a failure between the
/// operational UPSERT and the host_rollout_state Healthy-marker
/// INSERT left host_dispatch_state at `confirmed` with NO matching
/// host_rollout_state row. The snapshot's LEFT JOIN then projected
/// the absence as "Healthy with NULL last_healthy_since"; the soak
/// timer in handle_wave (`if let Some(since) = last_healthy_since.get(host)`)
/// never fires; the host stayed Healthy forever and blocked the
/// whole rollout's wave promotion.
///
/// Now: one transaction. Either both rows land or neither — the
/// next checkin re-runs orphan-confirm cleanly.
fn synthesise_orphan_confirm_rows(
    db: &crate::db::Db,
    req: &ConfirmRequest,
    target_closure: &str,
    channel: &str,
) -> bool {
    let now = Utc::now();
    if let Err(err) = db
        .host_dispatch_state()
        .record_confirmed_dispatch_with_healthy_marker(
            &req.hostname,
            &req.rollout,
            channel,
            req.wave,
            target_closure,
            &req.rollout,
            now,
        )
    {
        tracing::warn!(
            hostname = %req.hostname,
            rollout = %req.rollout,
            error = %err,
            "orphan-confirm recovery: atomic operational+Healthy write failed; \
             no rows committed (next checkin retries)",
        );
        return false;
    }
    tracing::info!(
        target: "confirm",
        hostname = %req.hostname,
        rollout = %req.rollout,
        target_closure = %target_closure,
        "orphan-confirm recovery: synthesised confirmed host_dispatch_state row + Healthy marker (atomic)",
    );
    true
}

/// Revives `pending`/`rolled-back` rows to `confirmed` when agent reports matching closure + rollout_id.
pub(super) async fn try_recover_pending_from_checkin(
    state: &Arc<AppState>,
    req: &CheckinRequest,
) -> bool {
    let Some(db) = state.db.as_ref() else {
        return false;
    };

    let row = match db.host_dispatch_state().host_state(&req.hostname) {
        Ok(Some(r)) => r,
        Ok(None) => return false,
        Err(err) => {
            tracing::warn!(
                hostname = %req.hostname,
                error = %err,
                "checkin-orphan recovery: host_state query failed",
            );
            return false;
        }
    };
    if row.state != "pending" && row.state != "rolled-back" {
        return false;
    }

    let Some(snap) = state.verified_fleet.read().await.clone() else {
        return false;
    };
    let fleet = snap.fleet;
    let fleet_resolved_hash = snap.fleet_resolved_hash;
    let Some(host_decl) = fleet.hosts.get(&req.hostname) else {
        return false;
    };
    let Some(target_closure) = host_decl.closure_hash.as_ref() else {
        return false;
    };
    if target_closure != &req.current_generation.closure_hash {
        return false;
    }

    let expected_rollout_id = match nixfleet_reconciler::compute_rollout_id_for_channel(
        &fleet,
        &fleet_resolved_hash,
        &host_decl.channel,
    ) {
        Ok(Some(id)) => id,
        Ok(None) | Err(_) => return false,
    };
    if expected_rollout_id != row.rollout_id {
        return false;
    }

    let now = Utc::now();
    if let Err(err) = db.host_dispatch_state().record_confirmed_dispatch(
        &req.hostname,
        &row.rollout_id,
        &host_decl.channel,
        row.wave,
        target_closure,
        &row.target_channel_ref,
        now,
    ) {
        tracing::warn!(
            hostname = %req.hostname,
            rollout = %row.rollout_id,
            error = %err,
            "checkin-orphan recovery: record_confirmed_dispatch failed",
        );
        return false;
    }
    if let Err(err) = db.rollout_state().transition_host_state(
        &req.hostname,
        &row.rollout_id,
        crate::state::HostRolloutState::Healthy,
        crate::state::HealthyMarker::Set(now),
        None,
    ) {
        tracing::warn!(
            hostname = %req.hostname,
            rollout = %row.rollout_id,
            error = %err,
            "checkin-orphan recovery: transition to Healthy failed (operational row already revived)",
        );
    }
    tracing::info!(
        target: "confirm",
        hostname = %req.hostname,
        rollout = %row.rollout_id,
        prior_state = %row.state,
        target_closure = %target_closure,
        "checkin-orphan recovery: agent on target, revived dispatch row to confirmed",
    );
    true
}

/// Stamp `last_healthy_since` from `min(now, attested)` when no host_rollout_state row exists.
pub(super) async fn recover_soak_state_from_attestation(
    state: &Arc<AppState>,
    req: &CheckinRequest,
    now: DateTime<Utc>,
) {
    let Some(attested) = req.last_confirmed_at else {
        return;
    };
    let Some(db) = state.db.as_ref() else {
        return;
    };
    let Some(snap) = state.verified_fleet.read().await.clone() else {
        return;
    };
    let fleet = snap.fleet;
    let fleet_resolved_hash = snap.fleet_resolved_hash;
    let Some(host_decl) = fleet.hosts.get(&req.hostname) else {
        return;
    };
    let Some(target_closure) = host_decl.closure_hash.as_ref() else {
        return;
    };
    if target_closure != &req.current_generation.closure_hash {
        return;
    }

    // Must match dispatch's projection so per-rollout event grouping lines up.
    let rollout_id = match nixfleet_reconciler::compute_rollout_id_for_channel(
        &fleet,
        &fleet_resolved_hash,
        &host_decl.channel,
    ) {
        Ok(Some(id)) => id,
        Ok(None) | Err(_) => return,
    };

    match db
        .rollout_state()
        .host_rollout_state_exists(&req.hostname, &rollout_id)
    {
        Ok(true) => return,
        Ok(false) => {}
        Err(err) => {
            tracing::warn!(
                hostname = %req.hostname,
                rollout = %rollout_id,
                error = %err,
                "soak-state recovery: existence check failed",
            );
            return;
        }
    }

    let stamp = std::cmp::min(now, attested);

    // Pull wave from the agent's last_evaluated_target; hard-coding 0 corrupts the audit row for wave ≥1 hosts.
    let recovered_wave = req
        .last_evaluated_target
        .as_ref()
        .and_then(|t| t.wave_index)
        .unwrap_or(0);

    if let Err(err) = db.host_dispatch_state().record_confirmed_dispatch(
        &req.hostname,
        &rollout_id,
        &host_decl.channel,
        recovered_wave,
        target_closure,
        &rollout_id,
        now,
    ) {
        tracing::warn!(
            hostname = %req.hostname,
            rollout = %rollout_id,
            error = %err,
            "soak-state recovery: record_confirmed_dispatch failed",
        );
        return;
    }
    if let Err(err) = db.rollout_state().transition_host_state(
        &req.hostname,
        &rollout_id,
        crate::state::HostRolloutState::Healthy,
        crate::state::HealthyMarker::Set(stamp),
        None,
    ) {
        tracing::warn!(
            hostname = %req.hostname,
            rollout = %rollout_id,
            error = %err,
            "soak-state recovery: transition to Healthy failed (synthetic confirmed row already inserted)",
        );
        return;
    }
    tracing::info!(
        target: "soak",
        hostname = %req.hostname,
        rollout = %rollout_id,
        attested = %attested.to_rfc3339(),
        stamped = %stamp.to_rfc3339(),
        "soak-state recovery: stamped last_healthy_since from agent attestation",
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::{
        checkin_req_with_attestation, confirm_req, expected_rollout_id_for, fleet_with_host,
        state_with_fleet_and_db,
    };
    use super::*;
    use crate::db::Db;
    use std::sync::Arc;

    fn insert_dispatch_row(
        db: &Db,
        hostname: &str,
        rollout_id: &str,
        target_closure: &str,
        state: &str,
    ) {
        let target_channel_ref = rollout_id.to_string();
        let row = crate::db::DispatchInsert {
            hostname,
            rollout_id,
            channel: "stable",
            wave: 0,
            target_closure_hash: target_closure,
            target_channel_ref: &target_channel_ref,
            confirm_deadline: Utc::now(),
        };
        db.host_dispatch_state().record_dispatch(&row).unwrap();
        if state == "rolled-back" {
            db.host_dispatch_state()
                .mark_rolled_back(&[(hostname.to_string(), rollout_id.to_string())])
                .unwrap();
        }
    }

    #[tokio::test]
    async fn checkin_recovery_revives_rolled_back_when_agent_on_target() {
        let fleet = fleet_with_host("test-host", Some("system-r1"));
        let expected_id = expected_rollout_id_for(&fleet, "stable");
        let (state, db) = state_with_fleet_and_db(fleet).await;

        insert_dispatch_row(&db, "test-host", &expected_id, "system-r1", "rolled-back");

        let req = checkin_req_with_attestation("test-host", "system-r1", None);
        assert!(
            try_recover_pending_from_checkin(&state, &req).await,
            "rolled-back row + on-target agent should revive",
        );

        let row = db
            .host_dispatch_state()
            .host_state("test-host")
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "confirmed");
        assert!(row.confirmed_at.is_some());
    }

    #[tokio::test]
    async fn checkin_recovery_revives_pending_before_deadline() {
        let fleet = fleet_with_host("test-host", Some("system-r1"));
        let expected_id = expected_rollout_id_for(&fleet, "stable");
        let (state, db) = state_with_fleet_and_db(fleet).await;

        insert_dispatch_row(&db, "test-host", &expected_id, "system-r1", "pending");

        let req = checkin_req_with_attestation("test-host", "system-r1", None);
        assert!(try_recover_pending_from_checkin(&state, &req).await);

        let row = db
            .host_dispatch_state()
            .host_state("test-host")
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "confirmed");
    }

    #[tokio::test]
    async fn checkin_recovery_skips_when_agent_on_wrong_closure() {
        let fleet = fleet_with_host("test-host", Some("system-r1"));
        let expected_id = expected_rollout_id_for(&fleet, "stable");
        let (state, db) = state_with_fleet_and_db(fleet).await;

        insert_dispatch_row(&db, "test-host", &expected_id, "system-r1", "rolled-back");

        let req = checkin_req_with_attestation("test-host", "stale-closure", None);
        assert!(
            !try_recover_pending_from_checkin(&state, &req).await,
            "agent on wrong closure must not revive",
        );

        let row = db
            .host_dispatch_state()
            .host_state("test-host")
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "rolled-back", "row must remain rolled-back");
    }

    #[tokio::test]
    async fn checkin_recovery_skips_confirmed_rows() {
        let fleet = fleet_with_host("test-host", Some("system-r1"));
        let expected_id = expected_rollout_id_for(&fleet, "stable");
        let (state, db) = state_with_fleet_and_db(fleet).await;

        db.host_dispatch_state()
            .record_confirmed_dispatch(
                "test-host",
                &expected_id,
                "stable",
                0,
                "system-r1",
                &expected_id,
                Utc::now() - chrono::Duration::minutes(5),
            )
            .unwrap();

        let req = checkin_req_with_attestation("test-host", "system-r1", None);
        assert!(
            !try_recover_pending_from_checkin(&state, &req).await,
            "already-confirmed row should not retrigger recovery",
        );
    }

    #[tokio::test]
    async fn checkin_recovery_skips_when_no_row_exists() {
        let fleet = fleet_with_host("test-host", Some("system-r1"));
        let (state, _db) = state_with_fleet_and_db(fleet).await;
        let req = checkin_req_with_attestation("test-host", "system-r1", None);
        assert!(!try_recover_pending_from_checkin(&state, &req).await);
    }

    #[tokio::test]
    async fn orphan_recovery_succeeds_when_closure_matches() {
        // Happy path. CP rebuilt mid-flight; agent posts a confirm
        // whose closure matches the verified target. The recovery
        // path synthesises a confirmed row + Healthy marker and
        // returns true so the handler emits 204 instead of forcing a
        // local rollback.
        let fleet = fleet_with_host("test-host", Some("target-system-r1"));
        let expected_id = expected_rollout_id_for(&fleet, "stable");
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let req = confirm_req("test-host", &expected_id, "target-system-r1");

        assert!(
            try_recover_orphan_confirm(&state, &req).await,
            "matching closure should recover",
        );

        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].rollout_id, expected_id);
        assert_eq!(snap[0].target_closure_hash, "target-system-r1");
        // Healthy marker stamped in the same call.
        let healthy = db
            .rollout_state()
            .healthy_rollouts_for_host("test-host")
            .unwrap();
        assert_eq!(healthy.len(), 1);
    }

    #[tokio::test]
    async fn orphan_recovery_rejects_closure_mismatch() {
        // Genuine wrong-rollout case. Agent claims to have
        // activated something the fleet doesn't agree with — must
        // fall through to 410.
        let fleet = fleet_with_host("test-host", Some("target-system-r1"));
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let req = confirm_req("test-host", "stable@evil", "target-system-different");

        assert!(
            !try_recover_orphan_confirm(&state, &req).await,
            "mismatched closure must not recover",
        );
        assert!(db
            .host_dispatch_state()
            .active_rollouts_snapshot()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn orphan_recovery_rejects_when_host_not_in_fleet() {
        // Agent claims to be a host the verified fleet doesn't
        // know about — recovery refuses to invent state for it.
        let fleet = fleet_with_host("known-host", Some("target"));
        let (state, _db) = state_with_fleet_and_db(fleet).await;
        let req = confirm_req("rogue-host", "stable@abc", "target");

        assert!(!try_recover_orphan_confirm(&state, &req).await);
    }

    #[tokio::test]
    async fn orphan_recovery_rejects_when_no_verified_fleet() {
        // First-boot CP with no verified snapshot yet — recovery
        // can't validate the agent's claim, so it stays
        // conservative.
        let db = Arc::new(Db::open_in_memory().unwrap());
        db.migrate().unwrap();
        let state = Arc::new(AppState {
            db: Some(Arc::clone(&db)),
            ..AppState::default()
        });
        let req = confirm_req("test-host", "stable@abc", "target");
        assert!(!try_recover_orphan_confirm(&state, &req).await);
    }

    #[tokio::test]
    async fn orphan_recovery_rejects_when_host_lacks_closure_declaration() {
        // The fleet lists the host but with no closureHash (CI
        // didn't produce one). Without a target to validate
        // against, recovery refuses.
        let fleet = fleet_with_host("test-host", None);
        let (state, _db) = state_with_fleet_and_db(fleet).await;
        let req = confirm_req("test-host", "stable@abc", "anything");
        assert!(!try_recover_orphan_confirm(&state, &req).await);
    }

    #[tokio::test]
    async fn b_cp_recovery_stamps_attested_timestamp_when_no_existing_row() {
        // Happy path. Host is converged on the verified target, no
        // host_rollout_state row exists (CP rebuilt), attestation
        // arrives → stamp last_healthy_since.
        let fleet = fleet_with_host("test-host", Some("system-r1"));
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let attested = Utc::now() - chrono::Duration::minutes(3);
        let req = checkin_req_with_attestation("test-host", "system-r1", Some(attested));

        recover_soak_state_from_attestation(&state, &req, Utc::now()).await;

        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert_eq!(
            snap.len(),
            1,
            "snapshot should contain the recovered rollout"
        );
        let stamped = snap[0]
            .last_healthy_since
            .get("test-host")
            .expect("host has stamped soak marker");
        assert_eq!(
            stamped.timestamp(),
            attested.timestamp(),
            "stamp must clamp to min(now, attested) — attested is in the past so it wins",
        );
    }

    #[tokio::test]
    async fn b_cp_recovery_clamps_future_attestation_to_now() {
        // Defensive clamp: a clock-skewed agent claims attestation
        // in the future. CP must clamp to `now` so the agent can't
        // short-circuit the soak gate.
        let fleet = fleet_with_host("test-host", Some("system-r1"));
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let now = Utc::now();
        let future = now + chrono::Duration::minutes(60);
        let req = checkin_req_with_attestation("test-host", "system-r1", Some(future));

        recover_soak_state_from_attestation(&state, &req, now).await;

        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        let stamped = snap[0].last_healthy_since.get("test-host").unwrap();
        assert_eq!(
            stamped.timestamp(),
            now.timestamp(),
            "future-dated attestation must clamp to now",
        );
    }

    #[tokio::test]
    async fn b_cp_recovery_skips_when_host_not_converged() {
        // Host reports a closure that doesn't match the verified
        // target — it's still rolling out, not in the recovery
        // window. Skip.
        let fleet = fleet_with_host("test-host", Some("target-r1"));
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let attested = Utc::now() - chrono::Duration::minutes(1);
        let req = checkin_req_with_attestation("test-host", "different-closure", Some(attested));

        recover_soak_state_from_attestation(&state, &req, Utc::now()).await;
        assert!(db
            .host_dispatch_state()
            .active_rollouts_snapshot()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn b_cp_recovery_skips_when_host_state_already_exists() {
        // host_rollout_state already has a row. Re-attestation must
        // NOT overwrite — the existing row is authoritative.
        let fleet = fleet_with_host("test-host", Some("system-r1"));
        let expected_id = expected_rollout_id_for(&fleet, "stable");
        let (state, db) = state_with_fleet_and_db(fleet).await;

        // Pre-populate a Healthy row for the rolloutId the host
        // would derive from the projected manifest.
        let original = Utc::now() - chrono::Duration::seconds(5);
        db.rollout_state()
            .transition_host_state(
                "test-host",
                &expected_id,
                crate::state::HostRolloutState::Healthy,
                crate::state::HealthyMarker::Set(original),
                None,
            )
            .unwrap();

        let attested = Utc::now() - chrono::Duration::hours(2);
        let req = checkin_req_with_attestation("test-host", "system-r1", Some(attested));

        recover_soak_state_from_attestation(&state, &req, Utc::now()).await;

        let map = db
            .rollout_state()
            .host_soak_state_for_rollout(&expected_id)
            .unwrap();
        let stamped = map.get("test-host").unwrap();
        assert_eq!(
            stamped.timestamp(),
            original.timestamp(),
            "existing row must not be overwritten by attestation",
        );
    }

    #[tokio::test]
    async fn b_cp_recovery_noop_for_legacy_agents_without_attestation() {
        // Legacy agent — no last_confirmed_at. CP behaviour is
        // unchanged: no soak-state writes happen.
        let fleet = fleet_with_host("test-host", Some("system-r1"));
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let req = checkin_req_with_attestation("test-host", "system-r1", None);

        recover_soak_state_from_attestation(&state, &req, Utc::now()).await;
        assert!(db
            .host_dispatch_state()
            .active_rollouts_snapshot()
            .unwrap()
            .is_empty());
    }
}
