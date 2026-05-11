//! Host-edges gate - per-host DAG predecessors must reach terminal-for-ordering.
//!
//! Migrated from `crate::host_state::edges::predecessor_blocking`.
//! `Edge { gated: A, gates: B }` semantics: A's dispatch is held until B
//! reaches Soaked/Converged within the same rollout. (Renamed from the
//! prior `before`/`after` field names which read backwards - see schema
//! note on `nixfleet_proto::Edge`.)
//!
//! `Soaked` and `Converged` count as terminal-for-ordering (matching
//! channelEdges semantics - host has cleared its soak, the gating
//! purpose is satisfied).

use crate::host_state::HostRolloutState;

use super::{GateBlock, GateInput};

// `is_terminal_for_ordering` is centralised on HostRolloutState - both
// gates use the same predicate so "what counts as done" can't drift.

pub fn check(input: &GateInput) -> Option<GateBlock> {
    // No rollout = no per-host states to gate against. Channel-level
    // gates (channelEdges) hold dispatch in this case until the rollout
    // is recorded.
    let rollout = input.rollout?;

    // The gated host's channel - used to skip cross-channel edges below.
    // Cross-channel host ordering is what `channelEdges` is for; allowing
    // host edges across channels would silently brick the gated host
    // because the gate operates within a single rollout's `host_states`,
    // which only contains hosts on the same channel.
    let host_channel = input
        .fleet
        .hosts
        .get(input.host)
        .map(|h| h.channel.as_str())?;

    input
        .fleet
        .edges
        .iter()
        .filter(|e| e.gated == input.host)
        .filter(|e| {
            // Cross-channel guard. Without this, an edge like
            // `Edge { gated: krach (stable), gates: lab (edge) }` would
            // look up `lab` in the stable rollout's host_states, find
            // nothing, default to `Queued`, and block krach forever.
            // Cross-channel ordering is `channelEdges`'s job, not host
            // edges'. Silently skipping mismatched edges is preferable
            // to bricking the gated host.
            input
                .fleet
                .hosts
                .get(&e.gates)
                .map(|h| h.channel == host_channel)
                .unwrap_or(false)
        })
        .find_map(|e| {
            let other_state = rollout
                .host_states
                .get(&e.gates)
                .copied()
                .unwrap_or(HostRolloutState::Queued);
            if other_state.is_terminal_for_ordering() {
                None
            } else {
                Some(GateBlock::HostEdge {
                    gating_host: e.gates.clone(),
                })
            }
        })
}
