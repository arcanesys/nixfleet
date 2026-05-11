//! Wave-promotion gate - host's wave_index must not exceed rollout's current_wave.
//!
//! Migrated from `nixfleet_control_plane::dispatch::decide_target`'s
//! inline check (`Decision::WaveNotReached`). The reconciler's
//! `handle_wave` doesn't trip this gate because it iterates only the
//! current wave's hosts - but having the check live in the shared
//! gates module makes the invariant explicit and discoverable, and
//! catches future code paths that bypass `handle_wave`'s iteration
//! convention.
//!
//! `host_wave = None` (host not in any declared wave for its channel)
//! is ungated - that path covers single-wave channels with
//! `selector.all = true` where the wave doesn't filter by host.
//! `current_wave = None` from `rollout` (no rollout recorded yet)
//! defaults to 0 - start of every staged rollout.

use super::{GateBlock, GateInput};

pub fn check(input: &GateInput) -> Option<GateBlock> {
    let host_channel = input
        .fleet
        .hosts
        .get(input.host)
        .map(|h| h.channel.as_str())?;

    let host_wave = input.fleet.waves.get(host_channel).and_then(|waves| {
        waves
            .iter()
            .position(|w| w.hosts.iter().any(|h| h == input.host))
            .map(|i| i as u32)
    });
    let host_wave = host_wave?; // None == ungated

    let current_wave = input.rollout.map(|r| r.current_wave as u32).unwrap_or(0);

    if host_wave > current_wave {
        Some(GateBlock::WavePromotion {
            host_wave,
            current_wave,
        })
    } else {
        None
    }
}
