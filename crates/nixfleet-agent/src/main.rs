#![allow(clippy::doc_lazy_continuation)]
//! `nixfleet-agent` - main poll + activation loop.

mod dispatch;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use nixfleet_agent::{checkin_state, comms};
use nixfleet_proto::agent_wire::{CheckinRequest, ReportEvent};

use dispatch::{handle_cp_rollback_signal, process_dispatch_target};

const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "nixfleet-agent", version, about = "NixFleet fleet agent.")]
pub(crate) struct Args {
    #[arg(long, env = "NIXFLEET_AGENT_CP_URL")]
    pub(crate) control_plane_url: String,

    /// Must match the CN in the agent's client cert.
    #[arg(long, env = "NIXFLEET_AGENT_MACHINE_ID")]
    machine_id: String,

    #[arg(long, default_value_t = 60, env = "NIXFLEET_AGENT_POLL_INTERVAL")]
    poll_interval: u64,

    #[arg(long, env = "NIXFLEET_AGENT_TRUST_FILE")]
    trust_file: PathBuf,

    #[arg(long, env = "NIXFLEET_AGENT_CA_CERT")]
    ca_cert: Option<PathBuf>,

    #[arg(long, env = "NIXFLEET_AGENT_CLIENT_CERT")]
    client_cert: Option<PathBuf>,

    #[arg(long, env = "NIXFLEET_AGENT_CLIENT_KEY")]
    client_key: Option<PathBuf>,

    /// When `client_cert` is absent and this is set, agent enrolls via /v1/enroll.
    #[arg(long, env = "NIXFLEET_AGENT_BOOTSTRAP_TOKEN_FILE")]
    bootstrap_token_file: Option<PathBuf>,

    #[arg(
        long,
        env = "NIXFLEET_AGENT_STATE_DIR",
        default_value = "/var/lib/nixfleet-agent"
    )]
    state_dir: PathBuf,

    /// One of `"disabled"`, `"permissive"`, `"enforce"`, `"auto"`.
    /// CP-relayed channel mode wins when present.
    #[arg(long, env = "NIXFLEET_AGENT_COMPLIANCE_GATE_MODE")]
    compliance_gate_mode: Option<String>,

    /// Signs evidence payloads; absent file -> events post unsigned.
    #[arg(
        long,
        env = "NIXFLEET_AGENT_SSH_HOST_KEY_FILE",
        default_value = "/etc/ssh/ssh_host_ed25519_key"
    )]
    ssh_host_key_file: PathBuf,

    /// JSON config for declarative health probes. Absent ⇒ no scheduler,
    /// checkin omits `healthProbes`. Materialised by the NixOS module.
    #[arg(long, env = "NIXFLEET_AGENT_HEALTH_CHECKS_CONFIG")]
    health_checks_config: Option<PathBuf>,

    /// Fraction of cert validity remaining below which the agent
    /// self-renews. Default 0.5 (renew at half-life). Operators MAY
    /// raise (e.g. 0.8) for short-cycle hardware testing. Must be
    /// strictly between 0 and 1.
    #[arg(
        long,
        default_value_t = 0.5,
        env = "NIXFLEET_AGENT_RENEWAL_THRESHOLD_FRACTION"
    )]
    renewal_threshold_fraction: f64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let args = Args::parse();
    validate_renewal_threshold(args.renewal_threshold_fraction)?;
    let started_at = Instant::now();

    let evidence_signer = load_evidence_signer(&args.ssh_host_key_file);
    if let Err(err) = parse_trust_file(&args.trust_file) {
        report_pre_cert_failure(
            &args,
            nixfleet_proto::agent_wire::ReportEvent::TrustError {
                reason: format!("{err:#}"),
            },
        )
        .await;
        return Err(err);
    }
    if let Err(err) = maybe_run_first_boot_enrollment(&args).await {
        report_pre_cert_failure(
            &args,
            nixfleet_proto::agent_wire::ReportEvent::EnrollmentFailed {
                reason: format!("{err:#}"),
            },
        )
        .await;
        return Err(err);
    }

    let client = comms::build_client(
        args.ca_cert.as_deref(),
        args.client_cert.as_deref(),
        args.client_key.as_deref(),
    )?;

    let recovery_reporter = comms::ReqwestReporter::new(
        client.clone(),
        args.control_plane_url.clone(),
        args.machine_id.clone(),
        AGENT_VERSION,
    );
    if let Err(err) =
        check_boot_recovery(&client, &args, &recovery_reporter, &evidence_signer).await
    {
        tracing::warn!(
            error = %err,
            "boot-recovery path errored (non-fatal); main loop will re-converge",
        );
    }

    tracing::info!(
        machine_id = %args.machine_id,
        cp = %args.control_plane_url,
        interval_secs = args.poll_interval,
        "agent starting poll loop"
    );

    // Health-check config + probe scheduler. Absent config = no scheduler.
    // Parse failures are fatal - operator declared probes we can't honour.
    let health_cache = match args.health_checks_config.as_deref() {
        Some(path) => match nixfleet_agent::health::load_config(path) {
            Ok(Some(cfg)) => {
                let mode = cfg.mode;
                let cache = std::sync::Arc::new(nixfleet_agent::health::ProbeStateCache::new(
                    nixfleet_agent::health::initial_results(&cfg),
                    mode,
                ));
                tokio::spawn({
                    let cache = cache.clone();
                    async move { nixfleet_agent::health::run_scheduler(cfg, cache).await }
                });
                cache
            }
            Ok(None) => {
                tracing::info!(
                    path = %path.display(),
                    "health-checks config absent at declared path - proceeding without probe scheduler",
                );
                std::sync::Arc::new(nixfleet_agent::health::ProbeStateCache::default())
            }
            Err(err) => {
                tracing::error!(
                    path = %path.display(),
                    error = %err,
                    "failed to load health-checks config",
                );
                return Err(err);
            }
        },
        None => std::sync::Arc::new(nixfleet_agent::health::ProbeStateCache::default()),
    };

    run_poll_loop(client, &args, started_at, evidence_signer, health_cache).await
}

