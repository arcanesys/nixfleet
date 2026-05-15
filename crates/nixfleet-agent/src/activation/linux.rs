//! Linux (NixOS) activation primitives. fire_* uses `systemd-run --unit=...`
//! so the agent's SIGTERM can't kill the activation mid-run.

use std::path::Path;

use anyhow::{Context, Result};
use nixfleet_proto::agent_wire::EvaluatedTarget;
use tokio::process::Command;

use super::{ActivationBackend, ActivationOutcome, RollbackOutcome};

#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxBackend;

impl ActivationBackend for LinuxBackend {
    async fn is_switch_in_progress(&self) -> bool {
        is_switch_in_progress().await
    }
    async fn read_unit_exit_code(&self, unit_name: &str) -> Option<i32> {
        read_unit_exit_code(unit_name).await
    }
    async fn fire_switch(
        &self,
        target: &EvaluatedTarget,
        store_path: &str,
    ) -> Result<Option<ActivationOutcome>> {
        fire_switch(target, store_path).await
    }
    async fn fire_rollback(&self, target_basename: &str) -> Result<Option<RollbackOutcome>> {
        fire_rollback(target_basename).await
    }
}

const SWITCH_LOCK_PATH: &str = "/run/nixos/switch-to-configuration.lock";

/// Fail-open: absent lock file or missing flock binary -> false.
async fn is_switch_in_progress() -> bool {
    is_switch_in_progress_at(Path::new(SWITCH_LOCK_PATH)).await
}

async fn is_switch_in_progress_at(lock_path: &Path) -> bool {
    if !lock_path.exists() {
        return false;
    }
    let status = Command::new("flock")
        .arg("--nonblock")
        .arg("--shared")
        .arg(lock_path)
        .arg("true")
        .status()
        .await;
    match status {
        Ok(s) if s.success() => false,
        Ok(_) => true,
        Err(_) => false,
    }
}

/// `None` on failure / empty / non-numeric (never synthesise a misleading 0).
async fn read_unit_exit_code(unit_name: &str) -> Option<i32> {
    let output = Command::new("systemctl")
        .arg("show")
        .arg("--property=ExecMainStatus")
        .arg("--value")
        .arg(unit_name)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<i32>().ok()
}

/// Critical components whose live-swap nixos-rebuild refuses. Detection is
/// canonicalize-equality on the symlink target between current + new closure.
/// `init` is NOT listed: it's a regenerated-per-system stub that always
/// differs across closures regardless of whether anything runtime-relevant
/// changed; listing it would force a defer on every update. The actually-
/// unsafe components are systemd (PID 1), kernel, and dbus.
const SWITCH_INHIBITORS: &[(&str, &str)] = &[
    // dbus.service is the unit symlink - broker↔dbus swaps surface as a
    // different canonicalised target inside the new closure.
    ("dbus", "etc/systemd/system/dbus.service"),
    ("systemd", "sw/lib/systemd/systemd"),
    ("kernel", "kernel"),
];

/// Returns `Some(component)` when a critical-component swap is detected
/// between the running system and the new closure. Either side missing the
/// path is out-of-scope (returns `None` for that component) - we only flag
/// genuine swaps, not absences.
fn detect_switch_inhibitors(current_system: &Path, new_store_path: &Path) -> Option<&'static str> {
    for (name, rel_path) in SWITCH_INHIBITORS {
        let cur = current_system.join(rel_path);
        let new = new_store_path.join(rel_path);
        match (std::fs::canonicalize(&cur), std::fs::canonicalize(&new)) {
            (Ok(c), Ok(n)) if c != n => return Some(name),
            _ => {}
        }
    }
    None
}

const CURRENT_SYSTEM_PATH: &str = "/run/current-system";

// FOOTGUN: --scope / --pipe --wait inherit the caller's cgroup; agent
// SIGTERM would kill the switch mid-run. Use --unit for cgroup isolation.
async fn fire_switch(
    target: &EvaluatedTarget,
    store_path: &str,
) -> Result<Option<ActivationOutcome>> {
    if let Some(component) =
        detect_switch_inhibitors(Path::new(CURRENT_SYSTEM_PATH), Path::new(store_path))
    {
        tracing::warn!(
            target_closure = %target.closure_hash,
            component = component,
            "agent: deferring live switch - critical-component swap requires reboot",
        );
        // LOADBEARING: `nix-env --set` creates the generation but does NOT
        // write bootloader entries - only switch-to-configuration does.
        // Without `boot` here, the next reboot lands back on the previous
        // default and the defer-then-reboot lifecycle breaks.
        let switch_bin = format!("{store_path}/bin/switch-to-configuration");
        let boot_status = Command::new(&switch_bin)
            .arg("boot")
            .status()
            .await
            .with_context(|| format!("spawn {switch_bin} boot"))?;
        if !boot_status.success() {
            tracing::error!(
                target_closure = %target.closure_hash,
                exit_code = ?boot_status.code(),
                "agent: switch-to-configuration boot failed in defer path; bootloader NOT updated",
            );
            return Ok(Some(ActivationOutcome::SwitchFailed {
                phase: "defer-bootloader-update".to_string(),
                exit_code: boot_status.code(),
            }));
        }
        return Ok(Some(ActivationOutcome::DeferredPendingReboot {
            component: component.to_string(),
        }));
    }

    let _ = Command::new("systemctl")
        .arg("reset-failed")
        .arg("nixfleet-switch.service")
        .status()
        .await;

    let switch_bin = format!("{store_path}/bin/switch-to-configuration");
    tracing::info!(
        target_closure = %target.closure_hash,
        "agent: firing switch via systemd-run --unit=nixfleet-switch (detached)",
    );
    let fire_status = Command::new("systemd-run")
        .arg("--unit=nixfleet-switch")
        .arg("--collect")
        .arg("--")
        .arg(&switch_bin)
        .arg("switch")
        .status()
        .await
        .with_context(|| "spawn systemd-run --unit=nixfleet-switch")?;

    if !fire_status.success() {
        tracing::error!(
            target_closure = %target.closure_hash,
            exit_code = ?fire_status.code(),
            "agent: systemd-run failed to queue switch unit",
        );
        return Ok(Some(ActivationOutcome::SwitchFailed {
            phase: "systemd-run-fire".to_string(),
            exit_code: fire_status.code(),
        }));
    }
    Ok(None)
}

