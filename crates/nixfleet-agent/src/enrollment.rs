//! Bootstrap enrollment + cert renewal client (Phase 3 PR-5).
//!
//! - On first boot, when the agent's `--client-cert` / `--client-key`
//!   files don't exist, it reads `--bootstrap-token-file`, generates
//!   a fresh keypair + CSR, POSTs `/v1/enroll`, writes the issued
//!   cert + private key atomically.
//! - During the regular poll loop, when the existing cert has < 50%
//!   remaining validity, the agent generates a fresh keypair + CSR,
//!   POSTs `/v1/agent/renew` over the current valid mTLS, writes
//!   the new cert + key atomically.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nixfleet_proto::enroll_wire::{
    BootstrapToken, EnrollRequest, EnrollResponse, RenewRequest, RenewResponse,
};
use rcgen::{CertificateParams, DnType, KeyPair};
use reqwest::Client;
use x509_parser::prelude::*;

/// Generate a fresh keypair + a CSR with `CN=hostname`. Returns the
/// (PEM CSR, PEM key, raw pubkey bytes for fingerprinting).
pub fn generate_csr(hostname: &str) -> Result<(String, String, Vec<u8>)> {
    let key = KeyPair::generate().context("generate agent keypair")?;
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, hostname);
    let csr = params.serialize_request(&key).context("serialize CSR")?;
    Ok((csr.pem().context("CSR PEM encode")?, key.serialize_pem(), key.public_key_der()))
}

/// SHA-256 fingerprint (base64) of pubkey DER bytes — matches the CP's
/// `expected_pubkey_fingerprint` shape in the bootstrap token.
pub fn fingerprint_pubkey_der(pubkey_der: &[u8]) -> String {
    use base64::Engine;
    let digest = sha2::Sha256::digest(pubkey_der);
    base64::engine::general_purpose::STANDARD.encode(digest)
}
use sha2::Digest;

/// First-boot enrollment. Reads token file, generates CSR, POSTs
/// `/v1/enroll`, writes the cert + key atomically to the configured
/// paths.
pub async fn enroll(
    client: &Client,
    cp_url: &str,
    hostname: &str,
    token_file: &Path,
    cert_path: &Path,
    key_path: &Path,
) -> Result<()> {
    let token_raw = std::fs::read_to_string(token_file)
        .with_context(|| format!("read bootstrap token {}", token_file.display()))?;
    let token: BootstrapToken =
        serde_json::from_str(&token_raw).context("parse bootstrap token")?;

    let (csr_pem, key_pem, _pubkey_der) = generate_csr(hostname)?;

    let url = format!("{}/v1/enroll", cp_url.trim_end_matches('/'));
    let req = EnrollRequest { token, csr_pem };
    let resp = client.post(&url).json(&req).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("enroll {}: {}: {}", url, resp.status(), resp.text().await.unwrap_or_default());
    }
    let body: EnrollResponse = resp.json().await.context("parse enroll response")?;

    write_atomic(cert_path, body.cert_pem.as_bytes())?;
    write_atomic(key_path, key_pem.as_bytes())?;
    tracing::info!(
        cert = %cert_path.display(),
        not_after = %body.not_after.to_rfc3339(),
        "enrolled — wrote cert + key"
    );
    Ok(())
}

/// Renew the existing cert. Generates a fresh keypair + CSR, POSTs
/// `/v1/agent/renew` over the current authenticated mTLS connection
/// (caller wires the existing client identity into `client`), writes
/// the new cert + key atomically.
pub async fn renew(
    client: &Client,
    cp_url: &str,
    hostname: &str,
    cert_path: &Path,
    key_path: &Path,
) -> Result<()> {
    let (csr_pem, key_pem, _pubkey_der) = generate_csr(hostname)?;
    let url = format!("{}/v1/agent/renew", cp_url.trim_end_matches('/'));
    let req = RenewRequest { csr_pem };
    let resp = client.post(&url).json(&req).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("renew {}: {}: {}", url, resp.status(), resp.text().await.unwrap_or_default());
    }
    let body: RenewResponse = resp.json().await.context("parse renew response")?;
    write_atomic(cert_path, body.cert_pem.as_bytes())?;
    write_atomic(key_path, key_pem.as_bytes())?;
    tracing::info!(
        cert = %cert_path.display(),
        not_after = %body.not_after.to_rfc3339(),
        "renewed — wrote cert + key"
    );
    Ok(())
}

/// Atomic write: write to a sibling tempfile then rename, so a crash
/// mid-write doesn't leave a half-written cert at the canonical path.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    let tmp = parent.join(format!(
        ".{}-tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "out".to_string())
    ));
    std::fs::write(&tmp, contents).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Read an existing cert PEM and decide whether it needs renewal.
/// Returns `(remaining_fraction, not_after)` where
/// `remaining_fraction < 0.5` means time to renew.
pub fn cert_remaining_fraction(cert_path: &Path, now: DateTime<Utc>) -> Result<(f64, DateTime<Utc>)> {
    let pem = std::fs::read_to_string(cert_path)
        .with_context(|| format!("read cert {}", cert_path.display()))?;
    let der = pem::parse(pem.as_bytes())
        .map_err(|e| anyhow::anyhow!("parse cert PEM: {e}"))?;
    let (_, cert) = X509Certificate::from_der(der.contents())
        .map_err(|e| anyhow::anyhow!("parse cert DER: {e}"))?;
    let not_before = cert.validity().not_before.timestamp();
    let not_after = cert.validity().not_after.timestamp();
    let total = (not_after - not_before).max(1) as f64;
    let elapsed = (now.timestamp() - not_before).max(0) as f64;
    let remaining = (total - elapsed).max(0.0) / total;
    let na_dt = DateTime::<Utc>::from_timestamp(not_after, 0)
        .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(1));
    Ok((remaining, na_dt))
}

/// Lightweight PEM parser fallback so we don't pull a full pem crate.
mod pem {
    use anyhow::{Context, Result};
    pub struct Parsed {
        contents: Vec<u8>,
    }
    impl Parsed {
        pub fn contents(&self) -> &[u8] {
            &self.contents
        }
    }
    pub fn parse(input: &[u8]) -> Result<Parsed> {
        use base64::Engine;
        let s = std::str::from_utf8(input).context("PEM not UTF-8")?;
        let body: String = s
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect::<Vec<_>>()
            .join("");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body)
            .context("PEM base64 decode")?;
        Ok(Parsed { contents: bytes })
    }
}
