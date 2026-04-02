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
#[serde(rename_all = "snake_case", tag = "status")]
pub enum MachineHealthStatus {
    Pending,
    Healthy,
    Unhealthy { reason: String },
    TimedOut,
    RolledBack,
}

impl fmt::Display for MachineHealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Healthy => write!(f, "healthy"),
            Self::Unhealthy { reason } => write!(f, "unhealthy: {}", reason),
            Self::TimedOut => write!(f, "timed_out"),
            Self::RolledBack => write!(f, "rolled_back"),
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
// Request / Response types
// ---------------------------------------------------------------------------

/// Request body to create a new rollout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRolloutRequest {
    pub generation_hash: String,
    pub target: RolloutTarget,
    pub strategy: RolloutStrategy,
    #[serde(default)]
    pub on_failure: OnFailure,
    #[serde(default)]
    pub health_check_timeout_seconds: Option<u64>,
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
    pub status: RolloutStatus,
    pub batches: Vec<BatchSummary>,
    pub created_at: DateTime<Utc>,
}

/// Detailed view of a single batch (includes per-machine health).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDetail {
    pub batch_index: u32,
    pub machine_ids: Vec<String>,
    pub status: BatchStatus,
    pub machine_health: HashMap<String, MachineHealthStatus>,
}

