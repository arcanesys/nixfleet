//! HTTP client wiring for talking to the control plane.
//!
//! Builds an mTLS `reqwest::Client` from the operator-supplied PEM
//! paths. Provides typed `checkin` and `report` calls that round-
//! trip the wire types defined in `nixfleet_proto::agent_wire`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use nixfleet_proto::agent_wire::{
    CheckinRequest, CheckinResponse, ReportRequest, ReportResponse,
};
use reqwest::{Certificate, Client, Identity};

/// Connect timeout. Generous because lab is often on Tailscale and
/// the first connect after a sleep can be slow. The poll cadence
/// itself is 60s, so even ~10s connects don't compound badly.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-request timeout (handshake + full request lifecycle).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Construct an mTLS-enabled HTTP client. CA cert pins the CP's
/// fleet CA; the client identity is the agent's per-host cert +
/// key. PR-1's TLS-only mode is supported (caller passes None for
/// `client_cert` and `client_key`); PR-3 onwards always wires both.
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
        let mut pem = std::fs::read(cert)
            .with_context(|| format!("read client cert {}", cert.display()))?;
        let key_pem = std::fs::read(key)
            .with_context(|| format!("read client key {}", key.display()))?;
        pem.extend_from_slice(&key_pem);
        let identity = Identity::from_pem(&pem).context("parse client identity PEM")?;
        builder = builder.identity(identity);
    }

    builder.build().context("build reqwest client")
}

/// POST /v1/agent/checkin. Returns the typed response for the agent
/// to consume — Phase 3 always sees `target: None` and a 60s
/// `next_checkin_secs`.
pub async fn checkin(
    client: &Client,
    cp_url: &str,
    req: &CheckinRequest,
) -> Result<CheckinResponse> {
    let url = format!("{}/v1/agent/checkin", cp_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(req)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{url}: {status}: {body}");
    }
    Ok(resp.json::<CheckinResponse>().await.context("parse checkin response")?)
}

/// POST /v1/agent/report. Used for out-of-band failure events
/// (verify-failed, fetch-failed, trust-error). Phase 3 doesn't have
/// a fetch path yet, but the function lands here so PR-4's poll
/// loop can call it directly.
pub async fn report(
    client: &Client,
    cp_url: &str,
    req: &ReportRequest,
) -> Result<ReportResponse> {
    let url = format!("{}/v1/agent/report", cp_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(req)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{url}: {status}: {body}");
    }
    Ok(resp.json::<ReportResponse>().await.context("parse report response")?)
}
