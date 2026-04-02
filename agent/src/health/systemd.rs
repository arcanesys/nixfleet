use async_trait::async_trait;
use nixfleet_types::health::HealthCheckResult;
use std::time::Instant;
use tokio::process::Command;

use super::Check;

/// Checks whether a specific systemd unit is active.
pub struct SystemdChecker {
    pub unit: String,
}

#[async_trait]
impl Check for SystemdChecker {
    fn name(&self) -> &str {
        &self.unit
    }

    async fn run(&self) -> HealthCheckResult {
        let start = Instant::now();
        let result = Command::new("systemctl")
            .args(["is-active", &self.unit])
            .output()
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if state == "active" {
                    HealthCheckResult::Pass {
                        check_name: self.unit.clone(),
                        duration_ms,
                    }
                } else {
                    HealthCheckResult::Fail {
                        check_name: self.unit.clone(),
                        duration_ms,
                        message: format!("unit state: {state}"),
                    }
                }
            }
            Err(e) => HealthCheckResult::Fail {
                check_name: self.unit.clone(),
                duration_ms,
                message: format!("failed to run systemctl: {e}"),
            },
        }
    }
}

/// Fallback check: runs `systemctl is-system-running`.
/// Returns Pass if the system is "running" or "degraded".
pub struct SystemdFallback;

#[async_trait]
impl Check for SystemdFallback {
    fn name(&self) -> &str {
        "systemd-system"
    }

    async fn run(&self) -> HealthCheckResult {
        let start = Instant::now();
        let result = Command::new("systemctl")
            .arg("is-system-running")
            .output()
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if state == "running" || state == "degraded" {
                    HealthCheckResult::Pass {
                        check_name: "systemd-system".to_string(),
                        duration_ms,
                    }
                } else {
                    HealthCheckResult::Fail {
                        check_name: "systemd-system".to_string(),
                        duration_ms,
                        message: format!("system state: {state}"),
                    }
                }
            }
            Err(e) => HealthCheckResult::Fail {
                check_name: "systemd-system".to_string(),
                duration_ms,
                message: format!("failed to run systemctl: {e}"),
            },
        }
    }
}
