use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod health;
pub mod rollout;

/// Well-known API path constants shared between agent and control plane.
pub mod api {
    /// GET: Returns the desired generation for a machine.
    /// Path parameter: `{id}` = machine ID.
    pub const DESIRED_GENERATION: &str = "/api/v1/machines/{id}/desired-generation";

    /// POST: Agent reports its status after a state transition.
    /// Path parameter: `{id}` = machine ID.
    pub const REPORT: &str = "/api/v1/machines/{id}/report";

    /// GET: List all known machines and their status.
    pub const MACHINES: &str = "/api/v1/machines";

    /// POST: Set the desired generation for a machine (admin endpoint).
    /// Path parameter: `{id}` = machine ID.
    pub const SET_GENERATION: &str = "/api/v1/machines/{id}/set-generation";

    /// POST: Pre-register a machine (admin endpoint).
    /// Path parameter: `{id}` = machine ID.
    pub const REGISTER: &str = "/api/v1/machines/{id}/register";

    /// PATCH: Change machine lifecycle state (admin endpoint).
    /// Path parameter: `{id}` = machine ID.
    pub const LIFECYCLE: &str = "/api/v1/machines/{id}/lifecycle";

    /// GET: List audit events with optional filters.
    pub const AUDIT: &str = "/api/v1/audit";

    /// GET: List rollouts. POST: Create a new rollout.
    pub const ROLLOUTS: &str = "/api/v1/rollouts";

    /// GET: Get rollout detail.
    /// Path parameter: `{id}` = rollout ID.
    pub const ROLLOUT: &str = "/api/v1/rollouts/{id}";

    /// POST: Resume a paused rollout.
    /// Path parameter: `{id}` = rollout ID.
    pub const ROLLOUT_RESUME: &str = "/api/v1/rollouts/{id}/resume";

    /// POST: Cancel a rollout.
    /// Path parameter: `{id}` = rollout ID.
    pub const ROLLOUT_CANCEL: &str = "/api/v1/rollouts/{id}/cancel";

    /// GET/PUT: Manage tags for a machine.
    /// Path parameter: `{id}` = machine ID.
    pub const MACHINE_TAGS: &str = "/api/v1/machines/{id}/tags";

    /// DELETE: Remove a specific tag from a machine.
    /// Path parameters: `{id}` = machine ID, `{tag}` = tag name.
    pub const MACHINE_TAG: &str = "/api/v1/machines/{id}/tags/{tag}";
}

/// Machine lifecycle states for fleet management.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MachineLifecycle {
    /// Pre-registered, no agent report yet.
    Pending,
    /// Install in progress.
    Provisioning,
    /// Agent reporting normally.
    #[default]
    Active,
    /// Manually paused (skip deploys).
    Maintenance,
    /// Removed from fleet.
    Decommissioned,
}

impl fmt::Display for MachineLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Provisioning => write!(f, "provisioning"),
            Self::Active => write!(f, "active"),
            Self::Maintenance => write!(f, "maintenance"),
            Self::Decommissioned => write!(f, "decommissioned"),
        }
    }
}

impl MachineLifecycle {
    /// Parse a lifecycle state from a string (as stored in the database).
    pub fn from_str_lc(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "provisioning" => Some(Self::Provisioning),
            "active" => Some(Self::Active),
            "maintenance" => Some(Self::Maintenance),
            "decommissioned" => Some(Self::Decommissioned),
            _ => None,
        }
    }

    /// Check if a transition from the current state to `target` is valid.
    pub fn can_transition_to(&self, target: &Self) -> bool {
        matches!(
            (self, target),
            (Self::Pending, Self::Active)
                | (Self::Pending, Self::Provisioning)
                | (Self::Pending, Self::Decommissioned)
                | (Self::Provisioning, Self::Active)
                | (Self::Provisioning, Self::Pending)
                | (Self::Active, Self::Maintenance)
                | (Self::Active, Self::Decommissioned)
                | (Self::Maintenance, Self::Active)
                | (Self::Maintenance, Self::Decommissioned)
        )
    }
}

/// Desired generation returned by the control plane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesiredGeneration {
    /// Full nix store path, e.g. `/nix/store/abc123...-nixos-system-web-01-25.05`
    pub hash: String,
    /// Optional cache URL override (per-generation)
    #[serde(default)]
    pub cache_url: Option<String>,
}

/// Report sent to the control plane after each state transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub machine_id: String,
    pub current_generation: String,
    pub success: bool,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub health: Option<health::HealthReport>,
}

