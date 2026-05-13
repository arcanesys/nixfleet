//! Operator-side bootstrap-token minter. Signs a `TokenClaims` block with the
//! org root key, derives the host fingerprint from either fleet.resolved or
//! a flag override.

use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
// Alias avoids clashing with `struct Args` below.
use clap::Args as ClapArgs;
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_proto::enroll_wire::{BootstrapToken, TokenClaims};
use rand::RngCore;

#[derive(ClapArgs, Debug)]
#[command(about = "Mint a bootstrap token for first-boot fleet enrollment.")]
pub struct Args {
    /// Must match the fleet.nix entry + CSR CN at enroll time.
    #[arg(long)]
    hostname: String,

    /// base64 SHA-256 of the CSR's pubkey; binds the token to the key.
    /// Mutually exclusive with `--fleet-resolved`. Flag-driven path is for
    /// dev/test; declarative fleets use `--fleet-resolved`.
    #[arg(long, conflicts_with = "fleet_resolved")]
    csr_pubkey_fingerprint: Option<String>,

    /// Path to signed `releases/fleet.resolved.json`. Derives the fingerprint
    /// from `hosts.<hostname>.pubkey` so the token is scoped to what the
    /// operator declared, no manual SHA-256 dance.
    #[arg(long)]
    fleet_resolved: Option<PathBuf>,

    /// Org root ed25519 private key: PKCS#8 PEM, 32 raw bytes, or hex.
    #[arg(long)]
    org_root_key: PathBuf,

    #[arg(long, default_value_t = 24)]
    validity_hours: u32,

    #[arg(long, default_value_t = 1)]
    version: u32,
}

pub fn run(args: Args) -> Result<()> {
    let signing_key = read_signing_key(&args.org_root_key)?;

    let fingerprint = match (&args.csr_pubkey_fingerprint, &args.fleet_resolved) {
        (Some(fp), None) => fp.clone(),
        (None, Some(fleet_path)) => fingerprint_from_fleet(fleet_path, &args.hostname)?,
        (None, None) => anyhow::bail!(
            "must pass --csr-pubkey-fingerprint OR --fleet-resolved (declarative path)",
        ),
        (Some(_), Some(_)) => unreachable!("clap's `conflicts_with` rejects this combo"),
    };

    let now = Utc::now();
    let claims = TokenClaims {
        hostname: args.hostname,
        expected_pubkey_fingerprint: fingerprint,
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
    eprintln!("expiresAt: {}", token.claims.expires_at.to_rfc3339());
    eprintln!();
    eprintln!("Add to fleet.nix `bootstrapNonces`, commit, and push:");
    eprintln!();
    eprintln!("  {{");
    eprintln!("    nonce = \"{}\";", token.claims.nonce);
    eprintln!("    hostname = \"{}\";", token.claims.hostname);
    eprintln!(
        "    expiresAt = \"{}\";",
        token.claims.expires_at.to_rfc3339()
    );
    eprintln!(
        "    mintedAt = \"{}\";",
        token.claims.issued_at.to_rfc3339()
    );
    if let Ok(user) = std::env::var("USER") {
        eprintln!("    mintedBy = \"{}\";", user);
    }
    eprintln!("  }}");
    eprintln!();
    eprintln!("Once CI signs the sidecar (~2 min), deploy the token bytes and");
    eprintln!("restart the agent.");
    Ok(())
}

fn read_signing_key(path: &PathBuf) -> Result<SigningKey> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read org root key {}", path.display()))?;
    // FOOTGUN: detect PEM before whitespace strip - strip would collapse
    // BEGIN/body/END lines.
    if let Ok(orig) = std::str::from_utf8(&bytes) {
        if orig.trim_start().starts_with("-----BEGIN") {
            let body: String = orig
                .lines()
                .filter(|l| !l.starts_with("-----"))
                .collect::<Vec<_>>()
                .join("");
            let der = base64::engine::general_purpose::STANDARD
                .decode(&body)
                .context("base64 decode PEM body")?;
            // PKCS#8 ed25519 OCTET STRING tail: 0x04 0x20 + 32 bytes.
            if der.len() < 34 {
                anyhow::bail!("PEM too short for PKCS#8 ed25519");
            }
            let arr: [u8; 32] = der[der.len() - 32..]
                .try_into()
                .map_err(|_| anyhow::anyhow!("PKCS#8 tail wrong size"))?;
            return Ok(SigningKey::from_bytes(&arr));
        }
    }

    let trimmed: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if trimmed.len() == 32 {
        let arr: [u8; 32] = trimmed[..32]
            .try_into()
            .expect("len 32 checked above");
        return Ok(SigningKey::from_bytes(&arr));
    }
    if let Ok(s) = std::str::from_utf8(&trimmed) {
        let s = s.trim_start_matches("0x").trim();
        if s.len() == 64 {
            let raw = hex::decode(s).context("hex decode org root key")?;
            let arr: [u8; 32] = raw[..32]
                .try_into()
                .expect("hex-64 decodes to 32 bytes");
            return Ok(SigningKey::from_bytes(&arr));
        }
    }
    anyhow::bail!("couldn't parse org root key - expected 32 raw bytes, hex, or PEM PKCS#8");
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use std::io::Write;

    fn pkcs8_pem_for_seed(seed: &[u8; 32]) -> String {
        let mut der = hex::decode("302e020100300506032b657004220420").unwrap();
        der.extend_from_slice(seed);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
        format!("-----BEGIN PRIVATE KEY-----\n{b64}\n-----END PRIVATE KEY-----\n")
    }

    #[test]
    fn read_signing_key_accepts_pkcs8_pem() {
        let seed = [0x42u8; 32];
        let pem = pkcs8_pem_for_seed(&seed);
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(pem.as_bytes()).unwrap();
        let key = read_signing_key(&tmp.path().to_path_buf()).expect("PEM should parse");
        assert_eq!(key.to_bytes(), seed);
    }

    #[test]
    fn read_signing_key_accepts_raw_32_bytes() {
        let seed = [0x55u8; 32];
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&seed).unwrap();
        let key = read_signing_key(&tmp.path().to_path_buf()).unwrap();
        assert_eq!(key.to_bytes(), seed);
    }

    #[test]
    fn read_signing_key_accepts_hex() {
        let seed = [0x77u8; 32];
        let hex = hex::encode(seed);
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(hex.as_bytes()).unwrap();
        let key = read_signing_key(&tmp.path().to_path_buf()).unwrap();
        assert_eq!(key.to_bytes(), seed);
    }
}

