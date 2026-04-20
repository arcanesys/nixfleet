use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Load a client certificate and key for mTLS.
pub fn load_client_identity(cert_path: &Path, key_path: &Path) -> Result<reqwest::Identity> {
    let cert_pem = fs::read(cert_path)
        .with_context(|| format!("failed to read client cert: {}", cert_path.display()))?;
    let key_pem = fs::read(key_path)
        .with_context(|| format!("failed to read client key: {}", key_path.display()))?;

    let mut combined = cert_pem;
    combined.extend_from_slice(&key_pem);

    reqwest::Identity::from_pem(&combined).context("failed to build client identity from PEM")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_missing_cert_fails() {
        let result = load_client_identity(
            Path::new("/nonexistent/client.pem"),
            Path::new("/nonexistent/key.pem"),
        );
        assert!(result.is_err());
    }
}
