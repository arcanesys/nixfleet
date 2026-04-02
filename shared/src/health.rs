use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Result of a single health check performed on a machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "status")]
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
                ..
            } => write!(f, "PASS: {} ({}ms)", check_name, duration_ms),
            Self::Fail {
                check_name,
                duration_ms,
                ..
            } => write!(f, "FAIL: {} ({}ms)", check_name, duration_ms),
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
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_health_check_pass_roundtrip() {
        let check = HealthCheckResult::Pass {
            check_name: "disk_space".to_string(),
            duration_ms: 42,
        };
        let json = serde_json::to_string(&check).unwrap();
        let back: HealthCheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(check, back);
        assert!(back.is_pass());
        assert_eq!(back.check_name(), "disk_space");
    }

    #[test]
    fn test_health_check_fail_roundtrip() {
        let check = HealthCheckResult::Fail {
            check_name: "http_ping".to_string(),
            duration_ms: 5000,
            message: "timeout".to_string(),
        };
        let json = serde_json::to_string(&check).unwrap();
        let back: HealthCheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(check, back);
        assert!(!back.is_pass());
        assert_eq!(back.check_name(), "http_ping");
    }

    #[test]
    fn test_health_report_roundtrip() {
        let report = HealthReport {
            results: vec![
                HealthCheckResult::Pass {
                    check_name: "disk_space".to_string(),
                    duration_ms: 10,
                },
                HealthCheckResult::Fail {
                    check_name: "memory".to_string(),
                    duration_ms: 5,
                    message: "low".to_string(),
                },
            ],
            all_passed: false,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: HealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report.results.len(), back.results.len());
        assert!(!back.all_passed);
    }

    #[test]
    fn test_health_report_all_passed() {
        let report = HealthReport {
            results: vec![HealthCheckResult::Pass {
                check_name: "nix_daemon".to_string(),
                duration_ms: 3,
            }],
            all_passed: true,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: HealthReport = serde_json::from_str(&json).unwrap();
        assert!(back.all_passed);
        assert_eq!(back.results.len(), 1);
    }

    #[test]
    fn test_health_check_display() {
        let pass = HealthCheckResult::Pass {
            check_name: "disk".to_string(),
            duration_ms: 10,
        };
        assert_eq!(pass.to_string(), "PASS: disk (10ms)");

        let fail = HealthCheckResult::Fail {
            check_name: "mem".to_string(),
            duration_ms: 5,
            message: "low".to_string(),
        };
        assert_eq!(fail.to_string(), "FAIL: mem (5ms)");
    }
}
