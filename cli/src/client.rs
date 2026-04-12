use anyhow::{Context, Result};
use std::fs;

/// Read the body of a failed HTTP response as a String, falling back to
/// a descriptive placeholder (not an empty string) if the read itself
/// fails. Centralizing this so callers can't accidentally print a blank
/// error body.
pub async fn read_error_body(resp: reqwest::Response) -> String {
    resp.text()
        .await
        .unwrap_or_else(|e| format!("<failed to read response body: {e}>"))
}

/// TLS configuration for the HTTP client.
pub struct TlsConfig<'a> {
    pub client_cert: &'a str,
    pub client_key: &'a str,
    pub ca_cert: &'a str,
}

/// Build a reqwest client with optional mTLS and Bearer auth.
pub fn build_client(tls: &TlsConfig, api_key: &str) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30));

    // mTLS client identity
    if !tls.client_cert.is_empty() && !tls.client_key.is_empty() {
        let cert_pem = fs::read(tls.client_cert)
            .with_context(|| format!("failed to read client cert: {}", tls.client_cert))?;
        let key_pem = fs::read(tls.client_key)
            .with_context(|| format!("failed to read client key: {}", tls.client_key))?;
        // reqwest Identity requires cert + key concatenated in PEM format
        let mut combined = cert_pem;
        combined.extend_from_slice(&key_pem);
        let identity = reqwest::Identity::from_pem(&combined)
            .context("failed to parse client certificate/key")?;
        builder = builder.identity(identity);
    }

    // Custom CA certificate
    if !tls.ca_cert.is_empty() {
        let ca_pem = fs::read(tls.ca_cert)
            .with_context(|| format!("failed to read CA cert: {}", tls.ca_cert))?;
        let ca_cert =
            reqwest::Certificate::from_pem(&ca_pem).context("failed to parse CA certificate")?;
        builder = builder.add_root_certificate(ca_cert);
    }

    // Default auth header
    if !api_key.is_empty() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", api_key))
                .context("invalid API key value")?,
        );
        builder = builder.default_headers(headers);
    }

    builder.build().context("failed to build HTTP client")
}
