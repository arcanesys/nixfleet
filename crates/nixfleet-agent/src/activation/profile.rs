//! Profile-flip helpers: self-correction after a concurrent profile-mutator,
//! and resolution of the rolled-back target's basename.

use anyhow::{anyhow, Context, Result};
use tokio::process::Command;

/// `Err` only when self-correction itself failed; caller treats as non-fatal.
pub(super) async fn self_correct_profile(expected_store_path: &str) -> Result<()> {
    let profile = "/nix/var/nix/profiles/system";
    if profile_matches(expected_store_path, profile) {
        return Ok(());
    }

    tracing::warn!(
        expected = %expected_store_path,
        profile = profile,
        "agent: profile mismatch after fire-and-forget - re-running nix-env --set",
    );
    let status = Command::new("nix-env")
        .arg("--profile")
        .arg(profile)
        .arg("--set")
        .arg(expected_store_path)
        .status()
        .await
        .with_context(|| "spawn nix-env --set (self-correction)")?;
    if !status.success() {
        return Err(anyhow!(
            "nix-env --set self-correction exited {:?}",
            status.code()
        ));
    }
    if !profile_matches(expected_store_path, profile) {
        return Err(anyhow!(
            "profile still mismatched after nix-env --set self-correction",
        ));
    }
    tracing::info!("agent: profile self-corrected successfully");
    Ok(())
}

// GOTCHA: profile is two-level symlink: profile -> `system-<N>-link` -> `/nix/store/<basename>`.
fn profile_matches(expected_store_path: &str, profile_path: &str) -> bool {
    let Ok(gen_link) = std::fs::read_link(profile_path) else {
        return false;
    };
    let abs_gen_link = if gen_link.is_relative() {
        let parent = std::path::Path::new(profile_path)
            .parent()
            .unwrap_or(std::path::Path::new("/"));
        parent.join(&gen_link)
    } else {
        gen_link
    };
    let final_target = match std::fs::read_link(&abs_gen_link) {
        Ok(t) => t,
        Err(_) => abs_gen_link,
    };
    final_target.to_string_lossy() == expected_store_path
}

pub(super) fn resolve_profile_target() -> Result<String> {
    let profile = std::path::Path::new("/nix/var/nix/profiles/system");
    let gen_link =
        std::fs::read_link(profile).with_context(|| "readlink /nix/var/nix/profiles/system")?;
    let abs_gen_link = if gen_link.is_relative() {
        profile
            .parent()
            .unwrap_or(std::path::Path::new("/"))
            .join(&gen_link)
    } else {
        gen_link.clone()
    };
    let store_path = std::fs::read_link(&abs_gen_link)
        .with_context(|| format!("readlink {}", abs_gen_link.display()))?;
    let basename = store_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("non-utf8 basename: {}", store_path.display()))?
        .to_string();
    Ok(basename)
}
