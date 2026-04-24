//! `nixfleet-verify-artifact` — thin CLI wrapping
//! `nixfleet_reconciler::verify_artifact`.
//!
//! Harness scaffold per `docs/phase-2-entry-spec.md §6`. Exists purely so
//! the Phase 2 signed-roundtrip scenario can call `verify_artifact` from
//! a shell-friendly entry point before Stream C's v0.2 agent takes over
//! the same call site. Retire this crate once the agent inlines
//! `verify_artifact` internally.
//!
//! Exit codes (per spec §6):
//! - 0 — artifact verified
//! - 1 — verify error (stderr carries the `VerifyError` variant + detail)
//! - 2 — argument / I/O / parse error

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clap::Parser;
use nixfleet_proto::TrustConfig;
use nixfleet_reconciler::verify_artifact;

#[derive(Parser, Debug)]
#[command(
    name = "nixfleet-verify-artifact",
    about = "Verify a signed fleet.resolved artifact against a trust.json file.",
    version
)]
struct Args {
    /// Path to the canonical (JCS) signed artifact bytes.
    #[arg(long)]
    artifact: PathBuf,

    /// Path to the raw signature bytes (64 bytes for ed25519 / ecdsa-p256).
    #[arg(long)]
    signature: PathBuf,

    /// Path to the trust.json file (shape per docs/trust-root-flow.md §3.4).
    #[arg(long)]
    trust_file: PathBuf,

    /// Reference timestamp for the freshness check (RFC 3339).
    #[arg(long)]
    now: DateTime<Utc>,

    /// Maximum age (in seconds) of `meta.signedAt` relative to `--now`.
    #[arg(long)]
    freshness_window_secs: u64,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let artifact = match std::fs::read(&args.artifact) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("read artifact {}: {err}", args.artifact.display());
            return ExitCode::from(2);
        }
    };
    let signature = match std::fs::read(&args.signature) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("read signature {}: {err}", args.signature.display());
            return ExitCode::from(2);
        }
    };
    let trust_raw = match std::fs::read_to_string(&args.trust_file) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("read trust-file {}: {err}", args.trust_file.display());
            return ExitCode::from(2);
        }
    };

    let trust: TrustConfig = match serde_json::from_str(&trust_raw) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("parse trust-file {}: {err}", args.trust_file.display());
            return ExitCode::from(2);
        }
    };

    // trust.json §7.4 requires schemaVersion 1; refuse unknown versions
    // at the CLI boundary so operators see a clear arg-level failure
    // rather than a downstream verify error.
    if trust.schema_version != TrustConfig::CURRENT_SCHEMA_VERSION {
        eprintln!(
            "trust-file schemaVersion {} unsupported (accepted: {})",
            trust.schema_version,
            TrustConfig::CURRENT_SCHEMA_VERSION
        );
        return ExitCode::from(2);
    }

    let trusted_keys = trust.ci_release_key.active_keys();
    let reject_before = trust.ci_release_key.reject_before;
    let freshness_window = Duration::from_secs(args.freshness_window_secs);

    match verify_artifact(
        &artifact,
        &signature,
        &trusted_keys,
        args.now,
        freshness_window,
        reject_before,
    ) {
        Ok(fleet) => {
            println!(
                "schemaVersion={} hosts={}",
                fleet.schema_version,
                fleet.hosts.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}
