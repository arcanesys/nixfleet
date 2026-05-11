//! `nixfleet` operator CLI thin wrapper. Logic lives in the lib so
//! integration tests can exercise it in-process.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use nixfleet_cli::{run_status, run_trace, ResolvedClientConfig};

#[derive(Parser, Debug)]
#[command(name = "nixfleet", about = "NixFleet operator CLI", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show fleet state: convergence, staleness, outstanding compliance per host.
    Status(StatusArgs),
    /// Rollout-scoped operations.
    #[command(subcommand)]
    Rollout(RolloutCommands),
    /// Operator-side config management.
    #[command(subcommand)]
    Config(ConfigCommands),
    /// Derive base64 ed25519 pubkey from a raw private key file.
    DerivePubkey(nixfleet_cli::commands::derive_pubkey::Args),
    /// Mint an mTLS client cert from the offline fleet root CA.
    MintOperatorCert(nixfleet_cli::commands::mint_operator_cert::Args),
    /// Mint a bootstrap token for first-boot fleet enrollment.
    MintToken(nixfleet_cli::commands::mint_token::Args),
}

#[derive(Subcommand, Debug)]
enum RolloutCommands {
    /// Wave-by-wave dispatch history for a rollout.
    Trace(TraceArgs),
}

#[derive(Subcommand, Debug)]
enum ConfigCommands {
    /// Write ~/.config/nixfleet/config.toml from the given flags.
    Init(ConfigInitArgs),
}

#[derive(clap::Args, Debug)]
struct ConfigInitArgs {
    /// CP base URL, e.g. https://cp.example.com:8080.
    #[arg(long)]
    cp_url: String,
    /// Path to fleet CA cert (PEM).
    #[arg(long)]
    ca_cert: PathBuf,
    /// Operator client cert (PEM).
    #[arg(long)]
    client_cert: PathBuf,
    /// Operator client key (PEM).
    #[arg(long)]
    client_key: PathBuf,
    /// Override the config path (defaults to dirs::config_dir/nixfleet/config.toml).
    #[arg(long)]
    path: Option<PathBuf>,
    /// Overwrite an existing config file.
    #[arg(long)]
    force: bool,
}

#[derive(clap::Args, Debug)]
struct StatusArgs {
    #[command(flatten)]
    conn: ConnArgs,
    /// Emit JSON of the raw HostsResponse instead of a rendered table.
    #[arg(long)]
    json: bool,
    /// Disable ANSI colour even when stdout is a TTY.
    #[arg(long)]
    no_color: bool,
}

#[derive(clap::Args, Debug)]
struct TraceArgs {
    rollout_id: String,
    #[command(flatten)]
    conn: ConnArgs,
    /// Emit JSON of the raw RolloutTrace instead of a rendered table.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args, Debug)]
struct ConnArgs {
    /// CP base URL (https://host:port). Falls back to `NIXFLEET_CP_URL` env, then config file.
    #[arg(long)]
    cp_url: Option<String>,
    /// Path to fleet CA cert (PEM). Falls back to `NIXFLEET_CA_CERT` env, then config file.
    #[arg(long)]
    ca_cert: Option<PathBuf>,
    /// Operator client cert (PEM). Falls back to `NIXFLEET_CLIENT_CERT` env, then config file.
    #[arg(long)]
    client_cert: Option<PathBuf>,
    /// Operator client key (PEM). Falls back to `NIXFLEET_CLIENT_KEY` env, then config file.
    #[arg(long)]
    client_key: Option<PathBuf>,
    /// Override config-file path (default: dirs::config_dir/nixfleet/config.toml).
    #[arg(long)]
    config: Option<PathBuf>,
}

impl ConnArgs {
    fn resolve(&self) -> Result<ResolvedClientConfig> {
        let path = self
            .config
            .clone()
            .or_else(|| std::env::var_os("NIXFLEET_CONFIG").map(PathBuf::from))
            .unwrap_or_else(nixfleet_cli::config::default_config_path);
        let env = nixfleet_cli::Overrides {
            cp_url: std::env::var("NIXFLEET_CP_URL").ok(),
            ca_cert: std::env::var_os("NIXFLEET_CA_CERT").map(PathBuf::from),
            client_cert: std::env::var_os("NIXFLEET_CLIENT_CERT").map(PathBuf::from),
            client_key: std::env::var_os("NIXFLEET_CLIENT_KEY").map(PathBuf::from),
        };
        let flags = nixfleet_cli::Overrides {
            cp_url: self.cp_url.clone(),
            ca_cert: self.ca_cert.clone(),
            client_cert: self.client_cert.clone(),
            client_key: self.client_key.clone(),
        };
        nixfleet_cli::config::resolve(Some(&path), &env, &flags).map_err(|e| anyhow::anyhow!("{e}"))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Status(args) => {
            let cfg = args.conn.resolve()?;
            let color = nixfleet_cli::color::detect(args.no_color);
            print!("{}", run_status(&cfg, args.json, color).await?);
            Ok(())
        }
        Commands::Rollout(RolloutCommands::Trace(args)) => {
            let cfg = args.conn.resolve()?;
            print!("{}", run_trace(&cfg, &args.rollout_id, args.json).await?);
            Ok(())
        }
        Commands::Config(ConfigCommands::Init(args)) => {
            let path = args
                .path
                .unwrap_or_else(nixfleet_cli::config::default_config_path);
            let written = nixfleet_cli::run_config_init(
                &path,
                args.cp_url,
                args.ca_cert,
                args.client_cert,
                args.client_key,
                args.force,
            )?;
            eprintln!("wrote {}", written.display());
            Ok(())
        }
        Commands::DerivePubkey(args) => nixfleet_cli::commands::derive_pubkey::run(args),
        Commands::MintOperatorCert(args) => nixfleet_cli::commands::mint_operator_cert::run(args),
        Commands::MintToken(args) => nixfleet_cli::commands::mint_token::run(args),
    }
}
