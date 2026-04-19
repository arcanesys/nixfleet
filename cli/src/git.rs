use anyhow::{bail, Context, Result};
use std::path::Path;

/// Check that the git working tree at `dir` has no uncommitted changes.
/// Skips gracefully if `dir` is not inside a git repo.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a git command fully isolated in `dir`. Uses both `-C` and
    /// `GIT_CEILING_DIRECTORIES` to prevent any interaction with parent repos.
    async fn git(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
        let dir_str = dir.to_string_lossy().to_string();
        let ceiling = dir.parent().unwrap().to_string_lossy().to_string();
        let mut full_args = vec!["-C", &dir_str];
        full_args.extend_from_slice(args);
        tokio::process::Command::new("git")
            .args(&full_args)
            .env("GIT_CEILING_DIRECTORIES", &ceiling)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn check_clean_dirty_repo_fails() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]).await;
        std::fs::write(dir.path().join("dirty.txt"), "hello").unwrap();
        let result = check_clean(dir.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("uncommitted changes"), "got: {err}");
    }

    #[tokio::test]
    async fn check_clean_clean_repo_passes() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]).await;
        git(dir.path(), &["config", "user.email", "test@test.com"]).await;
        git(dir.path(), &["config", "user.name", "Test"]).await;
        std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
        git(dir.path(), &["add", "."]).await;
        git(dir.path(), &["commit", "-m", "init"]).await;
        assert!(check_clean(dir.path()).await.is_ok());
    }
}
