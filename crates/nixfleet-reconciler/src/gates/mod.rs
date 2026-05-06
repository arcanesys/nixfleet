//! Shared dispatch-time gates: reconciler `handle_wave` + CP dispatch checkin
//! both go through `evaluate_for_host` so split-brain enforcement (a gate fires
//! on one path but not the other) becomes a registration error, not a regression.
//!
//! Adding a gate: `pub fn check(input: &GateInput) -> Option<GateBlock>` in a
//! new file here, register in `evaluate_for_host`, add a parity test.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use nixfleet_proto::FleetResolved;

use crate::observed::{Observed, Rollout};

pub mod channel_edges;
pub mod compliance_wave;
pub mod disruption_budget;
pub mod host_edges;
pub mod wave_promotion;

#[cfg(test)]
mod tests;

/// Reason a host can't be dispatched right now. Each gate maps to one
/// variant. The variants carry enough detail to render a useful log line
/// + observability event without re-querying state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateBlock {
    /// Channel-level: the host's channel has an unconverged predecessor
    /// per `fleet.channelEdges`.
    ChannelEdges { predecessor_channel: String },
    /// Wave-promotion: host's wave hasn't been reached by the rollout.
    WavePromotion { host_wave: u32, current_wave: u32 },
    /// Per-host DAG: a host this host depends on hasn't reached
    /// terminal-for-ordering (Soaked / Converged).
    HostEdge { gating_host: String },
    /// Disruption budget: too many hosts in this host's budget already
    /// in-flight.
    DisruptionBudget {
        in_flight: u32,
        max: u32,
        selector_summary: String,
    },
    /// Compliance wave staging: earlier-wave host has outstanding
    /// compliance failures under `enforce` mode.
    ComplianceWave {
        failing_events_count: usize,
        host_wave: u32,
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
        }
    }
}

/// Reconciler vs dispatch divergence on missing-predecessor: reconciler trusts
/// `emitted_opens_in_tick` (don't block); dispatch conservatively blocks until
/// polling records the predecessor (else fresh-boot checkins race the recorder
/// and bypass channelEdges).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    /// Reconciler tick — non-conservative on missing predecessor.
    Reconcile,
    /// Per-checkin dispatch endpoint — conservative on missing.
    Dispatch,
}

impl GateMode {
    /// Returns true iff this mode treats a missing predecessor as
    /// a blocker (the conservative branch of channel_edges, etc).
    pub fn conservative_on_missing(self) -> bool {
        matches!(self, GateMode::Dispatch)
    }
}

/// Superset bundle — gates pick the fields they care about.
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

/// First block wins. Order is cheapest-first so a blocked host short-circuits:
/// channel_edges → wave_promotion → host_edges → disruption_budget → compliance_wave.
pub fn evaluate_for_host(input: &GateInput) -> Option<GateBlock> {
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
