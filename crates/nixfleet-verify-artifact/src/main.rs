//! `nixfleet-verify-artifact` - offline verifier CLI.
//!
//! Exit codes: 0 verified, 1 verify error, 2 argument / I/O / parse error.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use nixfleet_proto::TrustConfig;
use nixfleet_reconciler::evidence::{verify_canonical_payload, SignatureStatus};
use nixfleet_reconciler::{verify_artifact, verify_rollout_manifest};

#[derive(Parser, Debug)]
#[command(name = "nixfleet-verify-artifact", version)]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Verify a signed fleet.resolved artifact against a trust.json.
    Artifact {
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        signature: PathBuf,
        #[arg(long)]
        trust_file: PathBuf,
        #[arg(long)]
        now: DateTime<Utc>,
        #[arg(long)]
        freshness_window_secs: u64,
    },
    /// Verify a signed manifest; recompute hash matches `--rollout-id`.
    RolloutManifest {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        signature: PathBuf,
        #[arg(long)]
        trust_file: PathBuf,
        #[arg(long)]
        now: DateTime<Utc>,
        #[arg(long)]
        freshness_window_secs: u64,
        /// Catches mix-and-match / rename attacks: filename ≠ content hash.
        #[arg(long)]
        rollout_id: String,
    },
    /// Verify a signed probe-output payload against a host's pubkey.
    Probe {
        /// JSON payload; will be JCS-canonicalized then verified.
        #[arg(long)]
        payload: PathBuf,
        /// File containing the base64 ed25519 signature.
        #[arg(long)]
        signature: PathBuf,
        /// File containing the host's OpenSSH-format `ssh-ed25519` pubkey.
        #[arg(long)]
        pubkey: PathBuf,
    },
}

fn main() -> ExitCode {
    match Args::parse().cmd {
        Cmd::Artifact {
            artifact,
            signature,
            trust_file,
            now,
            freshness_window_secs,
        } => run_artifact(artifact, signature, trust_file, now, freshness_window_secs),
        Cmd::RolloutManifest {
            manifest,
            signature,
            trust_file,
            now,
            freshness_window_secs,
            rollout_id,
        } => run_rollout_manifest(
            manifest,
            signature,
            trust_file,
            now,
            freshness_window_secs,
            rollout_id,
        ),
        Cmd::Probe {
            payload,
            signature,
            pubkey,
        } => run_probe(payload, signature, pubkey),
    }
}

fn run_rollout_manifest(
    manifest_path: PathBuf,
    signature_path: PathBuf,
    trust_file: PathBuf,
    now: DateTime<Utc>,
    freshness_window_secs: u64,
    expected_rollout_id: String,
) -> ExitCode {
    let manifest_bytes = match std::fs::read(&manifest_path) {
        Ok(v) => v,
        Err(err) => return arg_error(format!("read manifest {}: {err}", manifest_path.display())),
    };
    let signature_bytes = match std::fs::read(&signature_path) {
        Ok(v) => v,
        Err(err) => {
            return arg_error(format!(
                "read signature {}: {err}",
                signature_path.display()
            ))
        }
    };
    let trust_raw = match std::fs::read_to_string(&trust_file) {
        Ok(v) => v,
        Err(err) => return arg_error(format!("read trust-file {}: {err}", trust_file.display())),
    };
    let trust: TrustConfig = match serde_json::from_str(&trust_raw) {
        Ok(t) => t,
        Err(err) => return arg_error(format!("parse trust-file {}: {err}", trust_file.display())),
    };
    if trust.schema_version != TrustConfig::CURRENT_SCHEMA_VERSION {
        return arg_error(format!(
            "trust-file schemaVersion {} unsupported (accepted: {})",
            trust.schema_version,
            TrustConfig::CURRENT_SCHEMA_VERSION
        ));
    }

    let manifest = match verify_rollout_manifest(
        &manifest_bytes,
        &signature_bytes,
        &trust.ci_release_key.active_keys_at(now),
        now,
        Duration::from_secs(freshness_window_secs),
        trust.ci_release_key.reject_before,
    ) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };

    // LOADBEARING: hash the bytes the auditor was handed, NOT a re-serialised
    // parse. An auditor running an older nixfleet build (proto missing
    // fields the producer added) would otherwise compute a different hash
    // and reject perfectly-signed manifests. CONTRACTS §V Pattern A's
    // additive-evolution guarantee depends on this property.
    let recomputed = match nixfleet_reconciler::rollout_id_from_bytes(&manifest_bytes) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("rollout_id_from_bytes failed: {err}");
            return ExitCode::from(1);
        }
    };
    if recomputed != expected_rollout_id {
        eprintln!("rolloutId mismatch: expected {expected_rollout_id}, recomputed {recomputed}");
        return ExitCode::from(1);
    }

    println!(
        "schemaVersion={} channel={} hostSet={} fleetResolvedHash={} rolloutId={}",
        manifest.schema_version,
        manifest.channel,
        manifest.host_set.len(),
        manifest.fleet_resolved_hash,
        recomputed,
    );
    ExitCode::SUCCESS
}

