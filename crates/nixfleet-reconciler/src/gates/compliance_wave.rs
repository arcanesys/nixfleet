//! Compliance-wave gate. Earlier-wave hosts with outstanding evidence
//! failures hold later-wave dispatch under `enforce` mode. Mode handling:
//!   - `disabled`: no-op.
//!   - `permissive`: counts outstanding events for observability, never blocks.
//!   - `enforce`: blocks dispatch when any host in an EARLIER wave (strictly
//!     less than the requesting host's wave) has outstanding events recorded
//!     against THIS rollout.
//!
//! Aggregates both `ComplianceFailure` and `RuntimeGateError` (single DB-side
//! filter at `db::reports::outstanding_compliance_events_by_rollout`). Per-
//! rollout grouping enforces resolution-by-replacement so events under a
//! superseded rollout never gate the new one.

use nixfleet_proto::compliance::GateMode;
use nixfleet_proto::Wave;

use crate::observed::Observed;

use super::{GateBlock, GateInput};

/// Outstanding evidence failures per host, restricted to `wave_range`. Sorted+
/// deduped. LOADBEARING: shared by the dispatch gate (`0..host_wave`, only
/// earlier waves) and the reconciler's wave-promotion `WaveBlocked` emission
/// (`0..=current_wave`, includes current). One helper keeps filtering /
/// signature handling consistent.
pub fn outstanding_failures_in_waves(
    observed: &Observed,
    rollout_id: &str,
    waves: &[Wave],
    wave_range: std::ops::Range<usize>,
) -> Vec<(String, usize)> {
    let Some(per_host) = observed
        .outstanding_compliance_events_by_rollout
        .get(rollout_id)
    else {
        return Vec::new();
    };
    let mut out: Vec<(String, usize)> = Vec::new();
    for w in waves.iter().take(wave_range.end).skip(wave_range.start) {
        for h in &w.hosts {
            if let Some(&n) = per_host.get(h) {
                if n > 0 {
                    out.push((h.clone(), n));
                }
            }
        }
    }
    out.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

pub fn check(input: &GateInput) -> Option<GateBlock> {
    let host_channel = input
        .fleet
        .hosts
        .get(input.host)
        .map(|h| h.channel.as_str())?;

    let channel = input.fleet.channels.get(host_channel)?;
    let mode = GateMode::from_wire_str(&channel.compliance.mode);
    if !mode.is_enforcing() {
        return None;
    }

    let host_wave = input.fleet.waves.get(host_channel).and_then(|waves| {
        waves
            .iter()
            .position(|w| w.hosts.iter().any(|h| h == input.host))
    });

    // No wave plan or wave 0 ⇒ no earlier wave can hold this dispatch.
    // (Same-wave hosts are the budget gate's concern.)
    let host_wave_idx = match host_wave {
        Some(0) | None => return None,
        Some(n) => n,
    };

    let rollout = input.rollout?;
    let waves = input.fleet.waves.get(host_channel)?;

    let earlier =
        outstanding_failures_in_waves(input.observed, &rollout.id, waves, 0..host_wave_idx);
    let failing_count: usize = earlier.iter().map(|(_, n)| *n).sum();

    if failing_count > 0 {
        Some(GateBlock::ComplianceWave {
            failing_events_count: failing_count,
            host_wave: host_wave_idx as u32,
        })
    } else {
        None
    }
}
