#![allow(clippy::doc_lazy_continuation)]
//! `nixfleet-control-plane` CLI: `serve` or `tick`. Tick exit codes: 0=ok, 1=verify failed, 2=pre-verify error.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use chrono::Utc;
use clap::{Parser, Subcommand};
use nixfleet_control_plane::{TickInputs, VerifyOutcome, render_plan, server, tick};

#[derive(Parser, Debug)]
#[command(
    name = "nixfleet-control-plane",
    version,
    about = "NixFleet control plane: long-running TLS server + reconciler."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Boxed: ServeFlags is ~470 bytes.
    Serve(Box<ServeFlags>),
    Tick(TickFlags),
}

#[derive(Parser, Debug, Clone)]
struct ServeFlags {
    #[arg(long, default_value = "0.0.0.0:8080", env = "NIXFLEET_CP_LISTEN")]
    listen: String,

    #[arg(long, env = "NIXFLEET_CP_TLS_CERT")]
    tls_cert: PathBuf,

    #[arg(long, env = "NIXFLEET_CP_TLS_KEY")]
    tls_key: PathBuf,

    /// When set, requires verified mTLS.
    #[arg(long, env = "NIXFLEET_CP_CLIENT_CA")]
    client_ca: Option<PathBuf>,

    #[arg(long)]
    artifact: PathBuf,

    #[arg(long)]
    signature: PathBuf,

    #[arg(long, default_value = "/etc/nixfleet/cp/trust.json")]
    trust_file: PathBuf,

    /// Dev/test fallback; runtime prefers the in-memory projection from check-ins.
    #[arg(long)]
    observed: PathBuf,

    #[arg(long, default_value_t = 2592000)]
    freshness_window_secs: u64,

    /// Must exceed agent poll budget (~300s) plus slack to avoid CP-rollback / agent-poll races.
    #[arg(long, default_value_t = 360)]
    confirm_deadline_secs: i64,

    /// Raw bytes of canonical signed fleet.resolved.json; unset disables channel-refs polling.
    #[arg(long, env = "NIXFLEET_CP_CHANNEL_REFS_ARTIFACT_URL")]
    channel_refs_artifact_url: Option<String>,

    /// Matching signature URL; unset disables channel-refs polling.
    #[arg(long, env = "NIXFLEET_CP_CHANNEL_REFS_SIGNATURE_URL")]
    channel_refs_signature_url: Option<String>,

    /// Bearer token file, re-read each poll so rotation propagates without restart.
    #[arg(long, env = "NIXFLEET_CP_CHANNEL_REFS_TOKEN_FILE")]
    channel_refs_token_file: Option<PathBuf>,

    /// When unset, revocations polling is disabled.
    #[arg(long, env = "NIXFLEET_CP_REVOCATIONS_ARTIFACT_URL")]
    revocations_artifact_url: Option<String>,

    #[arg(long, env = "NIXFLEET_CP_REVOCATIONS_SIGNATURE_URL")]
    revocations_signature_url: Option<String>,

    /// Defaults to the channel-refs token when unset.
    #[arg(long, env = "NIXFLEET_CP_REVOCATIONS_TOKEN_FILE")]
    revocations_token_file: Option<PathBuf>,

    /// When unset, bootstrap-nonces polling is disabled. Strict-mode CP
    /// will refuse all enrolments without this artifact.
    #[arg(long, env = "NIXFLEET_CP_BOOTSTRAP_NONCES_ARTIFACT_URL")]
    bootstrap_nonces_artifact_url: Option<String>,

    #[arg(long, env = "NIXFLEET_CP_BOOTSTRAP_NONCES_SIGNATURE_URL")]
    bootstrap_nonces_signature_url: Option<String>,

    /// Defaults to the channel-refs token when unset (same trust class).
    #[arg(long, env = "NIXFLEET_CP_BOOTSTRAP_NONCES_TOKEN_FILE")]
    bootstrap_nonces_token_file: Option<PathBuf>,

    /// Unset causes /v1/enroll and /v1/agent/renew to return 500.
    #[arg(long, env = "NIXFLEET_CP_FLEET_CA_CERT")]
    fleet_ca_cert: Option<PathBuf>,

    #[arg(long, env = "NIXFLEET_CP_FLEET_CA_KEY")]
    fleet_ca_key: Option<PathBuf>,

