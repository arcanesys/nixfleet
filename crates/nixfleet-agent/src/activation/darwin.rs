//! Darwin (nix-darwin) activation primitives. `setsid`-detached children
//! survive launchd's process-group SIGTERM during plist reload; `nohup`
//! doesn't work in launchd's no-controlling-tty context.

use std::process::Stdio;

use anyhow::Result;
use nixfleet_proto::agent_wire::EvaluatedTarget;

use super::{ActivationBackend, ActivationOutcome, RollbackOutcome};

#[derive(Clone, Copy, Debug, Default)]
pub struct DarwinBackend;

impl ActivationBackend for DarwinBackend {
    async fn is_switch_in_progress(&self) -> bool {
        false
    }
    async fn read_unit_exit_code(&self, _unit_name: &str) -> Option<i32> {
        None
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

async fn fire_switch(
    target: &EvaluatedTarget,
    store_path: &str,
) -> Result<Option<ActivationOutcome>> {
    use std::os::unix::process::CommandExt;

    tracing::info!(
        target_closure = %target.closure_hash,
        "agent: firing darwin activation (setsid-detached activate-user + activate)",
    );

    // GOTCHA: activate-user is legacy - modern closures often omit it; spawn errors non-fatal.
    let activate_user = format!("{store_path}/activate-user");
    if std::path::Path::new(&activate_user).exists() {
        let mut cmd = std::process::Command::new(&activate_user);
        cmd.stdin(Stdio::null());
        attach_activate_log_to(&mut cmd, ACTIVATE_LOG);
        // SAFETY: setsid is async-signal-safe; no alloc/lock in the closure.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        match cmd.spawn() {
            Ok(_child) => {
                tracing::debug!(
                    target_closure = %target.closure_hash,
                    "agent: darwin activate-user fired (detached)",
                );
            }
            Err(err) => {
                tracing::warn!(
                    target_closure = %target.closure_hash,
                    error = %err,
                    "agent: darwin activate-user spawn failed (non-fatal); continuing to system activate",
                );
            }
        }
    } else {
        tracing::debug!(
            target_closure = %target.closure_hash,
            "agent: darwin activate-user absent; skipping (modern closure shape)",
        );
    }

    // LOADBEARING: setsid detach survives launchd plist reload; nohup doesn't work without controlling tty.
    let activate = format!("{store_path}/activate");
    let mut cmd = std::process::Command::new(&activate);
    cmd.stdin(Stdio::null());
    attach_activate_log_to(&mut cmd, ACTIVATE_LOG);
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(_child) => {
            tracing::info!(
                target_closure = %target.closure_hash,
                "agent: darwin activate fired (setsid-detached); polling current-system",
            );
            Ok(None)
        }
        Err(err) => {
            tracing::error!(
                target_closure = %target.closure_hash,
                error = %err,
                "agent: darwin activate spawn failed",
            );
            Ok(Some(ActivationOutcome::SwitchFailed {
                phase: "darwin-activate-spawn".to_string(),
                exit_code: None,
            }))
        }
    }
}

async fn fire_rollback(target_basename: &str) -> Result<Option<RollbackOutcome>> {
    use std::os::unix::process::CommandExt;

    let store_path = format!("/nix/store/{target_basename}");
    let activate = format!("{store_path}/activate");
    if !std::path::Path::new(&activate).exists() {
        tracing::error!(
            activate = %activate,
            "agent: darwin rollback target has no activate script",
        );
        return Ok(Some(RollbackOutcome::Failed {
            phase: "darwin-activate-missing".to_string(),
            exit_code: None,
        }));
    }

    tracing::info!(
        target = %target_basename,
        "agent: firing darwin rollback (setsid-detached activate)",
    );
    let mut cmd = std::process::Command::new(&activate);
    cmd.stdin(Stdio::null());
    attach_activate_log_to(&mut cmd, ACTIVATE_LOG);
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(_child) => Ok(None),
        Err(err) => {
            tracing::error!(
                target = %target_basename,
                error = %err,
                "agent: darwin rollback activate spawn failed",
            );
            Ok(Some(RollbackOutcome::Failed {
                phase: "darwin-activate-spawn".to_string(),
                exit_code: None,
            }))
        }
    }
}

const ACTIVATE_LOG: &str = "/var/log/nixfleet-activate.log";

/// Falls back to inherit on IO error; launchd's StandardOutPath catches it.
fn attach_activate_log_to(cmd: &mut std::process::Command, path: &str) {
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(out) => {
            let err = match out.try_clone() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        path = path,
                        error = %e,
                        "could not clone activate log handle; using inherit",
                    );
                    cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
                    return;
                }
            };
            cmd.stdout(out).stderr(err);
        }
        Err(e) => {
            tracing::warn!(
                path = path,
                error = %e,
                "could not open activate log; using inherit",
            );
            cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn darwin_backend_is_switch_in_progress_returns_false() {
        assert!(!DarwinBackend.is_switch_in_progress().await);
    }

    #[tokio::test]
    async fn darwin_backend_read_unit_exit_code_returns_none() {
        assert_eq!(DarwinBackend.read_unit_exit_code("anything").await, None);
    }

    #[test]
    fn attach_activate_log_falls_back_to_inherit_when_path_unwritable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let unwritable = dir.path().join("does-not-exist").join("nope.log");
        let mut cmd = std::process::Command::new("true");
        attach_activate_log_to(&mut cmd, unwritable.to_str().unwrap());
    }

    #[test]
    fn attach_activate_log_succeeds_when_path_writable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("activate.log");
        let mut cmd = std::process::Command::new("true");
        attach_activate_log_to(&mut cmd, path.to_str().unwrap());
        assert!(path.exists(), "log file should be created");
    }

    #[test]
    fn darwin_backend_default_is_unit_struct() {
        let _b: DarwinBackend = DarwinBackend;
        #[allow(clippy::default_constructed_unit_structs)]
        let _: DarwinBackend = DarwinBackend::default();
    }
}
