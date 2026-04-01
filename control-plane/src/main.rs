use clap::Parser;
use nixfleet_control_plane::{build_app, db, state};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Parser)]
#[command(
    name = "nixfleet-control-plane",
    about = "NixFleet control plane server"
)]
struct Cli {
    /// Address to listen on
    #[arg(long, default_value = "0.0.0.0:8080", env = "NIXFLEET_CP_LISTEN")]
    listen: String,

    /// SQLite database path for persistent state
    #[arg(
        long,
        default_value = "/var/lib/nixfleet-cp/state.db",
        env = "NIXFLEET_CP_DB_PATH"
    )]
    db_path: String,

    /// TLS certificate PEM file (enables HTTPS when set)
    #[arg(long, env = "NIXFLEET_CP_TLS_CERT")]
    tls_cert: Option<String>,

    /// TLS private key PEM file
    #[arg(long, env = "NIXFLEET_CP_TLS_KEY")]
    tls_key: Option<String>,

    /// Client CA PEM file (enables mTLS when set)
    #[arg(long, env = "NIXFLEET_CP_CLIENT_CA")]
    client_ca: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nixfleet=info,tower_http=info".into()),
        )
        .init();

    let cli = Cli::parse();

    let fleet_state = Arc::new(RwLock::new(state::FleetState::new()));
    let db = Arc::new(db::Db::new(&cli.db_path)?);
    db.migrate()?;

    // Hydrate in-memory state from DB on startup
    state::hydrate_from_db(&fleet_state, &db).await?;

    let app = build_app(fleet_state, db);

    match (&cli.tls_cert, &cli.tls_key) {
        (Some(cert), Some(key)) => {
            let config = nixfleet_control_plane::tls::build_server_config(
                std::path::Path::new(cert),
                std::path::Path::new(key),
                cli.client_ca.as_ref().map(std::path::Path::new),
            )?;
            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_config(std::sync::Arc::new(config));
            tracing::info!("Control plane listening on {} (TLS)", cli.listen);
            axum_server::bind_rustls(cli.listen.parse()?, tls_config)
                .serve(app.into_make_service())
                .await?;
        }
        (None, None) => {
            tracing::warn!("Running WITHOUT TLS — not recommended for production");
            let listener = tokio::net::TcpListener::bind(&cli.listen).await?;
            tracing::info!("Control plane listening on {}", cli.listen);
            axum::serve(listener, app).await?;
        }
        _ => anyhow::bail!("Both --tls-cert and --tls-key must be set together"),
    }
    Ok(())
}