    /// TPM-backed CA: 64-byte raw P-256 pubkey (`pubkey.raw` from the
    /// keyslot scope). Pair with `--tpm-ca-sign-wrapper`; TPM wins over
    /// `--fleet-ca-key` when both are set.
    #[arg(long, env = "NIXFLEET_CP_TPM_CA_PUBKEY_RAW")]
    tpm_ca_pubkey_raw: Option<PathBuf>,

    /// TPM-backed CA: `tpm-sign-<keyname>` wrapper from the keyslot scope.
    #[arg(long, env = "NIXFLEET_CP_TPM_CA_SIGN_WRAPPER")]
    tpm_ca_sign_wrapper: Option<PathBuf>,

    /// FQDN suffix appended to agent cert CNs (`agent-<machineId>.<suffix>`).
    /// Must match the issuance CA's `dNSName` constraint (D14).
    /// Required: no default. The CP refuses to start if unset, to prevent
    /// silent CN-mismatch rejections when the operator's CA suffix differs
    /// from a framework-guessed placeholder.
    #[arg(long, env = "NIXFLEET_CP_AGENT_CN_SUFFIX")]
    agent_cn_suffix: String,

    /// Validity for issued agent certs, in seconds. Default 30 days
    /// (2 592 000). Operators MAY shorten this to exercise renewal +
    /// revocation flows on real hardware. Floor: 60s; values below
    /// are refused at startup.
    #[arg(
        long,
        default_value_t = 2_592_000,
        env = "NIXFLEET_CP_AGENT_CERT_VALIDITY_SECS"
    )]
    agent_cert_validity_secs: u64,

    /// JSON-lines per issuance; best-effort writes.
    #[arg(
        long,
        default_value = "/var/lib/nixfleet-cp/issuance.log",
        env = "NIXFLEET_CP_AUDIT_LOG"
    )]
    audit_log: PathBuf,

    /// Unset means in-memory state only.
    #[arg(long, env = "NIXFLEET_CP_DB_PATH")]
    db_path: Option<PathBuf>,

    /// Any nix-cache-protocol server base URL; unset returns 501.
    #[arg(long, env = "NIXFLEET_CP_CLOSURE_UPSTREAM")]
    closure_upstream: Option<String>,

    /// Pre-signed rollout manifests dir; falls back to `--rollouts-source-*` templates, then 503.
    #[arg(long, env = "NIXFLEET_CP_ROLLOUTS_DIR")]
    rollouts_dir: Option<PathBuf>,

    /// FOOTGUN: must contain `{rolloutId}` substitution token; paired with
    /// signature template (both required, or both omitted).
    #[arg(long, env = "NIXFLEET_CP_ROLLOUTS_SOURCE_ARTIFACT_URL_TEMPLATE")]
    rollouts_source_artifact_url_template: Option<String>,

    /// Paired with artifact template - both required to enable HTTP fetch.
    #[arg(long, env = "NIXFLEET_CP_ROLLOUTS_SOURCE_SIGNATURE_URL_TEMPLATE")]
    rollouts_source_signature_url_template: Option<String>,

    /// Defaults to channel-refs token when unset.
    #[arg(long, env = "NIXFLEET_CP_ROLLOUTS_SOURCE_TOKEN_FILE")]
    rollouts_source_token_file: Option<PathBuf>,

    /// Refuse to start when any security-relevant flag is unset.
    #[arg(long, env = "NIXFLEET_CP_STRICT")]
    strict: bool,
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

    #[arg(long, default_value_t = 2592000)]
    freshness_window_secs: u64,
}

fn install_crypto_provider() {
    // Rustls 0.23 needs explicit provider selection when both ring and
    // aws-lc-rs are linked.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[tokio::main]
async fn main() -> ExitCode {
    // JSON logs feed Loki/Grafana panels keyed off `target`, `level`, and
    // `fields.{hostname,rollout}` - NO regex on free-text. Tracing targets
    // are part of the dashboard contract (dispatch, confirm, soak, converge,
    // promote, rollback, closure_proxy, report, checkin, channel_refs_poll);
    // renaming any silently breaks a panel.
    // Field convention: `hostname = %h` (never `host = ...`).
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    install_crypto_provider();

    match Args::parse().command {
        Command::Serve(flags) => match run_serve(*flags).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("serve: {err:#}");
                ExitCode::from(2)
            }
        },
        Command::Tick(flags) => run_tick(flags),
    }
}

/// Both flags Some -> build; both None -> None; mixed -> bail with `name` in the message.
fn paired_source<A, B, T, F>(
    name: &str,
    a: Option<A>,
    b: Option<B>,
    build: F,
) -> anyhow::Result<Option<T>>
where
    F: FnOnce(A, B) -> anyhow::Result<T>,
{
    match (a, b) {
        (Some(a), Some(b)) => Ok(Some(build(a, b)?)),
        (None, None) => Ok(None),
        _ => anyhow::bail!("{name}: both URL flags must be passed together (or both omitted)."),
    }
}

