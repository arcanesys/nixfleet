//! Read-model views of fleet state served by the CP for operator-facing
//! consumers (`/v1/hosts`, CLI, metrics exporter). One `HostStatusEntry`
//! per declared host; outstanding-event counts apply resolution-by-
//! replacement (events from older rollouts are considered resolved).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::HostRolloutState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostStatusEntry {
    pub hostname: String,
    pub channel: String,
    #[serde(default)]
    pub declared_closure_hash: Option<String>,
    #[serde(default)]
    pub current_closure_hash: Option<String>,
    #[serde(default)]
    pub pending_closure_hash: Option<String>,
    #[serde(default)]
    pub last_checkin_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_rollout_id: Option<String>,
    pub converged: bool,
    pub outstanding_compliance_failures: usize,
    pub outstanding_runtime_gate_errors: usize,
    pub verified_event_count: usize,
    /// Reported by the agent at every checkin. Surfaces crash-loops that
    /// don't show up as offline (rapid restart, low value despite recent
    /// `last_checkin_at`).
    #[serde(default)]
    pub last_uptime_secs: Option<u64>,
    /// Per-host rollout state machine position for the channel's CURRENT
    /// rolloutId (computed from verified_fleet, not the agent-reported
    /// last_rollout_id which may be stale after a fresh deploy). `None`
    /// when no DB row exists yet for the current rollout — a freshly
    /// opened rollout shows None until the host transitions.
    #[serde(default)]
    pub rollout_state: Option<HostRolloutState>,
    /// Agent posted `ActivationDeferred` for the host's current rollout —
    /// profile is set, but a critical-component swap forced a reboot to
    /// finish activation. Cleared once the host converges (post-reboot).
    #[serde(default)]
    pub pending_reboot: bool,
    /// Agent posted `RolloutQuarantined` for the host's current rollout —
    /// the closure_hash already failed activation and the agent has
    /// stopped retrying it. Operator surface for "this release is
    /// permanently broken on this host, fix it in CI". Cleared
    /// automatically when the channel-ref advances to a fresher
    /// closure_hash (the agent's suppression check stops matching).
    #[serde(default)]
    pub quarantined_closure: Option<String>,
    /// Active operator pin for this host (issue #88). Populated from
    /// the fleet snapshot's `hosts.<name>.pin`, which mkFleet emitted
    /// from the most-specific declaration in the host > tag > channel
    /// chain. Pre-filtered by `nixfleet-release`'s expiry sweep, so
    /// what's here is by construction non-expired at signing time.
    #[serde(default)]
    pub pin: Option<crate::Pin>,
    /// Count of health probes (issue #86) currently in non-Pass state on
    /// this host's latest checkin — `Fail` and `Unknown` both count.
    /// `0` when no probes are declared, all probes are passing, or the
    /// host's mode is permissive/disabled (no probe constraint surfaced
    /// to the soak gate in those modes either — same semantics as
    /// `host_probes_passing` returning `true`).
    #[serde(default)]
    pub outstanding_health_failures: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostsResponse {
    pub hosts: Vec<HostStatusEntry>,
}

/// Wave-by-wave dispatch trace for a single rollout. One entry per
/// dispatch_history row; the rollout's lifecycle reads top-to-bottom as
/// wave 0 hosts dispatch first, then wave 1, then wave 2, etc.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RolloutTrace {
    pub rollout_id: String,
    pub events: Vec<RolloutTraceEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RolloutTraceEvent {
    pub host: String,
    pub channel: String,
    pub wave: u32,
    pub target_closure_hash: String,
    pub target_channel_ref: String,
    /// RFC3339 — kept as string because the DB writes it as text and
    /// re-parsing would mask malformed historical rows the operator
    /// needs to see.
    pub dispatched_at: String,
    /// `None` while the dispatch is still open (no confirm, no rollback).
    #[serde(default)]
    pub terminal_state: Option<String>,
    #[serde(default)]
    pub terminal_at: Option<String>,
}
