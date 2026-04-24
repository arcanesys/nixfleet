//! `nixfleet-control-plane` — Phase 2 reconciler runner CLI.
//!
//! One-shot: read fleet.resolved + signature + trust + observed, verify,
//! reconcile, print the action plan as JSON lines on stdout, exit.
//! Intended to run from a systemd timer on the M70q (lab) so every tick
//! lands as a journal entry the operator can grep / `jq` / alert on.
//!
//! Phase 3 will graft the agent endpoints (RFC-0003) into this binary
//! and replace the file-backed `observed.json` with a live SQLite-backed
//! projection. For Phase 2, observed.json is hand-written.
//!
//! Exit codes:
//! - 0 — verify ok, plan emitted (the plan may be empty — no drift).
//! - 1 — verify failed; one summary line emitted with the reason.
//! - 2 — input/IO/parse error before verify could run.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use chrono::Utc;
use clap::Parser;
use nixfleet_control_plane::{render_plan, tick, TickInputs, VerifyOutcome};

#[derive(Parser, Debug)]
#[command(
    name = "nixfleet-control-plane",
    version,
    about = "Phase 2 reconciler runner: verify fleet.resolved + reconcile against observed.json + emit plan."
)]
struct Args {
    /// Path to releases/fleet.resolved.json (the bytes CI signed).
    #[arg(long)]
    artifact: PathBuf,

    /// Path to releases/fleet.resolved.json.sig.
    #[arg(long)]
    signature: PathBuf,

    /// Path to trust.json (shape per docs/trust-root-flow.md §3.4).
    #[arg(long, default_value = "/etc/nixfleet/cp/trust.json")]
    trust_file: PathBuf,

    /// Path to a JSON file holding `Observed` state (channel refs,
    /// host states, active rollouts). Phase 2 hand-writes this; Phase
    /// 3 swaps to a SQLite projection.
    #[arg(long)]
    observed: PathBuf,

    /// Maximum age (seconds) of meta.signedAt relative to now.
    #[arg(long, default_value_t = 86400)]
    freshness_window_secs: u64,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let inputs = TickInputs {
        artifact_path: args.artifact,
        signature_path: args.signature,
        trust_path: args.trust_file,
        observed_path: args.observed,
        now: Utc::now(),
        freshness_window: Duration::from_secs(args.freshness_window_secs),
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