async fn run_serve(flags: ServeFlags) -> anyhow::Result<()> {
    let listen = flags
        .listen
        .parse()
        .map_err(|e| anyhow::anyhow!("--listen {}: {e}", flags.listen))?;

    if flags.agent_cert_validity_secs < 60 {
        anyhow::bail!(
            "--agent-cert-validity-secs must be >= 60 (got {})",
            flags.agent_cert_validity_secs
        );
    }

    let freshness_window = Duration::from_secs(flags.freshness_window_secs);

    let channel_refs = paired_source(
        "channel-refs poll",
        flags.channel_refs_artifact_url,
        flags.channel_refs_signature_url,
        |artifact_url, signature_url| {
            Ok(
                nixfleet_control_plane::polling::channel_refs_poll::ChannelRefsSource {
                    artifact_url,
                    signature_url,
                    token_file: flags.channel_refs_token_file.clone(),
                    trust_path: flags.trust_file.clone(),
                    freshness_window,
                },
            )
        },
    )?;

    let rollouts_source = paired_source(
        "rollouts source",
        flags.rollouts_source_artifact_url_template.clone(),
        flags.rollouts_source_signature_url_template.clone(),
        |artifact_tpl, signature_tpl| {
            nixfleet_control_plane::rollouts_source::RolloutsSource::new(
                artifact_tpl,
                signature_tpl,
                flags
                    .rollouts_source_token_file
                    .clone()
                    .or(flags.channel_refs_token_file.clone()),
            )
        },
    )?;

    let revocations = paired_source(
        "revocations poll",
        flags.revocations_artifact_url,
        flags.revocations_signature_url,
        |artifact_url, signature_url| {
            Ok(
                nixfleet_control_plane::polling::revocations_poll::RevocationsSource {
                    artifact_url,
                    signature_url,
                    token_file: flags
                        .revocations_token_file
                        .or(flags.channel_refs_token_file.clone()),
                    trust_path: flags.trust_file.clone(),
                    freshness_window,
                },
            )
        },
    )?;

    let bootstrap_nonces = paired_source(
        "bootstrap-nonces poll",
        flags.bootstrap_nonces_artifact_url,
        flags.bootstrap_nonces_signature_url,
        |artifact_url, signature_url| {
            Ok(
                nixfleet_control_plane::polling::bootstrap_nonces_poll::BootstrapNoncesSource {
                    artifact_url,
                    signature_url,
                    token_file: flags
                        .bootstrap_nonces_token_file
                        .clone()
                        .or(flags.channel_refs_token_file.clone()),
                    trust_path: flags.trust_file.clone(),
                    freshness_window,
                },
            )
        },
    )?;

    server::serve(server::ServeArgs {
        listen,
        tls_cert: flags.tls_cert,
        tls_key: flags.tls_key,
        client_ca: flags.client_ca,
        fleet_ca_cert: flags.fleet_ca_cert,
        fleet_ca_key: flags.fleet_ca_key,
        tpm_ca_pubkey_raw: flags.tpm_ca_pubkey_raw,
        tpm_ca_sign_wrapper: flags.tpm_ca_sign_wrapper,
        agent_cn_suffix: flags.agent_cn_suffix,
        agent_cert_validity: Duration::from_secs(flags.agent_cert_validity_secs),
        audit_log_path: Some(flags.audit_log),
        artifact_path: flags.artifact,
        signature_path: flags.signature,
        trust_path: flags.trust_file,
        observed_path: flags.observed,
        freshness_window: Duration::from_secs(flags.freshness_window_secs),
        confirm_deadline_secs: flags.confirm_deadline_secs,
        channel_refs,
        revocations,
        bootstrap_nonces,
        rollouts_source,
        db_path: flags.db_path,
        closure_upstream: flags.closure_upstream,
        rollouts_dir: flags.rollouts_dir,
        strict: flags.strict,
        mark_ready_at_startup: false,
        initial_nonces: None,
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
        VerifyOutcome::Ok(ok) => {
            tracing::info!(actions = ok.actions.len(), "tick ok");
            ExitCode::SUCCESS
        }
        VerifyOutcome::Failed { reason } => {
            tracing::warn!(%reason, "verify failed");
            ExitCode::from(1)
        }
    }
}
