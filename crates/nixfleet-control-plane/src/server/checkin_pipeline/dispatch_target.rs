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

    // Fleet-level dispatch gates. Centralised in `nixfleet_reconciler::gates`
    // so the reconciler (handle_wave) and this dispatch endpoint reach the
    // same conclusion from the same Observed snapshot. Adding a new gate
    // touches one module + one parity test; both layers pick it up
    // automatically. See nixfleet-reconciler/src/gates/mod.rs.
    if let Some(host) = fleet.hosts.get(&req.hostname) {
        let observed =
            crate::observed_view::build_for_gates_from_state(state, &fleet, &fleet_resolved_hash)
                .await;
        // Locate the rollout for this host's channel — host-edges + budget
        // gates need the host's current rollout to read frozen budgets +
        // host_states. None when no rollout recorded yet (fresh boot /
        // fresh rev); the gates handle that conservatively per their
        // own semantics.
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
            // Conservative: at fresh CP boot or fresh fleet rev, polling
            // hasn't yet recorded the predecessor's rollout. Without this
            // flag, the FIRST agent on a held successor channel would
            // race ahead via record_dispatched_target's defensive
            // record_active_rollout. See gates/channel_edges.rs.
            mode: nixfleet_reconciler::gates::GateMode::Dispatch,
        };
        if let Some(block) = nixfleet_reconciler::gates::evaluate_for_host(&input) {
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

    // Look up the rollout's persisted `current_wave` so decide_target's
    // wave-promotion gate can compare against the host's wave_index.
    // Computed from the same fleet snapshot the decision will use, so
    // the rolloutId resolved here matches the one inside decide_target.
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
            // Compliance wave gate is now part of `gates::evaluate_for_host`
            // (called above pre-decide_target). The legacy
            // `wave_gate_blocks_dispatch` call here is superseded —
            // see `gates/compliance_wave.rs`.
            //
            // Behaviour change to note: previously the compliance wave
            // gate fired ONLY in the Decision::Dispatch arm, so a
            // host's converged-at-dispatch path (host already on
            // target closure) bypassed it. With the unified gate, a
            // converged-at-dispatch host whose earlier-wave peers have
            // outstanding compliance failures will also be held until
            // the failures resolve. Strictly safer; rare in practice.

            // Persist channel explicitly: content-addressed rolloutIds don't encode it.
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
            // Host is already running the declared target. Materialize the
            // rollout-layer rows so the reconciler sees this host as Soaked
            // for the current rollout (fixes the convergence-stamping panel
            // and stops the active-rollouts panel showing ghosts).
            //
            // This is the only path where we INSERT host_dispatch_state +
            // host_rollout_state for a host without ever sending it a target.
            // The agent never sees a confirm — the breadcrumb on its
            // checkin still references the LAST rollout it actually
            // confirmed, but the CP's view of "this host is on rollout R"
            // is now authoritative via these rows.
            record_converged_at_dispatch(db, req, &fleet, &fleet_resolved_hash, now);
            None
        }
        crate::dispatch::Decision::WaveNotReached => {
            tracing::debug!(
                target: "dispatch",
                hostname = %req.hostname,
                current_wave = ?current_wave,
                "dispatch: wave-promotion gate held target — host's wave hasn't been promoted yet",
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

/// LOADBEARING: each row materialization is best-effort + idempotent.
/// Failures here only delay reconciler convergence — they don't break the
/// agent (which got `Decision::Converged` and has nothing to confirm anyway).
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

    // record_active_rollout is idempotent — safe to call every checkin.
    // (Channel-refs poll also calls it; both converge to the same row.)
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

    // LOADBEARING: dispatch_history insert is APPEND-ONLY (autoincrement id,
    // no UNIQUE constraint). Three states to disambiguate:
    //
    //   1. Host is already Converged for this rollout → nothing to do.
    //      Both rows already exist; advancing them again would leak a
    //      duplicate dispatch_history row on every checkin (~5 hosts × 2
    //      rollouts × 30s = unbounded growth).
    //   2. Host has a host_rollout_state row but it's NOT Converged
    //      (typically Healthy, set by recover_soak_state_from_attestation
    //      earlier in the same checkin). Both rows already exist — only
    //      the state transition needs to advance to Converged.
    //   3. No host_rollout_state row at all → full materialisation
    //      (host_dispatch_state + dispatch_history + host_rollout_state).
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

    // Case 2 (existing row, NOT Converged): the host went through a
    // real dispatch → activation cycle. host_state is currently
    // Dispatched/Activating/ConfirmWindow/Healthy/etc. Jumping
    // straight to Converged would silently bypass the operator's
    // `soakMinutes` window — the entire point of wave-staging.
    // Leave the existing state alone; the reconciler's SoakHost
    // (Healthy → Soaked after soak window) and ConvergeRollout
    // (Soaked → Converged when wave_all_soaked + last wave) run the
    // natural progression.
    if existing_state.is_some() {
        return;
    }

    // Case 3: no row → host was on the target closure BEFORE any
    // dispatch attempt (steady-state, or post-state.db-wipe with a
    // host that's been stable). Full atomic materialisation directly
    // to Converged is correct: there's no transient state to ride
    // out. `last_healthy_since = now` is an audit-trail nicety —
    // gates don't read the soak anchor on Converged hosts.
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
    let confirm_deadline = now + chrono::Duration::seconds(state.confirm_deadline_secs);
    // Defensive: ensure the rollouts table reflects this rid as active for
    // the channel even if the polling tick hasn't recorded it yet (first
    // dispatch can race the polling loop on startup).
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
