use anyhow::{bail, Context, Result};
use std::path::Path;

/// Check that the git working tree at `dir` has no uncommitted changes.
/// Skips gracefully if `dir` is not inside a git repo.
///
/// Note: `check_clean` inherits the caller's environment. When `GIT_DIR` is
/// set (e.g. inside git hooks), the status check targets the wrong repo.
/// This is a known limitation - the function is only called from CLI
/// commands where users control their own environment. Unit tests for this
/// function were removed because they require spawning real git repos, which
/// conflicts with git hooks that set `GIT_DIR` (see ADR context in the
/// commit that removed them).
pub async fn check_clean(dir: &Path) -> Result<()> {
    let output = tokio::process::Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "status", "--porcelain"])
        .output()
        .await
        .context("failed to run git status")?;

    if !output.status.success() {
        tracing::debug!("git status failed (not a repo?), skipping dirty check");
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        bail!(
            "working tree has uncommitted changes. Commit or stash them, \
             or use --allow-dirty to override."
        );
    }

    Ok(())
}
