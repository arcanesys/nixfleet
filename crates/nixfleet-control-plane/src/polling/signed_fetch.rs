//! Shared HTTP fetch + Bearer-token primitive; verification stays per-task.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nixfleet_proto::TrustedPubkey;

/// 15s timeout.
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("build signed-fetch client (rustls + 15s timeout)")
}

/// Re-read each call so trust.json rotation propagates without restart.
pub fn read_trust_config(path: &Path) -> Result<nixfleet_proto::TrustConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read trust file {}", path.display()))?;
    serde_json::from_str(&raw).context("parse trust file")
}

/// Re-read each call so trust.json rotation propagates without restart.
/// `now` lets the verifier accept successor keys during the rotation
/// overlap window (`now < retire_at`).
pub fn read_trust_roots(
    path: &Path,
    now: DateTime<Utc>,
) -> Result<(Vec<TrustedPubkey>, Option<DateTime<Utc>>)> {
    let trust = read_trust_config(path)?;
    Ok((
        trust.ci_release_key.active_keys_at(now),
        trust.ci_release_key.reject_before,
    ))
}

/// Re-read each poll so token rotation propagates; `None` skips auth.
pub fn read_token(path: Option<&Path>) -> Result<Option<String>> {
    match path {
        Some(p) => Ok(Some(
            std::fs::read_to_string(p)
                .with_context(|| format!("read token file {}", p.display()))?
                .trim()
                .to_string(),
        )),
        None => Ok(None),
    }
}

/// Non-2xx or network error -> `Err`; caller retains previous state.
pub async fn fetch_signed_pair(
    client: &reqwest::Client,
    artifact_url: &str,
    signature_url: &str,
    token: Option<&str>,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let artifact = fetch_url(client, artifact_url, token).await?;
    let signature = fetch_url(client, signature_url, token).await?;
    Ok((artifact, signature))
}

async fn fetch_url(client: &reqwest::Client, url: &str, token: Option<&str>) -> Result<Vec<u8>> {
    let mut req = client.get(url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await.with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{url}: {status}: {body}");
    }
    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("read body {url}"))?;
    Ok(bytes.to_vec())
}