fn random_nonce() -> String {
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn fingerprint_from_fleet(fleet_path: &PathBuf, hostname: &str) -> Result<String> {
    let raw = std::fs::read_to_string(fleet_path)
        .with_context(|| format!("read fleet.resolved.json {}", fleet_path.display()))?;
    let fleet: nixfleet_proto::FleetResolved = serde_json::from_str(&raw)
        .with_context(|| format!("parse fleet.resolved.json {}", fleet_path.display()))?;
    let host = fleet.hosts.get(hostname).ok_or_else(|| {
        anyhow::anyhow!("host {hostname} not declared in {}", fleet_path.display())
    })?;
    let openssh = host.pubkey.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "host {hostname} has no `pubkey` declared in fleet.nix - set it before minting"
        )
    })?;
    nixfleet_proto::host_key::fingerprint_openssh_pubkey(openssh)
        .map_err(|err| anyhow::anyhow!("derive fingerprint from declared pubkey: {err}"))
}

#[cfg(test)]
mod fleet_resolved_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn fingerprint_from_fleet_matches_proto_helper() {
        let raw_pubkey = [0x42u8; 32];
        let mut blob = Vec::new();
        blob.extend_from_slice(&(b"ssh-ed25519".len() as u32).to_be_bytes());
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&(raw_pubkey.len() as u32).to_be_bytes());
        blob.extend_from_slice(&raw_pubkey);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        let openssh = format!("ssh-ed25519 {b64} test@host");

        let fleet_json = serde_json::json!({
            "schemaVersion": 1,
            "hosts": {
                "test-host": {
                    "system": "x86_64-linux",
                    "tags": [],
                    "channel": "stable",
                    "closureHash": null,
                    "pubkey": openssh,
                }
            },
            "channels": {
                "stable": {
                    "rolloutPolicy": "default",
                    "reconcileIntervalMinutes": 5,
                    "freshnessWindow": 60,
                    "signingIntervalMinutes": 30,
                    "compliance": { "frameworks": [], "mode": "disabled" },
                }
            },
            "rolloutPolicies": {
                "default": {
                    "strategy": "waves",
                    "waves": [],
                    "healthGate": {},
                    "onHealthFailure": "halt",
                }
            },
            "waves": {},
            "edges": [],
            "disruptionBudgets": [],
            "meta": {
                "schemaVersion": 1,
                "signedAt": null,
                "ciCommit": null,
                "signatureAlgorithm": null,
            }
        });
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(fleet_json.to_string().as_bytes()).unwrap();

        let got = fingerprint_from_fleet(&tmp.path().to_path_buf(), "test-host").unwrap();
        let expected = nixfleet_proto::host_key::fingerprint_openssh_pubkey(&openssh).unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn fingerprint_from_fleet_errors_when_host_missing() {
        let fleet_json = serde_json::json!({
            "schemaVersion": 1,
            "hosts": {},
            "channels": {},
            "rolloutPolicies": {},
            "waves": {},
            "edges": [],
            "disruptionBudgets": [],
            "meta": { "schemaVersion": 1, "signedAt": null, "ciCommit": null, "signatureAlgorithm": null }
        });
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(fleet_json.to_string().as_bytes()).unwrap();

        let err = fingerprint_from_fleet(&tmp.path().to_path_buf(), "test-host").unwrap_err();
        assert!(format!("{err:#}").contains("not declared"), "msg = {err:#}");
    }

    #[test]
    fn fingerprint_from_fleet_errors_when_pubkey_missing() {
        let fleet_json = serde_json::json!({
            "schemaVersion": 1,
            "hosts": {
                "test-host": {
                    "system": "x86_64-linux",
                    "tags": [],
                    "channel": "stable",
                    "closureHash": null,
                    "pubkey": null,
                }
            },
            "channels": {
                "stable": {
                    "rolloutPolicy": "default",
                    "reconcileIntervalMinutes": 5,
                    "freshnessWindow": 60,
                    "signingIntervalMinutes": 30,
                    "compliance": { "frameworks": [], "mode": "disabled" },
                }
            },
            "rolloutPolicies": {
                "default": {
                    "strategy": "waves",
                    "waves": [],
                    "healthGate": {},
                    "onHealthFailure": "halt",
                }
            },
            "waves": {},
            "edges": [],
            "disruptionBudgets": [],
            "meta": { "schemaVersion": 1, "signedAt": null, "ciCommit": null, "signatureAlgorithm": null }
        });
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(fleet_json.to_string().as_bytes()).unwrap();

        let err = fingerprint_from_fleet(&tmp.path().to_path_buf(), "test-host").unwrap_err();
        assert!(format!("{err:#}").contains("no `pubkey`"), "msg = {err:#}");
    }
}
