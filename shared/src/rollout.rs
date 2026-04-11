use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Strategy for rolling out a new generation across machines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RolloutStrategy {
    Canary,
    Staged,
    AllAtOnce,
}

impl fmt::Display for RolloutStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canary => write!(f, "canary"),
            Self::Staged => write!(f, "staged"),
            Self::AllAtOnce => write!(f, "all_at_once"),
        }
    }
}

/// What to do when a health check fails during rollout.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OnFailure {
    #[default]
    Pause,
    Revert,
}

impl fmt::Display for OnFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pause => write!(f, "pause"),
            Self::Revert => write!(f, "revert"),
        }
    }
}

/// Overall status of a rollout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RolloutStatus {
    Created,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl fmt::Display for RolloutStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl RolloutStatus {
    /// Parse from a lowercase string (as stored in the database).
    pub fn from_str_lc(s: &str) -> Option<Self> {
        match s {
            "created" => Some(Self::Created),
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Returns `true` if the rollout is still in progress (created, running, or paused).
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Created | Self::Running | Self::Paused)
    }
}

/// Status of a single batch within a rollout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    Pending,
    Deploying,
    WaitingHealth,
    Succeeded,
    Failed,
}

impl fmt::Display for BatchStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Deploying => write!(f, "deploying"),
            Self::WaitingHealth => write!(f, "waiting_health"),
            Self::Succeeded => write!(f, "succeeded"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl BatchStatus {
    /// Parse from a lowercase string (as stored in the database).
    pub fn from_str_lc(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "deploying" => Some(Self::Deploying),
            "waiting_health" => Some(Self::WaitingHealth),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Health status of an individual machine during a rollout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MachineHealthStatus {
    Pending,
    Healthy,
    Unhealthy(String),
}

impl fmt::Display for MachineHealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Healthy => write!(f, "healthy"),
            Self::Unhealthy(reason) => write!(f, "unhealthy: {}", reason),
        }
    }
}

/// Which machines a rollout targets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RolloutTarget {
    Tags(Vec<String>),
    Hosts(Vec<String>),
}

// ---------------------------------------------------------------------------
// Rollout event types
// ---------------------------------------------------------------------------

/// A single event in a rollout's timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutEvent {
    pub id: i64,
    pub rollout_id: String,
    pub event_type: String,
    pub detail: String,
    pub actor: String,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

fn default_failure_threshold() -> String {
    "1".to_string()
}

/// Request body to create a new rollout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRolloutRequest {
    pub release_id: String,
    #[serde(default)]
    pub cache_url: Option<String>,
    pub strategy: RolloutStrategy,
    #[serde(default)]
    pub batch_sizes: Option<Vec<String>>,
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: String,
    #[serde(default)]
    pub on_failure: OnFailure,
    #[serde(default)]
    pub health_timeout: Option<u64>,
    pub target: RolloutTarget,
    /// Optional policy name — if set, policy values are used as defaults.
    #[serde(default)]
    pub policy: Option<String>,
}

/// Summary of a single batch returned in the create response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSummary {
    pub batch_index: u32,
    pub machine_ids: Vec<String>,
    pub status: BatchStatus,
}

/// Response returned after creating a rollout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRolloutResponse {
    pub rollout_id: String,
    pub batches: Vec<BatchSummary>,
    pub total_machines: usize,
}

/// Detailed view of a single batch (includes per-machine health).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDetail {
    pub batch_index: u32,
    pub machine_ids: Vec<String>,
    pub status: BatchStatus,
    pub machine_health: HashMap<String, MachineHealthStatus>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Full detail view of a rollout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutDetail {
    pub id: String,
    pub status: RolloutStatus,
    pub strategy: RolloutStrategy,
    pub release_id: String,
    pub on_failure: OnFailure,
    pub failure_threshold: String,
    pub health_timeout: u64,
    pub batches: Vec<BatchDetail>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: String,
    #[serde(default)]
    pub events: Vec<RolloutEvent>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Only behaviour that is NOT enforced by the `Serialize` / `Deserialize`
    //! derives lives here:
    //!   * `from_str_lc` — custom parse function with case-sensitive
    //!     matching + unknown-variant rejection.
    //!   * `is_active` — non-trivial method on `RolloutStatus`.
    //!   * `#[serde(default)]` shape for `CreateRolloutRequest` — this
    //!     pins the contract that older clients can omit optional
    //!     fields and get predictable defaults.
    //!   * `OnFailure::default()` pins the "pause, don't revert" policy
    //!     choice.

    use super::*;

    #[test]
    fn on_failure_default_is_pause() {
        let default: OnFailure = Default::default();
        assert_eq!(default, OnFailure::Pause);
    }

    #[test]
    fn rollout_status_from_str_lc() {
        assert_eq!(
            RolloutStatus::from_str_lc("created"),
            Some(RolloutStatus::Created)
        );
        assert_eq!(
            RolloutStatus::from_str_lc("running"),
            Some(RolloutStatus::Running)
        );
        assert_eq!(
            RolloutStatus::from_str_lc("cancelled"),
            Some(RolloutStatus::Cancelled)
        );
        assert_eq!(RolloutStatus::from_str_lc("unknown"), None);
    }

    #[test]
    fn rollout_status_is_active_matrix() {
        assert!(RolloutStatus::Created.is_active());
        assert!(RolloutStatus::Running.is_active());
        assert!(RolloutStatus::Paused.is_active());
        assert!(!RolloutStatus::Completed.is_active());
        assert!(!RolloutStatus::Failed.is_active());
        assert!(!RolloutStatus::Cancelled.is_active());
    }

    #[test]
    fn batch_status_from_str_lc() {
        assert_eq!(
            BatchStatus::from_str_lc("pending"),
            Some(BatchStatus::Pending)
        );
        assert_eq!(
            BatchStatus::from_str_lc("waiting_health"),
            Some(BatchStatus::WaitingHealth)
        );
        assert_eq!(BatchStatus::from_str_lc("nope"), None);
    }

    /// Pins every `#[serde(default)]` on `CreateRolloutRequest`. Older
    /// clients that omit optional fields must still produce a valid
    /// request with the documented defaults (on_failure=pause,
    /// failure_threshold="1", health_timeout=None, etc.). This is a
    /// wire-contract test, not a serde-derive test.
    #[test]
    fn create_rollout_request_serde_defaults() {
        let json = r#"{
            "release_id": "rel-abc123",
            "target": {"tags": ["web"]},
            "strategy": "staged"
        }"#;
        let request: CreateRolloutRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.release_id, "rel-abc123");
        assert_eq!(request.on_failure, OnFailure::Pause);
        assert_eq!(request.health_timeout, None);
        assert_eq!(request.cache_url, None);
        assert_eq!(request.batch_sizes, None);
        assert_eq!(request.failure_threshold, "1");
    }
}
