//! Live cross-channel deferral state, computed from `(channel_edges,
//! active_rollouts, channel_refs)`. Shared between `GET /v1/deferrals`
//! (operator API) and the `/metrics` recorder so both surfaces read the
//! same domain truth.
//!
//! NOT consulted: the CP's in-memory `last_deferrals` debounce snapshot —
//! debounce state and observability state answer different questions and
//! must not converge on the same source.

use std::collections::HashMap;

use crate::server::AppState;

#[derive(Debug, Clone)]
pub struct ChannelDeferral {
    pub channel: String,
    pub target_ref: String,
    pub blocked_by: String,
    pub reason: String,
}

/// Compute the current deferred-channels view. Returns an empty vec when
/// no verified-fleet snapshot is primed yet.
///
/// LOADBEARING: filters dispatch_snapshot to the CURRENT fleet's expected
/// rolloutIds so a stale Converged rollout from the previous rev doesn't
/// satisfy the predecessor check and hide the actual deferral.
pub async fn compute_channel_deferrals(state: &AppState) -> Vec<ChannelDeferral> {
    let snapshot = match state.verified_fleet.read().await.clone() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let fleet = &snapshot.fleet;

    let channel_refs = state.channel_refs_cache.read().await.refs.clone();
    let checkins = state.host_checkins.read().await.clone();
    let dispatch_snapshot = match state
        .db
        .as_deref()
        .map(|db| db.host_dispatch_state().active_rollouts_snapshot())
    {
        Some(Ok(v)) => v,
        Some(Err(err)) => {
            tracing::warn!(error = %err, "deferrals_view: active_rollouts_snapshot failed");
            Vec::new()
        }
        None => Vec::new(),
    };
    let superseded: std::collections::HashSet<String> = state
        .db
        .as_deref()
        .map(|db| db.rollouts().superseded_rollout_ids())
        .and_then(|r| r.ok())
        .unwrap_or_default()
        .into_iter()
        .collect();
    let current_rollout_ids: std::collections::HashSet<String> =
        nixfleet_reconciler::current_rollout_ids(fleet, &snapshot.fleet_resolved_hash);
    let dispatch_snapshot: Vec<_> = dispatch_snapshot
        .into_iter()
        .filter(|r| !superseded.contains(&r.rollout_id))
        .filter(|r| current_rollout_ids.contains(&r.rollout_id))
        .collect();

    let mut observed = crate::observed_projection::project(
        &checkins,
        &channel_refs,
        &dispatch_snapshot,
        HashMap::new(),
        HashMap::new(),
        &HashMap::new(),
    );

    // Augment with rollouts that exist in the rollouts table but have no
    // host_dispatch_state rows yet — newly-recorded rollouts the polling
    // layer just opened, where no agent has checked in to receive a
    // dispatch. Without this, the deferrals view of "predecessor active"
    // lags the polling layer's view by up to one agent-checkin interval.
    if let Some(db) = state.db.as_deref() {
        if let Ok(table_rollouts) = db.rollouts().list_active() {
            let known: std::collections::HashSet<String> = observed
                .active_rollouts
                .iter()
                .map(|r| r.id.clone())
                .collect();
            for r in table_rollouts {
                if known.contains(&r.rollout_id)
                    || superseded.contains(&r.rollout_id)
                    || !current_rollout_ids.contains(&r.rollout_id)
                {
                    continue;
                }
                let target_ref = channel_refs.get(&r.channel).cloned().unwrap_or_default();
                observed
                    .active_rollouts
                    .push(nixfleet_reconciler::observed::Rollout {
                        id: r.rollout_id,
                        channel: r.channel,
                        target_ref,
                        state: nixfleet_reconciler::RolloutState::Executing,
                        current_wave: r.current_wave as usize,
                        host_states: HashMap::new(),
                        last_healthy_since: HashMap::new(),
                        budgets: vec![],
                        terminal_at: r.terminal_at,
                    });
            }
        }
    }

    let mut deferrals: Vec<ChannelDeferral> = Vec::new();
    for (channel, current_ref) in &channel_refs {
        if !fleet.channels.contains_key(channel) {
            continue;
        }
        // Channels already running an active rollout aren't "deferred" —
        // they're executing.
        let has_active = observed
            .active_rollouts
            .iter()
            .any(|r| &r.channel == channel);
        if has_active {
            continue;
        }
        // Empty in-tick set: live snapshot read, not a reconcile tick.
        let no_in_tick_opens = std::collections::HashSet::new();
        if let Some(blocker) = nixfleet_reconciler::gates::channel_edges::check_for_channel(
            fleet,
            &observed,
            &no_in_tick_opens,
            channel,
            // Live read for the dashboard, not the dispatch path —
            // missing predecessor here just means "not yet recorded";
            // we don't want a fresh-boot CP to surface every successor
            // as `deferred`. Reconcile mode is the right one.
            nixfleet_reconciler::gates::GateMode::Reconcile,
        ) {
            let reason = fleet
                .channel_edges
                .iter()
                .find(|e| e.gated == *channel && e.gates == blocker)
                .and_then(|e| e.reason.clone())
                .unwrap_or_else(|| {
                    format!("predecessor channel '{blocker}' has an unfinished rollout")
                });
            deferrals.push(ChannelDeferral {
                channel: channel.clone(),
                target_ref: current_ref.clone(),
                blocked_by: blocker,
                reason,
            });
        }
    }

    // Stable order: alphabetical by channel — deterministic across ticks
    // for both the JSON response and the Prometheus label set.
    deferrals.sort_by(|a, b| a.channel.cmp(&b.channel));
    deferrals
}
