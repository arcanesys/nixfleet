//! Per-checkin dispatch: gate, decide, persist operational + audit rows.

use chrono::{DateTime, Utc};
use nixfleet_proto::agent_wire::CheckinRequest;

use super::super::state::AppState;

/// Failures log + return None; transient errors must not surface as 500 to the agent.
pub(super) async fn dispatch_target_for_checkin(
    state: &AppState,
    req: &CheckinRequest,
    now: DateTime<Utc>,
) -> Option<nixfleet_proto::agent_wire::EvaluatedTarget> {
    let db = state.db.as_ref()?;
    let snap = state.verified_fleet.read().await.clone()?;
    let fleet = snap.fleet;
    let fleet_resolved_hash = snap.fleet_resolved_hash;

    // Fleet-level gates shared with the reconciler (parity in
    // nixfleet_reconciler::gates). Same Observed snapshot ⇒ same conclusion.
    if let Some(host) = fleet.hosts.get(&req.hostname) {
        let observed =
            crate::observed_view::build_for_gates_from_state(state, &fleet, &fleet_resolved_hash)
                .await;
        // Rollout for the host's channel; None ⇒ no rollout recorded yet,
        // handled by per-gate semantics.
        let rollout_for_host = observed
            .active_rollouts
            .iter()
            .find(|r| r.channel == host.channel);
        let empty_emitted_opens: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let input = nixfleet_reconciler::gates::GateInput {
            fleet: &fleet,
            observed: &observed,
            rollout: rollout_for_host,
            host: &req.hostname,
            now,
            emitted_opens_in_tick: &empty_emitted_opens,
            // Conservative on missing predecessor (fresh-boot protection).
            mode: nixfleet_reconciler::gates::GateMode::Dispatch,
        };
        if let Some(block) = nixfleet_reconciler::gates::evaluate_for_host(&input) {
            crate::metrics::record_gate_block(block.discriminator());
            tracing::info!(
                target: "dispatch",
                hostname = %req.hostname,
                gate = %block.discriminator(),
                reason = %block.reason(),
                "dispatch: held by fleet-level gate",
            );
            return None;
        }
    }

    let pending_for_host = match db
        .host_dispatch_state()
        .pending_dispatch_exists(&req.hostname)
    {
        Ok(b) => b,
        Err(err) => {
            tracing::error!(
                hostname = %req.hostname,
                error = %err,
                "dispatch: pending_dispatch_exists query failed",
            );
            return None;
        }
    };

    // Look up persisted `current_wave` for the wave-promotion gate. Same
    // fleet snapshot as decide_target ⇒ rolloutIds match.
    let current_wave: Option<u32> = if let Some(host) = fleet.hosts.get(&req.hostname) {
        match nixfleet_reconciler::compute_rollout_id_for_channel(
            &fleet,
            &fleet_resolved_hash,
            &host.channel,
        ) {
            Ok(Some(rid)) => match db.rollouts().current_wave(&rid) {
                Ok(w) => w,
                Err(err) => {
                    tracing::warn!(
                        rollout = %rid,
                        error = %err,
                        "dispatch: rollouts.current_wave read failed; gate-blocking is unreachable, defaulting to 0",
                    );
                    Some(0)
                }
            },
            _ => None,
        }
    } else {
        None
    };

    let decision = crate::dispatch::decide_target(
        &req.hostname,
        req,
        &fleet,
        &fleet_resolved_hash,
        pending_for_host,
        now,
        state.confirm_deadline_secs as u32,
        current_wave,
    );
    match decision {
        crate::dispatch::Decision::Dispatch {
            target,
            rollout_id,
            wave_index,
        } => {
            // Persist channel explicitly: content-addressed rolloutIds
            // don't encode it.
            let channel = fleet
                .hosts
                .get(&req.hostname)
                .map(|h| h.channel.clone())
                .unwrap_or_default();
            record_dispatched_target(
                db,
                &req.hostname,
                &rollout_id,
                &channel,
                wave_index,
                target,
                state,
                now,
            )
        }
        crate::dispatch::Decision::Converged => {
            // Host already on declared target. Materialise rollout-layer
            // rows without sending a target - CP's view of "host is on
            // rollout R" becomes authoritative even though the agent never
            // sees a confirm.
            record_converged_at_dispatch(db, req, &fleet, &fleet_resolved_hash, now);
            None
        }
        crate::dispatch::Decision::WaveNotReached => {
            tracing::debug!(
                target: "dispatch",
                hostname = %req.hostname,
                current_wave = ?current_wave,
                "dispatch: wave-promotion gate held target - host's wave hasn't been promoted yet",
            );
            None
        }
        other => {
            tracing::debug!(
                target: "dispatch",
                hostname = %req.hostname,
                decision = ?other,
                "dispatch: no target",
            );
            None
        }
    }
}

