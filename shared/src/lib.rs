use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod health;
pub mod metrics;
pub mod release;
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

    /// Tags base path for a machine.
    /// Path parameter: `{id}` = machine ID.
    pub const MACHINE_TAGS: &str = "/api/v1/machines/{id}/tags";

    /// DELETE: Remove a specific tag from a machine.
    /// Path parameters: `{id}` = machine ID, `{tag}` = tag name.
    pub const MACHINE_TAG: &str = "/api/v1/machines/{id}/tags/{tag}";

    /// GET: List releases. POST: Create a new release.
    pub const RELEASES: &str = "/api/v1/releases";

    /// GET: Get release detail. DELETE: Delete a release.
    /// Path parameter: `{id}` = release ID.
    pub const RELEASE: &str = "/api/v1/releases/{id}";

    /// GET: Diff two releases.
    /// Path parameters: `{id}` = release A, `{other_id}` = release B.
    pub const RELEASE_DIFF: &str = "/api/v1/releases/{id}/diff/{other_id}";
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
    /// Suggested poll interval in seconds (control plane hints faster polling during rollouts)
    #[serde(default)]
    pub poll_hint: Option<u64>,
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
    /// Wire contract for `DesiredGeneration`: `hash` is required,
    /// `cache_url` and `poll_hint` are optional and must default to
    /// `None` when absent. Pins the shape agents parse from the CP's
    /// `/desired-generation` response.
    #[test]
    fn desired_generation_serde_defaults() {
        // Minimal payload — only `hash`.
        let minimal: DesiredGeneration =
            serde_json::from_str(r#"{"hash": "/nix/store/abc"}"#).unwrap();
        assert_eq!(minimal.hash, "/nix/store/abc");
        assert!(minimal.cache_url.is_none());
        assert!(minimal.poll_hint.is_none());

        // Full payload — all three optional fields populated.
        let full: DesiredGeneration = serde_json::from_str(
            r#"{"hash": "/nix/store/abc", "cache_url": "https://c", "poll_hint": 5}"#,
        )
        .unwrap();
        assert_eq!(full.cache_url.as_deref(), Some("https://c"));
        assert_eq!(full.poll_hint, Some(5));
    }

    #[test]
    fn test_api_paths_are_consistent() {
        assert!(api::DESIRED_GENERATION.starts_with("/api/v1/machines/"));
        assert!(api::REPORT.starts_with("/api/v1/machines/"));
        assert!(api::MACHINES.starts_with("/api/v1/machines"));
    }

    /// Exhaustive matrix for `can_transition_to`. Pins every cell of
    /// the 5×5 lifecycle state graph so adding or removing a valid
    /// transition requires updating both the impl AND this table.
    /// Self-transitions are invalid per the current impl.
    #[test]
    fn test_lifecycle_can_transition_to_full_matrix() {
        use MachineLifecycle::*;

        // (from, to, expected_valid)
        let cases: &[(MachineLifecycle, MachineLifecycle, bool)] = &[
            // From Pending
            (Pending, Pending, false),
            (Pending, Provisioning, true),
            (Pending, Active, true),
            (Pending, Maintenance, false),
            (Pending, Decommissioned, true),
            // From Provisioning
            (Provisioning, Pending, true),
            (Provisioning, Provisioning, false),
            (Provisioning, Active, true),
            (Provisioning, Maintenance, false),
            (Provisioning, Decommissioned, false),
            // From Active
            (Active, Pending, false),
            (Active, Provisioning, false),
            (Active, Active, false),
            (Active, Maintenance, true),
            (Active, Decommissioned, true),
            // From Maintenance
            (Maintenance, Pending, false),
            (Maintenance, Provisioning, false),
            (Maintenance, Active, true),
            (Maintenance, Maintenance, false),
            (Maintenance, Decommissioned, true),
            // From Decommissioned (terminal)
            (Decommissioned, Pending, false),
            (Decommissioned, Provisioning, false),
            (Decommissioned, Active, false),
            (Decommissioned, Maintenance, false),
            (Decommissioned, Decommissioned, false),
        ];

        for (from, to, expected) in cases {
            assert_eq!(
                from.can_transition_to(to),
                *expected,
                "transition {from:?} → {to:?}: expected {expected}, got {}",
                from.can_transition_to(to)
            );
        }
    }

    /// from_str_lc must round-trip every variant + reject unknown.
    #[test]
    fn test_lifecycle_from_str_lc_exhaustive() {
        use MachineLifecycle::*;
        for v in [Pending, Provisioning, Active, Maintenance, Decommissioned] {
            let s = v.to_string();
            assert_eq!(
                MachineLifecycle::from_str_lc(&s),
                Some(v.clone()),
                "round-trip failed for {v:?}"
            );
        }
        assert_eq!(MachineLifecycle::from_str_lc("not-a-state"), None);
        assert_eq!(MachineLifecycle::from_str_lc(""), None);
        assert_eq!(MachineLifecycle::from_str_lc("ACTIVE"), None); // case-sensitive
    }
}
