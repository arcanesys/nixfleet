//! `nixfleet-control-plane` — CLI shell.
//!
//! Two subcommands:
//!
//! * `serve` (default) — long-running TLS server. axum + tokio +
//!   axum-server. Internal 30s reconcile loop. Phase 3 PR-1 ships
//!   `GET /healthz`; PR-2+ light up the agent endpoints.
//!
//! * `tick` — Phase 2's oneshot behaviour: read inputs, verify,
//!   reconcile, print plan, exit. Preserved for tests + ad-hoc
//!   operator runs (handy for diffing what the loop is doing
//!   without tailing journald).
//!
//! Exit codes for `tick` (preserved from Phase 2):
//! - 0 — verify ok, plan emitted (the plan may be empty — no drift).
//! - 1 — verify failed; one summary line emitted with the reason.
//! - 2 — input/IO/parse error before verify could run.
//!
//! `serve` runs until interrupted; exit code 0 on graceful shutdown,
//! non-zero if startup (cert load, port bind) fails.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use chrono::Utc;
use clap::{Parser, Subcommand};
use nixfleet_control_plane::{render_plan, server, tick, TickInputs, VerifyOutcome};

#[derive(Parser, Debug)]
#[command(
    name = "nixfleet-control-plane",
    version,
    about = "NixFleet control plane (Phase 3): long-running TLS server + reconciler."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Long-running TLS server with internal reconcile loop. The
    /// natural operator default — `nixfleet-control-plane serve`.
    Serve(ServeFlags),
    /// One-shot tick: read inputs, verify, reconcile, print, exit.
    /// Preserves Phase 2's CLI contract for tests + ad-hoc operator
    /// runs (handy for diffing what the loop is doing without
    /// tailing journald).
    Tick(TickFlags),
}

#[derive(Parser, Debug, Clone)]
struct ServeFlags {
    /// Address to listen on (HOST:PORT).
    #[arg(long, default_value = "0.0.0.0:8080", env = "NIXFLEET_CP_LISTEN")]
    listen: String,

    /// TLS server certificate PEM file.
    #[arg(long, env = "NIXFLEET_CP_TLS_CERT")]
    tls_cert: PathBuf,

    /// TLS server private key PEM file.
    #[arg(long, env = "NIXFLEET_CP_TLS_KEY")]
    tls_key: PathBuf,

    /// Client CA PEM file. When set, server requires verified client
    /// certs (mTLS). PR-1 leaves this optional; PR-2 onwards sets it
    /// as part of the standard deploy.
    #[arg(long, env = "NIXFLEET_CP_CLIENT_CA")]
    client_ca: Option<PathBuf>,

    /// Path to releases/fleet.resolved.json (the bytes CI signed).
    #[arg(long)]
    artifact: PathBuf,

    /// Path to releases/fleet.resolved.json.sig.
    #[arg(long)]
    signature: PathBuf,

    /// Path to trust.json (shape per docs/trust-root-flow.md §3.4).
    #[arg(long, default_value = "/etc/nixfleet/cp/trust.json")]
    trust_file: PathBuf,

    /// Path to observed state JSON (shape per
    /// `nixfleet_reconciler::Observed`). PR-4 swaps this default
    /// path for an in-memory projection from agent check-ins; the
    /// flag stays as a dev/test fallback.
    #[arg(long)]
    observed: PathBuf,

    /// Maximum age (seconds) of meta.signedAt relative to now.
    #[arg(long, default_value_t = 86400)]
    freshness_window_secs: u64,

    // PR-4: Forgejo channel-refs poll. All four flags must be set
    // together; if any are missing the poll task is not spawned and
    // the CP falls back to reading channel-refs from the file-backed
    // observed.json.
    /// Forgejo base URL (e.g. https://git.lab.internal). When unset,
    /// channel-refs polling is disabled.
    #[arg(long, env = "NIXFLEET_CP_FORGEJO_URL")]
    forgejo_base_url: Option<String>,

    /// Forgejo repo owner for the fleet repo (e.g. abstracts33d).
    #[arg(long, env = "NIXFLEET_CP_FORGEJO_OWNER")]
    forgejo_owner: Option<String>,

    /// Forgejo repo name (e.g. fleet).
    #[arg(long, default_value = "fleet", env = "NIXFLEET_CP_FORGEJO_REPO")]
    forgejo_repo: String,