/// LOADBEARING: each row materialisation is best-effort + idempotent.
/// Failures delay reconciler convergence but don't break the agent.
fn record_converged_at_dispatch(
    db: &crate::db::Db,
    req: &CheckinRequest,
    fleet: &nixfleet_proto::FleetResolved,
    fleet_resolved_hash: &str,
    now: DateTime<Utc>,
) {
    let host_decl = match fleet.hosts.get(&req.hostname) {
        Some(h) => h,
        None => return,
    };
    let target_closure = match host_decl.closure_hash.as_ref() {
        Some(h) => h,
        None => return,
    };
    let rollout_id = match nixfleet_reconciler::compute_rollout_id_for_channel(
        fleet,
        fleet_resolved_hash,
        &host_decl.channel,
    ) {
        Ok(Some(id)) => id,
        _ => return,
    };
    let wave = wave_index_for(fleet, &host_decl.channel, &req.hostname).unwrap_or(0);
    let target_channel_ref = rollout_id.clone();

    // Idempotent; channel-refs poll also calls it.
    if let Err(err) = db
        .rollouts()
        .record_active_rollout(&rollout_id, &host_decl.channel)
    {
        tracing::warn!(
            rollout = %rollout_id,
            channel = %host_decl.channel,
            error = %err,
            "converged-at-dispatch: record_active_rollout failed (non-fatal)",
        );
    }

    // dispatch_history is APPEND-ONLY (autoincrement id, no UNIQUE). Three
    // states to disambiguate:
    //   1. Already Converged ⇒ no-op (don't leak duplicate history rows).
    //   2. host_rollout_state exists but != Converged ⇒ let reconciler
    //      advance through the soak window naturally.
    //   3. No row ⇒ full atomic materialisation directly to Converged.
    let existing_state = match db.rollout_state().host_state(&req.hostname, &rollout_id) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(
                hostname = %req.hostname,
                rollout = %rollout_id,
                error = %err,
                "converged-at-dispatch: host_state probe failed (non-fatal; will re-attempt materialization)",
            );
            None
        }
    };

    if existing_state.as_deref() == Some("Converged") {
        return;
    }

    // Case 2: respect the operator's soakMinutes window. Reconciler runs
    // the natural Healthy → Soaked → Converged progression.
    if existing_state.is_some() {
        return;
    }

    // Case 3: host was on target before any dispatch attempt. Atomic
    // materialisation directly to Converged is correct - no transient state
    // to ride out.
    if let Err(err) = db
        .host_dispatch_state()
        .record_confirmed_dispatch_with_state(
            &req.hostname,
            &rollout_id,
            &host_decl.channel,
            wave,
            target_closure,
            &target_channel_ref,
            now,
            crate::state::HostRolloutState::Converged,
            now,
        )
    {
        tracing::warn!(
            hostname = %req.hostname,
            rollout = %rollout_id,
            error = %err,
            "converged-at-dispatch: atomic operational+Converged write failed; \
             no rows committed (next checkin retries)",
        );
        return;
    }

    tracing::info!(
        target: "dispatch",
        hostname = %req.hostname,
        rollout = %rollout_id,
        target_closure = %target_closure,
        "dispatch: host converged-at-dispatch (rollout-layer state materialized)",
    );
}

pub(super) fn wave_index_for(
    fleet: &nixfleet_proto::FleetResolved,
    channel_name: &str,
    hostname: &str,
) -> Option<u32> {
    fleet.waves.get(channel_name).and_then(|waves| {
        waves
            .iter()
            .position(|w| w.hosts.iter().any(|h| h == hostname))
            .map(|i| i as u32)
    })
}

