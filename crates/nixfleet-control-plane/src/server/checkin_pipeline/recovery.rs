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
/// Single transaction via `record_confirmed_dispatch_with_state`:
/// either both rows land or neither does. The orphan-confirm path
/// writes target_state = Healthy with healthy_since = now (the host
/// is healthy from this confirm).
fn synthesise_orphan_confirm_rows(
    db: &crate::db::Db,
    req: &ConfirmRequest,
    target_closure: &str,
    channel: &str,
) -> bool {
    let now = Utc::now();
    if let Err(err) = db
        .host_dispatch_state()
        .record_confirmed_dispatch_with_state(
            &req.hostname,
            &req.rollout,
            channel,
            req.wave,
            target_closure,
            &req.rollout,
            now,
            crate::state::HostRolloutState::Healthy,
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
    if let Err(err) = db
        .host_dispatch_state()
        .record_confirmed_dispatch_with_state(
            &req.hostname,
            &row.rollout_id,
            &host_decl.channel,
            row.wave,
            target_closure,
            &row.target_channel_ref,
            now,
            crate::state::HostRolloutState::Healthy,
            now,
        )
    {
        tracing::warn!(
            hostname = %req.hostname,
            rollout = %row.rollout_id,
            error = %err,
            "checkin-orphan recovery: atomic operational+Healthy write failed; \
             no rows committed (next checkin retries)",
        );
        return false;
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

/// True iff the agent-attested `last_confirmed_at` carries a signature
/// that verifies against the host's declared SSH host pubkey. False
/// (= drop the attestation) on any failure path: no signature posted,
/// no `pubkey` declared in fleet.nix, OpenSSH parse failure, base64
/// decode failure, ed25519 verification mismatch.
///
/// Logs at `warn` for "configured but failed verification" cases
/// (suspicious — operator wants to know) and `debug` for "not
/// configured" (no signature, no pubkey).
fn verify_attestation_signature(
    req: &CheckinRequest,
    rollout_id: &str,
    attested: DateTime<Utc>,
    host_decl: &nixfleet_proto::Host,
) -> bool {
    use base64::Engine;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let Some(sig_b64) = req.attestation_signature.as_deref() else {
        tracing::debug!(
            host = %req.hostname,
            "soak recovery: no attestation_signature on checkin; ignoring last_confirmed_at",
        );
        return false;
    };
    let Some(declared_pubkey) = host_decl.pubkey.as_deref() else {
        tracing::warn!(
            host = %req.hostname,
            "soak recovery: host has no `pubkey` declared in fleet.nix; ignoring attestation",
        );
        return false;
    };
    let pubkey_raw = match nixfleet_proto::host_key::ed25519_pubkey_raw_from_openssh(
        declared_pubkey,
    ) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(host = %req.hostname, error = %err, "soak recovery: declared OpenSSH pubkey parse failed");
            return false;
        }
    };
    let verifying_key = match VerifyingKey::from_bytes(&pubkey_raw) {
        Ok(vk) => vk,
        Err(err) => {
            tracing::warn!(host = %req.hostname, error = %err, "soak recovery: ed25519 VerifyingKey from declared pubkey failed");
            return false;
        }
    };
    let sig_bytes = match base64::engine::general_purpose::STANDARD.decode(sig_b64) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(host = %req.hostname, error = %err, "soak recovery: attestation signature base64 decode failed");
            return false;
        }
    };
    let signature = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(host = %req.hostname, error = %err, "soak recovery: attestation signature wire-shape invalid");
            return false;
        }
    };
    let payload = nixfleet_proto::evidence_signing::LastConfirmedAtSignedPayload {
        hostname: &req.hostname,
        rollout_id,
        last_confirmed_at: attested,
    };
    let canonical = match serde_jcs::to_vec(&payload) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(host = %req.hostname, error = %err, "soak recovery: JCS canonicalisation failed");
            return false;
        }
    };
    if let Err(err) = verifying_key.verify(&canonical, &signature) {
        tracing::warn!(host = %req.hostname, error = %err, "soak recovery: attestation signature mismatch — REPLAY ATTACK SUSPECTED OR STALE ROLLOUT_ID");
        return false;
    }
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

    // Verify the agent-attested last_confirmed_at against the host's
    // declared SSH host pubkey before applying. Without this, a
    // compromised host could replay an older timestamp + valid old
    // signature to short-circuit the soak gate from `soak_minutes`
    // down to zero. Falls back to "ignore the attestation" on every
    // failure path (no signature, no declared pubkey, parse failure,
    // verify failure) — same effect as the agent never having sent
    // last_confirmed_at, which is conservative.
    if !verify_attestation_signature(req, &rollout_id, attested, host_decl) {
        return;
    }

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

    if let Err(err) = db
        .host_dispatch_state()
        .record_confirmed_dispatch_with_state(
            &req.hostname,
            &rollout_id,
            &host_decl.channel,
            recovered_wave,
            target_closure,
            &rollout_id,
            now,
            crate::state::HostRolloutState::Healthy,
            stamp,
        )
    {
        tracing::warn!(
            hostname = %req.hostname,
            rollout = %rollout_id,
            error = %err,
            "soak-state recovery: atomic operational+Healthy write failed; \
             no rows committed (next checkin retries)",
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

        let confirmed_at = Utc::now() - chrono::Duration::minutes(5);
        db.host_dispatch_state()
            .record_confirmed_dispatch_with_state(
                "test-host",
                &expected_id,
                "stable",
                0,
                "system-r1",
                &expected_id,
                confirmed_at,
                crate::state::HostRolloutState::Healthy,
                confirmed_at,
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
        use super::super::tests::{sign_attestation, signed_attestation_fixture};
        let (fleet, sk, rollout_id) = signed_attestation_fixture("test-host", "system-r1");
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let attested = Utc::now() - chrono::Duration::minutes(3);
        let mut req = checkin_req_with_attestation("test-host", "system-r1", Some(attested));
        req.attestation_signature =
            Some(sign_attestation(&sk, "test-host", &rollout_id, attested));

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
        use super::super::tests::{sign_attestation, signed_attestation_fixture};
        let (fleet, sk, rollout_id) = signed_attestation_fixture("test-host", "system-r1");
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let now = Utc::now();
        let future = now + chrono::Duration::minutes(60);
        let mut req = checkin_req_with_attestation("test-host", "system-r1", Some(future));
        req.attestation_signature =
            Some(sign_attestation(&sk, "test-host", &rollout_id, future));

        recover_soak_state_from_attestation(&state, &req, now).await;

        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        let stamped = snap[0].last_healthy_since.get("test-host").unwrap();
        assert_eq!(
            stamped.timestamp(),
            now.timestamp(),
            "future-dated attestation must clamp to now",
        );
    }

    /// LOADBEARING regression for the 2026-05-01 finding folded into #43.
    /// A compromised host replaying an old `last_confirmed_at` with a
    /// signature minted for that older timestamp must NOT advance the
    /// soak clamp on a fresh attestation. The wave gate carries the
    /// per-rollout grouping; the signature-binding to (hostname,
    /// rollout_id, last_confirmed_at) is the cryptographic backstop —
    /// without it, a leaked agent state file is enough to short-circuit
    /// soak from `soak_minutes` to zero.
    #[tokio::test]
    async fn b_cp_recovery_rejects_unsigned_attestation_under_enforce() {
        // Same fleet shape with declared pubkey, but the agent posts
        // last_confirmed_at WITHOUT a signature. The CP must drop the
        // attestation entirely (no row written), preventing the silent
        // "fall back to no-binding" failure mode.
        use super::super::tests::signed_attestation_fixture;
        let (fleet, _sk, _rollout_id) =
            signed_attestation_fixture("test-host", "system-r1");
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let attested = Utc::now() - chrono::Duration::minutes(3);
        let req = checkin_req_with_attestation("test-host", "system-r1", Some(attested));
        // attestation_signature is None — the unsigned path.

        recover_soak_state_from_attestation(&state, &req, Utc::now()).await;

        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert!(
            snap.is_empty(),
            "unsigned attestation must NOT stamp a soak marker; got {snap:?}",
        );
    }

    /// Tampered signature: agent supplied a sig that doesn't verify
    /// against `hosts.<host>.pubkey`. Must be rejected.
    #[tokio::test]
    async fn b_cp_recovery_rejects_tampered_signature() {
        use super::super::tests::{sign_attestation, signed_attestation_fixture};
        let (fleet, sk, rollout_id) = signed_attestation_fixture("test-host", "system-r1");
        let (state, db) = state_with_fleet_and_db(fleet).await;
        let attested = Utc::now() - chrono::Duration::minutes(3);
        let mut req = checkin_req_with_attestation("test-host", "system-r1", Some(attested));
        // Sign for a different rollout_id — simulates either replay
        // across rollouts OR tampering. Either way the sig won't verify
        // against the rollout the CP computes from the fleet.
        req.attestation_signature = Some(sign_attestation(
            &sk,
            "test-host",
            "different-rollout-id",
            attested,
        ));
        let _ = rollout_id; // bound only to keep the binding obvious.

        recover_soak_state_from_attestation(&state, &req, Utc::now()).await;

        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert!(
            snap.is_empty(),
            "tampered/replay signature must NOT stamp a soak marker; got {snap:?}",
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
