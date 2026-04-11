use anyhow::Context;
use clap::Parser;
use std::time::Duration;
use tokio::signal;
use tokio::time::{interval_at, Instant, Interval, MissedTickBehavior};
use tracing::{error, info, warn};

mod comms;
mod config;
mod health;
mod metrics;
mod nix;
mod store;
mod tls;
mod types;

use config::Config;
use health::HealthRunner;
use store::Store;

#[derive(Parser)]
#[command(name = "nixfleet-agent", about = "NixFleet fleet management agent")]
struct Cli {
    /// Control plane URL
    #[arg(long, env = "NIXFLEET_CONTROL_PLANE_URL")]
    control_plane_url: String,

    /// Machine ID (hostname)
    #[arg(long, env = "NIXFLEET_MACHINE_ID")]
    machine_id: String,

    /// Poll interval in seconds (steady-state)
    #[arg(long, default_value = "300", env = "NIXFLEET_POLL_INTERVAL")]
    poll_interval: u64,

    /// Retry interval in seconds after a failed poll
    #[arg(long, default_value = "30", env = "NIXFLEET_RETRY_INTERVAL")]
    retry_interval: u64,

    /// Binary cache URL (optional, for nix copy --from)
    #[arg(long, env = "NIXFLEET_CACHE_URL")]
    cache_url: Option<String>,

    /// SQLite database path
    #[arg(
        long,
        default_value = "/var/lib/nixfleet/state.db",
        env = "NIXFLEET_DB_PATH"
    )]
    db_path: String,

    /// Dry run (check + fetch but don't apply)
    #[arg(long)]
    dry_run: bool,

    /// Allow insecure HTTP connections (dev only)
    #[arg(long, env = "NIXFLEET_ALLOW_INSECURE", default_value = "false")]
    allow_insecure: bool,

    /// Path to client certificate PEM file (for mTLS)
    #[arg(long, env = "NIXFLEET_CLIENT_CERT")]
    client_cert: Option<String>,

    /// Path to client private key PEM file (for mTLS)
    #[arg(long, env = "NIXFLEET_CLIENT_KEY")]
    client_key: Option<String>,

    /// Path to health-checks JSON configuration
    #[arg(
        long,
        default_value = "/etc/nixfleet/health-checks.json",
        env = "NIXFLEET_HEALTH_CONFIG"
    )]
    health_config: String,

    /// Health check interval in seconds
    #[arg(long, default_value = "60", env = "NIXFLEET_HEALTH_INTERVAL")]
    health_interval: u64,

    /// Machine tags (comma-separated)
    #[arg(long, env = "NIXFLEET_TAGS", value_delimiter = ',')]
    tags: Vec<String>,

    /// Port for Prometheus metrics (disabled when omitted)
    #[arg(long, env = "NIXFLEET_METRICS_PORT")]
    metrics_port: Option<u16>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize structured JSON logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nixfleet_agent=info".into()),
        )
        .json()
        .init();

    let cli = Cli::parse();

    let config = Config {
        control_plane_url: cli.control_plane_url,
        machine_id: cli.machine_id,
        poll_interval: Duration::from_secs(cli.poll_interval),
        retry_interval: Duration::from_secs(cli.retry_interval),
        cache_url: cli.cache_url,
        db_path: cli.db_path.clone(),
        dry_run: cli.dry_run,
        allow_insecure: cli.allow_insecure,
        client_cert: cli.client_cert,
        client_key: cli.client_key,
        health_config_path: cli.health_config.clone(),
        health_interval: Duration::from_secs(cli.health_interval),
        tags: cli.tags,
        metrics_port: cli.metrics_port,
    };

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

    let store = Store::new(&cli.db_path)?;
    store.init()?;

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
            info!(poll_hint = h, "Initial poll: adjusting interval from CP hint");
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
fn build_interval(period: Duration) -> Interval {
    let mut tick = interval_at(Instant::now() + period, period);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tick
}

