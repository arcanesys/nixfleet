//! TLS server config builder.
//!
//! Ported from v0.1's `crates/control-plane/src/tls.rs` (tag v0.1.1).
//! v0.1 shipped this against the same rustls 0.23 + rustls-pki-types 1
//! stack we're now re-adopting in Phase 3, so the implementation is
//! drop-in. PR #36 removed the entire wire surface when Phase 2's
//! reconciler runner had no listening socket; this re-introduces the
//! TLS-only path for PR-1. PR-2 layers `WebPkiClientVerifier` on top
//! for mTLS — the `client_ca_path` parameter already plumbs through
//! so no signature change is needed when that lands.

use anyhow::{Context, Result};
use rustls::server::WebPkiClientVerifier;
use rustls::ServerConfig;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::path::Path;
use std::sync::Arc;

/// Build a rustls `ServerConfig`. When `client_ca_path` is `Some`, the
/// returned config requires verified client certs signed by the CA at
/// that path (mTLS). When `None`, the listener accepts any client
/// without authentication — appropriate for PR-1 + `/healthz` only;
/// PR-2 onwards always pass a CA path.
///
/// All file IO is synchronous and happens once at startup. Failures
/// here should crash the process — they indicate misconfigured agenix
/// paths or a damaged fleet CA, neither of which the runtime can
/// recover from.
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
    fn build_server_config_missing_cert_fails() {
        let result = build_server_config(
            Path::new("/nonexistent/cert.pem"),
            Path::new("/nonexistent/key.pem"),
            None,
        );
        assert!(result.is_err());
    }
}
