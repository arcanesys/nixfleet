//! Library entry point for the NixFleet agent.
//!
//! Exists so in-process integration tests can drive the poll loop with
//! `tokio::time::pause()` for deterministic cadence assertions. The
//! binary at `src/main.rs` is a thin wrapper that builds `Config` and
//! calls `run_loop`. Process-level fake time is impossible — paused
//! time only works in-process — which is why this lib exists at all.

use anyhow::Context;
use std::time::Duration;
use tokio::signal;
use tokio::time::{interval_at, Instant, Interval, MissedTickBehavior};
use tracing::{error, info, warn};

use crate::nix::ApplyOutcome;

/// Maximum retries on activation lock contention.
const MAX_APPLY_RETRIES: u32 = 3;

/// Base delay for exponential backoff on lock contention.
const APPLY_RETRY_BASE: Duration = Duration::from_secs(5);

pub mod comms;
pub mod config;
pub mod health;
pub mod metrics;
pub mod nix;
pub mod store;
pub mod tls;
pub mod types;

pub use config::Config;
use health::HealthRunner;
use store::{AsyncStore, Store};

/// Read host uptime from /proc/uptime (Linux). Returns 0 on failure.
fn read_uptime_seconds() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(String::from))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0)
}

/// Outcome of a poll cycle — tells the main loop how to schedule the next tick.
#[derive(Debug)]
pub enum PollOutcome {
    /// Poll succeeded (regardless of whether any work was done).
    /// If `poll_hint` is set, the CP suggested a faster next poll.
    Success { poll_hint: Option<u64> },
    /// Poll failed — network error, auth error, fetch failure, etc.
    /// The main loop should retry sooner than the configured poll_interval.
    Failed,
}

/// Run the agent's main event loop until shutdown.
///
/// This is the function `main.rs` calls. In-process integration tests
/// can call it directly under `tokio::time::pause()` to drive the loop
/// deterministically.
pub async fn run_loop(config: Config) -> anyhow::Result<()> {
    config.validate().context("invalid agent configuration")?;

    info!(
        machine_id = %config.machine_id,
        control_plane = %config.control_plane_url,
        poll_interval = ?config.poll_interval,
        dry_run = config.dry_run,
        "NixFleet agent starting"
    );

    if let Some(port) = config.metrics_port {
        metrics::init(port);
    }

    let store = Store::new(&config.db_path)?;
    store.init()?;
    // Wrap in an async facade so every subsequent call offloads the
    // blocking SQLite work via spawn_blocking instead of pinning a
    // tokio worker thread to a Mutex acquire.
    let store = AsyncStore::new(store);

    let client = comms::Client::new(&config)?;
    let health_runner = HealthRunner::from_config_path(&config.health_config_path);

    let mut health_tick = build_interval(config.health_interval);

    // Run an initial deploy cycle at startup so the agent checks for pending
    // deploys right away. The result sets the initial poll_tick interval:
    //   - Success+hint → fast polling (active rollout)
    //   - Success      → configured poll_interval
    //   - Failed       → retry_interval (short) to recover from bootstrap races
    let initial = run_deploy_cycle(&client, &config, &store, &health_runner).await;
    let mut poll_tick = build_interval(match initial {
        PollOutcome::Success { poll_hint: Some(h) } => {
            info!(
                poll_hint = h,
                "Initial poll: adjusting interval from CP hint"
            );
            Duration::from_secs(h)
        }
        PollOutcome::Success { poll_hint: None } => config.poll_interval,
        PollOutcome::Failed => {
            info!(retry_in = ?config.retry_interval, "Initial poll failed, scheduling retry");
            config.retry_interval
        }
    });

    // Main event loop — waits for one of: shutdown, health tick, poll tick.
    // Each branch runs to completion sequentially (no state machine inside select).
    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("Received shutdown signal, exiting gracefully");
                break;
            }
            _ = health_tick.tick() => {
                run_health_report(&client, &config, &health_runner).await;
            }
            _ = poll_tick.tick() => {
                match run_deploy_cycle(&client, &config, &store, &health_runner).await {
                    PollOutcome::Success { poll_hint: Some(hint) } => {
                        // CP suggested a faster interval (active rollout)
                        info!(poll_hint = hint, "Adjusting poll interval from CP hint");
                        poll_tick = build_interval(Duration::from_secs(hint));
                    }
                    PollOutcome::Success { poll_hint: None } => {
                        // Steady-state — ensure we're on the configured poll_interval.
                        // (No-op if already there; resets from a prior retry/hint state.)
                        poll_tick = build_interval(config.poll_interval);
                    }
                    PollOutcome::Failed => {
                        // Transient error or bootstrap race — retry sooner than full interval.
                        info!(retry_in = ?config.retry_interval, "Poll failed, scheduling retry");
                        poll_tick = build_interval(config.retry_interval);
                    }
                }
            }
        }
    }

    info!("Agent shut down cleanly");
    Ok(())
}

