//! Read-model views served by CP for operator-facing consumers (`/v1/hosts`,
//! CLI, metrics exporter). Outstanding-event counts apply resolution-by-
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
    /// don't show up as offline (low value despite recent `last_checkin_at`).
    #[serde(default)]
    pub last_uptime_secs: Option<u64>,
    /// Per-host rollout state for the channel's CURRENT rolloutId (computed
    /// from verified_fleet, not the agent-reported `last_rollout_id` which
    /// may be stale). `None` until the host transitions in a freshly opened
    /// rollout.
    #[serde(default)]
    pub rollout_state: Option<HostRolloutState>,
    /// Agent posted `ActivationDeferred`: profile is set but a critical-
    /// component swap forced a reboot to finish activation. Cleared once
    /// the host converges.
    #[serde(default)]
    pub pending_reboot: bool,
    /// Agent posted `ClosureQuarantined`: this closure failed activation and
    /// the agent stopped retrying. Cleared automatically when the channel-ref
    /// advances to a fresher closure_hash.
    #[serde(default)]
    pub quarantined_closure: Option<String>,
    /// Active operator pin. Populated from `hosts.<name>.pin` in the fleet
    /// snapshot, pre-filtered for expiry by `nixfleet-release` - non-expired
    /// at signing time by construction.
    #[serde(default)]
    pub pin: Option<crate::Pin>,
    /// Health probes currently in non-Pass state (`Fail` and `Unknown` both
    /// count). `0` when no probes declared, all probes passing, or mode is
    /// permissive/disabled.
    #[serde(default)]
    pub outstanding_health_failures: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostsResponse {
    pub hosts: Vec<HostStatusEntry>,
}

/// Wave-by-wave dispatch trace for a single rollout. One entry per
/// dispatch_history row, ordered wave 0, 1, 2…
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
    /// RFC3339, kept as string so malformed historical rows surface to the
    /// operator instead of being masked by a re-parse.
    pub dispatched_at: String,
    /// `None` while the dispatch is still open (no confirm, no rollback).
    #[serde(default)]
    pub terminal_state: Option<String>,
    #[serde(default)]
    pub terminal_at: Option<String>,
}
