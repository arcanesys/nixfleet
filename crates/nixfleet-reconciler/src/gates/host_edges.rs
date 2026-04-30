//! Host-edges gate: per-host DAG. `Edge { gated: A, gates: B }` holds A's
//! dispatch until B reaches Soaked/Converged within the same rollout.
//! Cross-channel ordering is `channelEdges`'s job.

use crate::host_state::HostRolloutState;

use super::{GateBlock, GateInput};

pub fn check(input: &GateInput) -> Option<GateBlock> {
    // No rollout = no per-host states; channel-level gates hold dispatch.
    let rollout = input.rollout?;

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
            // Cross-channel guard: an edge like
            // `Edge { gated: host-01 (stable), gates: host-05 (edge) }` would look
            // up `host-05` in the stable rollout's host_states (always missing),
            // default to `Queued`, and block host-01 forever. Silently skip
            // mismatched edges so cross-channel ordering remains
            // `channelEdges`'s job.
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