fn run_artifact(
    artifact: PathBuf,
    signature: PathBuf,
    trust_file: PathBuf,
    now: DateTime<Utc>,
    freshness_window_secs: u64,
) -> ExitCode {
    let artifact_bytes = match std::fs::read(&artifact) {
        Ok(v) => v,
        Err(err) => return arg_error(format!("read artifact {}: {err}", artifact.display())),
    };
    let signature_bytes = match std::fs::read(&signature) {
        Ok(v) => v,
        Err(err) => return arg_error(format!("read signature {}: {err}", signature.display())),
    };
    let trust_raw = match std::fs::read_to_string(&trust_file) {
        Ok(v) => v,
        Err(err) => return arg_error(format!("read trust-file {}: {err}", trust_file.display())),
    };
    let trust: TrustConfig = match serde_json::from_str(&trust_raw) {
        Ok(t) => t,
        Err(err) => return arg_error(format!("parse trust-file {}: {err}", trust_file.display())),
    };
    if trust.schema_version != TrustConfig::CURRENT_SCHEMA_VERSION {
        return arg_error(format!(
            "trust-file schemaVersion {} unsupported (accepted: {})",
            trust.schema_version,
            TrustConfig::CURRENT_SCHEMA_VERSION
        ));
    }

    match verify_artifact(
        &artifact_bytes,
        &signature_bytes,
        &trust.ci_release_key.active_keys_at(now),
        now,
        Duration::from_secs(freshness_window_secs),
        trust.ci_release_key.reject_before,
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

fn run_probe(payload: PathBuf, signature: PathBuf, pubkey: PathBuf) -> ExitCode {
    let payload_raw = match std::fs::read_to_string(&payload) {
        Ok(v) => v,
        Err(err) => return arg_error(format!("read payload {}: {err}", payload.display())),
    };
    let payload_value: serde_json::Value = match serde_json::from_str(&payload_raw) {
        Ok(v) => v,
        Err(err) => return arg_error(format!("parse payload {}: {err}", payload.display())),
    };
    let canonical = match serde_jcs::to_vec(&payload_value) {
        Ok(v) => v,
        Err(err) => return arg_error(format!("canonicalize payload: {err}")),
    };
    let sig_b64 = match std::fs::read_to_string(&signature) {
        Ok(v) => v.trim().to_string(),
        Err(err) => return arg_error(format!("read signature {}: {err}", signature.display())),
    };
    let pubkey_str = match std::fs::read_to_string(&pubkey) {
        Ok(v) => v.trim().to_string(),
        Err(err) => return arg_error(format!("read pubkey {}: {err}", pubkey.display())),
    };

    let status = verify_canonical_payload(&canonical, Some(&pubkey_str), Some(&sig_b64));
    println!(
        "{}",
        serde_json::to_string(&status).expect("SignatureStatus serialize")
    );
    match status {
        SignatureStatus::Verified => ExitCode::SUCCESS,
        _ => ExitCode::from(1),
    }
}

fn arg_error(msg: String) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(2)
}
