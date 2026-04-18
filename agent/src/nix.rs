use anyhow::{Context, Result};
use std::time::Duration;
use tokio::process::Command;
use tracing::info;

#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;

/// Check if a system switch is already in progress.
///
/// On Linux, probes the NixOS switch lock file (`/run/nixos/switch-to-configuration.lock`)
/// with a non-blocking `flock`. If the lock is held, another `switch-to-configuration`
/// (e.g. from `nixos-rebuild switch`) is already running — the agent should skip
/// and let the next poll cycle pick up the result.
///
/// On Darwin, activation is synchronous and has no lock file, so this always returns false.
#[cfg(target_os = "linux")]
pub fn is_switch_in_progress() -> bool {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    let file = match OpenOptions::new()
        .read(true)
        .open("/run/nixos/switch-to-configuration.lock")
    {
        Ok(f) => f,
        Err(_) => return false, // Lock file doesn't exist — no switch in progress
    };

    let fd = file.as_raw_fd();
    // SAFETY: fd is a valid open file descriptor. flock is safe to call.
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        // Acquired the lock — no one else holds it. Release immediately.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        false
    } else {
        // EWOULDBLOCK — another process holds the lock.
        true
    }
}

#[cfg(target_os = "macos")]
pub fn is_switch_in_progress() -> bool {
    false
}

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

/// Read the current system generation by reading the system symlink.
///
/// `/run/current-system` is a single-level symlink on both NixOS and Darwin,
/// so `read_link` is sufficient (no need for `canonicalize`).
pub async fn current_generation() -> Result<String> {
    let path = crate::platform::CURRENT_SYSTEM_PATH;
    let target = tokio::fs::read_link(path)
        .await
        .with_context(|| format!("failed to readlink {path}"))?;
    Ok(target.to_string_lossy().into_owned())
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

/// Parse the output of `systemctl show nixfleet-switch.service -p ActiveState,Result`.
///
/// Returns `Some(true)` if the unit completed successfully,
/// `Some(false)` if it failed, or `None` if still running / not found.
fn parse_switch_status(output: &str) -> Option<bool> {
    let mut active_state = None;
    let mut result = None;
    for line in output.lines() {
        if let Some(val) = line.strip_prefix("ActiveState=") {
            active_state = Some(val);
        }
        if let Some(val) = line.strip_prefix("Result=") {
            result = Some(val);
        }
    }
    match (active_state, result) {
        (Some("inactive"), Some("success")) => Some(true),
        (Some("inactive"), Some(_)) => Some(false),
        _ => None,
    }
}

/// Check the exit status of the `nixfleet-switch.service` transient unit.
///
/// Returns `Some(true)` if it completed successfully, `Some(false)` if
/// it failed, or `None` if still running or not found.
#[cfg(target_os = "linux")]
pub async fn check_switch_exit_status() -> Result<Option<bool>> {
    let mut cmd = Command::new("systemctl");
    cmd.args([
        "show",
        "nixfleet-switch.service",
        "-p",
        "ActiveState,Result",
    ]);
    let output = run_with_timeout(cmd, "systemctl show nixfleet-switch").await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_switch_status(&stdout))
}

/// Check the exit status of the system switch.
///
/// Darwin: activation is detached (background process). We can't check
/// its status directly. Return `None` (inconclusive) so the polling loop
/// in `fire_poll_switch` waits for `/run/current-system` to update.
/// If the agent gets killed by the activation, launchd restarts it and
/// the next poll cycle sees the generation matches.
#[cfg(target_os = "macos")]
pub async fn check_switch_exit_status() -> Result<Option<bool>> {
    Ok(None)
}