/// Send a periodic health report to the control plane.
async fn run_health_report(
    client: &comms::Client,
    config: &Config,
    health_runner: &HealthRunner,
) {
    info!("Running periodic health check");
    let health_report = health_runner.run_all().await;
    let report = types::Report {
        machine_id: config.machine_id.clone(),
        current_generation: nix::current_generation().await.unwrap_or_default(),
        success: health_report.all_passed,
        message: "health-check".to_string(),
        timestamp: chrono::Utc::now(),
        tags: config.tags.clone(),
        health: Some(health_report),
    };
    match client.post_report(&report).await {
        Ok(()) => info!("Health report sent"),
        Err(e) => warn!("Failed to send health report: {e}"),
    }
}

/// Outcome of a poll cycle — tells the main loop how to schedule the next tick.
#[derive(Debug)]
enum PollOutcome {
    /// Poll succeeded (regardless of whether any work was done).
    /// If `poll_hint` is set, the CP suggested a faster next poll.
    Success { poll_hint: Option<u64> },
    /// Poll failed — network error, auth error, fetch failure, etc.
    /// The main loop should retry sooner than the configured poll_interval.
    Failed,
}

/// Run a full deploy cycle: check → fetch → apply → verify → report.
/// Returns a PollOutcome telling the main loop how to schedule the next tick.
async fn run_deploy_cycle(
    client: &comms::Client,
    config: &Config,
    store: &Store,
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
            store
                .log_error(&format!("check failed: {e}"))
                .unwrap_or_else(|e| warn!("store error: {e}"));
            metrics::record_poll(poll_start.elapsed());
            metrics::record_state_transition("checking", "idle");
            return PollOutcome::Failed;
        }
    };
    metrics::record_poll(poll_start.elapsed());

    let poll_hint = desired.poll_hint;
    let current = nix::current_generation().await.unwrap_or_default();
    metrics::record_generation(&current);

    if current == desired.hash {
        info!("Already at desired generation");
        store
            .log_check(&desired.hash, "up-to-date")
            .unwrap_or_else(|e| warn!("store error: {e}"));
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
    let cache = desired
        .cache_url
        .as_deref()
        .or(config.cache_url.as_deref());
    if let Err(e) = nix::fetch_closure(&desired.hash, cache).await {
        error!("Failed to fetch closure: {e}");
        store
            .log_error(&format!("fetch failed: {e}"))
            .unwrap_or_else(|e| warn!("store error: {e}"));
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

    // Apply: switch-to-configuration.
    metrics::record_state_transition("fetching", "applying");
    if let Err(e) = nix::apply_generation(&desired.hash).await {
        error!("Failed to apply generation: {e}");
        metrics::record_state_transition("applying", "rolling_back");
        rollback_and_report(client, config, store, &format!("apply failed: {e}")).await;
        return PollOutcome::Success { poll_hint };
    }
    info!(hash = %desired.hash, "Generation applied");

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
    store
        .log_deploy(&desired.hash, true)
        .unwrap_or_else(|e| warn!("store error: {e}"));
    metrics::record_state_transition("verifying", "reporting");
    send_report(client, config, true, "deployed").await;

    PollOutcome::Success { poll_hint }
}

/// Roll back to the previous generation and report the failure.
async fn rollback_and_report(
    client: &comms::Client,
    config: &Config,
    store: &Store,
    reason: &str,
) {
    warn!(reason, "Rolling back to previous generation");
    match nix::rollback().await {
        Ok(()) => {
            store
                .log_rollback(reason)
                .unwrap_or_else(|e| warn!("store error: {e}"));
            metrics::record_state_transition("rolling_back", "reporting");
            send_report(client, config, false, &format!("rolled back: {reason}")).await;
        }
        Err(e) => {
            error!("Rollback failed: {e}");
            store
                .log_error(&format!("rollback failed: {e}"))
                .unwrap_or_else(|e| warn!("store error: {e}"));
            metrics::record_state_transition("rolling_back", "idle");
        }
    }
}

/// Post a status report to the control plane.
async fn send_report(client: &comms::Client, config: &Config, success: bool, message: &str) {
    let report = types::Report {
        machine_id: config.machine_id.clone(),
        current_generation: nix::current_generation().await.unwrap_or_default(),
        success,
        message: message.to_string(),
        timestamp: chrono::Utc::now(),
        tags: config.tags.clone(),
        health: None,
    };
    match client.post_report(&report).await {
        Ok(()) => info!("Report sent"),
        Err(e) => warn!("Failed to send report: {e}"),
    }
    metrics::record_state_transition("reporting", "idle");
}
