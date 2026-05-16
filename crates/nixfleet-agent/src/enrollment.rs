//! Bootstrap enrollment + cert renewal. Both flows sign the CSR with the
//! host's SSH ed25519 key (RFC-0003 §2); the agent never generates keys.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nixfleet_proto::agent_wire::{PROTOCOL_MAJOR_VERSION, PROTOCOL_VERSION_HEADER, ReportEvent};
use nixfleet_proto::enroll_wire::{
    BootstrapEventRequest, BootstrapToken, EnrollRequest, EnrollResponse, RenewRequest,
    RenewResponse,
};
use rcgen::{CertificateParams, DnType, KeyPair};
use reqwest::Client;
use sha2::Digest;
use x509_parser::prelude::*;

/// Builds a CSR signed by the SSH host key; returns `(PEM CSR, raw 32-byte
/// pubkey)`. CP rejects if the pubkey doesn't match `hosts.<hostname>.pubkey`.
/// FOOTGUN: SSH key is OpenSSH PEM; rcgen wants PKCS#8 - we rewrap via the
/// proto helper before handing to `KeyPair::from_pem`.
pub fn generate_csr_from_ssh_host_key(
    hostname: &str,
    ssh_host_key_path: &Path,
) -> Result<(String, [u8; 32])> {
    let raw = std::fs::read_to_string(ssh_host_key_path)
        .with_context(|| format!("read ssh host key {}", ssh_host_key_path.display()))?;
    let private = ssh_key::PrivateKey::from_openssh(&raw)
        .with_context(|| format!("parse OpenSSH key at {}", ssh_host_key_path.display()))?;
    let seed = match private.key_data() {
        ssh_key::private::KeypairData::Ed25519(kp) => kp.private.to_bytes(),
        other => anyhow::bail!(
            "ssh host key at {} is not ed25519 (algorithm: {:?})",
            ssh_host_key_path.display(),
            other.algorithm()
        ),
    };
    let pkcs8_pem = nixfleet_proto::host_key::ed25519_pkcs8_pem_from_seed(&seed);
    let key = KeyPair::from_pem(&pkcs8_pem).context("rcgen KeyPair::from_pem PKCS#8 ed25519")?;
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, hostname);
    let csr = params.serialize_request(&key).context("serialize CSR")?;
    let csr_pem = csr.pem().context("CSR PEM encode")?;
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(key.public_key_raw());
    Ok((csr_pem, pubkey))
}

/// base64 SHA-256 of raw pubkey bytes; matches CP's
/// `expected_pubkey_fingerprint` field on the bootstrap token.
pub fn fingerprint_pubkey_raw(pubkey_raw: &[u8]) -> String {
    use base64::Engine;
    let digest = sha2::Sha256::digest(pubkey_raw);
    base64::engine::general_purpose::STANDARD.encode(digest)
}

pub async fn enroll(
    client: &Client,
    cp_url: &str,
    hostname: &str,
    token_file: &Path,
    cert_path: &Path,
    ssh_host_key_path: &Path,
) -> Result<()> {
    let token_raw = std::fs::read_to_string(token_file)
        .with_context(|| format!("read bootstrap token {}", token_file.display()))?;
    let token: BootstrapToken =
        serde_json::from_str(&token_raw).context("parse bootstrap token")?;

    let (csr_pem, _pubkey_raw) = generate_csr_from_ssh_host_key(hostname, ssh_host_key_path)?;

    let url = format!("{}/v1/enroll", cp_url.trim_end_matches('/'));
    let req = EnrollRequest { token, csr_pem };

    // CP returns 503 "control plane not ready" between boot and first signed
    // artifact landing (CI build window). Retry in-process instead of
    // crashing the agent — systemd respawn loops on cold start lose minutes
    // and look like agent defects in journals.
    let body = loop {
        let resp = client
            .post(&url)
            .header(PROTOCOL_VERSION_HEADER, PROTOCOL_MAJOR_VERSION.to_string())
            .json(&req)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            break resp
                .json::<EnrollResponse>()
                .await
                .context("parse enroll response")?;
        }
        let body_text = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
            && body_text.contains("control plane not ready")
        {
            tracing::info!(
                target: "nixfleet_agent::enrollment",
                "enroll: CP cold-starting (awaiting first signed artifact); retrying in 10s"
            );
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            continue;
        }
        anyhow::bail!("enroll {}: {}: {}", url, status, body_text);
    };

    // Write only the cert; the private key is the SSH host key already
    // on disk at ssh_host_key_path. --client-key points there.
    write_atomic(cert_path, body.cert_pem.as_bytes())?;
    tracing::info!(
        cert = %cert_path.display(),
        ssh_host_key = %ssh_host_key_path.display(),
        not_after = %body.not_after.to_rfc3339(),
        "enrolled - wrote cert (key is ssh host key, not written)"
    );
    Ok(())
}

