use clap::Parser;
use std::time::Duration;
use tokio::signal;
use tracing::{error, info, warn};

mod comms;
mod config;
mod health;
mod nix;
mod state;
mod store;
mod tls;
mod types;

use config::Config;
use state::AgentState;
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

    /// Poll interval in seconds
    #[arg(long, default_value = "300", env = "NIXFLEET_POLL_INTERVAL")]
    poll_interval: u64,

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
        cache_url: cli.cache_url,
        db_path: cli.db_path.clone(),
        dry_run: cli.dry_run,
        allow_insecure: cli.allow_insecure,
        client_cert: cli.client_cert,
        client_key: cli.client_key,
    };

    info!(
        machine_id = %config.machine_id,
        control_plane = %config.control_plane_url,
        poll_interval = ?config.poll_interval,
        dry_run = config.dry_run,
        "NixFleet agent starting"
    );

    // Initialize SQLite store
    let store = Store::new(&cli.db_path)?;
    store.init()?;

    // Create HTTP client for control plane communication
    let client = comms::Client::new(&config)?;
    let mut agent_state = AgentState::Idle;

    // Main poll loop — state machine drives all transitions
    // Graceful shutdown on SIGTERM/SIGINT
    loop {
        // Check for shutdown signal at each iteration
        let next_state = tokio::select! {
            _ = signal::ctrl_c() => {
                info!("Received shutdown signal, exiting gracefully");
                break;
            }
            state = async {
                match agent_state {
                    AgentState::Idle => {
                        tokio::time::sleep(config.poll_interval).await;
                        AgentState::Checking
                    }
                    AgentState::Checking => {
                        match client.get_desired_generation(&config.machine_id).await {
                            Ok(desired) => {
                                let current = nix::current_generation().await.unwrap_or_default();
                                if current == desired.hash {
                                    info!("Already at desired generation");
                                    store.log_check(&desired.hash, "up-to-date")
                                        .unwrap_or_else(|e| warn!("store error: {e}"));
                                    AgentState::Reporting {
                                        success: true,
                                        message: "up-to-date".into(),
                                    }
                                } else {
                                    info!(
                                        current = %current,
                                        desired = %desired.hash,
                                        "Generation mismatch, fetching"
                                    );
                                    AgentState::Fetching { desired }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to check desired generation: {e}");
                                store.log_error(&format!("check failed: {e}"))
                                    .unwrap_or_else(|e| warn!("store error: {e}"));
                                AgentState::Idle
                            }
                        }
                    }
                    AgentState::Fetching { desired } => {
                        // Use per-generation cache URL if provided, else fall back to global config
                        let cache = desired
                            .cache_url
                            .as_deref()
                            .or(config.cache_url.as_deref());
                        match nix::fetch_closure(&desired.hash, cache).await {
                            Ok(()) => {
                                info!(hash = %desired.hash, "Closure fetched");
                                if config.dry_run {
                                    info!("Dry run -- skipping apply");
                                    AgentState::Reporting {
                                        success: true,
                                        message: "dry-run: would apply".into(),
                                    }
                                } else {
                                    AgentState::Applying { desired }
                                }
                            }
                            Err(e) => {
                                error!("Failed to fetch closure: {e}");
                                store.log_error(&format!("fetch failed: {e}"))
                                    .unwrap_or_else(|e| warn!("store error: {e}"));
                                AgentState::Idle
                            }
                        }
                    }
                    AgentState::Applying { desired } => match nix::apply_generation(&desired.hash).await {
                        Ok(()) => {
                            info!(hash = %desired.hash, "Generation applied");
                            AgentState::Verifying { desired }
                        }
                        Err(e) => {
                            error!("Failed to apply generation: {e}");
                            AgentState::RollingBack {
                                reason: format!("apply failed: {e}"),
                            }
                        }
                    },
                    AgentState::Verifying { desired } => match health::check_system().await {
                        Ok(healthy) if healthy => {
                            info!("Health check passed");
                            store.log_deploy(&desired.hash, true)
                                .unwrap_or_else(|e| warn!("store error: {e}"));
                            AgentState::Reporting {
                                success: true,
                                message: "deployed".into(),
                            }
                        }
                        Ok(_) => {
                            warn!("Health check failed after apply");
                            AgentState::RollingBack {
                                reason: "health check failed".into(),
                            }
                        }
                        Err(e) => {
                            error!("Health check error: {e}");
                            AgentState::RollingBack {
                                reason: format!("health check error: {e}"),
                            }
                        }
                    },
                    AgentState::RollingBack { reason } => {
                        warn!(reason = %reason, "Rolling back to previous generation");
                        match nix::rollback().await {
                            Ok(()) => {
                                store.log_rollback(&reason)
                                    .unwrap_or_else(|e| warn!("store error: {e}"));
                                AgentState::Reporting {
                                    success: false,
                                    message: format!("rolled back: {reason}"),
                                }
                            }
                            Err(e) => {
                                error!("Rollback failed: {e}");
                                store.log_error(&format!("rollback failed: {e}"))
                                    .unwrap_or_else(|e| warn!("store error: {e}"));
                                AgentState::Idle
                            }
                        }
                    }
                    AgentState::Reporting { success, message } => {
                        let report = types::Report {
                            machine_id: config.machine_id.clone(),
                            current_generation: nix::current_generation().await.unwrap_or_default(),
                            success,
                            message,
                            timestamp: chrono::Utc::now(),
                        };
                        match client.post_report(&report).await {
                            Ok(()) => info!("Report sent"),
                            Err(e) => warn!("Failed to send report: {e}"),
                        }
                        AgentState::Idle
                    }
                }
            } => state,
        };
        agent_state = next_state;
    }

    info!("Agent shut down cleanly");
    Ok(())
}
