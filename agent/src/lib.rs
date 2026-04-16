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

/// Maximum retries when a fired switch fails (poll timeout + bad exit status).
const MAX_SWITCH_RETRIES: u32 = 3;

/// Timeout for polling the current system path after firing a switch.
const SWITCH_POLL_TIMEOUT: Duration = Duration::from_secs(300);

/// Interval between polls of the current system path.
const SWITCH_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub mod comms;
pub mod config;
pub mod health;
pub mod metrics;
pub mod nix;
pub mod platform;
pub mod store;
pub mod tls;
pub mod types;

pub use config::Config;
use health::HealthRunner;
use store::{AsyncStore, Store};

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
        version = env!("CARGO_PKG_VERSION"),
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
        uptime_seconds: platform::uptime_seconds(),
    };
    match client.post_report(&report).await {
        Ok(()) => info!("Health report sent"),
        Err(e) => warn!("Failed to send health report: {e}"),
    }
}

/// Fire switch-to-configuration and poll until the generation matches.
///
/// Retries the entire fire+poll cycle up to `MAX_SWITCH_RETRIES` times
/// if the switch unit exits with failure. Returns `Ok(true)` on success,
/// `Ok(false)` if all retries exhausted or the switch outcome is unknown.
async fn fire_poll_switch(store_path: &str) -> anyhow::Result<bool> {
    nix::fire_switch(store_path).await?;

    let path = std::path::Path::new(platform::CURRENT_SYSTEM_PATH);
    if nix::poll_generation(store_path, path, SWITCH_POLL_TIMEOUT, SWITCH_POLL_INTERVAL).await? {
        return Ok(true);
    }

    // Poll timed out — check transient unit status and retry if it failed.
    for attempt in 1..=MAX_SWITCH_RETRIES {
        match nix::check_switch_exit_status().await? {
            Some(false) => {
                warn!(
                    attempt,
                    max = MAX_SWITCH_RETRIES,
                    "Switch unit failed, retrying"
                );
                nix::fire_switch(store_path).await?;
                if nix::poll_generation(store_path, path, SWITCH_POLL_TIMEOUT, SWITCH_POLL_INTERVAL)
                    .await?
                {
                    return Ok(true);
                }
            }
            _ => {
                // Still running, succeeded-but-mismatch, or unit not found.
                warn!("Switch poll timed out, unit status inconclusive");
                return Ok(false);
            }
        }
    }
    Ok(false)
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

    // Check if another switch is already in progress (e.g. manual nixos-rebuild).
    // If so, skip — the next poll cycle will see the updated generation.
    if nix::is_switch_in_progress() {
        info!("System switch already in progress, deferring to next poll");
        metrics::record_state_transition("fetching", "idle");
        return PollOutcome::Success { poll_hint };
    }

    // Apply: fire switch-to-configuration in a detached transient service,
    // then poll /run/current-system until it matches the desired generation.
    // The agent may be killed mid-switch (self-switch); on restart the
    // initial deploy cycle re-enters this path and the poll succeeds.
    metrics::record_state_transition("fetching", "applying");
    send_report(client, config, true, "applying").await;

    let applied = match fire_poll_switch(&desired.hash).await {
        Ok(true) => {
            info!(hash = %desired.hash, "Generation applied");
            true
        }
        Ok(false) => {
            error!("Failed to apply generation after retries");
            false
        }
        Err(e) => {
            error!("Fatal error during apply: {e}");
            false
        }
    };

    if !applied {
        metrics::record_state_transition("applying", "rolling_back");
        rollback_and_report(client, config, store, "switch timed out after retries").await;
        return PollOutcome::Success { poll_hint };
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
        uptime_seconds: platform::uptime_seconds(),
    };
    match client.post_report(&report).await {
        Ok(()) => info!("Report sent"),
        Err(e) => warn!("Failed to send report: {e}"),
    }
    metrics::record_state_transition("reporting", "idle");
}