/// Build a tokio Interval whose first tick fires after `period` (not immediately).
/// This is critical when rebuilding intervals on retry — we do NOT want an
/// immediate re-tick that would cause a tight loop on repeated failures.
pub fn build_interval(period: Duration) -> Interval {
    let mut tick = interval_at(Instant::now() + period, period);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tick
}

/// Read the current system generation, logging a warning on failure.
///
/// The agent's loop must not abort when `/run/current-system` cannot be
/// read (e.g. during rare early-boot windows), so we fall back to an
/// empty string but surface the warning to operators instead of
/// silently swallowing it with `.unwrap_or_default()`.
async fn current_generation_or_warn() -> String {
    match nix::current_generation().await {
        Ok(gen) => gen,
        Err(e) => {
            warn!(error = %e, "failed to read current generation");
            String::new()
        }
    }
}

/// Send a periodic health report to the control plane.
async fn run_health_report(client: &comms::Client, config: &Config, health_runner: &HealthRunner) {
    info!("Running periodic health check");
    let health_report = health_runner.run_all().await;
    let report = types::Report {
        machine_id: config.machine_id.clone(),
        current_generation: current_generation_or_warn().await,
        success: health_report.all_passed,
        message: "health-check".to_string(),
        timestamp: chrono::Utc::now(),
        tags: config.tags.clone(),
        health: Some(health_report),
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: read_uptime_seconds(),
    };
    match client.post_report(&report).await {
        Ok(()) => info!("Health report sent"),
        Err(e) => warn!("Failed to send health report: {e}"),
    }
}

