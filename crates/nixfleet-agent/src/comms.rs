//! mTLS HTTP client to the control plane: typed checkin/confirm/report calls.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use nixfleet_proto::agent_wire::{
    CheckinRequest, CheckinResponse, ConfirmRequest, ReportEvent, ReportRequest, ReportResponse,
    PROTOCOL_MAJOR_VERSION, PROTOCOL_VERSION_HEADER,
};
use reqwest::{Certificate, Client, Identity, StatusCode};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// TLS-only mode (None cert/key) supported but production always wires both.
pub fn build_client(
    ca_cert: Option<&Path>,
    client_cert: Option<&Path>,
    client_key: Option<&Path>,
) -> Result<Client> {
    let mut builder = Client::builder()
        .use_rustls_tls()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT);

    if let Some(ca_path) = ca_cert {
        let pem = std::fs::read(ca_path)
            .with_context(|| format!("read CA cert {}", ca_path.display()))?;
        let cert = Certificate::from_pem(&pem).context("parse CA cert PEM")?;
        builder = builder.add_root_certificate(cert);
    }

    if let (Some(cert), Some(key)) = (client_cert, client_key) {
        let mut pem =
            std::fs::read(cert).with_context(|| format!("read client cert {}", cert.display()))?;
        let key_pem = read_client_key_as_pem(key)
            .with_context(|| format!("read client key {}", key.display()))?;
        pem.extend_from_slice(key_pem.as_bytes());
        let identity = Identity::from_pem(&pem).context("parse client identity PEM")?;
        builder = builder.identity(identity);
    }

    builder.build().context("build reqwest client")
}

/// Return PEM bytes that `reqwest::Identity::from_pem` accepts. FOOTGUN:
/// the agent's client key is the host SSH key, which is OpenSSH format -
/// neither reqwest nor rustls parses it. We extract the 32-byte ed25519
/// seed and re-emit as PKCS#8 PEM. PEM inputs pass through unchanged.
fn read_client_key_as_pem(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if raw.contains("-----BEGIN OPENSSH PRIVATE KEY-----") {
        let private = ssh_key::PrivateKey::from_openssh(&raw)
            .with_context(|| format!("parse OpenSSH key at {}", path.display()))?;
        let seed = match private.key_data() {
            ssh_key::private::KeypairData::Ed25519(kp) => kp.private.to_bytes(),
            other => anyhow::bail!(
                "ssh host key at {} is not ed25519 (algorithm: {:?})",
                path.display(),
                other.algorithm(),
            ),
        };
        Ok(nixfleet_proto::host_key::ed25519_pkcs8_pem_from_seed(&seed))
    } else {
        Ok(raw)
    }
}

pub async fn checkin(
    client: &Client,
    cp_url: &str,
    req: &CheckinRequest,
) -> Result<CheckinResponse> {
    let url = format!("{}/v1/agent/checkin", cp_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header(PROTOCOL_VERSION_HEADER, PROTOCOL_MAJOR_VERSION.to_string())
        .json(req)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{url}: {status}: {body}");
    }
    resp.json::<CheckinResponse>()
        .await
        .context("parse checkin response")
}

/// 204 -> Acknowledged; 410 -> Cancelled (agent must rollback); else Other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    Acknowledged,
    Cancelled,
    Other,
}

/// `endpoint` is wire-carried `target.activate.confirm_endpoint` - required,
/// not optional. Agents refuse to confirm without an activate block.
pub async fn confirm(
    client: &Client,
    cp_url: &str,
    endpoint: &str,
    req: &ConfirmRequest,
) -> Result<ConfirmOutcome> {
    let url = format!(
        "{}{}",
        cp_url.trim_end_matches('/'),
        if endpoint.starts_with('/') {
            endpoint.to_string()
        } else {
            format!("/{endpoint}")
        }
    );
    let resp = client
        .post(&url)
        .header(PROTOCOL_VERSION_HEADER, PROTOCOL_MAJOR_VERSION.to_string())
        .json(req)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let outcome = match resp.status() {
        StatusCode::NO_CONTENT => ConfirmOutcome::Acknowledged,
        StatusCode::GONE => ConfirmOutcome::Cancelled,
        other => {
            tracing::warn!(
                status = other.as_u16(),
                "confirm: unexpected status - treating as 'other'"
            );
            ConfirmOutcome::Other
        }
    };
    Ok(outcome)
}