    /// Path inside the repo to fleet.resolved.json.
    #[arg(
        long,
        default_value = "releases/fleet.resolved.json",
        env = "NIXFLEET_CP_FORGEJO_ARTIFACT_PATH"
    )]
    forgejo_artifact_path: String,

    /// Path to the Forgejo API token file (agenix-mounted, read-only).
    #[arg(long, env = "NIXFLEET_CP_FORGEJO_TOKEN_FILE")]
    forgejo_token_file: Option<PathBuf>,

    // PR-5: cert issuance (enroll + renew). The CP holds the fleet
    // CA private key online — see issue #41 for the deferred TPM-
    // bound replacement. When these are unset, /v1/enroll and
    // /v1/agent/renew return 500.
    /// Fleet CA cert path (read on each issuance for the chain).
    #[arg(long, env = "NIXFLEET_CP_FLEET_CA_CERT")]
    fleet_ca_cert: Option<PathBuf>,

    /// Fleet CA private key path (used to sign agent certs).
    #[arg(long, env = "NIXFLEET_CP_FLEET_CA_KEY")]
    fleet_ca_key: Option<PathBuf>,

    /// Audit log path. JSON-lines, one record per issuance (enroll
    /// or renew). Best-effort writes; failure logs a warn but
    /// doesn't fail the issuance.
    #[arg(long, default_value = "/var/lib/nixfleet-cp/issuance.log",
          env = "NIXFLEET_CP_AUDIT_LOG")]
    audit_log: PathBuf,
}

#[derive(Parser, Debug, Clone)]
struct TickFlags {
    #[arg(long)]
    artifact: PathBuf,

    #[arg(long)]
    signature: PathBuf,

    #[arg(long, default_value = "/etc/nixfleet/cp/trust.json")]
    trust_file: PathBuf,

    #[arg(long)]
    observed: PathBuf,

    #[arg(long, default_value_t = 86400)]
    freshness_window_secs: u64,
}

fn install_crypto_provider() {
    // Rustls 0.23 requires an explicit process-level CryptoProvider
    // when more than one crypto backend is compiled into the binary.
    // Our direct `rustls = "0.23"` dependency pulls in `aws-lc-rs`
    // (its default feature) while `reqwest` (dev-dep) with
    // `rustls-tls` pulls in `ring`. Without this call, the first
    // `ServerConfig::builder()` in `tls::build_server_config` panics
    // with "Could not automatically determine the process-level
    // CryptoProvider from Rustls crate features".
    //
    // `install_default` returns `Err` if a provider is already set
    // (e.g. test harness already installed one). Idempotent for our
    // purposes — the important thing is that *some* aws_lc_rs
    // provider is registered before we build a `ServerConfig`.
    //
    // Provenance: workaround discovered in v0.1's CP main.rs (tag
    // v0.1.1). Documented at length there to spare the next person
    // a wasted afternoon — comment ported here.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    install_crypto_provider();

    match Args::parse().command {
        Command::Serve(flags) => match run_serve(flags).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("serve: {err:#}");
                ExitCode::from(2)
            }
        },
        Command::Tick(flags) => run_tick(flags),
    }
}

async fn run_serve(flags: ServeFlags) -> anyhow::Result<()> {
    let listen = flags
        .listen
        .parse()
        .map_err(|e| anyhow::anyhow!("--listen {}: {e}", flags.listen))?;

    // Forgejo poll config: all-or-nothing. Either all the inputs
    // line up, or the poll is disabled. Partial config is rejected
    // with a clear error rather than silently falling back to file
    // mode.
    let forgejo = match (
        flags.forgejo_base_url,
        flags.forgejo_owner,
        flags.forgejo_token_file,
    ) {
        (Some(base_url), Some(owner), Some(token_file)) => {
            Some(nixfleet_control_plane::forgejo_poll::ForgejoConfig {
                base_url,
                owner,
                repo: flags.forgejo_repo,
                artifact_path: flags.forgejo_artifact_path,
                token_file,
            })
        }
        (None, None, None) => None,
        _ => {
            anyhow::bail!(
                "Forgejo poll config is all-or-nothing — pass --forgejo-base-url, \
                 --forgejo-owner, and --forgejo-token-file together (or none)."
            );
        }
    };

    server::serve(server::ServeArgs {
        listen,
        tls_cert: flags.tls_cert,
        tls_key: flags.tls_key,
        client_ca: flags.client_ca,
        fleet_ca_cert: flags.fleet_ca_cert,
        fleet_ca_key: flags.fleet_ca_key,
        audit_log_path: Some(flags.audit_log),
        artifact_path: flags.artifact,
        signature_path: flags.signature,
        trust_path: flags.trust_file,
        observed_path: flags.observed,
        freshness_window: Duration::from_secs(flags.freshness_window_secs),
        forgejo,
    })
    .await
}

fn run_tick(flags: TickFlags) -> ExitCode {
    let inputs = TickInputs {
        artifact_path: flags.artifact,
        signature_path: flags.signature,
        trust_path: flags.trust_file,
        observed_path: flags.observed,
        now: Utc::now(),
        freshness_window: Duration::from_secs(flags.freshness_window_secs),
    };

    let result = match tick(&inputs) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("tick: {err:#}");
            return ExitCode::from(2);
        }
    };

    print!("{}", render_plan(&result));

    match &result.verify {
        VerifyOutcome::Ok { actions, .. } => {
            tracing::info!(actions = actions.len(), "tick ok");
            ExitCode::SUCCESS
        }
        VerifyOutcome::Failed { reason } => {
            tracing::warn!(%reason, "verify failed");
            ExitCode::from(1)
        }
    }
}
