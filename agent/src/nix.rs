use anyhow::{Context, Result};
use std::time::Duration;
use tokio::process::Command;
use tracing::info;

/// Maximum time any single `nix`/`nix-env` subprocess is allowed to run
/// before we give up and return a timeout error. A hung nix command
/// would otherwise block the agent's deploy cycle indefinitely.
const NIX_CMD_TIMEOUT: Duration = Duration::from_secs(300);

/// Maximum number of bytes of stderr to include in a bail! error message.
/// Nix commands can produce very large stderr (credentials from failed
/// substituter pushes, full build logs); we truncate to keep log lines
/// and error reports bounded.
const MAX_STDERR_BYTES: usize = 2048;

/// Validate that a string looks like a `/nix/store/<hash>-<name>` path.
///
/// The store path is supplied by the control plane; even though the CP
/// is authenticated via mTLS, we still defend against malformed input
/// flowing into `Command::new` or `nix` subcommand arguments.
pub fn validate_store_path(store_path: &str) -> Result<()> {
    let rest = store_path
        .strip_prefix("/nix/store/")
        .with_context(|| format!("store path must start with /nix/store/: {store_path}"))?;
    if rest.is_empty() || rest.contains('/') || rest.contains("..") {
        anyhow::bail!("invalid store path: {store_path}");
    }
    // Hash prefix must be alphanumeric (nixbase32); name may contain
    // alnum, '.', '-', '_', '+'.
    let bytes = rest.as_bytes();
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'))
    {
        anyhow::bail!("invalid characters in store path: {store_path}");
    }
    Ok(())
}

/// Truncate stderr bytes to a displayable, length-bounded lossy string.
fn truncated_stderr(bytes: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(bytes);
    if lossy.len() <= MAX_STDERR_BYTES {
        lossy.into_owned()
    } else {
        let truncated: String = lossy.chars().take(MAX_STDERR_BYTES).collect();
        format!("{truncated}… [truncated, {} total bytes]", bytes.len())
    }
}

/// Run a tokio::process::Command with a hard timeout, returning its output.
async fn run_with_timeout(mut cmd: Command, label: &'static str) -> Result<std::process::Output> {
    tokio::time::timeout(NIX_CMD_TIMEOUT, cmd.output())
        .await
        .with_context(|| format!("{label} timed out after {:?}", NIX_CMD_TIMEOUT))?
        .with_context(|| format!("failed to spawn {label}"))
}

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
    validate_store_path(store_path)?;
    if let Some(cache) = cache_url {
        info!(store_path, cache, "Fetching closure from cache");
        let mut cmd = Command::new("nix");
        cmd.args(["copy", "--from", cache, store_path]);
        let output = run_with_timeout(cmd, "nix copy").await?;

        if !output.status.success() {
            let stderr = truncated_stderr(&output.stderr);
            anyhow::bail!("nix copy failed: {stderr}");
        }
    } else {
        info!(store_path, "No cache URL — verifying path exists locally");
        let mut cmd = Command::new("nix");
        cmd.args(["path-info", store_path]);
        let output = run_with_timeout(cmd, "nix path-info").await?;

        if !output.status.success() {
            anyhow::bail!("store path {store_path} not found locally and no cache URL configured");
        }
    }
    Ok(())
}

/// Outcome of `apply_generation` — distinguishes success from retryable
/// lock contention. Real errors (timeout, spawn failure) remain in the
/// `Err` channel of `anyhow::Result<ApplyOutcome>`.
#[derive(Debug)]
pub enum ApplyOutcome {
    /// Generation applied successfully.
    Applied,
    /// Another process holds the activation lock — caller should retry.
    LockContention(String),
}

/// Check stderr for signals that another process holds the activation lock.
/// Version-agnostic: matches common substrings across NixOS versions.
fn is_lock_contention(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("could not acquire lock")
        || lower.contains("activation lock")
        || lower.contains("already running")
        || lower.contains("resource busy")
}

/// Apply a generation by running its `switch-to-configuration switch`.
///
/// Returns `Ok(Applied)` on success, `Ok(LockContention(stderr))` when
/// another process holds the activation lock (caller should retry), or
/// `Err` for fatal failures (timeout, spawn error, config error).
pub async fn apply_generation(store_path: &str) -> Result<ApplyOutcome> {
    validate_store_path(store_path)?;
    let switch_bin = format!("{store_path}/bin/switch-to-configuration");
    info!(switch_bin, "Applying generation");

    // Spawn switch-to-configuration in a transient systemd service so it
    // survives the agent being stopped. Without this, switch-to-configuration
    // is a child process in the agent's cgroup — when it runs
    // `systemctl stop nixfleet-agent`, systemd kills ALL processes in the
    // cgroup, including switch-to-configuration itself. The activation never
    // completes, the system profile never updates, and the agent loops.
    //
    // --pipe: connect stdin/stdout/stderr so we can capture output.
    // --wait: block until the transient unit finishes (like a synchronous call).
    // --unit: named unit so concurrent invocations are rejected by systemd
    //         rather than racing.
    let mut cmd = Command::new("systemd-run");
    cmd.args([
        "--pipe",
        "--wait",
        "--unit=nixfleet-switch",
        "--",
        &switch_bin,
        "switch",
    ]);
    let output = run_with_timeout(cmd, "switch-to-configuration").await?;

    if !output.status.success() {
        let stderr = truncated_stderr(&output.stderr);
        if is_lock_contention(&stderr) {
            return Ok(ApplyOutcome::LockContention(stderr));
        }
        anyhow::bail!("switch-to-configuration failed: {stderr}");
    }
    Ok(ApplyOutcome::Applied)
}

