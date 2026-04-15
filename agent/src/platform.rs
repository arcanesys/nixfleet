/// Platform-specific constants and helpers.

/// Path to the symlink representing the current active system generation.
///
/// NixOS:  `/run/current-system` → `/nix/store/<hash>-nixos-system-...`
/// Darwin: `/nix/var/nix/profiles/system` → `/nix/store/<hash>-darwin-system-...`
#[cfg(target_os = "linux")]
pub const CURRENT_SYSTEM_PATH: &str = "/run/current-system";

#[cfg(target_os = "macos")]
pub const CURRENT_SYSTEM_PATH: &str = "/nix/var/nix/profiles/system";

/// System profile path for nix-env generation listing.
pub const SYSTEM_PROFILE: &str = "/nix/var/nix/profiles/system";

/// Read host uptime in seconds.
#[cfg(target_os = "linux")]
pub fn uptime_seconds() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(String::from))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0)
}

/// Read host uptime in seconds via sysctl on macOS.
#[cfg(target_os = "macos")]
pub fn uptime_seconds() -> u64 {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "kern.boottime"])
        .output()
        .ok();
    let Some(output) = output else { return 0 };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let boot_epoch: u64 = stdout
        .split("sec = ")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if boot_epoch == 0 {
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(boot_epoch)
}