pub async fn report(client: &Client, cp_url: &str, req: &ReportRequest) -> Result<ReportResponse> {
    let url = format!("{}/v1/agent/report", cp_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header(PROTOCOL_VERSION_HEADER, PROTOCOL_MAJOR_VERSION.to_string())
        .json(req)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{url}: {status}: {body}");
    }
    resp.json::<ReportResponse>()
        .await
        .context("parse report response")
}

/// Best-effort by contract: telemetry must never crash the activation loop.
pub trait Reporter: Send + Sync {
    fn post_report(
        &self,
        rollout: Option<&str>,
        event: ReportEvent,
    ) -> impl std::future::Future<Output = ()> + Send;
}

pub struct ReqwestReporter {
    client: Client,
    cp_url: String,
    hostname: String,
    agent_version: String,
}

impl ReqwestReporter {
    pub fn new(
        client: Client,
        cp_url: impl Into<String>,
        hostname: impl Into<String>,
        agent_version: impl Into<String>,
    ) -> Self {
        Self {
            client,
            cp_url: cp_url.into(),
            hostname: hostname.into(),
            agent_version: agent_version.into(),
        }
    }

    pub fn replace_client(&mut self, client: Client) {
        self.client = client;
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn cp_url(&self) -> &str {
        &self.cp_url
    }
}

impl Reporter for ReqwestReporter {
    async fn post_report(&self, rollout: Option<&str>, event: ReportEvent) {
        let req = ReportRequest {
            hostname: self.hostname.clone(),
            agent_version: self.agent_version.clone(),
            occurred_at: chrono::Utc::now(),
            rollout: rollout.map(String::from),
            event,
        };
        if let Err(err) = report(&self.client, &self.cp_url, &req).await {
            tracing::warn!(
                error = %err,
                hostname = %self.hostname,
                "report post failed; event is in local journal only",
            );
        }
    }
}

#[cfg(test)]
mod read_client_key_tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::RngCore;
    use ssh_key::{LineEnding, PrivateKey};

    fn write_test_ssh_host_key(dir: &std::path::Path) -> std::path::PathBuf {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
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
    fn openssh_input_converts_to_pkcs8() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_test_ssh_host_key(dir.path());
        let pem = read_client_key_as_pem(&path).expect("convert");
        assert!(
            pem.starts_with("-----BEGIN PRIVATE KEY-----"),
            "expected PKCS#8 PEM, got: {}",
            pem.lines().next().unwrap_or(""),
        );
        assert!(pem.contains("-----END PRIVATE KEY-----"));
    }

    #[test]
    fn pem_input_passes_through() {
        // Legacy PEM keys must be returned unchanged so reqwest sees the
        // exact bytes the operator deployed.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("agent.key");
        let pem_input = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n";
        std::fs::write(&path, pem_input).expect("write");
        let got = read_client_key_as_pem(&path).expect("read");
        assert_eq!(got, pem_input);
    }

    /// Pubkey round-trip: protects against accidental seed swap during
    /// OpenSSH -> PKCS#8 conversion.
    #[test]
    fn openssh_to_pkcs8_pubkey_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_test_ssh_host_key(dir.path());

        let raw = std::fs::read_to_string(&path).expect("read");
        let priv_key = PrivateKey::from_openssh(&raw).expect("parse");
        let expected_pubkey = match priv_key.key_data() {
            ssh_key::private::KeypairData::Ed25519(kp) => kp.public.0,
            _ => panic!("not ed25519"),
        };

        let pkcs8_pem = read_client_key_as_pem(&path).expect("convert");
        let key = rcgen::KeyPair::from_pem(&pkcs8_pem).expect("rcgen parse");
        let mut got = [0u8; 32];
        got.copy_from_slice(key.public_key_raw());

        assert_eq!(got, expected_pubkey);
    }
}