/// Machine status for inventory reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineStatus {
    pub machine_id: String,
    pub current_generation: String,
    pub desired_generation: Option<String>,
    pub agent_version: String,
    pub system_state: String,
    pub uptime_seconds: u64,
    pub last_report: Option<DateTime<Utc>>,
    pub lifecycle: MachineLifecycle,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Audit event for compliance reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: i64,
    pub timestamp: String,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_report_serialization() {
        let report = Report {
            machine_id: "web-01".to_string(),
            current_generation: "/nix/store/abc123-nixos-system".to_string(),
            success: true,
            message: "deployed".to_string(),
            timestamp: Utc::now(),
            tags: vec!["web".to_string(), "prod".to_string()],
            health: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(report.machine_id, back.machine_id);
        assert_eq!(report.current_generation, back.current_generation);
        assert_eq!(report.success, back.success);
        assert_eq!(report.message, back.message);
    }

    #[test]
    fn test_report_failure_serialization() {
        let report = Report {
            machine_id: "dev-01".to_string(),
            current_generation: "/nix/store/xyz789-nixos-system".to_string(),
            success: false,
            message: "rolled back: health check failed".to_string(),
            timestamp: Utc::now(),
            tags: vec![],
            health: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(report.machine_id, back.machine_id);
        assert!(!back.success);
        assert!(back.message.contains("rolled back"));
    }

    #[test]
    fn test_desired_generation_deserialization() {
        let json = r#"{"hash": "/nix/store/abc123-nixos-system"}"#;
        let gen: DesiredGeneration = serde_json::from_str(json).unwrap();
        assert_eq!(gen.hash, "/nix/store/abc123-nixos-system");
        assert!(gen.cache_url.is_none());
    }

    #[test]
    fn test_desired_generation_with_cache_url() {
        let json = r#"{"hash": "/nix/store/abc123-nixos-system", "cache_url": "https://cache.example.com"}"#;
        let gen: DesiredGeneration = serde_json::from_str(json).unwrap();
        assert_eq!(gen.hash, "/nix/store/abc123-nixos-system");
        assert_eq!(gen.cache_url, Some("https://cache.example.com".to_string()));
    }

    #[test]
    fn test_desired_generation_serialization_roundtrip() {
        let gen = DesiredGeneration {
            hash: "/nix/store/def456-nixos-system-web-01-25.05".to_string(),
            cache_url: Some("https://cache.nixos.org".to_string()),
        };
        let json = serde_json::to_string(&gen).unwrap();
        let back: DesiredGeneration = serde_json::from_str(&json).unwrap();
        assert_eq!(gen.hash, back.hash);
        assert_eq!(gen.cache_url, back.cache_url);
    }

    #[test]
    fn test_desired_generation_equality() {
        let a = DesiredGeneration {
            hash: "/nix/store/abc123".to_string(),
            cache_url: None,
        };
        let b = DesiredGeneration {
            hash: "/nix/store/abc123".to_string(),
            cache_url: None,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_machine_status_serialization() {
        let status = MachineStatus {
            machine_id: "web-01".to_string(),
            current_generation: "/nix/store/abc123-nixos-system".to_string(),
            desired_generation: Some("/nix/store/def456-nixos-system".to_string()),
            agent_version: "0.1.0".to_string(),
            system_state: "running".to_string(),
            uptime_seconds: 3600,
            last_report: Some(Utc::now()),
            lifecycle: MachineLifecycle::Active,
            tags: vec!["web".to_string()],
        };
        let json = serde_json::to_string(&status).unwrap();
        let back: MachineStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status.machine_id, back.machine_id);
        assert_eq!(status.uptime_seconds, back.uptime_seconds);
        assert_eq!(back.lifecycle, MachineLifecycle::Active);
    }

    #[test]
    fn test_lifecycle_serialization() {
        let lc = MachineLifecycle::Pending;
        let json = serde_json::to_string(&lc).unwrap();
        assert_eq!(json, "\"pending\"");
        let back: MachineLifecycle = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MachineLifecycle::Pending);
    }

    #[test]
    fn test_lifecycle_display() {
        assert_eq!(MachineLifecycle::Active.to_string(), "active");
        assert_eq!(MachineLifecycle::Pending.to_string(), "pending");
        assert_eq!(MachineLifecycle::Maintenance.to_string(), "maintenance");
        assert_eq!(
            MachineLifecycle::Decommissioned.to_string(),
            "decommissioned"
        );
    }

    #[test]
    fn test_lifecycle_from_str() {
        assert_eq!(
            MachineLifecycle::from_str_lc("pending"),
            Some(MachineLifecycle::Pending)
        );
        assert_eq!(
            MachineLifecycle::from_str_lc("active"),
            Some(MachineLifecycle::Active)
        );
        assert_eq!(MachineLifecycle::from_str_lc("invalid"), None);
    }

    #[test]
    fn test_lifecycle_valid_transitions() {
        assert!(MachineLifecycle::Pending.can_transition_to(&MachineLifecycle::Active));
        assert!(MachineLifecycle::Pending.can_transition_to(&MachineLifecycle::Decommissioned));
        assert!(MachineLifecycle::Active.can_transition_to(&MachineLifecycle::Maintenance));
        assert!(MachineLifecycle::Active.can_transition_to(&MachineLifecycle::Decommissioned));
        assert!(MachineLifecycle::Maintenance.can_transition_to(&MachineLifecycle::Active));
    }

    #[test]
    fn test_lifecycle_invalid_transitions() {
        assert!(!MachineLifecycle::Active.can_transition_to(&MachineLifecycle::Pending));
        assert!(!MachineLifecycle::Decommissioned.can_transition_to(&MachineLifecycle::Active));
        assert!(!MachineLifecycle::Maintenance.can_transition_to(&MachineLifecycle::Pending));
    }

    #[test]
    fn test_report_json_contains_expected_fields() {
        let report = Report {
            machine_id: "mac-01".to_string(),
            current_generation: "/nix/store/ghi012-nixos-system".to_string(),
            success: true,
            message: "up-to-date".to_string(),
            timestamp: Utc::now(),
            tags: vec!["staging".to_string()],
            health: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("machine_id"));
        assert!(json.contains("mac-01"));
        assert!(json.contains("success"));
        assert!(json.contains("timestamp"));
    }

    #[test]
    fn test_api_paths_are_consistent() {
        assert!(api::DESIRED_GENERATION.starts_with("/api/v1/machines/"));
        assert!(api::REPORT.starts_with("/api/v1/machines/"));
        assert!(api::MACHINES.starts_with("/api/v1/machines"));
        assert!(api::SET_GENERATION.starts_with("/api/v1/machines/"));
    }
}
