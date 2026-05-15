//! TLS server config builder; mTLS layered via `WebPkiClientVerifier` when `client_ca_path` is set.

use anyhow::{Context, Result};
use rustls::ServerConfig;
use rustls::server::WebPkiClientVerifier;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::path::Path;
use std::sync::Arc;

/// LOADBEARING: `allow_unauthenticated()` is required because `/v1/enroll`
/// cannot present a client cert (it bootstraps the agent's identity). Per-
/// route middleware enforces auth - don't tighten the TLS layer to require
/// client certs without first carving out enroll.
pub fn build_server_config(
    cert_path: &Path,
    key_path: &Path,
    client_ca_path: Option<&Path>,
) -> Result<ServerConfig> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert_path)
        .with_context(|| format!("failed to open cert: {}", cert_path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to parse server certificates")?;

    let key = PrivateKeyDer::from_pem_file(key_path)
        .with_context(|| format!("failed to read private key: {}", key_path.display()))?;

    let builder = if let Some(ca_path) = client_ca_path {
        let mut root_store = rustls::RootCertStore::empty();
        for cert in CertificateDer::pem_file_iter(ca_path)
            .with_context(|| format!("failed to open CA: {}", ca_path.display()))?
        {
            root_store.add(cert.context("failed to parse CA cert")?)?;
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(root_store))
            .allow_unauthenticated()
            .build()
            .context("failed to build client verifier")?;
        ServerConfig::builder().with_client_cert_verifier(verifier)
    } else {
        ServerConfig::builder().with_no_client_auth()
    };

    builder
        .with_single_cert(certs, key)
        .context("failed to configure server TLS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_server_config_missing_cert_fails() {
        let result = build_server_config(
            Path::new("/nonexistent/cert.pem"),
            Path::new("/nonexistent/key.pem"),
            None,
        );
        assert!(result.is_err());
    }
}
