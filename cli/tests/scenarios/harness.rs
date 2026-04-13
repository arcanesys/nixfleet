//! Shared test harness for CLI scenario tests.
//!
//! All CLI integration tests compile into a single binary (`scenarios.rs`).
//! Two process-global resources need serialization:
//!
//! 1. **Environment variables** — `std::env` is process-global. Tests that
//!    read or write `NIXFLEET_*` vars (or `HOSTNAME`) must hold `env_lock()`
//!    so parallel tests don't see each other's mutations.
//!
//! 2. **Binary spawning** — tests that launch the real `nixfleet` binary
//!    via `assert_cmd` against a wiremock `MockServer` can race on
//!    ephemeral port allocation under high parallelism. The spawned
//!    binary also inherits the parent's environment, so env-var leakage
//!    from a concurrent test can misroute it to the wrong mock. Holding
//!    `cli_lock()` serializes these tests, eliminating flaky failures
//!    that only reproduce under `cargo test` (not `cargo nextest`).
//!
//! Both locks are independent: `config.rs` tests that call `resolve()`
//! in-process only need `env_lock()`. Tests that spawn the binary need
//! `cli_lock()` (which implicitly covers env safety since the binary
//! inherits a stable environment when only one test runs at a time).

#![allow(dead_code)]

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serialize tests that spawn the real `nixfleet` binary via `assert_cmd`.
///
/// Prevents ephemeral-port races between concurrent wiremock listeners
/// and env-var leakage into the spawned binary's inherited environment.
pub fn cli_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Serialize tests that mutate `NIXFLEET_*` or `HOSTNAME` environment
/// variables. Held by `config.rs` tests that call `resolve()` in-process.
pub fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Clear every `NIXFLEET_*` env var that `resolve()` reads.
///
/// Called at the start and end of every env-sensitive test so the test
/// starts from a known blank-env baseline regardless of leakage from
/// sibling tests or the developer's outer shell.
pub fn clear_nixfleet_env() {
    for k in [
        "NIXFLEET_CONTROL_PLANE_URL",
        "NIXFLEET_API_KEY",
        "NIXFLEET_CA_CERT",
        "NIXFLEET_CLIENT_CERT",
        "NIXFLEET_CLIENT_KEY",
    ] {
        std::env::remove_var(k);
    }
}
