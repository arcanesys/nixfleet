//! Wave-promotion gate: host's wave_index must not exceed rollout's
//! current_wave. `host_wave = None` (host not in any declared wave) is
//! ungated. `current_wave = None` (no rollout recorded) defaults to 0.

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
    let host_wave = host_wave?;

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
