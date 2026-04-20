use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Result of a single health check performed on a machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "status")]
#[non_exhaustive]
pub enum HealthCheckResult {
    Pass {
        check_name: String,
        duration_ms: u64,
    },
    Fail {
        check_name: String,
        duration_ms: u64,
        message: String,
    },
}

impl HealthCheckResult {
    /// Returns `true` if this check passed.
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass { .. })
    }

    /// Returns the name of this check.
    pub fn check_name(&self) -> &str {
        match self {
            Self::Pass { check_name, .. } | Self::Fail { check_name, .. } => check_name,
        }
    }
}

impl fmt::Display for HealthCheckResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pass {
                check_name,
                duration_ms,
            } => write!(f, "PASS: {check_name} ({duration_ms}ms)"),
            Self::Fail {
                check_name,
                duration_ms,
                message,
            } => write!(f, "FAIL: {check_name} ({duration_ms}ms): {message}"),
        }
    }
}

/// Aggregated health report for a machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthReport {
    pub results: Vec<HealthCheckResult>,
    pub all_passed: bool,
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    //! Only the non-derived helpers on `HealthCheckResult` are tested here.
    //! Serde roundtrips are enforced by the `#[derive(Serialize,
    //! Deserialize)]` - compile-time guaranteed - and are exercised
    //! end-to-end by every CP scenario test that posts a real report.

    use super::*;

    #[test]
    fn health_check_result_helpers() {
        let pass = HealthCheckResult::Pass {
            check_name: "disk_space".to_string(),
            duration_ms: 42,
        };
        assert!(pass.is_pass());
        assert_eq!(pass.check_name(), "disk_space");

        let fail = HealthCheckResult::Fail {
            check_name: "http_ping".to_string(),
            duration_ms: 5000,
            message: "timeout".to_string(),
        };
        assert!(!fail.is_pass());
        assert_eq!(fail.check_name(), "http_ping");
    }

    #[test]
    fn display_includes_fail_message() {
        let fail = HealthCheckResult::Fail {
            check_name: "disk".to_string(),
            duration_ms: 12,
            message: "free space below 5%".to_string(),
        };
        let rendered = fail.to_string();
        assert!(rendered.contains("disk"));
        assert!(rendered.contains("12ms"));
        assert!(
            rendered.contains("free space below 5%"),
            "Display should surface the failure message; got {rendered:?}"
        );
    }
}
