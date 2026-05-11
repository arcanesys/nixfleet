//! Disruption-budget gate. `max_in_flight` enforced at dispatch time.
//!
//! Cross-rollout enforcement: budgets match by `selector` equality (not list
//! index) so reordering `fleet.disruptionBudgets` doesn't conflate distinct
//! budgets, and the in-flight sum spans all active rollouts that carry a
//! budget with the same selector. "Max one workstation in flight, ever"
//! semantics hold even with multiple rollouts active simultaneously.
//!
//! Snapshots are read from `rollout.budgets` (frozen at OpenRollout, signed
//! into the rollout manifest); mid-rollout retags do NOT reshape membership -
//! the cascading-dispatch hazard from live resolution is structurally
//! impossible by design.

use crate::observed::Observed;
use nixfleet_proto::{RolloutBudget, Selector};

use super::{GateBlock, GateInput};

pub fn check(input: &GateInput) -> Option<GateBlock> {
    let rollout = input.rollout?;

    rollout
        .budgets
        .iter()
        .filter(|b| b.hosts.iter().any(|h| h == input.host))
        .filter_map(|b: &RolloutBudget| {
            b.max_in_flight.map(|max| {
                let in_flight = in_flight_count(input.observed, &b.selector);
                (in_flight, max, b.selector.clone())
            })
        })
        .find(|(in_flight, max, _)| in_flight >= max)
        .map(|(in_flight, max, selector)| GateBlock::DisruptionBudget {
            in_flight,
            max,
            selector_summary: selector.summary(),
        })
}

/// Sum of in-flight hosts across active rollouts with a matching `selector`.
fn in_flight_count(observed: &Observed, selector: &Selector) -> u32 {
    observed
        .active_rollouts
        .iter()
        .map(|r| {
            let Some(b) = r.budgets.iter().find(|rb| &rb.selector == selector) else {
                return 0;
            };
            r.host_states
                .iter()
                .filter(|(h, st)| {
                    if !b.hosts.iter().any(|bh| bh == *h) {
                        return false;
                    }
                    st.is_in_flight()
                })
                .count() as u32
        })
        .sum()
}