/// Strict (0,1) - boundary values would make the threshold meaningless
/// (0 = never renew, 1 = renew every poll).
fn validate_renewal_threshold(fraction: f64) -> anyhow::Result<()> {
    if !(0.0 < fraction && fraction < 1.0) {
        anyhow::bail!(
            "--renewal-threshold-fraction must be strictly between 0 and 1 (got {fraction})",
        );
    }
    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

/// Fail fast on misconfiguration; parsed value is otherwise unused.
fn parse_trust_file(path: &std::path::Path) -> anyhow::Result<()> {
    let trust_raw = std::fs::read_to_string(path)
        .with_context(|| format!("read trust file {}", path.display()))?;
    let _trust: nixfleet_proto::TrustConfig =
        serde_json::from_str(&trust_raw).context("parse trust file")?;
    Ok(())
}

/// Missing/unreadable key -> events post unsigned. Hard-fail only on corrupt key.
fn load_evidence_signer(
    path: &std::path::Path,
) -> std::sync::Arc<Option<nixfleet_agent::evidence_signer::EvidenceSigner>> {
    let signer = match nixfleet_agent::evidence_signer::EvidenceSigner::load(path) {
        Ok(Some(s)) => {
            tracing::info!(
                path = %path.display(),
                "loaded SSH host key - evidence signing active",
            );
            Some(s)
        }
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %format!("{err:#}"),
                "ssh host key parse error - evidence signing disabled",
            );
            None
        }
    };
    std::sync::Arc::new(signer)
}

/// Best-effort POST of a pre-cert failure event via bootstrap-token auth.
/// Agent is already exiting; this only leaves an operator-visible breadcrumb.
async fn report_pre_cert_failure(args: &Args, event: nixfleet_proto::agent_wire::ReportEvent) {
    let Some(token_file) = args.bootstrap_token_file.as_deref() else {
        return;
    };
    let bootstrap_client = match comms::build_client(args.ca_cert.as_deref(), None, None) {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(
                error = %format!("{err:#}"),
                "pre-cert failure event: build_client failed; skipping bootstrap-report",
            );
            return;
        }
    };
    if let Err(err) = nixfleet_agent::enrollment::post_bootstrap_event(
        &bootstrap_client,
        &args.control_plane_url,
        AGENT_VERSION,
        token_file,
        event,
    )
    .await
    {
        tracing::warn!(
            error = %format!("{err:#}"),
            "pre-cert failure event: bootstrap-report POST failed; CP has no signal of this failure",
        );
    } else {
        tracing::info!("pre-cert failure event: bootstrap-report posted");
    }
}

async fn maybe_run_first_boot_enrollment(args: &Args) -> anyhow::Result<()> {
    let (Some(cert_path), Some(key_path), Some(token_file)) = (
        args.client_cert.as_deref(),
        args.client_key.as_deref(),
        args.bootstrap_token_file.as_deref(),
    ) else {
        return Ok(());
    };
    if cert_path.exists() {
        return Ok(());
    }
    tracing::info!(token = %token_file.display(), "no client cert - starting enrollment");
    let enroll_client = comms::build_client(args.ca_cert.as_deref(), None, None)?;
    nixfleet_agent::enrollment::enroll(
        &enroll_client,
        &args.control_plane_url,
        &args.machine_id,
        token_file,
        cert_path,
        key_path,
    )
    .await
}