/// Retry an apply operation with exponential backoff on lock contention.
///
/// Calls `apply_fn` up to `MAX_APPLY_RETRIES + 1` times (initial + retries).
/// Returns `Ok(())` on success, `Ok(ApplyOutcome::LockContention(msg))` if
/// all retries exhausted, or `Err` on fatal failure.
async fn apply_with_retry<F, Fut>(apply_fn: F) -> anyhow::Result<nix::ApplyOutcome>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<nix::ApplyOutcome>>,
{
    let mut attempts = 0u32;
    loop {
        match apply_fn().await {
            Ok(ApplyOutcome::Applied) => return Ok(ApplyOutcome::Applied),
            Ok(ApplyOutcome::LockContention(msg)) => {
                attempts += 1;
                if attempts > MAX_APPLY_RETRIES {
                    return Ok(ApplyOutcome::LockContention(msg));
                }
                let delay = APPLY_RETRY_BASE * 2u32.pow(attempts - 1);
                warn!(
                    attempt = attempts,
                    max = MAX_APPLY_RETRIES,
                    delay_secs = delay.as_secs(),
                    stderr = %msg,
                    "Activation lock held, retrying"
                );
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Run a full deploy cycle: check → fetch → apply → verify → report.
/// Returns a PollOutcome telling the main loop how to schedule the next tick.
async fn run_deploy_cycle(
    client: &comms::Client,
    config: &Config,
    store: &AsyncStore,
    health_runner: &HealthRunner,
) -> PollOutcome {
    metrics::record_state_transition("idle", "checking");

    // Check: fetch desired generation from control plane.
    //
    // The CP returns 404 ("no generation set yet") on fresh-DB / first-
    // boot conditions; that maps to `Ok(None)` and is NOT an error.
    // We log INFO, return Success with no poll_hint, and let the loop
    // schedule the next tick at the configured `poll_interval` (the
    // steady-state interval) — NOT the retry interval. Real errors
    // (network, TLS, 5xx) still go through the WARN + Failed path.
    let poll_start = std::time::Instant::now();
    let desired = match client.get_desired_generation(&config.machine_id).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            info!("No desired generation set yet (CP returned 404); steady-state poll");
            metrics::record_poll(poll_start.elapsed());
            metrics::record_state_transition("checking", "idle");
            return PollOutcome::Success { poll_hint: None };
        }
        Err(e) => {
            warn!("Failed to check desired generation: {e}");
            if let Err(se) = store.log_error(&format!("check failed: {e}")).await {
                warn!("store error: {se}");
            }
            metrics::record_poll(poll_start.elapsed());
            metrics::record_state_transition("checking", "idle");
            return PollOutcome::Failed;
        }
    };
    metrics::record_poll(poll_start.elapsed());

    let poll_hint = desired.poll_hint;
    let current = current_generation_or_warn().await;
    metrics::record_generation(&current);

    if current == desired.hash {
        info!("Already at desired generation");
        if let Err(e) = store.log_check(&desired.hash, "up-to-date").await {
            warn!("store error: {e}");
        }
        metrics::record_state_transition("checking", "reporting");
        send_report(client, config, true, "up-to-date").await;
        return PollOutcome::Success { poll_hint };
    }

    info!(
        current = %current,
        desired = %desired.hash,
        "Generation mismatch, fetching"
    );

    // Fetch: pull closure from binary cache (or verify local presence).
    metrics::record_state_transition("checking", "fetching");
    let cache = desired.cache_url.as_deref().or(config.cache_url.as_deref());
    if let Err(e) = nix::fetch_closure(&desired.hash, cache).await {
        error!("Failed to fetch closure: {e}");
        if let Err(se) = store.log_error(&format!("fetch failed: {e}")).await {
            warn!("store error: {se}");
        }
        metrics::record_state_transition("fetching", "idle");
        // Fetch failure is transient — retry sooner than full poll_interval.
        return PollOutcome::Failed;
    }
    info!(hash = %desired.hash, "Closure fetched");

    if config.dry_run {
        info!("Dry run -- skipping apply");
        metrics::record_state_transition("fetching", "reporting");
        send_report(client, config, true, "dry-run: would apply").await;
        return PollOutcome::Success { poll_hint };
    }

    // Apply: switch-to-configuration with retry on lock contention.
    metrics::record_state_transition("fetching", "applying");

    match apply_with_retry(|| nix::apply_generation(&desired.hash)).await {
        Ok(ApplyOutcome::Applied) => {
            info!(hash = %desired.hash, "Generation applied");
        }
        Ok(ApplyOutcome::LockContention(msg)) => {
            error!(
                retries = MAX_APPLY_RETRIES,
                "Lock contention after all retries: {msg}"
            );
            if let Err(se) = store.log_error(&format!("lock contention: {msg}")).await {
                warn!("store error: {se}");
            }
            metrics::record_state_transition("applying", "reporting");
            send_report(
                client,
                config,
                false,
                &format!("lock contention after {MAX_APPLY_RETRIES} retries: {msg}"),
            )
            .await;
            return PollOutcome::Failed;
        }
        Err(e) => {
            error!("Failed to apply generation: {e}");
            metrics::record_state_transition("applying", "rolling_back");
            rollback_and_report(client, config, store, &format!("apply failed: {e}")).await;
            return PollOutcome::Success { poll_hint };
        }
    }

    // Verify: run health checks after apply.
    metrics::record_state_transition("applying", "verifying");
    let verify_report = health_runner.run_all().await;
    if !verify_report.all_passed {
        let failed: Vec<_> = verify_report
            .results
            .iter()
            .filter(|r| !r.is_pass())
            .map(|r| r.to_string())
            .collect();
        warn!(?failed, "Health checks failed after apply");
        metrics::record_state_transition("verifying", "rolling_back");
        rollback_and_report(
            client,
            config,
            store,
            &format!("health check failed: {}", failed.join(", ")),
        )
        .await;
        return PollOutcome::Success { poll_hint };
    }

    info!("Health checks passed");
    if let Err(e) = store.log_deploy(&desired.hash, true).await {
        warn!("store error: {e}");
    }
    metrics::record_state_transition("verifying", "reporting");
    send_report(client, config, true, "deployed").await;

    PollOutcome::Success { poll_hint }
}

/// Roll back to the previous generation and report the failure.
async fn rollback_and_report(
    client: &comms::Client,
    config: &Config,
    store: &AsyncStore,
    reason: &str,
) {
    warn!(reason, "Rolling back to previous generation");
    match nix::rollback().await {
        Ok(()) => {
            if let Err(e) = store.log_rollback(reason).await {
                warn!("store error: {e}");
            }
            metrics::record_state_transition("rolling_back", "reporting");
            send_report(client, config, false, &format!("rolled back: {reason}")).await;
        }
        Err(e) => {
            error!("Rollback failed: {e}");
            if let Err(se) = store.log_error(&format!("rollback failed: {e}")).await {
                warn!("store error: {se}");
            }
            metrics::record_state_transition("rolling_back", "idle");
        }
    }
}

/// Post a status report to the control plane.
async fn send_report(client: &comms::Client, config: &Config, success: bool, message: &str) {
    let report = types::Report {
        machine_id: config.machine_id.clone(),
        current_generation: current_generation_or_warn().await,
        success,
        message: message.to_string(),
        timestamp: chrono::Utc::now(),
        tags: config.tags.clone(),
        health: None,
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: read_uptime_seconds(),
    };
    match client.post_report(&report).await {
        Ok(()) => info!("Report sent"),
        Err(e) => warn!("Failed to send report: {e}"),
    }
    metrics::record_state_transition("reporting", "idle");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nix::ApplyOutcome;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_apply_with_retry_succeeds_immediately() {
        let result = apply_with_retry(|| async { Ok(ApplyOutcome::Applied) }).await;
        assert!(matches!(result, Ok(ApplyOutcome::Applied)));
    }

    #[tokio::test]
    async fn test_apply_with_retry_succeeds_after_contention() {
        tokio::time::pause();
        let call_count = AtomicU32::new(0);
        let result = apply_with_retry(|| {
            let n = call_count.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Ok(ApplyOutcome::LockContention("busy".to_string()))
                } else {
                    Ok(ApplyOutcome::Applied)
                }
            }
        })
        .await;
        assert!(matches!(result, Ok(ApplyOutcome::Applied)));
        assert_eq!(call_count.load(Ordering::SeqCst), 3); // initial + 2 retries
    }

    #[tokio::test]
    async fn test_apply_with_retry_exhausts_retries() {
        tokio::time::pause();
        let call_count = AtomicU32::new(0);
        let result = apply_with_retry(|| {
            call_count.fetch_add(1, Ordering::SeqCst);
            async { Ok(ApplyOutcome::LockContention("still busy".to_string())) }
        })
        .await;
        assert!(matches!(result, Ok(ApplyOutcome::LockContention(_))));
        assert_eq!(call_count.load(Ordering::SeqCst), 4); // initial + 3 retries
    }

    #[tokio::test]
    async fn test_apply_with_retry_propagates_fatal_error() {
        let result =
            apply_with_retry(|| async { Err(anyhow::anyhow!("spawn failed")) }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_apply_with_retry_fatal_after_contention() {
        tokio::time::pause();
        let call_count = AtomicU32::new(0);
        let result = apply_with_retry(|| {
            let n = call_count.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Ok(ApplyOutcome::LockContention("busy".to_string()))
                } else {
                    Err(anyhow::anyhow!("real failure"))
                }
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }
}