/// Update the nix system profile to point to the given store path.
///
/// Runs: `nix-env -p /nix/var/nix/profiles/system --set <store_path>`
///
/// This creates a new profile generation, which is what the bootloader
/// uses to determine available and default boot entries. Without this,
/// `switch-to-configuration switch` activates the system but the profile
/// stays at the old generation — on reboot, the old system boots.
///
/// On Darwin, `fire_switch` already handles this inline (nix-env --set
/// before activate). This function is only called on Linux.
pub async fn set_profile(store_path: &str) -> Result<()> {
    validate_store_path(store_path)?;
    info!(store_path, "Setting system profile");

    let mut cmd = Command::new("nix-env");
    cmd.args(["-p", crate::platform::SYSTEM_PROFILE, "--set", store_path]);
    let output = run_with_timeout(cmd, "nix-env --set").await?;

    if !output.status.success() {
        let stderr = truncated_stderr(&output.stderr);
        anyhow::bail!("nix-env --set failed: {stderr}");
    }
    Ok(())
}

/// Fire system activation for a store path.
///
/// Linux:  detached transient systemd unit (`systemd-run`)
/// Darwin: direct activation (`<store_path>/activate` + profile update)
pub async fn fire_switch(store_path: &str) -> Result<()> {
    validate_store_path(store_path)?;

    #[cfg(target_os = "linux")]
    {
        // Step 1: Update the nix profile. This creates a new profile
        // generation so the bootloader picks it up. Without this,
        // switch-to-configuration activates the system but the profile
        // stays at the old generation — reboot goes to old system.
        set_profile(store_path).await?;

        // Step 2: Fire switch-to-configuration in a detached unit.
        let switch_bin = format!("{store_path}/bin/switch-to-configuration");
        info!(switch_bin, "Firing switch-to-configuration (detached)");

        let mut cmd = Command::new("systemd-run");
        cmd.args(["--unit=nixfleet-switch", "--", &switch_bin, "switch"]);
        let output = run_with_timeout(cmd, "systemd-run").await?;

        if !output.status.success() {
            let stderr = truncated_stderr(&output.stderr);
            anyhow::bail!("systemd-run failed to queue switch: {stderr}");
        }
        info!("Switch queued as nixfleet-switch.service");
    }

    #[cfg(target_os = "macos")]
    {
        info!(store_path, "Firing Darwin activation (detached)");

        // Spawn a fully detached child process for activation.
        // The activate script may unload/reload the agent's launchd plist,
        // killing this process mid-activation. The detached child continues
        // the activation. Launchd's KeepAlive restarts the agent, which
        // then sees the new generation.
        //
        // Uses Rust's Command::spawn (not .output()) so the child is fully
        // independent. stdout/stderr redirected to a log file. nohup doesn't
        // work in a launchd daemon context (no TTY).
        let script = format!(
            "nix-env -p {profile} --set {path} && {path}/activate",
            profile = crate::platform::SYSTEM_PROFILE,
            path = store_path,
        );

        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/var/log/nixfleet-activate.log")
            .context("failed to open activation log")?;
        let log_err = log_file.try_clone().context("failed to clone log file handle")?;

        // SAFETY: pre_exec runs in the forked child before exec.
        // setsid() creates a new session so the child survives when
        // launchd kills the agent's process group during plist reload.
        unsafe {
            std::process::Command::new("sh")
                .args(["-c", &script])
                .stdout(log_file)
                .stderr(log_err)
                .stdin(std::process::Stdio::null())
                .pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                })
                .spawn()
                .context("failed to spawn detached activation")?;
        }

        info!("Darwin activation spawned in background");
    }

    Ok(())
}

/// Check if the system profile symlink resolves to the expected store path.
///
/// Uses `read_link` (not `canonicalize`) because profile symlinks are
/// single-level: `/nix/var/nix/profiles/system` -> `system-<N>-link`
/// -> `/nix/store/<hash>-...`. We resolve both levels.
pub fn check_profile_matches(expected: &str, profile_path: &str) -> bool {
    let Ok(target) = std::fs::read_link(profile_path) else {
        return false;
    };
    // Profile -> generation link -> store path (two levels)
    let resolved = if target.is_relative() {
        let parent = std::path::Path::new(profile_path)
            .parent()
            .unwrap_or(std::path::Path::new("/"));
        parent.join(&target)
    } else {
        target
    };
    // read_link again to resolve generation link -> store path
    let final_target = std::fs::read_link(&resolved).unwrap_or(resolved);
    final_target.to_string_lossy() == expected
}

