use async_trait::async_trait;
use nixfleet_types::health::HealthCheckResult;
use std::time::Instant;
use tokio::process::Command;

use super::Check;

/// Checks whether a specific launchd service is loaded and running.
///
/// Uses `launchctl list <label>` - exit code 0 means the service is loaded.
/// Then parses stdout for `"PID" = <number>` to confirm it's actually running
/// (a loaded-but-stopped service also returns exit 0).
pub struct LaunchdChecker {
    pub label: String,
}

#[async_trait]
impl Check for LaunchdChecker {
    fn name(&self) -> &str {
        &self.label
    }

    fn check_type(&self) -> &str {
        "launchd"
    }

    async fn run(&self) -> HealthCheckResult {
        let start = Instant::now();
        let result = Command::new("launchctl")
            .args(["list", &self.label])
            .output()
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                if !output.status.success() {
                    return HealthCheckResult::Fail {
                        check_name: self.label.clone(),
                        duration_ms,
                        message: "service not loaded".to_string(),
                    };
                }
                let stdout = String::from_utf8_lossy(&output.stdout);
                let has_pid = stdout.lines().any(|line| {
                    let trimmed = line.trim();
                    trimmed.starts_with("\"PID\"") || trimmed.starts_with("PID")
                });
                if has_pid {
                    HealthCheckResult::Pass {
                        check_name: self.label.clone(),
                        duration_ms,
                    }
                } else {
                    HealthCheckResult::Fail {
                        check_name: self.label.clone(),
                        duration_ms,
                        message: "service loaded but not running (no PID)".to_string(),
                    }
                }
            }
            Err(e) => HealthCheckResult::Fail {
                check_name: self.label.clone(),
                duration_ms,
                message: format!("failed to run launchctl: {e}"),
            },
        }
    }
}

/// Fallback check for macOS: verifies the system is responsive.
pub struct LaunchdFallback;

#[async_trait]
impl Check for LaunchdFallback {
    fn name(&self) -> &str {
        "launchd-system"
    }

    fn check_type(&self) -> &str {
        "launchd"
    }

    async fn run(&self) -> HealthCheckResult {
        let start = Instant::now();
        let result = Command::new("launchctl")
            .arg("list")
            .output()
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(output) if output.status.success() => HealthCheckResult::Pass {
                check_name: "launchd-system".to_string(),
                duration_ms,
            },
            Ok(output) => HealthCheckResult::Fail {
                check_name: "launchd-system".to_string(),
                duration_ms,
                message: format!(
                    "launchctl list failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            },
            Err(e) => HealthCheckResult::Fail {
                check_name: "launchd-system".to_string(),
                duration_ms,
                message: format!("failed to run launchctl: {e}"),
            },
        }
    }
}
