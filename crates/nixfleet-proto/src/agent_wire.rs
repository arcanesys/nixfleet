//! Agent ↔ control-plane wire types (RFC-0003 §4).
//!
//! Defined in this crate (rather than in either binary) so the agent
//! and CP serialise/deserialise from one schema and Stream B can
//! reuse the same types for harness assertions. The Phase 3 expansion
//! adds `pendingGeneration`, `lastEvaluatedTarget`, `lastFetchOutcome`,
//! and `uptimeSecs` to the checkin body — all nullable, additive over
//! RFC-0003 §4.1's minimum.
//!
//! Unknown-field posture follows the crate-level convention: serde's
//! default is to ignore unknowns; consumers MUST treat additions
//! within the same major version as backwards-compatible.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// =====================================================================
// /v1/agent/checkin — RFC-0003 §4.1 + Phase 3 expansion
// =====================================================================

/// POST /v1/agent/checkin request body. Sent by the agent every
/// `pollInterval` seconds; CP records into in-memory state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinRequest {
    pub hostname: String,
    pub agent_version: String,

    /// What's running right now (`/run/current-system`).
    pub current_generation: GenerationRef,

    /// What's queued for next boot if it differs from current
    /// (`/run/booted-system` vs `/run/current-system`). Null when
    /// they match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_generation: Option<PendingGeneration>,

    /// The most recent target the agent saw from the CP. Null on
    /// first checkin or before the agent has fetched a target.
    /// Phase 3 doesn't activate, but it's useful for the operator
    /// to see what the agent *would* activate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_evaluated_target: Option<EvaluatedTarget>,

    /// Outcome of the most recent target fetch + verify attempt.
    /// Null if the agent hasn't tried to fetch anything yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fetch_outcome: Option<FetchOutcome>,

    /// Seconds since the agent process started. Useful for spotting
    /// agents that crash-loop without showing up as down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationRef {
    pub closure_hash: String,
    /// Channel ref this closure was published from, if known. Null
    /// during PR-1/PR-3 because the agent doesn't yet correlate
    /// channels (PR-4 introduces the projection that does).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_ref: Option<String>,
    pub boot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingGeneration {
    pub closure_hash: String,
    /// Wall-clock time the pending generation is scheduled to take
    /// over (typically `null` in Phase 3 — pending = "queued for
    /// next boot, no deadline").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedTarget {
    pub closure_hash: String,
    pub channel_ref: String,
    pub evaluated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchOutcome {
    pub result: FetchResult,
    /// Short error string when `result != Ok`. Null when ok.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FetchResult {
    Ok,
    VerifyFailed,
    FetchFailed,
    None,
}

/// POST /v1/agent/checkin response. Phase 3 always returns
/// `target: null` (no rollouts dispatched until Phase 4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinResponse {
    /// The closure hash + channel-ref the CP wants this host to
    /// move to. Null in Phase 3 — Phase 4's dispatch loop populates
    /// this once activation is wired up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<EvaluatedTarget>,
    pub next_checkin_secs: u32,
}

// =====================================================================
// /v1/agent/report — RFC-0003 §4.5 (event reports)
// =====================================================================

/// POST /v1/agent/report request body. Agent emits this when a
/// fetch/verify failure or other notable event occurs out-of-band
/// from the regular checkin cadence. Phase 3 records into a bounded
/// in-memory ring buffer per host; Phase 4 adds SQLite persistence
/// and correlation with rollouts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRequest {
    pub hostname: String,
    pub agent_version: String,
    pub kind: ReportKind,
    /// Free-form short error string. Logged at info; not surfaced
    /// to other endpoints in Phase 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Optional structured context — closure hash being fetched,
    /// channel ref, etc. Treated as an opaque blob in Phase 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportKind {
    /// Verifying a fetched target's signature failed.
    VerifyFailed,
    /// Couldn't fetch the target closure (network, attic, etc.).
    FetchFailed,
    /// Trust file (`trust.json`) failed to parse or wasn't found.
    TrustError,
    /// Catch-all for events the agent wants to surface but doesn't
    /// fit a typed kind. Phase 4 may split this further.
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportResponse {
    /// CP echoes the event ID it assigned (UUID-like opaque
    /// string). Useful for correlation in journals.
    pub event_id: String,
}