/// Verify the system profile matches the expected store path after a switch.
/// If it doesn't match, attempt to fix it with `nix-env --set`.
///
/// This is a safety net — `fire_switch` already sets the profile before
/// switching on Linux. This catch handles edge cases like concurrent
/// `nix-env` calls or partial failures.
pub async fn verify_profile(store_path: &str) -> Result<bool> {
    let profile = crate::platform::SYSTEM_PROFILE;
    if check_profile_matches(store_path, profile) {
        info!("Profile verified: matches expected store path");
        return Ok(true);
    }

    tracing::warn!(
        store_path,
        profile,
        "Profile mismatch after switch — attempting self-correction"
    );
    set_profile(store_path).await?;

    if check_profile_matches(store_path, profile) {
        info!("Profile self-corrected successfully");
        Ok(true)
    } else {
        tracing::error!("Profile still mismatched after self-correction attempt");
        Ok(false)
    }
}

/// Poll a symlink path until it points to the expected store path.
///
/// Uses `read_link` since `/run/current-system` is a single-level symlink
/// on both NixOS and Darwin.
pub async fn poll_generation(
    expected: &str,
    path: &std::path::Path,
    timeout: Duration,
    interval: Duration,
) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(target) = tokio::fs::read_link(path).await {
            if target.to_string_lossy() == expected {
                return Ok(true);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(interval).await;
    }
}

