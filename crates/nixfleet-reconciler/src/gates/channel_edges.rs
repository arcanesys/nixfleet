//! ChannelEdges gate — predecessor channel must converge before successor opens.
//!
//! Migrated from `crate::reconcile::predecessor_channel_blocking`. The
//! reconciler's `reconcile()` main loop still uses
//! `check_for_channel` directly (channel-level, not host-level — it
//! decides whether to emit `OpenRollout` for a channel whose ref
//! changed). The dispatch endpoint uses `check` via
//! `gates::evaluate_for_host`.
//!
//! Both call sites end up in the same predicate (`channel_blocked`),
//! so adding a new edge case touches one function and is enforced
//! everywhere.

use crate::observed::Observed;
use nixfleet_proto::FleetResolved;
use std::collections::HashSet;

use super::{GateBlock, GateInput, GateMode};

/// Per-host gate entry. Derives the host's channel from `fleet.hosts`
/// and dispatches to `check_for_channel`.
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

/// Channel-level entry. Returns the predecessor channel name when
/// `channel` is held, else `None`.
///
/// Public so the reconciler's `reconcile()` main loop and the
/// dashboard's live `/v1/deferrals` route can consult the same
/// predicate.
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
            channel_blocked(
                fleet,
                observed,
                emitted_opens_in_tick,
                &e.gates,
                mode,
            )
            .then(|| e.gates.clone())
        })
}

/// Single-predecessor check. The shared predicate behind every entry
/// point — `check`, `check_for_channel`, and the dashboard live read all
/// route here.
///
/// Source-of-truth precedence:
///   1. If a rollout for `predecessor` exists in `observed.active_rollouts`,
///      ITS state wins. A converged rollout (every host Soaked or
///      Converged) means the predecessor is done.
///   2. Otherwise, if the predecessor was emitted in this reconcile
///      tick, it counts as active.
///   3. Otherwise, in `Dispatch` mode (fresh-boot protection at the
///      dispatch endpoint), block if the fleet declares hosts on the
///      predecessor channel. `Reconcile` mode lets it through —
///      `emitted_opens_in_tick` is the authoritative in-tick signal
///      there.
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
            // Mode is the load-bearing axis here — keep this match
            // explicit so adding a future mode forces a decision.
            match mode {
                GateMode::Dispatch => fleet.hosts.values().any(|h| h.channel == predecessor),
                GateMode::Reconcile => false,
            }
        }
    }
}