/// Full detail view of a rollout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutDetail {
    pub rollout_id: String,
    pub generation_hash: String,
    pub target: RolloutTarget,
    pub strategy: RolloutStrategy,
    pub on_failure: OnFailure,
    pub status: RolloutStatus,
    pub batches: Vec<BatchDetail>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // -- RolloutStrategy --

    #[test]
    fn test_rollout_strategy_roundtrip() {
        for strategy in [
            RolloutStrategy::Canary,
            RolloutStrategy::Staged,
            RolloutStrategy::AllAtOnce,
        ] {
            let json = serde_json::to_string(&strategy).unwrap();
            let back: RolloutStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(strategy, back);
        }
    }

    #[test]
    fn test_rollout_strategy_display() {
        assert_eq!(RolloutStrategy::Canary.to_string(), "canary");
        assert_eq!(RolloutStrategy::Staged.to_string(), "staged");
        assert_eq!(RolloutStrategy::AllAtOnce.to_string(), "all_at_once");
    }

    // -- OnFailure --

    #[test]
    fn test_on_failure_default_is_pause() {
        let default: OnFailure = Default::default();
        assert_eq!(default, OnFailure::Pause);
    }

    #[test]
    fn test_on_failure_roundtrip() {
        for variant in [OnFailure::Pause, OnFailure::Revert] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: OnFailure = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    // -- RolloutStatus --

    #[test]
    fn test_rollout_status_roundtrip() {
        for status in [
            RolloutStatus::Created,
            RolloutStatus::Running,
            RolloutStatus::Paused,
            RolloutStatus::Completed,
            RolloutStatus::Failed,
            RolloutStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: RolloutStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn test_rollout_status_from_str_lc() {
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
    fn test_rollout_status_is_active() {
        assert!(RolloutStatus::Created.is_active());
        assert!(RolloutStatus::Running.is_active());
        assert!(RolloutStatus::Paused.is_active());
        assert!(!RolloutStatus::Completed.is_active());
        assert!(!RolloutStatus::Failed.is_active());
        assert!(!RolloutStatus::Cancelled.is_active());
    }

    #[test]
    fn test_rollout_status_display() {
        assert_eq!(RolloutStatus::Created.to_string(), "created");
        assert_eq!(RolloutStatus::Running.to_string(), "running");
        assert_eq!(RolloutStatus::Failed.to_string(), "failed");
    }

    // -- BatchStatus --

    #[test]
    fn test_batch_status_roundtrip() {
        for status in [
            BatchStatus::Pending,
            BatchStatus::Deploying,
            BatchStatus::WaitingHealth,
            BatchStatus::Succeeded,
            BatchStatus::Failed,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: BatchStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn test_batch_status_from_str_lc() {
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

    #[test]
    fn test_batch_status_display() {
        assert_eq!(BatchStatus::Pending.to_string(), "pending");
        assert_eq!(BatchStatus::WaitingHealth.to_string(), "waiting_health");
    }

    // -- MachineHealthStatus --

    #[test]
    fn test_machine_health_status_roundtrip() {
        let variants = vec![
            MachineHealthStatus::Pending,
            MachineHealthStatus::Healthy,
            MachineHealthStatus::Unhealthy {
                reason: "disk full".to_string(),
            },
            MachineHealthStatus::TimedOut,
            MachineHealthStatus::RolledBack,
        ];
        for variant in variants {
            let json = serde_json::to_string(&variant).unwrap();
            let back: MachineHealthStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn test_machine_health_status_display() {
        assert_eq!(MachineHealthStatus::Pending.to_string(), "pending");
        assert_eq!(MachineHealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(
            MachineHealthStatus::Unhealthy {
                reason: "oom".to_string()
            }
            .to_string(),
            "unhealthy: oom"
        );
        assert_eq!(MachineHealthStatus::TimedOut.to_string(), "timed_out");
        assert_eq!(MachineHealthStatus::RolledBack.to_string(), "rolled_back");
    }

    // -- RolloutTarget --

    #[test]
    fn test_rollout_target_tags_roundtrip() {
        let target = RolloutTarget::Tags(vec!["web".to_string(), "prod".to_string()]);
        let json = serde_json::to_string(&target).unwrap();
        let back: RolloutTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(target, back);
    }

    #[test]
    fn test_rollout_target_hosts_roundtrip() {
        let target = RolloutTarget::Hosts(vec!["web-01".to_string(), "web-02".to_string()]);
        let json = serde_json::to_string(&target).unwrap();
        let back: RolloutTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(target, back);
    }

    // -- CreateRolloutRequest --

    #[test]
    fn test_create_rollout_request_roundtrip() {
        let request = CreateRolloutRequest {
            generation_hash: "/nix/store/abc123-nixos-system".to_string(),
            target: RolloutTarget::Tags(vec!["web".to_string()]),
            strategy: RolloutStrategy::Canary,
            on_failure: OnFailure::Pause,
            health_check_timeout_seconds: Some(300),
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: CreateRolloutRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.generation_hash, request.generation_hash);
        assert_eq!(back.strategy, RolloutStrategy::Canary);
        assert_eq!(back.on_failure, OnFailure::Pause);
        assert_eq!(back.health_check_timeout_seconds, Some(300));
    }

    #[test]
    fn test_create_rollout_request_defaults() {
        let json = r#"{
            "generation_hash": "/nix/store/abc123",
            "target": {"tags": ["web"]},
            "strategy": "staged"
        }"#;
        let request: CreateRolloutRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.on_failure, OnFailure::Pause);
        assert_eq!(request.health_check_timeout_seconds, None);
    }

    // -- CreateRolloutResponse --

    #[test]
    fn test_create_rollout_response_roundtrip() {
        let response = CreateRolloutResponse {
            rollout_id: "r-001".to_string(),
            status: RolloutStatus::Created,
            batches: vec![BatchSummary {
                batch_index: 0,
                machine_ids: vec!["web-01".to_string()],
                status: BatchStatus::Pending,
            }],
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let back: CreateRolloutResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rollout_id, "r-001");
        assert_eq!(back.status, RolloutStatus::Created);
        assert_eq!(back.batches.len(), 1);
    }

    // -- RolloutDetail --

    #[test]
    fn test_rollout_detail_roundtrip() {
        let mut machine_health = HashMap::new();
        machine_health.insert(
            "web-01".to_string(),
            MachineHealthStatus::Healthy,
        );
        machine_health.insert(
            "web-02".to_string(),
            MachineHealthStatus::Unhealthy {
                reason: "health check timeout".to_string(),
            },
        );

        let detail = RolloutDetail {
            rollout_id: "r-002".to_string(),
            generation_hash: "/nix/store/def456-nixos-system".to_string(),
            target: RolloutTarget::Hosts(vec!["web-01".to_string(), "web-02".to_string()]),
            strategy: RolloutStrategy::Staged,
            on_failure: OnFailure::Revert,
            status: RolloutStatus::Running,
            batches: vec![BatchDetail {
                batch_index: 0,
                machine_ids: vec!["web-01".to_string(), "web-02".to_string()],
                status: BatchStatus::WaitingHealth,
                machine_health,
            }],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&detail).unwrap();
        let back: RolloutDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rollout_id, "r-002");
        assert_eq!(back.strategy, RolloutStrategy::Staged);
        assert_eq!(back.on_failure, OnFailure::Revert);
        assert_eq!(back.batches.len(), 1);
        assert_eq!(back.batches[0].machine_health.len(), 2);
    }
}