/// LOADBEARING: `target_basename` resolves to the rolled-back closure's
/// store path, NOT `/run/current-system`. The agent fires rollback while the
/// failed closure is still current, so its `switch-to-configuration` would
/// "switch to" itself - a no-op that leaves nginx (or whatever caused the
/// failure) still down. Use the freshly-flipped profile target's binary.
async fn fire_rollback(target_basename: &str) -> Result<Option<RollbackOutcome>> {
    let _ = Command::new("systemctl")
        .arg("reset-failed")
        .arg("nixfleet-rollback.service")
        .status()
        .await;

    let switch_bin = rollback_switch_bin(target_basename);
    tracing::info!(
        target_basename = %target_basename,
        switch_bin = %switch_bin,
        "agent: firing rollback via systemd-run --unit=nixfleet-rollback (detached)",
    );
    let fire_status = Command::new("systemd-run")
        .arg("--unit=nixfleet-rollback")
        .arg("--collect")
        .arg("--")
        .arg(&switch_bin)
        .arg("switch")
        .status()
        .await
        .with_context(|| "spawn systemd-run --unit=nixfleet-rollback")?;

    if !fire_status.success() {
        tracing::error!(
            exit_code = ?fire_status.code(),
            "agent: systemd-run failed to queue rollback unit",
        );
        return Ok(Some(RollbackOutcome::Failed {
            phase: "systemd-run-fire".to_string(),
            exit_code: fire_status.code(),
        }));
    }
    Ok(None)
}

