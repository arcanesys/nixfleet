//! Git plumbing - shells out to `git`, never embeds a library.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};

use crate::{GitPushTarget, ReleaseConfig};

pub(crate) fn git_head_sha(repo: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .context("invoke `git rev-parse HEAD`")?;
    if !output.status.success() {
        bail!(
            "git rev-parse HEAD: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn git_commit_release(
    config: &ReleaseConfig,
    files: &[PathBuf],
    ci_commit: Option<&str>,
    signed_at: DateTime<Utc>,
) -> Result<bool> {
    if let Some(name) = &config.git_user_name {
        run_git(&config.flake_dir, &["config", "user.name", name])?;
    }
    if let Some(email) = &config.git_user_email {
        run_git(&config.flake_dir, &["config", "user.email", email])?;
    }
    let mut add_args = vec!["add", "--"];
    let file_strs: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&config.flake_dir)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    for f in &file_strs {
        add_args.push(f);
    }
    run_git(&config.flake_dir, &add_args)?;

    let cached_diff = Command::new("git")
        .args(["diff", "--cached", "--quiet", "--"])
        .args(&file_strs)
        .current_dir(&config.flake_dir)
        .status()
        .context("invoke `git diff --cached --quiet`")?;
    if cached_diff.success() {
        tracing::info!("git: no release change");
        return Ok(false);
    }

    let message = render_commit_message(
        &config.commit_template,
        ci_commit.unwrap_or("HEAD"),
        signed_at,
    );
    run_git(&config.flake_dir, &["commit", "-m", &message])?;
    tracing::info!(message = %message, "git commit");
    Ok(true)
}

pub fn render_commit_message(template: &str, sha: &str, ts: DateTime<Utc>) -> String {
    let short = if sha.len() >= 8 { &sha[..8] } else { sha };
    template
        .replace("{sha:0:8}", short)
        .replace("{sha}", sha)
        .replace("{ts}", &ts.to_rfc3339())
}

pub(crate) fn git_push_release(repo: &Path, target: &GitPushTarget) -> Result<()> {
    let refspec = format!("HEAD:{}", target.branch);
    run_git(repo, &["push", &target.remote, &refspec])?;
    tracing::info!(
        remote = %target.remote,
        branch = %target.branch,
        "git push",
    );
    Ok(())
}

fn run_git(repo: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .with_context(|| format!("invoke git {args:?}"))?;
    if !status.success() {
        bail!("git {:?} exited {}", args, status.code().unwrap_or(-1));
    }
    Ok(())
}
