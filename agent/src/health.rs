use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::debug;

/// Check system health by running `systemctl is-system-running`.
///
/// Returns `Ok(true)` if the system is `running` or `degraded` (some non-critical
/// units may fail during a switch — `degraded` is acceptable).
/// Returns `Ok(false)` if the system is in another state (e.g. `starting`, `stopping`).
/// Returns `Err` if the command cannot be executed.
pub async fn check_system() -> Result<bool> {
    let output = Command::new("systemctl")
        .arg("is-system-running")
        .output()
        .await
        .context("failed to run systemctl is-system-running")?;

    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    debug!(state, "System state");

    // `running` = all units OK, `degraded` = some non-critical units failed
    // Both are acceptable post-switch states.
    Ok(state == "running" || state == "degraded")
}
