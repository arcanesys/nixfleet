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
    // Rustls 0.23 requires an explicit process-level CryptoProvider
    // when more than one crypto backend is compiled into the binary.
    // Our direct `rustls = "0.23"` dependency pulls in `aws-lc-rs`
    // (its default feature) while `reqwest` with `rustls-tls` pulls
    // in `ring`. Without this call, the first `ServerConfig::builder()`
    // call in `tls::build_server_config` panics with
    // "Could not automatically determine the process-level
    // CryptoProvider from Rustls crate features".
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("install default rustls CryptoProvider");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nixfleet=info".into()),
        )
        .init();

    let metrics_handle = Arc::new(nixfleet_control_plane::metrics::init());

    let cli = Cli::parse();

    let fleet_state = Arc::new(RwLock::new(state::FleetState::new()));
    let db = Arc::new(db::Db::new(&cli.db_path)?);
    db.migrate()?;

    // Hydrate in-memory state from DB on startup
    state::hydrate_from_db(&fleet_state, &db).await?;

    // Spawn rollout executor background task
    let _executor =
        nixfleet_control_plane::rollout::executor::spawn(fleet_state.clone(), db.clone());

    // Spawn health report cleanup task (hourly)
    let cleanup_db = db.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            match cleanup_db.cleanup_old_health_reports(24) {
                Ok(deleted) => {
                    if deleted > 0 {
                        tracing::info!(deleted, "Cleaned up old health reports");
                    }
                }
                Err(e) => tracing::warn!("Health report cleanup failed: {e}"),
            }
        }
    });

    let app = build_app(fleet_state, db, metrics_handle);

    match (&cli.tls_cert, &cli.tls_key) {
        (Some(cert), Some(key)) => {
            if cli.client_ca.is_none() {
                tracing::warn!(
                    "Running TLS WITHOUT client CA — agent endpoints are not mTLS-protected. \
                     Set --client-ca for production to authenticate fleet agents."
                );
            }
            let config = nixfleet_control_plane::tls::build_server_config(
                std::path::Path::new(cert),
                std::path::Path::new(key),
                cli.client_ca.as_ref().map(std::path::Path::new),
            )?;
            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_config(std::sync::Arc::new(config));

            // Wrap the rustls acceptor so peer certs are extracted
            // after the handshake and injected into request extensions
            // via PeerCertificates. The cn_matches_path_machine_id
            // middleware (wired in lib.rs::build_app on agent routes
            // only) reads the extension and enforces CN-vs-path-id.
            let rustls_acceptor = axum_server::tls_rustls::RustlsAcceptor::new(tls_config);
            let mtls_acceptor =
                nixfleet_control_plane::auth_cn::MtlsAcceptor::new(rustls_acceptor);

            tracing::info!("Control plane listening on {} (TLS+mTLS-CN)", cli.listen);
            axum_server::bind(cli.listen.parse()?)
                .acceptor(mtls_acceptor)
                .serve(app.into_make_service())
                .await?;
        }
        (None, None) => {
            tracing::warn!(
                "Running WITHOUT TLS — agent endpoints are unprotected. \
                 Not recommended for production."
            );
            let listener = tokio::net::TcpListener::bind(&cli.listen).await?;
            tracing::info!("Control plane listening on {}", cli.listen);
            axum::serve(listener, app).await?;
        }
        _ => anyhow::bail!("Both --tls-cert and --tls-key must be set together"),
    }
    Ok(())
}
