//! `nixfleet-mint-token` — operator-side bootstrap token minter.
//!
//! Phase 3 PR-5. Run once on the operator's workstation per new
//! fleet host (typically as part of declaring the host in
//! fleet.nix and committing an agenix-encrypted token).
//!
//! Usage:
//!
//! ```text
//! nixfleet-mint-token \
//!     --hostname krach \
//!     --csr-pubkey-fingerprint <sha256-base64-of-CSR-spki> \
//!     --org-root-key /path/to/org-root.ed25519.key \
//!     --validity-hours 24 \
//!     > bootstrap-token-krach.json
//! ```
//!
//! The agent's first-boot enrollment generates its own keypair
//! before posting the CSR; in practice the operator runs
//! `nixfleet-mint-token` AFTER the host has booted and produced
//! its CSR (typically captured by the deploy tooling). For an even
//! simpler workflow, omit `--csr-pubkey-fingerprint` and accept any
//! pubkey — but that weakens the binding (a leaked token can be
//! used with an attacker-controlled key). Default keeps the
//! fingerprint required.

use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use clap::Parser;
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_proto::enroll_wire::{BootstrapToken, TokenClaims};
use rand::RngCore;

#[derive(Parser, Debug)]
#[command(
    name = "nixfleet-mint-token",
    about = "Mint a bootstrap token for first-boot fleet enrollment."
)]
struct Args {
    /// Target hostname (must match the fleet.nix entry + the CSR's CN
    /// at enroll time).
    #[arg(long)]
    hostname: String,

    /// SHA-256 fingerprint of the CSR's pubkey, base64-encoded. Lets
    /// the CP refuse a leaked token used with the wrong key.
    #[arg(long)]
    csr_pubkey_fingerprint: String,

    /// Path to the org root ed25519 private key (PEM-encoded
    /// `BEGIN PRIVATE KEY` PKCS#8 raw 32 bytes, or hex). When the
    /// path doesn't exist, the tool errors — never silently mints
    /// an unsigned token.
    #[arg(long)]
    org_root_key: PathBuf,

    /// Token validity window in hours. Default 24h.
    #[arg(long, default_value_t = 24)]
    validity_hours: u32,

    /// Token schema version. Always 1 in PR-5.
    #[arg(long, default_value_t = 1)]
    version: u32,
}

fn read_signing_key(path: &PathBuf) -> Result<SigningKey> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("read org root key {}", path.display()))?;
    // Accept three formats:
    // - 32 raw bytes
    // - hex-encoded 64 chars (with optional 0x/whitespace)
    // - PEM PKCS#8 (BEGIN PRIVATE KEY ... END PRIVATE KEY)
    let trimmed: Vec<u8> = bytes.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    if trimmed.len() == 32 {
        let arr: [u8; 32] = trimmed[..32].try_into().unwrap();
        return Ok(SigningKey::from_bytes(&arr));
    }
    if let Ok(s) = std::str::from_utf8(&trimmed) {
        let s = s.trim_start_matches("0x").trim();
        if s.len() == 64 {
            let raw = hex::decode(s).context("hex decode org root key")?;
            let arr: [u8; 32] = raw[..32].try_into().unwrap();
            return Ok(SigningKey::from_bytes(&arr));
        }
        if s.starts_with("-----BEGIN") {
            // PKCS#8 PEM. Manual extraction since we don't pull
            // rustls-pemfile here. The 32-byte raw key is at the end
            // of the DER blob.
            let body: String = s
                .lines()
                .filter(|l| !l.starts_with("-----"))
                .collect::<Vec<_>>()
                .join("");
            let der = base64::engine::general_purpose::STANDARD
                .decode(&body)
                .context("base64 decode PEM body")?;
            // Heuristic: PKCS#8 ed25519 PrivateKey OCTET STRING is
            // the last 34 bytes (0x04 0x20 + 32 bytes).
            if der.len() < 34 {
                anyhow::bail!("PEM too short for PKCS#8 ed25519");
            }
            let arr: [u8; 32] = der[der.len() - 32..]
                .try_into()
                .map_err(|_| anyhow::anyhow!("PKCS#8 tail wrong size"))?;
            return Ok(SigningKey::from_bytes(&arr));
        }
    }
    anyhow::bail!(
        "couldn't parse org root key — expected 32 raw bytes, hex, or PEM PKCS#8"
    );
}

fn random_nonce() -> String {
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let signing_key = read_signing_key(&args.org_root_key)?;

    let now = Utc::now();
    let claims = TokenClaims {
        hostname: args.hostname,
        expected_pubkey_fingerprint: args.csr_pubkey_fingerprint,
        issued_at: now,
        expires_at: now + ChronoDuration::hours(args.validity_hours as i64),
        nonce: random_nonce(),
    };

    let claims_json = serde_json::to_string(&claims).context("serialize claims")?;
    let canonical =
        nixfleet_canonicalize::canonicalize(&claims_json).context("canonicalize claims")?;
    let signature = signing_key.sign(canonical.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

    let token = BootstrapToken {
        version: args.version,
        claims,
        signature: sig_b64,
    };

    let out = serde_json::to_string_pretty(&token)?;
    println!("{out}");
    eprintln!("nonce: {}", token.claims.nonce);
    Ok(())
}
