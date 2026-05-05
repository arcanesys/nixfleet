//! Activation outcome types + `ActivationBackend` trait + cfg-selected default.
//!
//! Backend trait contract:
//! - `is_switch_in_progress` is fail-open (false = no contender OR unknown).
//! - `read_unit_exit_code` returns `None` rather than synthesising a 0.
//! - `fire_*` are fire-and-forget: `Ok(None)` -> caller polls; `Ok(Some)` ->
//!   fire-step failure, no poll; `Err` -> spawn-level I/O error only.

use std::time::Duration;

use anyhow::Result;
use nixfleet_proto::agent_wire::EvaluatedTarget;

// LOADBEARING: 300s must stay inside CP's DEFAULT_CONFIRM_DEADLINE_SECS=360 - exceeding splits state.
pub const POLL_BUDGET: Duration = Duration::from_secs(300);
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum ActivationOutcome {
    FiredAndPolled,
    RealiseFailed {
        reason: String,
    },
    /// Distinct from RealiseFailed so dashboards can route trust violations.
    SignatureMismatch {
        closure_hash: String,
        stderr_tail: String,
    },
    SwitchFailed {
        phase: String,
        exit_code: Option<i32>,
    },
    /// `/run/current-system` flipped to a basename that is neither expected
    /// nor pre-switch - caller rolls back.
    VerifyMismatch {
        expected: String,
        actual: String,
    },
    /// Profile flipped via `nix-env --set` but live switch was skipped because
    /// activating component `component` (dbus, systemd, kernel, init) on a
    /// running system is unsafe - nixos-rebuild refuses the same. New gen
    /// activates on next boot.
    DeferredPendingReboot {
        component: String,
    },
}

#[derive(Debug)]
pub enum RollbackOutcome {
    FiredAndPolled,
    Failed {
        phase: String,
        exit_code: Option<i32>,
    },
}

impl RollbackOutcome {
    pub fn success(&self) -> bool {
        matches!(self, RollbackOutcome::FiredAndPolled)
    }
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            RollbackOutcome::Failed { exit_code, .. } => *exit_code,
            RollbackOutcome::FiredAndPolled => None,
        }
    }
    pub fn phase(&self) -> Option<&str> {
        match self {
            RollbackOutcome::Failed { phase, .. } => Some(phase.as_str()),
            RollbackOutcome::FiredAndPolled => None,
        }
    }
}

#[cfg(target_os = "macos")]
pub use super::darwin::DarwinBackend;
#[cfg(target_os = "linux")]
pub use super::linux::LinuxBackend;

#[cfg(target_os = "linux")]
pub type DefaultBackend = LinuxBackend;
#[cfg(target_os = "macos")]
pub type DefaultBackend = DarwinBackend;

#[cfg(target_os = "linux")]
pub const DEFAULT_BACKEND: DefaultBackend = LinuxBackend;
#[cfg(target_os = "macos")]
pub const DEFAULT_BACKEND: DefaultBackend = DarwinBackend;

pub trait ActivationBackend: Send + Sync {
    fn is_switch_in_progress(&self) -> impl std::future::Future<Output = bool> + Send;
    fn read_unit_exit_code(
        &self,
        unit_name: &str,
    ) -> impl std::future::Future<Output = Option<i32>> + Send;
    fn fire_switch(
        &self,
        target: &EvaluatedTarget,
        store_path: &str,
    ) -> impl std::future::Future<Output = Result<Option<ActivationOutcome>>> + Send;
    fn fire_rollback(
        &self,
        target_basename: &str,
    ) -> impl std::future::Future<Output = Result<Option<RollbackOutcome>>> + Send;
}
