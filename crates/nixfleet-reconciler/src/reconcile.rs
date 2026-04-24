//! Top-level `reconcile` function.
//!
//! During Phase D (this plan), all reconcile logic lives here. Phase E
//! extracts concerns into `rollout_state`, `host_state`, `budgets`,
//! `edges` modules without changing behavior.

use crate::{Action, Observed};
use chrono::{DateTime, Utc};
use nixfleet_proto::FleetResolved;

pub fn reconcile(
    fleet: &FleetResolved,
    observed: &Observed,
    _now: DateTime<Utc>,
) -> Vec<Action> {
    let mut actions = Vec::new();

    // RFC-0002 §4 step 2: open rollouts for channels whose ref changed
    // and don't already have an in-progress rollout.
    for (channel, current_ref) in &observed.channel_refs {
        if observed.last_rolled_refs.get(channel) == Some(current_ref) {
            continue;
        }
        let has_active = observed.active_rollouts.iter().any(|r| {
            &r.channel == channel && (r.state == "Executing" || r.state == "Planning")
        });
        if !has_active && fleet.channels.contains_key(channel) {
            actions.push(Action::OpenRollout {
                channel: channel.clone(),
                target_ref: current_ref.clone(),
            });
        }
    }

    actions
}
