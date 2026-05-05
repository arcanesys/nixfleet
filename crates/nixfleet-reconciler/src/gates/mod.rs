//! Shared dispatch-time gates evaluated by both the reconciler (per host
//! in `handle_wave` once Slice 2 lands) and the CP dispatch endpoint
//! (per agent checkin).
//!
//! ## Why this module exists
//!
//! Before this module, gate enforcement was split:
//!   - Reconciler emitted `Action::Skip` for budget / host-edge / wave
//!     violations.
//!   - Dispatch endpoint independently checked a SUBSET of gates.
//!   - When the reconciler's `Skip` had no dispatch-side counterpart, the
//!     gate was silently bypassed at the agent-facing checkin path —
//!     reconciler's `Skip` reduced to a journal event with no effect.
//!
//! This is "split-brain enforcement": two concurrent decision-makers
//! reaching different conclusions from the same Observed state. We hit
//! it three times in two days (wave-promotion gap, channelEdges gap,
//! disruption-budget gap) before pulling the gates into one place.
//!
//! ## The convention
//!
//! Adding a new gate:
//!   1. Create a file in this module with a `pub fn check(input:
//!      &GateInput) -> Option<GateBlock>` function.
//!   2. Register it in `evaluate_for_host` below.
//!   3. Add a parity test asserting reconciler and CP-dispatch reach the
//!      same conclusion from the same `Observed`.
//!
//! Gate registration is the only call site — both layers must go through
//! `evaluate_for_host`. Forgetting to register means the new gate is
//! unenforced everywhere, which is at least visible (the gate file is
//! dead code) — far better than enforcement-in-one-layer-only.

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

/// Caller context for a gate evaluation.
///
/// The two callers — reconciler tick and dispatch endpoint — share
/// every gate predicate, but legitimately diverge on ONE decision:
/// what to do when a referenced predecessor rollout isn't yet in
/// observed.active_rollouts.
///
///   - `Reconcile`: trust `emitted_opens_in_tick` as the in-tick
///     authority. Absence means "predecessor not opened this tick"
///     → don't block; if anything's needed it'll fire next tick.
///
///   - `Dispatch`: conservative — if the fleet declares hosts on
///     the predecessor channel, BLOCK until polling has had a
///     chance to record the predecessor's rollout. Without this,
///     a fresh-boot agent checkin races
///     `record_dispatched_target`'s defensive `record_active_rollout`
///     and would silently bypass channelEdges on the first poll
///     after every release.
///
/// This used to be a `bool conservative_on_missing_state` field on
/// `GateInput`. Naming it as a mode at every call site makes the
/// asymmetry visible to readers (and to `grep`) — every gate that
/// branches on this enum has to handle BOTH variants explicitly,
/// which is exactly the property that prevents the regression we
/// just shipped fixes for.
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

/// Input bundle for a gate evaluation.
///
/// Both the reconciler (per-host iteration in handle_wave) and the CP
/// dispatch endpoint (per-checkin) construct one of these and feed it
/// into `evaluate_for_host`. The fields are intentionally a superset
/// of what any single gate needs — gates pick what they care about.
pub struct GateInput<'a> {
    pub fleet: &'a FleetResolved,
    pub observed: &'a Observed,
    /// The active rollout this host is being evaluated against. `None`
    /// when no rollout is recorded yet — fresh-boot dispatch path.
    pub rollout: Option<&'a Rollout>,
    pub host: &'a str,
    pub now: DateTime<Utc>,
    /// Channels for which the current reconcile tick has already decided
    /// to emit `OpenRollout`. Empty for the dispatch-endpoint context
    /// (no in-tick state at agent checkin time).
    pub emitted_opens_in_tick: &'a HashSet<String>,
    /// Caller context — reconciler vs dispatch endpoint. See [`GateMode`].
    pub mode: GateMode,
}

/// Run every registered gate in order. Returns the first block, or None
/// when all gates pass. Order matters: most general / cheapest-to-check
/// gates first so a blocked host doesn't pay for downstream gate work.
///
/// Order rationale:
///   1. `channel_edges` — channel-level gate; if the channel isn't
///      open yet, no point evaluating per-host concerns.
///   2. `wave_promotion` — cheap, host-only data.
///   3. `host_edges` — needs rollout host_states.
///   4. `disruption_budget` — needs cross-rollout in-flight sum.
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