async fn run_poll_loop(
    client: reqwest::Client,
    args: &Args,
    started_at: Instant,
    evidence_signer: std::sync::Arc<Option<nixfleet_agent::evidence_signer::EvidenceSigner>>,
    health_cache: std::sync::Arc<nixfleet_agent::health::ProbeStateCache>,
) -> anyhow::Result<()> {
    let mut ticker = tokio::time::interval(Duration::from_secs(args.poll_interval));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut client_handle = client;
    let mut reporter = comms::ReqwestReporter::new(
        client_handle.clone(),
        args.control_plane_url.clone(),
        args.machine_id.clone(),
        AGENT_VERSION,
    );
    // Exponential backoff with ±20% jitter; doubles per failure, capped at 8×.
    let mut consecutive_failures: u32 = 0;

    loop {
        if consecutive_failures > 0 {
            sleep_with_backoff(consecutive_failures, args.poll_interval).await;
        }
        ticker.tick().await;

        // LOADBEARING: retry boot-recovery every tick - startup POST can race
        // a CP restart, and a missed confirm rolls back a healthy host.
        if let Err(err) =
            check_boot_recovery(&client_handle, args, &reporter, &evidence_signer).await
        {
            tracing::warn!(
                error = %err,
                "boot-recovery retry (poll loop): non-fatal error; main loop continues",
            );
        }

        if let Some(new_client) = maybe_renew_cert(&client_handle, &reporter, args).await {
            client_handle = new_client;
            reporter.replace_client(client_handle.clone());
        }

        match send_checkin(
            &client_handle,
            args,
            started_at,
            &evidence_signer,
            &health_cache,
        )
        .await
        {
            Ok(resp) => {
                consecutive_failures = 0;
                // LOADBEARING: rollback before new dispatch - host must step
                // away from the failed gen first.
                if let Some(rb) = &resp.rollback {
                    handle_cp_rollback_signal(rb, &reporter, args, &evidence_signer).await;
                }
                if let Some(target) = &resp.target {
                    process_dispatch_target(
                        target,
                        &reporter,
                        &client_handle,
                        args,
                        &evidence_signer,
                    )
                    .await;
                }
            }
            Err(err) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                // `{:#}` walks the anyhow chain; `%err` alone hides
                // TLS/connect cause below POST context.
                tracing::warn!(
                    error = %format!("{err:#}"),
                    consecutive_failures,
                    "checkin failed; will retry with backoff"
                );
            }
        }
    }
}

async fn sleep_with_backoff(consecutive_failures: u32, poll_interval: u64) {
    let multiplier = 1u64 << (consecutive_failures.min(3));
    let base = poll_interval.saturating_mul(multiplier);
    let jitter_pct: f64 = {
        use rand::Rng;
        rand::rng().random_range(-0.2_f64..=0.2_f64)
    };
    let jittered = (base as f64 * (1.0 + jitter_pct)) as u64;
    tracing::debug!(
        consecutive_failures,
        backoff_secs = jittered,
        "agent: backoff sleep"
    );
    tokio::time::sleep(Duration::from_secs(jittered)).await;
}

/// Self-paced renewal at 50% of cert validity; returns the rebuilt client on success.
async fn maybe_renew_cert(
    client: &reqwest::Client,
    reporter: &impl comms::Reporter,
    args: &Args,
) -> Option<reqwest::Client> {
    let (Some(cert_path), Some(key_path)) =
        (args.client_cert.as_deref(), args.client_key.as_deref())
    else {
        return None;
    };
    let (remaining, _) =
        nixfleet_agent::enrollment::cert_remaining_fraction(cert_path, chrono::Utc::now()).ok()?;
    if remaining >= args.renewal_threshold_fraction {
        return None;
    }
    tracing::info!(
        remaining,
        threshold = args.renewal_threshold_fraction,
        "cert past renewal threshold - renewing",
    );
    if let Err(err) = nixfleet_agent::enrollment::renew(
        client,
        &args.control_plane_url,
        &args.machine_id,
        cert_path,
        key_path,
    )
    .await
    {
        tracing::warn!(error = %err, "renew failed; retry next tick");
        reporter
            .post_report(
                None,
                ReportEvent::RenewalFailed {
                    reason: err.to_string(),
                },
            )
            .await;
        return None;
    }
    match comms::build_client(
        args.ca_cert.as_deref(),
        args.client_cert.as_deref(),
        args.client_key.as_deref(),
    ) {
        Ok(new) => Some(new),
        Err(err) => {
            tracing::error!(error = %err, "rebuild client after renew");
            None
        }
    }
}

