//! `nixfleet-agent` — Phase 3 PR-3 poll loop.
//!
//! Real main loop. Reads cert paths + CP URL from CLI flags, builds
//! an mTLS reqwest client, polls `/v1/agent/checkin` every
//! `pollInterval` seconds with a richer body than RFC-0003 §4.1's
//! minimum (pending generation, last-fetch outcome, agent uptime).
//! No activation — the response's `target` is logged but never
//! acted on (Phase 4 wires that).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use nixfleet_agent::{checkin_state, comms};
use nixfleet_proto::agent_wire::CheckinRequest;

const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(
    name = "nixfleet-agent",
    version,
    about = "NixFleet v0.2 fleet agent (poll-only, Phase 3 PR-3)."
)]
struct Args {
    /// Control plane URL (e.g. https://lab:8080). Trailing slash
    /// optional.
    #[arg(long, env = "NIXFLEET_AGENT_CP_URL")]
    control_plane_url: String,

    /// This host's identifier — must match the CN in the agent
    /// client cert. Defaults to the system hostname when set by
    /// the NixOS module.
    #[arg(long, env = "NIXFLEET_AGENT_MACHINE_ID")]
    machine_id: String,

    /// Seconds between checkins. Default 60s, matching RFC-0003 §2
    /// and the CP's response `nextCheckinSecs`.
    #[arg(long, default_value_t = 60, env = "NIXFLEET_AGENT_POLL_INTERVAL")]
    poll_interval: u64,

    /// Path to trust.json. Read on startup; agent restarts on
    /// rebuild to pick up changes (docs/trust-root-flow.md §7.1).
    #[arg(long, env = "NIXFLEET_AGENT_TRUST_FILE")]
    trust_file: PathBuf,

    /// CA cert PEM for verifying the CP's TLS cert.
    #[arg(long, env = "NIXFLEET_AGENT_CA_CERT")]
    ca_cert: Option<PathBuf>,

    /// Client cert PEM (the agent's identity to the CP).
    #[arg(long, env = "NIXFLEET_AGENT_CLIENT_CERT")]
    client_cert: Option<PathBuf>,

    /// Client private key PEM paired with `client_cert`.
    #[arg(long, env = "NIXFLEET_AGENT_CLIENT_KEY")]
    client_key: Option<PathBuf>,

    /// Bootstrap token file. When `client_cert` doesn't exist on
    /// startup AND this is set, the agent enters first-boot
    /// enrollment: reads token, generates CSR, POSTs /v1/enroll,
    /// writes the issued cert + key to `client_cert` / `client_key`.
    #[arg(long, env = "NIXFLEET_AGENT_BOOTSTRAP_TOKEN_FILE")]
    bootstrap_token_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let started_at = Instant::now();

    // The trust file is parsed on startup just to fail fast if
    // misconfigured. The agent doesn't currently consume the trust
    // root for any in-process verification (PR-4 introduces the
    // direct-fetch fallback path that uses verify_artifact); for now
    // it's a contract-shape check.
    let trust_raw = std::fs::read_to_string(&args.trust_file).with_context(|| {
        format!("read trust file {}", args.trust_file.display())
    })?;
    let _trust: nixfleet_proto::TrustConfig =
        serde_json::from_str(&trust_raw).context("parse trust file")?;

    // PR-5: first-boot enrollment. When the agent starts and finds
    // no client cert at the configured path, AND a bootstrap token
    // is available, run /v1/enroll and write the issued cert + key
    // before continuing to the poll loop.
    if let (Some(cert_path), Some(key_path), Some(token_file)) = (
        args.client_cert.as_deref(),
        args.client_key.as_deref(),
        args.bootstrap_token_file.as_deref(),
    ) {
        if !cert_path.exists() {
            tracing::info!(token = %token_file.display(), "no client cert — starting enrollment");
            let enroll_client = comms::build_client(args.ca_cert.as_deref(), None, None)?;
            nixfleet_agent::enrollment::enroll(
                &enroll_client,
                &args.control_plane_url,
                &args.machine_id,
                token_file,
                cert_path,
                key_path,
            )
            .await?;
        }
    }

    let client = comms::build_client(
        args.ca_cert.as_deref(),
        args.client_cert.as_deref(),
        args.client_key.as_deref(),
    )?;

    tracing::info!(
        machine_id = %args.machine_id,
        cp = %args.control_plane_url,
        interval_secs = args.poll_interval,
        "agent starting poll loop"
    );

    let mut ticker = tokio::time::interval(Duration::from_secs(args.poll_interval));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut client_handle = client;

    loop {
        ticker.tick().await;

        // PR-5: self-paced renewal at 50% of cert validity. Each
        // tick checks the cert; if past 50%, generate a fresh CSR
        // and POST /v1/agent/renew via the current authenticated
        // client. Failure is non-fatal — next tick retries.
        if let (Some(cert_path), Some(key_path)) =
            (args.client_cert.as_deref(), args.client_key.as_deref())
        {
            if let Ok((remaining, _)) =
                nixfleet_agent::enrollment::cert_remaining_fraction(cert_path, chrono::Utc::now())
            {
                if remaining < 0.5 {
                    tracing::info!(remaining, "cert past 50% — renewing");
                    if let Err(err) = nixfleet_agent::enrollment::renew(
                        &client_handle,
                        &args.control_plane_url,
                        &args.machine_id,
                        cert_path,
                        key_path,
                    )
                    .await
                    {
                        tracing::warn!(error = %err, "renew failed; retry next tick");
                    } else {
                        // Rebuild the client with the new cert + key.
                        match comms::build_client(
                            args.ca_cert.as_deref(),
                            args.client_cert.as_deref(),
                            args.client_key.as_deref(),
                        ) {
                            Ok(new) => client_handle = new,
                            Err(err) => {
                                tracing::error!(error = %err, "rebuild client after renew");
                            }
                        }
                    }
                }
            }
        }

        match send_checkin(&client_handle, &args, started_at).await {
            Ok(resp) => {
                if let Some(target) = &resp.target {
                    // Phase 3 doesn't activate, but log what the CP
                    // wants so an operator can sanity-check the
                    // dispatch loop when Phase 4 turns it on.
                    tracing::info!(
                        target_closure = %target.closure_hash,
                        target_channel = %target.channel_ref,
                        "would activate (Phase 4 will run nixos-rebuild here)"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "checkin failed; will retry next tick");
            }
        }
    }
}

async fn send_checkin(
    client: &reqwest::Client,
    args: &Args,
    started_at: Instant,
) -> anyhow::Result<nixfleet_proto::agent_wire::CheckinResponse> {
    let current_generation = checkin_state::current_generation_ref()?;
    let pending_generation = checkin_state::pending_generation()?;
    let uptime_secs = checkin_state::uptime_secs(started_at);

    let req = CheckinRequest {
        hostname: args.machine_id.clone(),
        agent_version: AGENT_VERSION.to_string(),
        current_generation,
        pending_generation,
        last_evaluated_target: None,
        last_fetch_outcome: None,
        uptime_secs: Some(uptime_secs),
    };

    comms::checkin(client, &args.control_plane_url, &req).await
}
