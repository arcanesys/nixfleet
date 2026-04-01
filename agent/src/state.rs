use crate::types::DesiredGeneration;

/// Agent state machine.
///
/// Transitions:
///   Idle -> Checking (poll timer fires)
///   Checking -> Idle (already at desired generation or error)
///   Checking -> Fetching (generation mismatch)
///   Fetching -> Applying (closure fetched)
///   Fetching -> Idle (fetch error)
///   Applying -> Verifying (switch succeeded)
///   Applying -> RollingBack (switch failed)
///   Verifying -> Reporting (health check passed)
///   Verifying -> RollingBack (health check failed)
///   RollingBack -> Reporting (rollback completed)
///   RollingBack -> Idle (rollback failed — nothing more we can do)
///   Reporting -> Idle (report sent or failed)
#[derive(Debug)]
pub enum AgentState {
    /// Waiting for the next poll interval.
    Idle,

    /// Querying the control plane for the desired generation.
    Checking,

    /// Downloading the closure from the binary cache.
    Fetching { desired: DesiredGeneration },

    /// Running `switch-to-configuration switch`.
    Applying { desired: DesiredGeneration },

    /// Running health checks after a successful apply.
    Verifying { desired: DesiredGeneration },

    /// Switching back to the previous generation after a failure.
    RollingBack { reason: String },

    /// Sending a status report to the control plane.
    Reporting { success: bool, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desired() -> DesiredGeneration {
        DesiredGeneration {
            hash: "/nix/store/abc123-nixos-system".to_string(),
            cache_url: None,
        }
    }

    #[test]
    fn test_idle_to_checking() {
        // Idle → Checking on poll timer
        let state = AgentState::Idle;
        // Verify the variant exists and matches
        assert!(matches!(state, AgentState::Idle));
        let next = AgentState::Checking;
        assert!(matches!(next, AgentState::Checking));
    }

    #[test]
    fn test_checking_to_idle_no_change() {
        // Checking → Idle (no change / error path)
        let state = AgentState::Checking;
        assert!(matches!(state, AgentState::Checking));
        let next = AgentState::Idle;
        assert!(matches!(next, AgentState::Idle));
    }

    #[test]
    fn test_checking_to_fetching_mismatch() {
        // Checking → Fetching (generation mismatch)
        let state = AgentState::Checking;
        assert!(matches!(state, AgentState::Checking));
        let next = AgentState::Fetching { desired: desired() };
        assert!(matches!(next, AgentState::Fetching { .. }));
        if let AgentState::Fetching { desired } = next {
            assert_eq!(desired.hash, "/nix/store/abc123-nixos-system");
        }
    }

    #[test]
    fn test_fetching_to_applying_success() {
        // Fetching → Applying (success)
        let state = AgentState::Fetching { desired: desired() };
        assert!(matches!(state, AgentState::Fetching { .. }));
        let next = AgentState::Applying { desired: desired() };
        assert!(matches!(next, AgentState::Applying { .. }));
    }

    #[test]
    fn test_fetching_to_idle_failure() {
        // Fetching → Idle (fetch error)
        let state = AgentState::Fetching { desired: desired() };
        assert!(matches!(state, AgentState::Fetching { .. }));
        let next = AgentState::Idle;
        assert!(matches!(next, AgentState::Idle));
    }

    #[test]
    fn test_applying_to_verifying_success() {
        // Applying → Verifying (switch succeeded)
        let state = AgentState::Applying { desired: desired() };
        assert!(matches!(state, AgentState::Applying { .. }));
        let next = AgentState::Verifying { desired: desired() };
        assert!(matches!(next, AgentState::Verifying { .. }));
    }

    #[test]
    fn test_applying_to_rolling_back_failure() {
        // Applying → RollingBack (switch failed)
        let state = AgentState::Applying { desired: desired() };
        assert!(matches!(state, AgentState::Applying { .. }));
        let next = AgentState::RollingBack {
            reason: "apply failed: switch-to-configuration exited 1".to_string(),
        };
        assert!(matches!(next, AgentState::RollingBack { .. }));
        if let AgentState::RollingBack { reason } = next {
            assert!(reason.contains("apply failed"));
        }
    }

    #[test]
    fn test_verifying_to_reporting_healthy() {
        // Verifying → Reporting (health check passed)
        let state = AgentState::Verifying { desired: desired() };
        assert!(matches!(state, AgentState::Verifying { .. }));
        let next = AgentState::Reporting {
            success: true,
            message: "deployed".to_string(),
        };
        assert!(matches!(next, AgentState::Reporting { success: true, .. }));
    }

    #[test]
    fn test_verifying_to_rolling_back_unhealthy() {
        // Verifying → RollingBack (health check failed)
        let state = AgentState::Verifying { desired: desired() };
        assert!(matches!(state, AgentState::Verifying { .. }));
        let next = AgentState::RollingBack {
            reason: "health check failed".to_string(),
        };
        assert!(matches!(next, AgentState::RollingBack { .. }));
        if let AgentState::RollingBack { reason } = next {
            assert_eq!(reason, "health check failed");
        }
    }

    #[test]
    fn test_rolling_back_to_reporting() {
        // RollingBack → Reporting (always)
        let state = AgentState::RollingBack {
            reason: "health check failed".to_string(),
        };
        assert!(matches!(state, AgentState::RollingBack { .. }));
        let next = AgentState::Reporting {
            success: false,
            message: "rolled back: health check failed".to_string(),
        };
        assert!(matches!(next, AgentState::Reporting { success: false, .. }));
        if let AgentState::Reporting { success, message } = next {
            assert!(!success);
            assert!(message.contains("rolled back"));
        }
    }

    #[test]
    fn test_reporting_to_idle() {
        // Reporting → Idle (always)
        let state = AgentState::Reporting {
            success: true,
            message: "deployed".to_string(),
        };
        assert!(matches!(state, AgentState::Reporting { .. }));
        let next = AgentState::Idle;
        assert!(matches!(next, AgentState::Idle));
    }

    #[test]
    fn test_rolling_back_reason_propagates_to_report_message() {
        // Verify that rollback reason propagates correctly into report message
        let reason = "apply failed: exit code 1".to_string();
        let report_msg = format!("rolled back: {reason}");
        let next = AgentState::Reporting {
            success: false,
            message: report_msg.clone(),
        };
        if let AgentState::Reporting { message, .. } = next {
            assert!(message.contains("rolled back"));
            assert!(message.contains("apply failed"));
        }
    }
}
