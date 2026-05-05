//! Live cross-channel deferral state, computed from `(channel_edges,
//! active_rollouts, channel_refs)`. Shared between `GET /v1/deferrals`
//! (operator API) and the `/metrics` recorder so both surfaces read the
//! same domain truth.
//!
//! Reads the Observed view from `crate::observed_view` (the canonical
//! gate-input builder) — same projection the dispatch endpoint and the
//! disruption-budget metric consume. Three operator-facing surfaces,
//! one source of truth.
//!
//! NOT consulted: the CP's in-memory `last_deferrals` debounce snapshot —
//! debounce state and observability state answer different questions and
//! must not converge on the same source.

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
pub async fn compute_channel_deferrals(state: &AppState) -> Vec<ChannelDeferral> {
    let snapshot = match state.verified_fleet.read().await.clone() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let fleet = &snapshot.fleet;

    let channel_refs = state.channel_refs_cache.read().await.refs.clone();
    let observed = crate::observed_view::build_for_gates_from_state(
        state,
        fleet,
        &snapshot.fleet_resolved_hash,
    )
    .await;

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
