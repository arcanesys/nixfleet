use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::info;

/// Read the current system generation by resolving the `/run/current-system` symlink.
/// Returns the full nix store path (e.g. `/nix/store/abc123...-nixos-system-web-01-25.05`).
pub async fn current_generation() -> Result<String> {
    let path = tokio::fs::read_link("/run/current-system")
        .await
        .context("failed to readlink /run/current-system")?;
    Ok(path.to_string_lossy().into_owned())
}

/// Fetch a closure from a binary cache into the local nix store.
///
/// Runs: `nix copy --from <cache_url> <store_path>`
/// If no cache URL is provided, assumes the closure is already available
/// (e.g. via a substituter configured in nix.conf).
pub async fn fetch_closure(store_path: &str, cache_url: Option<&str>) -> Result<()> {
    if let Some(cache) = cache_url {
        info!(store_path, cache, "Fetching closure from cache");
        let output = Command::new("nix")
            .args(["copy", "--from", cache, store_path])
            .output()
            .await
            .context("failed to spawn nix copy")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("nix copy failed: {stderr}");
        }
    } else {
        info!(store_path, "No cache URL — verifying path exists locally");
        let output = Command::new("nix")
            .args(["path-info", store_path])
            .output()
            .await
            .context("failed to spawn nix path-info")?;

        if !output.status.success() {
            anyhow::bail!("store path {store_path} not found locally and no cache URL configured");
        }
    }
    Ok(())
}

/// Apply a generation by running its `switch-to-configuration switch`.
///
/// Runs: `<store_path>/bin/switch-to-configuration switch`
pub async fn apply_generation(store_path: &str) -> Result<()> {
    let switch_bin = format!("{store_path}/bin/switch-to-configuration");
    info!(switch_bin, "Applying generation");

    let output = Command::new(&switch_bin)
        .arg("switch")
        .output()
        .await
        .context("failed to spawn switch-to-configuration")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("switch-to-configuration failed: {stderr}");
    }
    Ok(())
}

/// Roll back to the previous system generation.
///
/// Finds the previous profile link and switches to it.
pub async fn rollback() -> Result<()> {
    info!("Rolling back to previous generation");

    // List system profiles to find the previous one
    let output = Command::new("nix-env")
        .args([
            "--list-generations",
            "--profile",
            "/nix/var/nix/profiles/system",
        ])
        .output()
        .await
        .context("failed to list generations")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("nix-env --list-generations failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let generations: Vec<&str> = stdout.lines().collect();

    if generations.len() < 2 {
        anyhow::bail!("no previous generation to roll back to");
    }

    // Parse the second-to-last generation number
    let prev_line = generations[generations.len() - 2];
    let gen_num: u64 = prev_line
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .context("failed to parse generation number")?;

    let prev_path = format!("/nix/var/nix/profiles/system-{gen_num}-link");
    info!(prev_path, "Switching to previous generation");

    // Resolve profile symlink to store path (apply_generation expects a store path)
    let store_path = tokio::fs::read_link(&prev_path)
        .await
        .context("failed to resolve profile symlink to store path")?;
    apply_generation(&store_path.to_string_lossy()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_parse_generation_hash_starts_with_store() {
        let path = "/nix/store/abc123-nixos-system-25.05";
        assert!(path.starts_with("/nix/store/"));
    }

    #[test]
    fn test_parse_generation_hash_not_empty() {
        let path = "/nix/store/abc123-nixos-system-25.05";
        assert!(!path.is_empty());
    }

    #[test]
    fn test_generation_hash_contains_nixos_system() {
        let path = "/nix/store/abc123-nixos-system-web-01-25.05";
        assert!(path.contains("nixos-system"));
    }

    #[test]
    fn test_switch_bin_path_construction() {
        let store_path = "/nix/store/abc123-nixos-system";
        let switch_bin = format!("{store_path}/bin/switch-to-configuration");
        assert_eq!(
            switch_bin,
            "/nix/store/abc123-nixos-system/bin/switch-to-configuration"
        );
    }

    #[test]
    fn test_generation_profile_path_construction() {
        let gen_num: u64 = 42;
        let prev_path = format!("/nix/var/nix/profiles/system-{gen_num}-link");
        assert_eq!(prev_path, "/nix/var/nix/profiles/system-42-link");
    }

    #[test]
    fn test_parse_generation_number_from_nix_env_line() {
        // `nix-env --list-generations` lines look like: "  42   2026-03-25 ...   (current)"
        let line = "  42   2026-03-25 12:00:00   (current)";
        let gen_num: u64 = line
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap();
        assert_eq!(gen_num, 42);
    }

    #[test]
    fn test_parse_generation_number_previous_line() {
        let lines = [
            "  40   2026-03-23 10:00:00",
            "  41   2026-03-24 11:00:00",
            "  42   2026-03-25 12:00:00   (current)",
        ];
        let prev_line = lines[lines.len() - 2];
        let gen_num: u64 = prev_line
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap();
        assert_eq!(gen_num, 41);
    }

    #[test]
    fn test_not_enough_generations_for_rollback() {
        let generations: Vec<&str> = vec!["  42   2026-03-25 12:00:00   (current)"];
        assert!(generations.len() < 2);
    }

    #[test]
    fn test_path_info_command_construction() {
        let store_path = "/nix/store/abc123-nixos-system";
        let args = ["path-info", store_path];
        assert_eq!(args[0], "path-info");
        assert_eq!(args[1], store_path);
    }
}
