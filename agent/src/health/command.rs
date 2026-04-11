use async_trait::async_trait;
use nixfleet_types::health::HealthCheckResult;
use std::time::{Duration, Instant};
use tokio::process::Command;

use super::Check;

/// Runs an arbitrary shell command and checks its exit code.
pub struct CommandChecker {
    pub name: String,
    pub command: String,
    pub timeout_secs: u64,
}

#[async_trait]
impl Check for CommandChecker {
    fn name(&self) -> &str {
        &self.name
    }

    fn check_type(&self) -> &str {
        "command"
    }

    async fn run(&self) -> HealthCheckResult {
        let start = Instant::now();
        // Use an absolute path to /bin/sh so the check does not depend
        // on whatever PATH the parent process (e.g. a systemd service)
        // happens to have. On NixOS, `Command::new("sh")` with no
        // absolute path and no custom PATH fails with ENOENT because
        // the default systemd unit PATH does not include a directory
        // providing `sh`. /bin/sh is a stable binary on every Linux
        // distribution (on NixOS it is a symlink managed by
        // environment.binsh, pointing at bash by default).
        let result = tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            Command::new("/bin/sh")
                .args(["-c", &self.command])
                .output(),
        )
        .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    HealthCheckResult::Pass {
                        check_name: self.name.clone(),
                        duration_ms,
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let code = output.status.code().unwrap_or(-1);
                    HealthCheckResult::Fail {
                        check_name: self.name.clone(),
                        duration_ms,
                        message: format!("exit code {code}: {stderr}"),
                    }
                }
            }
            Ok(Err(e)) => HealthCheckResult::Fail {
                check_name: self.name.clone(),
                duration_ms,
                message: format!("failed to run command: {e}"),
            },
            Err(_) => HealthCheckResult::Fail {
                check_name: self.name.clone(),
                duration_ms,
                message: "timed out".to_string(),
            },
        }
    }
}
