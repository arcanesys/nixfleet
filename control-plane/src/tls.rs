use anyhow::{Context, Result};
use rustls::server::WebPkiClientVerifier;
use rustls::ServerConfig;
use std::fs;
use std::io::BufReader;
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
    let cert_file = fs::File::open(cert_path)
        .with_context(|| format!("failed to open cert: {}", cert_path.display()))?;
    let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to parse server certificates")?;

    let key_file = fs::File::open(key_path)
        .with_context(|| format!("failed to open key: {}", key_path.display()))?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))
        .context("failed to read private key")?
        .context("no private key found in file")?;

    let builder = if let Some(ca_path) = client_ca_path {
        let ca_file = fs::File::open(ca_path)
            .with_context(|| format!("failed to open CA: {}", ca_path.display()))?;
        let mut root_store = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut BufReader::new(ca_file)) {
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
