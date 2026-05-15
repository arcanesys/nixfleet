//! Shared dispatch-time gates routed through `evaluate_for_host` from both the
//! reconciler and the CP dispatch checkin, so split-brain enforcement becomes
//! a registration error rather than a regression. To add one: implement
//! `check(input: &GateInput) -> Option<GateBlock>`, register in
//! `evaluate_for_host`, add a parity test.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use nixfleet_proto::FleetResolved;

use crate::observed::{Observed, Rollout};

pub mod channel_edges;
pub mod compliance_wave;
pub mod disruption_budget;
pub mod host_edges;
pub mod quarantine;
pub mod wave_promotion;

#[cfg(test)]
mod tests;

/// Reason a host can't be dispatched right now. Variants carry enough detail
/// to render the log line + observability event without re-querying state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateBlock {
    ChannelEdges {
        predecessor_channel: String,
    },
    WavePromotion {
        host_wave: u32,
        current_wave: u32,
    },
    HostEdge {
        gating_host: String,
    },
    DisruptionBudget {
        in_flight: u32,
        max: u32,
        selector_summary: String,
    },
    ComplianceWave {
        failing_events_count: usize,
        host_wave: u32,
    },
    Quarantined {
        channel: String,
        closure_hash: String,
    },
}

impl GateBlock {
    /// Short human-readable reason for log lines + Action::Skip events.
    pub fn reason(&self) -> String {
        match self {
            GateBlock::ChannelEdges {
                predecessor_channel,
            } => {
                format!("channelEdges predecessor channel '{predecessor_channel}' not converged")
            }
            GateBlock::WavePromotion {
                host_wave,
                current_wave,
            } => {
                format!("wave-promotion: host_wave={host_wave} > current_wave={current_wave}")
            }
            GateBlock::HostEdge { gating_host } => {
                format!("host-edge: gating host '{gating_host}' not yet Soaked/Converged")
            }
            GateBlock::DisruptionBudget {
                in_flight,
                max,
                selector_summary,
            } => format!("disruption-budget: {in_flight}/{max} in flight ({selector_summary})"),
            GateBlock::ComplianceWave {
                failing_events_count,
                host_wave,
            } => format!(
                "compliance-wave: {failing_events_count} outstanding failure(s) on hosts in wave < {host_wave}"
            ),
            GateBlock::Quarantined {
                channel,
                closure_hash,
            } => format!(
                "channel {channel} closure {closure_hash} quarantined (sustained probe failures); push a new closure to clear"
            ),
        }
    }

    /// Stable kebab-case discriminator for telemetry.
    pub fn discriminator(&self) -> &'static str {
        match self {
            GateBlock::ChannelEdges { .. } => "channel-edges",
            GateBlock::WavePromotion { .. } => "wave-promotion",
            GateBlock::HostEdge { .. } => "host-edge",
            GateBlock::DisruptionBudget { .. } => "disruption-budget",
            GateBlock::ComplianceWave { .. } => "compliance-wave",
            GateBlock::Quarantined { .. } => "quarantine",
        }
    }
}

/// Mode-dependent behaviour on missing-predecessor. `Reconcile` trusts the
/// in-tick `emitted_opens_in_tick` set and does not block; `Dispatch` blocks
/// until polling records the predecessor, else fresh-boot checkins race
/// the recorder and bypass channelEdges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    Reconcile,
    Dispatch,
}

impl GateMode {
    /// True iff a missing predecessor blocks (the conservative branch).
    pub fn conservative_on_missing(self) -> bool {
        matches!(self, GateMode::Dispatch)
    }
}

/// Superset bundle - gates pick the fields they care about.
pub struct GateInput<'a> {
    pub fleet: &'a FleetResolved,
    pub observed: &'a Observed,
    /// `None` on fresh-boot dispatch (no rollout recorded yet).
    pub rollout: Option<&'a Rollout>,
    pub host: &'a str,
    pub now: DateTime<Utc>,
    /// Channels with an `OpenRollout` decided in the current reconcile tick;
    /// empty in the dispatch-endpoint context.
    pub emitted_opens_in_tick: &'a HashSet<String>,
    pub mode: GateMode,
}

/// First block wins. Cheapest-first order:
/// quarantine -> channel_edges -> wave_promotion -> host_edges -> disruption_budget -> compliance_wave.
/// Quarantine is FIRST: a hash that just rolled back must stop instantly even if
/// channelEdges / waves / budgets would otherwise hold the host -- otherwise the
/// agent re-fetches and re-activates the bad closure on every reconcile cycle.
pub fn evaluate_for_host(input: &GateInput) -> Option<GateBlock> {
    if let Some(b) = quarantine::check(input) {
        return Some(b);
    }
    if let Some(b) = channel_edges::check(input) {
        return Some(b);
    }
    if let Some(b) = wave_promotion::check(input) {
        return Some(b);
    }
    if let Some(b) = host_edges::check(input) {
        return Some(b);
    }
    if let Some(b) = disruption_budget::check(input) {
        return Some(b);
    }
    if let Some(b) = compliance_wave::check(input) {
        return Some(b);
    }
    None
}
