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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use tokio::sync::{Mutex, MutexGuard};

    /// Process-wide lock serializing tests that mutate `PATH`. cargo test
    /// runs tests in parallel; std::env mutations are global so two
    /// concurrent tests touching PATH would race. Every PATH-mutating
    /// test below acquires this lock.
    ///
    /// `tokio::sync::Mutex` (not `std::sync::Mutex`) because the guard
    /// has to be held across an `.await` point — the checker's `run`
    /// future must execute with the modified PATH in place. A blocking
    /// std Mutex held across await would trip `clippy::await_holding_lock`
    /// and can deadlock the tokio runtime under contention.
    async fn path_env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().await
    }

    /// `CommandChecker` must use the absolute `/bin/sh` rather than a
    /// relative `sh` lookup. When systemd starts the agent unit the
    /// default service PATH does not include `/run/current-system/sw/bin`,
    /// so a relative `Command::new("sh")` fails with ENOENT before the
    /// shell even parses the command. `/bin/sh` (on NixOS, a symlink
    /// managed by `environment.binsh`) keeps the checker working under
    /// that minimal env. This test asserts the property by setting PATH
    /// to `/var/empty` (which contains no `sh`) and verifying the
    /// checker still returns Pass — that can only happen if `Command::new`
    /// uses the absolute path.
    #[tokio::test]
    async fn command_checker_runs_with_pathological_path_env() {
        let _guard = path_env_lock().await;

        let checker = CommandChecker {
            name: "echo".to_string(),
            command: "echo ok".to_string(),
            timeout_secs: 5,
        };

        // We have to await across the env-mutating block; can't use
        // with_empty_path's sync closure directly. Inline the swap.
        let saved = std::env::var_os("PATH");
        std::env::set_var("PATH", "/var/empty");
        let result = checker.run().await;
        match saved {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }

        assert!(
            matches!(result, HealthCheckResult::Pass { .. }),
            "CommandChecker must succeed when PATH excludes sh's directory \
             (proof that it uses an absolute /bin/sh); got {result:?}"
        );
    }

    /// Negative companion: a command that genuinely fails must surface
    /// as Fail with the exit-code branch, not silently as Pass and not
    /// as the spawn-error branch.
    #[tokio::test]
    async fn command_checker_returns_fail_on_nonzero_exit() {
        let _guard = path_env_lock().await;

        let checker = CommandChecker {
            name: "fail".to_string(),
            command: "exit 1".to_string(),
            timeout_secs: 5,
        };

        let saved = std::env::var_os("PATH");
        std::env::set_var("PATH", "/var/empty");
        let result = checker.run().await;
        match saved {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }

        match result {
            HealthCheckResult::Fail { message, .. } => {
                assert!(
                    message.contains("exit code 1"),
                    "expected exit-code branch in failure message; got {message:?}"
                );
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }
}
