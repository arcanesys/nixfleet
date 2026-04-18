//! Thin binary wrapper around `nixfleet_agent::run_loop`.
//!
//! All loop logic, helpers, and the deploy cycle live in `lib.rs`. This
//! file owns nothing but argument parsing, logging setup, and a single
//! call into the library.

use clap::Parser;
use nixfleet_agent::Config;
use std::time::Duration;

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
    #[arg(long, default_value = "60", env = "NIXFLEET_POLL_INTERVAL")]
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

    /// Path to CA certificate PEM file (trusted in addition to system roots)
    #[arg(long, env = "NIXFLEET_CA_CERT")]
    ca_cert: Option<String>,

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
async fn main() {
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
        db_path: cli.db_path,
        dry_run: cli.dry_run,
        allow_insecure: cli.allow_insecure,
        ca_cert: cli.ca_cert,
        client_cert: cli.client_cert,
        client_key: cli.client_key,
        health_config_path: cli.health_config,
        health_interval: Duration::from_secs(cli.health_interval),
        tags: cli.tags,
        metrics_port: cli.metrics_port,
    };

    // Never exit non-zero. On Darwin, launchd treats exit code 78
    // (EX_CONFIG) as "misconfigured, never restart" — permanently
    // disabling the agent. By always exiting 0, KeepAlive + ThrottleInterval
    // handle restarts reliably. Errors are logged, not swallowed.
    // On Linux, systemd's Restart=always handles this regardless of exit
    // code, so this is harmless there.
    if let Err(e) = nixfleet_agent::run_loop(config).await {
        tracing::error!(error = %e, "Agent exited with error, will be restarted by init system");
    }
}