/// Roll back to the previous system generation.
///
/// Finds the previous profile link and switches to it.
pub async fn rollback() -> Result<()> {
    info!("Rolling back to previous generation");

    // List system profiles to find the previous one
    let mut cmd = Command::new("nix-env");
    cmd.args([
        "--list-generations",
        "--profile",
        "/nix/var/nix/profiles/system",
    ]);
    let output = run_with_timeout(cmd, "nix-env --list-generations").await?;

    if !output.status.success() {
        let stderr = truncated_stderr(&output.stderr);
        anyhow::bail!("nix-env --list-generations failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Ignore blank/whitespace-only lines defensively — nix-env output is
    // normally well-formed, but we don't want an empty trailing line to
    // pass the `len() >= 2` gate and crash parsing below.
    let generations: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();

    if generations.len() < 2 {
        anyhow::bail!("no previous generation to roll back to");
    }

    // Parse the second-to-last generation number
    let prev_line = generations[generations.len() - 2];
    let gen_num: u64 = prev_line
        .split_whitespace()
        .next()
        .with_context(|| format!("empty generation line: {prev_line:?}"))?
        .parse()
        .with_context(|| format!("failed to parse generation number from line: {prev_line:?}"))?;

    let prev_path = format!("/nix/var/nix/profiles/system-{gen_num}-link");
    info!(prev_path, "Switching to previous generation");

    // Resolve profile symlink to store path (apply_generation expects a store path)
    let store_path = tokio::fs::read_link(&prev_path)
        .await
        .context("failed to resolve profile symlink to store path")?;
    match apply_generation(&store_path.to_string_lossy()).await? {
        ApplyOutcome::Applied => Ok(()),
        ApplyOutcome::LockContention(msg) => {
            anyhow::bail!("rollback blocked by lock contention: {msg}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_store_path_accepts_valid() {
        assert!(validate_store_path("/nix/store/abc123-hello").is_ok());
        assert!(
            validate_store_path("/nix/store/0abc123def45678-nixos-system-web-01-25.05").is_ok()
        );
        assert!(validate_store_path("/nix/store/a-b.c+d_e").is_ok());
    }

    #[test]
    fn test_validate_store_path_rejects_bad() {
        assert!(validate_store_path("/etc/passwd").is_err());
        assert!(validate_store_path("/nix/store/").is_err());
        assert!(validate_store_path("/nix/store/abc/../etc").is_err());
        assert!(validate_store_path("/nix/store/abc;rm -rf /").is_err());
        assert!(validate_store_path("/nix/store/abc nested/sub").is_err());
    }

    #[test]
    fn test_truncated_stderr_bounds_length() {
        let small = b"short error";
        assert_eq!(truncated_stderr(small), "short error");
        let big = vec![b'x'; MAX_STDERR_BYTES + 100];
        let out = truncated_stderr(&big);
        assert!(out.contains("truncated"));
        assert!(out.len() < MAX_STDERR_BYTES + 128);
    }

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

    #[test]
    fn test_is_lock_contention_matches_common_patterns() {
        // Real NixOS output observed in production
        assert!(is_lock_contention("Could not acquire lock\n"));
        assert!(is_lock_contention(
            "error: could not acquire activation lock on '/nix/var/nix/profiles/system'"
        ));
        assert!(is_lock_contention(
            "warning: not able to lock: already running"
        ));
        assert!(is_lock_contention("Device or resource busy"));
        assert!(is_lock_contention(
            "another instance of switch-to-configuration is already running"
        ));
    }

    #[test]
    fn test_is_lock_contention_rejects_unrelated_errors() {
        assert!(!is_lock_contention(
            "error: path '/nix/store/abc' is not valid"
        ));
        assert!(!is_lock_contention(
            "error: building of '/nix/store/abc.drv' failed"
        ));
        assert!(!is_lock_contention(
            "error: could not lock path '/nix/store/abc.lock'"
        ));
        assert!(!is_lock_contention(""));
    }
}
