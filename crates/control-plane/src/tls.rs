use anyhow::{Context, Result};
use rustls::server::WebPkiClientVerifier;
use rustls::ServerConfig;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::path::Path;
use std::sync::Arc;

/// Build a rustls ServerConfig with optional mTLS.
///
/// - cert_path: Server certificate PEM file.
/// - key_path: Server private key PEM file.
/// - client_ca_path: If set, require client certificates signed by this CA (mTLS).
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
    fn test_build_server_config_missing_cert_fails() {
        let result = build_server_config(
            Path::new("/nonexistent/cert.pem"),
            Path::new("/nonexistent/key.pem"),
            None,
        );
        assert!(result.is_err());
    }
}