fn rollback_switch_bin(target_basename: &str) -> String {
    format!("/nix/store/{target_basename}/bin/switch-to-configuration")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn is_switch_in_progress_returns_false_when_lock_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let absent = dir.path().join("does-not-exist.lock");
        assert!(!is_switch_in_progress_at(&absent).await);
    }

    #[tokio::test]
    async fn is_switch_in_progress_returns_false_for_uncontended_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("test.lock");
        std::fs::write(&lock, b"").expect("create lock file");
        assert!(!is_switch_in_progress_at(&lock).await);
    }

    #[test]
    #[allow(clippy::default_constructed_unit_structs)]
    fn linux_backend_default_is_unit_struct() {
        let _b: LinuxBackend = LinuxBackend;
        let _: LinuxBackend = LinuxBackend::default();
    }

    /// Regression: fire_rollback used to invoke
    /// `/run/current-system/bin/switch-to-configuration`, which is still the
    /// failed closure when rollback fires (profile pointer flipped, but
    /// /run/current-system unchanged until switch-to-configuration completes).
    /// Use the rolled-back target's own binary.
    #[test]
    fn rollback_switch_bin_uses_target_store_path_not_current_system() {
        let basename = "abc123-nixos-system-web-01-26.05";
        assert_eq!(
            rollback_switch_bin(basename),
            "/nix/store/abc123-nixos-system-web-01-26.05/bin/switch-to-configuration",
        );
    }

    /// Build a fake system tree at `root` where each `rel_path` is a symlink
    /// pointing to a uniquely-named file under `root.targets/`. Same targets
    /// across two trees -> canonicalize-equal; different `tag` -> unequal.
    fn make_fake_system(root: &Path, rel_paths: &[&str], tag: &str) {
        let targets_dir = root.join(format!("targets-{tag}"));
        std::fs::create_dir_all(&targets_dir).unwrap();
        for rel in rel_paths {
            let target = targets_dir.join(rel.replace('/', "_"));
            std::fs::write(&target, b"").unwrap();
            let link = root.join(rel);
            if let Some(parent) = link.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::os::unix::fs::symlink(&target, &link).unwrap();
        }
    }

    fn share_targets(src_root: &Path, dst_root: &Path, rel_paths: &[&str]) {
        // Make dst symlinks resolve to the same canonical paths as src - the
        // identical-systems case.
        for rel in rel_paths {
            let src_link = src_root.join(rel);
            let canonical = std::fs::canonicalize(&src_link).unwrap();
            let dst_link = dst_root.join(rel);
            if let Some(parent) = dst_link.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::os::unix::fs::symlink(&canonical, &dst_link).unwrap();
        }
    }

    #[test]
    fn detect_returns_none_when_systems_are_identical() {
        let dir = tempfile::tempdir().unwrap();
        let cur = dir.path().join("current");
        let new = dir.path().join("new");
        let rels: Vec<&str> = SWITCH_INHIBITORS.iter().map(|(_, p)| *p).collect();
        make_fake_system(&cur, &rels, "shared");
        share_targets(&cur, &new, &rels);
        assert_eq!(detect_switch_inhibitors(&cur, &new), None);
    }

    #[test]
    fn detect_returns_dbus_when_dbus_service_target_differs() {
        let dir = tempfile::tempdir().unwrap();
        let cur = dir.path().join("current");
        let new = dir.path().join("new");
        let rels: Vec<&str> = SWITCH_INHIBITORS.iter().map(|(_, p)| *p).collect();
        // Same targets except dbus.service - only dbus should fire.
        make_fake_system(&cur, &rels, "cur");
        share_targets(&cur, &new, &rels);
        // Overwrite the dbus link in `new` to point somewhere else.
        let dbus_rel = "etc/systemd/system/dbus.service";
        let new_dbus_target = dir.path().join("targets-new-dbus");
        std::fs::create_dir_all(&new_dbus_target).unwrap();
        let new_dbus_file = new_dbus_target.join("dbus.service");
        std::fs::write(&new_dbus_file, b"").unwrap();
        let new_dbus_link = new.join(dbus_rel);
        std::fs::remove_file(&new_dbus_link).unwrap();
        std::os::unix::fs::symlink(&new_dbus_file, &new_dbus_link).unwrap();
        assert_eq!(detect_switch_inhibitors(&cur, &new), Some("dbus"));
    }

    #[test]
    fn detect_returns_none_when_one_side_missing_a_path() {
        let dir = tempfile::tempdir().unwrap();
        let cur = dir.path().join("current");
        let new = dir.path().join("new");
        // Only populate cur - new is empty, every canonicalize on new fails.
        let rels: Vec<&str> = SWITCH_INHIBITORS.iter().map(|(_, p)| *p).collect();
        make_fake_system(&cur, &rels, "cur");
        std::fs::create_dir_all(&new).unwrap();
        // Per the contract: missing path on one side is out-of-scope, returns None.
        assert_eq!(detect_switch_inhibitors(&cur, &new), None);
    }

    /// Regression: `<closure>/init` is regenerated per-system, so an init-
    /// only delta must NOT trigger defer (otherwise every update defers).
    #[test]
    fn detect_ignores_init_only_delta() {
        let dir = tempfile::tempdir().unwrap();
        let cur = dir.path().join("current");
        let new = dir.path().join("new");
        let rels: Vec<&str> = SWITCH_INHIBITORS.iter().map(|(_, p)| *p).collect();
        make_fake_system(&cur, &rels, "cur");
        share_targets(&cur, &new, &rels);
        // Plant a differing `init` in `new` only - kernel/systemd/dbus stay
        // equal. With init removed from SWITCH_INHIBITORS this MUST be None.
        let init_target_dir = dir.path().join("targets-new-init");
        std::fs::create_dir_all(&init_target_dir).unwrap();
        let init_file = init_target_dir.join("init");
        std::fs::write(&init_file, b"").unwrap();
        std::os::unix::fs::symlink(&init_file, new.join("init")).unwrap();
        assert_eq!(detect_switch_inhibitors(&cur, &new), None);
    }

    /// Catches re-ordering or short-circuit regressions if SWITCH_INHIBITORS
    /// is reshuffled: kernel is the trailing entry and must still fire when
    /// dbus + systemd match.
    #[test]
    fn detect_returns_kernel_when_kernel_differs_first() {
        let dir = tempfile::tempdir().unwrap();
        let cur = dir.path().join("current");
        let new = dir.path().join("new");
        let rels: Vec<&str> = SWITCH_INHIBITORS.iter().map(|(_, p)| *p).collect();
        make_fake_system(&cur, &rels, "cur");
        share_targets(&cur, &new, &rels);
        let kernel_rel = "kernel";
        let new_kernel_target = dir.path().join("targets-new-kernel");
        std::fs::create_dir_all(&new_kernel_target).unwrap();
        let new_kernel_file = new_kernel_target.join("bzImage");
        std::fs::write(&new_kernel_file, b"").unwrap();
        let new_kernel_link = new.join(kernel_rel);
        std::fs::remove_file(&new_kernel_link).unwrap();
        std::os::unix::fs::symlink(&new_kernel_file, &new_kernel_link).unwrap();
        assert_eq!(detect_switch_inhibitors(&cur, &new), Some("kernel"));
    }
}
