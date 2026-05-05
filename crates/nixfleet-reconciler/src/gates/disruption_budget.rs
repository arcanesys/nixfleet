//! Disruption-budget gate — `max_in_flight` enforced at dispatch time.
//!
//! Migrated from `crate::host_state::budgets::{budget_max, in_flight_count}`.
//! The reconciler's `handle_wave` checks this; this is the missing
//! enforcement at the dispatch endpoint that bit us during the
//! supersession test (krach + aether dispatched within seconds despite
//! `dev maxInFlight=1` — only avoided because aether's closure was
//! unchanged and went through `Decision::Converged`).
//!
//! Cross-rollout enforcement: budgets match by `selector` equality
//! (not list index) so reordering `fleet.disruptionBudgets` between
//! rollout opens doesn't conflate distinct budgets, and the in-flight
//! sum spans all active rollouts that carry a budget with the same
//! selector. "Max one workstation in flight, ever" semantics hold even
//! with multiple rollouts active simultaneously.
//!
//! Budget snapshots are read from `rollout.budgets` (frozen at
//! OpenRollout time, signed into the rollout manifest). Mid-rollout
//! retags do NOT reshape budget membership — the cascading-dispatch
//! hazard from live resolution is structurally impossible by design.

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

/// Sum of in-flight hosts across all active rollouts whose snapshot has
/// a budget with the matching `selector`. Match by selector equality
/// (not list index) — see module doc.
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
