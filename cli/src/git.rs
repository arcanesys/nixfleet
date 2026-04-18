use anyhow::{bail, Context, Result};
use std::path::Path;

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

    #[tokio::test]
    async fn check_clean_non_git_dir_passes() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_clean(dir.path()).await.is_ok());
    }

    #[tokio::test]
    async fn check_clean_dirty_repo_fails() {
        let dir = tempfile::tempdir().unwrap();
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        std::fs::write(dir.path().join("dirty.txt"), "hello").unwrap();
        let result = check_clean(dir.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("uncommitted changes"), "got: {err}");
    }

    #[tokio::test]
    async fn check_clean_clean_repo_passes() {
        let dir = tempfile::tempdir().unwrap();
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
        tokio::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        assert!(check_clean(dir.path()).await.is_ok());
    }
}