async fn send_checkin(
    client: &reqwest::Client,
    args: &Args,
    started_at: Instant,
    evidence_signer: &std::sync::Arc<Option<nixfleet_agent::evidence_signer::EvidenceSigner>>,
    health_cache: &std::sync::Arc<nixfleet_agent::health::ProbeStateCache>,
) -> anyhow::Result<nixfleet_proto::agent_wire::CheckinResponse> {
    let current_generation = nixfleet_agent::host_facts::current_generation_ref()?;
    let pending_generation = nixfleet_agent::host_facts::pending_generation()?;
    let uptime_secs = checkin_state::uptime_secs(started_at);

    let last_confirmed_at = match checkin_state::read_last_confirmed(
        &args.state_dir,
        &current_generation.closure_hash,
        chrono::Utc::now(),
    ) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                error = %err,
                state_dir = %args.state_dir.display(),
                "read_last_confirmed failed; checkin proceeds without attestation",
            );
            None
        }
    };

    let last_evaluated_target = match checkin_state::read_last_target(&args.state_dir) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                error = %err,
                state_dir = %args.state_dir.display(),
                "read_last_target failed; checkin proceeds without last_evaluated_target",
            );
            None
        }
    };

    let last_fetch_outcome = match checkin_state::read_last_fetch_outcome(&args.state_dir) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                error = %err,
                state_dir = %args.state_dir.display(),
                "read_last_fetch_outcome failed; checkin proceeds without it",
            );
            None
        }
    };
    // Drop the outcome when no fetch is pending - meaningful only while a
    // target is in flight. Reporting a stale failure pins a red badge on
    // hosts that have recovered (e.g. a manual rebuild bypassed dispatch).
    let last_fetch_outcome = if pending_generation.is_some() {
        last_fetch_outcome
    } else {
        if let Err(err) = checkin_state::clear_last_fetch_outcome(&args.state_dir) {
            tracing::debug!(
                error = %err,
                "clear_last_fetch_outcome failed (non-fatal)",
            );
        }
        None
    };

    // Sign (hostname, rollout_id, last_confirmed_at) with the SSH host key.
    // CP verifies against hosts.<host>.pubkey before trusting the attested
    // timestamp for soak recovery; missing signature ⇒ timestamp ignored.
    // Rotation between sign and verify causes mismatch -> CP falls back to
    // unattested clamp (conservative).
    let attestation_signature = match (
        last_confirmed_at,
        last_evaluated_target.as_ref().map(|t| t.rollout_id.clone()),
        evidence_signer.as_ref().as_ref(),
    ) {
        (Some(lc), Some(rid), Some(signer)) => {
            let payload = nixfleet_proto::evidence_signing::LastConfirmedAtSignedPayload {
                hostname: args.machine_id.as_str(),
                rollout_id: rid.as_str(),
                last_confirmed_at: lc,
            };
            nixfleet_agent::evidence_signer::try_sign(signer, &payload)
        }
        _ => None,
    };

    let req = CheckinRequest {
        hostname: args.machine_id.clone(),
        agent_version: AGENT_VERSION.to_string(),
        current_generation,
        pending_generation,
        last_evaluated_target,
        last_fetch_outcome,
        uptime_secs: Some(uptime_secs),
        last_confirmed_at,
        attestation_signature,
        health_probes: health_cache.snapshot().await,
        health_check_mode: health_cache.mode(),
    };

    comms::checkin(client, &args.control_plane_url, &req).await
}

/// Closes the timing window where fire-and-forget activation self-kills the
/// agent mid-poll: matching dispatch + live closure ⇒ retroactive confirm.
async fn check_boot_recovery(
    client: &reqwest::Client,
    args: &Args,
    reporter: &comms::ReqwestReporter,
    evidence_signer: &std::sync::Arc<Option<nixfleet_agent::evidence_signer::EvidenceSigner>>,
) -> anyhow::Result<()> {
    let current = match checkin_state::current_closure_hash() {
        Ok(c) => Some(c),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "boot-recovery: cannot read /run/current-system; skipping recovery this boot",
            );
            None
        }
    };
    nixfleet_agent::recovery::run_boot_recovery(
        client,
        &args.state_dir,
        &args.control_plane_url,
        &args.machine_id,
        current,
        nixfleet_agent::recovery::GateInputs {
            reporter,
            evidence_signer,
            cli_default_mode: args.compliance_gate_mode.as_deref(),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renewal_threshold_accepts_strict_interior() {
        assert!(validate_renewal_threshold(0.5).is_ok());
        assert!(validate_renewal_threshold(0.1).is_ok());
        assert!(validate_renewal_threshold(0.99).is_ok());
    }

    #[test]
    fn renewal_threshold_rejects_boundaries_and_outside() {
        assert!(validate_renewal_threshold(0.0).is_err());
        assert!(validate_renewal_threshold(1.0).is_err());
        assert!(validate_renewal_threshold(-0.5).is_err());
        assert!(validate_renewal_threshold(1.5).is_err());
        assert!(validate_renewal_threshold(f64::NAN).is_err());
    }
}