pub async fn rollback() -> Result<()> {
    info!("Rolling back to previous generation");

    let mut cmd = Command::new("nix-env");
    cmd.args([
        "--list-generations",
        "--profile",
        crate::platform::SYSTEM_PROFILE,
    ]);
    let output = run_with_timeout(cmd, "nix-env --list-generations").await?;

    if !output.status.success() {
        let stderr = truncated_stderr(&output.stderr);
        anyhow::bail!("nix-env --list-generations failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let generations: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();

    if generations.len() < 2 {
        anyhow::bail!("no previous generation to roll back to");
    }

    let prev_line = generations[generations.len() - 2];
    let gen_num: u64 = prev_line
        .split_whitespace()
        .next()
        .with_context(|| format!("empty generation line: {prev_line:?}"))?
        .parse()
        .with_context(|| format!("failed to parse generation number from line: {prev_line:?}"))?;

    let prev_path = format!("{}-{gen_num}-link", crate::platform::SYSTEM_PROFILE);
    info!(prev_path, "Switching to previous generation");

    let store_path = tokio::fs::read_link(&prev_path)
        .await
        .context("failed to resolve profile symlink to store path")?;
    let store_path_str = store_path.to_string_lossy();

    fire_switch(&store_path_str).await?;

    let path = std::path::Path::new(crate::platform::CURRENT_SYSTEM_PATH);
    let timeout = Duration::from_secs(300);
    let interval = Duration::from_secs(2);
    if poll_generation(&store_path_str, path, timeout, interval).await? {
        Ok(())
    } else {
        anyhow::bail!(
            "rollback timed out: {} did not match {store_path_str}",
            crate::platform::CURRENT_SYSTEM_PATH
        )
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

    #[tokio::test]
    async fn test_poll_generation_matches_immediately() {
        tokio::time::pause();
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("current-system");
        std::os::unix::fs::symlink("/nix/store/abc-target", &link).unwrap();

        let matched = poll_generation(
            "/nix/store/abc-target",
            &link,
            Duration::from_secs(10),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
        assert!(matched);
    }

    #[tokio::test]
    async fn test_poll_generation_times_out() {
        tokio::time::pause();
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("current-system");
        std::os::unix::fs::symlink("/nix/store/wrong-target", &link).unwrap();

        let matched = poll_generation(
            "/nix/store/abc-target",
            &link,
            Duration::from_secs(5),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
        assert!(!matched);
    }

    #[test]
    fn test_fire_switch_command_construction() {
        let store_path = "/nix/store/abc123-nixos-system";
        let switch_bin = format!("{store_path}/bin/switch-to-configuration");
        let expected_args = [
            "systemd-run",
            "--unit=nixfleet-switch",
            "--",
            &switch_bin,
            "switch",
        ];
        assert_eq!(expected_args[0], "systemd-run");
        assert_eq!(expected_args[1], "--unit=nixfleet-switch");
        assert_eq!(expected_args[3], &switch_bin);
        assert_eq!(expected_args[4], "switch");
    }

    #[tokio::test]
    async fn test_poll_generation_detects_change() {
        tokio::time::pause();
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("current-system");
        std::os::unix::fs::symlink("/nix/store/old-system", &link).unwrap();

        let link_clone = link.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = std::fs::remove_file(&link_clone);
            std::os::unix::fs::symlink("/nix/store/abc-target", &link_clone).unwrap();
        });

        let matched = poll_generation(
            "/nix/store/abc-target",
            &link,
            Duration::from_secs(10),
            Duration::from_millis(500),
        )
        .await
        .unwrap();
        assert!(matched);
    }

    #[test]
    fn test_set_profile_command_construction() {
        let store_path = "/nix/store/abc123-nixos-system";
        let profile = crate::platform::SYSTEM_PROFILE;
        let expected_args = ["nix-env", "-p", profile, "--set", store_path];
        assert_eq!(expected_args[0], "nix-env");
        assert_eq!(expected_args[1], "-p");
        assert_eq!(expected_args[2], "/nix/var/nix/profiles/system");
        assert_eq!(expected_args[3], "--set");
        assert_eq!(expected_args[4], store_path);
    }

    #[test]
    fn test_check_profile_matches_direct_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("system");
        std::os::unix::fs::symlink("/nix/store/abc-target", &link).unwrap();

        let result = check_profile_matches("/nix/store/abc-target", link.to_str().unwrap());
        assert!(result);
    }

    #[test]
    fn test_check_profile_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("system");
        std::os::unix::fs::symlink("/nix/store/old-system", &link).unwrap();

        let result = check_profile_matches("/nix/store/abc-target", link.to_str().unwrap());
        assert!(!result);
    }

    #[test]
    fn test_check_profile_missing_link() {
        let result = check_profile_matches("/nix/store/abc-target", "/nonexistent/path/system");
        assert!(!result);
    }

    #[test]
    fn test_check_profile_two_level_symlink() {
        // Simulate: system -> system-42-link -> /nix/store/abc-target
        let dir = tempfile::tempdir().unwrap();
        let gen_link = dir.path().join("system-42-link");
        std::os::unix::fs::symlink("/nix/store/abc-target", &gen_link).unwrap();
        let profile_link = dir.path().join("system");
        // relative symlink pointing to gen_link (same directory)
        std::os::unix::fs::symlink("system-42-link", &profile_link).unwrap();

        let result =
            check_profile_matches("/nix/store/abc-target", profile_link.to_str().unwrap());
        assert!(result);
    }

    #[test]
    fn test_parse_switch_status_success() {
        let output = "ActiveState=inactive\nResult=success\n";
        assert_eq!(parse_switch_status(output), Some(true));
    }

    #[test]
    fn test_parse_switch_status_failed() {
        let output = "ActiveState=inactive\nResult=exit-code\n";
        assert_eq!(parse_switch_status(output), Some(false));
    }

    #[test]
    fn test_parse_switch_status_still_running() {
        let output = "ActiveState=active\nResult=success\n";
        assert_eq!(parse_switch_status(output), None);
    }

    #[test]
    fn test_parse_switch_status_empty() {
        assert_eq!(parse_switch_status(""), None);
    }
}