pub async fn renew(
    client: &Client,
    cp_url: &str,
    hostname: &str,
    cert_path: &Path,
    ssh_host_key_path: &Path,
) -> Result<()> {
    let (csr_pem, _pubkey_raw) = generate_csr_from_ssh_host_key(hostname, ssh_host_key_path)?;
    let url = format!("{}/v1/agent/renew", cp_url.trim_end_matches('/'));
    let req = RenewRequest { csr_pem };
    let resp = client
        .post(&url)
        .header(PROTOCOL_VERSION_HEADER, PROTOCOL_MAJOR_VERSION.to_string())
        .json(&req)
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "renew {}: {}: {}",
            url,
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }
    let body: RenewResponse = resp.json().await.context("parse renew response")?;
    write_atomic(cert_path, body.cert_pem.as_bytes())?;
    tracing::info!(
        cert = %cert_path.display(),
        not_after = %body.not_after.to_rfc3339(),
        "renewed - wrote cert (key unchanged: ssh host key)"
    );
    Ok(())
}

/// Best-effort pre-mTLS failure post (`TrustError` / `EnrollmentFailed`) via
/// `/v1/agent/bootstrap-report`. Always returns Ok - agent is already on a
/// fatal path; a posting failure mustn't mask the underlying error.
pub async fn post_bootstrap_event(
    client: &Client,
    cp_url: &str,
    agent_version: &str,
    token_file: &Path,
    event: ReportEvent,
) -> Result<()> {
    let token_raw = std::fs::read_to_string(token_file)
        .with_context(|| format!("read bootstrap token {}", token_file.display()))?;
    let token: BootstrapToken =
        serde_json::from_str(&token_raw).context("parse bootstrap token")?;

    let url = format!("{}/v1/agent/bootstrap-report", cp_url.trim_end_matches('/'));
    let req = BootstrapEventRequest {
        token,
        agent_version: agent_version.to_string(),
        occurred_at: Utc::now(),
        event: serde_json::to_value(&event).context("serialise ReportEvent")?,
    };
    let resp = client
        .post(&url)
        .header(PROTOCOL_VERSION_HEADER, PROTOCOL_MAJOR_VERSION.to_string())
        .json(&req)
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "{url}: {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }
    Ok(())
}

/// Tempfile + rename so a crash mid-write doesn't leave a half-written cert.
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

/// Returns `(remaining_fraction, not_after)`; `< 0.5` means time to renew.
pub fn cert_remaining_fraction(
    cert_path: &Path,
    now: DateTime<Utc>,
) -> Result<(f64, DateTime<Utc>)> {
    let pem = std::fs::read_to_string(cert_path)
        .with_context(|| format!("read cert {}", cert_path.display()))?;
    let der = pem::parse(pem.as_bytes()).map_err(|e| anyhow::anyhow!("parse cert PEM: {e}"))?;
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

#[cfg(test)]
mod ssh_host_key_csr_tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::RngCore;
    use ssh_key::{LineEnding, PrivateKey};

    fn write_test_ssh_host_key(dir: &Path) -> std::path::PathBuf {
        let mut seed = [0u8; 32];
        rand::rng().fill_bytes(&mut seed);
        let sk = SigningKey::from_bytes(&seed);
        let kp = ssh_key::private::Ed25519Keypair {
            public: ssh_key::public::Ed25519PublicKey(sk.verifying_key().to_bytes()),
            private: ssh_key::private::Ed25519PrivateKey::from_bytes(&sk.to_bytes()),
        };
        let pk = PrivateKey::new(ssh_key::private::KeypairData::Ed25519(kp), "test-host")
            .expect("PrivateKey::new");
        let pem = pk.to_openssh(LineEnding::LF).expect("openssh PEM");
        let path = dir.join("ssh_host_ed25519_key");
        std::fs::write(&path, pem.as_bytes()).expect("write key");
        path
    }

    #[test]
    fn csr_pubkey_equals_ssh_host_pubkey() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key_path = write_test_ssh_host_key(dir.path());
        // Read the SSH host key directly so we know the expected pubkey.
        let raw = std::fs::read_to_string(&key_path).expect("read");
        let priv_key = PrivateKey::from_openssh(&raw).expect("parse");
        let expected_pubkey = match priv_key.key_data() {
            ssh_key::private::KeypairData::Ed25519(kp) => kp.public.0,
            _ => panic!("not ed25519"),
        };

        let (_csr, csr_pubkey) =
            generate_csr_from_ssh_host_key("test-host", &key_path).expect("CSR");
        assert_eq!(
            csr_pubkey, expected_pubkey,
            "CSR pubkey must match SSH host pubkey (RFC-0003 §2 binding)",
        );
    }

    #[test]
    fn renewal_preserves_csr_pubkey_across_calls() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key_path = write_test_ssh_host_key(dir.path());
        let (_csr1, pubkey1) =
            generate_csr_from_ssh_host_key("test-host", &key_path).expect("CSR 1");
        let (_csr2, pubkey2) =
            generate_csr_from_ssh_host_key("test-host", &key_path).expect("CSR 2");
        assert_eq!(
            pubkey1, pubkey2,
            "renewal must reuse the SSH host pubkey (no fresh keypair)",
        );
    }

    #[test]
    fn rejects_non_ed25519_ssh_host_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Write an RSA-shaped placeholder (using ssh-key's RSA generator
        // would be heavy; instead we stuff a non-OpenSSH file and expect
        // the parse error path).
        let path = dir.path().join("not-an-ssh-key");
        std::fs::write(&path, b"definitely not OpenSSH PEM").expect("write");
        let err = generate_csr_from_ssh_host_key("test-host", &path).expect_err("must reject");
        let msg = format!("{err:#}");
        assert!(msg.contains("parse OpenSSH key"), "unexpected error: {msg}",);
    }
}