/// Returns None on DB failure: the row is the idempotency anchor.
#[allow(clippy::too_many_arguments)]
fn record_dispatched_target(
    db: &crate::db::Db,
    hostname: &str,
    rollout_id: &str,
    channel: &str,
    wave_index: Option<u32>,
    target: nixfleet_proto::agent_wire::EvaluatedTarget,
    state: &AppState,
    now: DateTime<Utc>,
) -> Option<nixfleet_proto::agent_wire::EvaluatedTarget> {
    if is_already_deferred_for_target(db, hostname, &target.closure_hash) {
        tracing::debug!(
            target: "dispatch",
            hostname = %hostname,
            target_closure = %target.closure_hash,
            "dispatch: host already deferred for this target - skipping re-issue (awaiting reboot)",
        );
        return None;
    }
    let confirm_deadline = now + chrono::Duration::seconds(state.confirm_deadline_secs);
    // Defensive: first dispatch can race the polling loop on startup.
    if let Err(err) = db.rollouts().record_active_rollout(rollout_id, channel) {
        tracing::warn!(
            rollout = %rollout_id,
            channel = %channel,
            error = %err,
            "dispatch: record_active_rollout failed (non-fatal)",
        );
    }
    match db
        .host_dispatch_state()
        .record_dispatch(&crate::db::DispatchInsert {
            hostname,
            rollout_id,
            channel,
            wave: wave_index.unwrap_or(0),
            target_closure_hash: &target.closure_hash,
            target_channel_ref: &target.channel_ref,
            confirm_deadline,
        }) {
        Ok(()) => {
            tracing::info!(
                target: "dispatch",
                hostname = %hostname,
                rollout = %rollout_id,
                target_closure = %target.closure_hash,
                confirm_deadline = %confirm_deadline.to_rfc3339(),
                "dispatch: target issued",
            );
            Some(target)
        }
        Err(err) => {
            tracing::warn!(
                hostname = %hostname,
                rollout = %rollout_id,
                error = %err,
                "dispatch: record_dispatch failed; returning no target",
            );
            None
        }
    }
}

/// Cross-tick guard: returns true iff the host's row is already
/// `deferred-pending-reboot` for the SAME closure. Without this,
/// `record_dispatch`'s unconditional Pending UPSERT would reset the row
/// and trip the 360s rollback timer (the agent's `last_deferred` silently
/// suppresses re-activation). Closure MISMATCH falls through so fresh CI
/// fixes can land. Read failures → false (fail-open).
pub(crate) fn is_already_deferred_for_target(
    db: &crate::db::Db,
    hostname: &str,
    target_closure_hash: &str,
) -> bool {
    matches!(
        db.host_dispatch_state().host_state(hostname),
        Ok(Some(row))
        if row.state == "deferred-pending-reboot"
            && row.target_closure_hash == target_closure_hash,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::{dispatch_insert, fresh_db};
    use chrono::Utc;

    #[test]
    fn guard_returns_true_when_existing_row_is_deferred_for_same_target() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "host-a",
                "stable@r1",
                "system-r1",
                deadline,
            ))
            .unwrap();
        db.host_dispatch_state()
            .mark_deferred("host-a", "stable@r1")
            .unwrap();
        assert!(
            is_already_deferred_for_target(&db, "host-a", "system-r1"),
            "deferred row + matching closure must trigger the guard",
        );
    }

    #[test]
    fn guard_returns_false_when_target_closure_mismatches() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "host-a",
                "stable@r1",
                "system-r1",
                deadline,
            ))
            .unwrap();
        db.host_dispatch_state()
            .mark_deferred("host-a", "stable@r1")
            .unwrap();
        assert!(
            !is_already_deferred_for_target(&db, "host-a", "system-r2-NEW"),
            "deferred row + different closure must NOT trigger the guard",
        );
    }

    #[test]
    fn guard_returns_false_when_row_is_pending() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "host-a",
                "stable@r1",
                "system-r1",
                deadline,
            ))
            .unwrap();
        assert!(!is_already_deferred_for_target(&db, "host-a", "system-r1"));
    }

    #[test]
    fn guard_returns_false_when_no_row_exists() {
        let db = fresh_db();
        assert!(!is_already_deferred_for_target(
            &db,
            "fresh-host",
            "anything"
        ));
    }

    /// Pins the boundary: only deferred state triggers the early-out.
    #[test]
    fn guard_returns_false_when_row_is_confirmed() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "host-a",
                "stable@r1",
                "system-r1",
                deadline,
            ))
            .unwrap();
        db.host_dispatch_state()
            .confirm("host-a", "stable@r1")
            .unwrap();
        assert!(!is_already_deferred_for_target(&db, "host-a", "system-r1"));
    }
}
