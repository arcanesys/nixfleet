//! ChannelEdges gate: predecessor channel must converge before successor
//! opens. The reconciler's main loop calls `check_for_channel` directly
//! (channel-level); the dispatch endpoint reaches `check` via
//! `gates::evaluate_for_host`. Both bottom out in `channel_blocked`.

use crate::observed::Observed;
use nixfleet_proto::FleetResolved;
use std::collections::HashSet;

use super::{GateBlock, GateInput, GateMode};

pub fn check(input: &GateInput) -> Option<GateBlock> {
    let host_channel = input
        .fleet
        .hosts
        .get(input.host)
        .map(|h| h.channel.as_str())?;
    check_for_channel(
        input.fleet,
        input.observed,
        input.emitted_opens_in_tick,
        host_channel,
        input.mode,
    )
    .map(|predecessor| GateBlock::ChannelEdges {
        predecessor_channel: predecessor,
    })
}

/// Returns the predecessor channel name when `channel` is held, else `None`.
/// Public so the reconciler's main loop and the dashboard's live deferrals
/// route consult the same predicate.
pub fn check_for_channel(
    fleet: &FleetResolved,
    observed: &Observed,
    emitted_opens_in_tick: &HashSet<String>,
    channel: &str,
    mode: GateMode,
) -> Option<String> {
    fleet
        .channel_edges
        .iter()
        .filter(|e| e.gated == channel)
        .find_map(|e| {
            channel_blocked(fleet, observed, emitted_opens_in_tick, &e.gates, mode)
                .then(|| e.gates.clone())
        })
}

/// Single-predecessor check. Source-of-truth precedence:
/// 1. Rollout in `observed.active_rollouts` wins; converged ⇒ done.
/// 2. Else, predecessor emitted this tick ⇒ active.
/// 3. Else, `Dispatch` blocks if fleet declares hosts on the predecessor
///    channel (fresh-boot protection); `Reconcile` lets it through
///    (`emitted_opens_in_tick` is the in-tick authority there).
fn channel_blocked(
    fleet: &FleetResolved,
    observed: &Observed,
    emitted_opens_in_tick: &HashSet<String>,
    predecessor: &str,
    mode: GateMode,
) -> bool {
    let db_rollout = observed
        .active_rollouts
        .iter()
        .find(|r| r.channel == predecessor);
    match db_rollout {
        Some(r) => r.is_active_for_ordering(),
        None => {
            if emitted_opens_in_tick.contains(predecessor) {
                return true;
            }
            // Explicit match so adding a future mode forces a decision.
            match mode {
                GateMode::Dispatch => fleet.hosts.values().any(|h| h.channel == predecessor),
                GateMode::Reconcile => false,
            }
        }
    }
}
