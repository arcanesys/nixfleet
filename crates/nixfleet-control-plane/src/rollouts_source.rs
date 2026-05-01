//! On-demand HTTP-fetched rollout manifests; CP only checks `sha256(manifest)==rolloutId`, agent verifies signature.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use crate::polling::signed_fetch;

pub const ROLLOUT_ID_PLACEHOLDER: &str = "{rolloutId}";

#[derive(Debug, Clone)]
pub struct RolloutsSource {
    /// Must contain `{rolloutId}`.
    pub artifact_url_template: String,
    /// Must contain `{rolloutId}`.
    pub signature_url_template: String,
    /// `None` -> unauthenticated GET.
    pub token_file: Option<PathBuf>,
    pub timeout: Duration,
}

impl RolloutsSource {
    /// Bails if either template lacks the placeholder.
    pub fn new(
        artifact_url_template: String,
        signature_url_template: String,
        token_file: Option<PathBuf>,
    ) -> Result<Self> {
        if !artifact_url_template.contains(ROLLOUT_ID_PLACEHOLDER) {
            return Err(anyhow!(
                "rollouts source: artifact_url_template must contain {ROLLOUT_ID_PLACEHOLDER}"
            ));
        }
        if !signature_url_template.contains(ROLLOUT_ID_PLACEHOLDER) {
            return Err(anyhow!(
                "rollouts source: signature_url_template must contain {ROLLOUT_ID_PLACEHOLDER}"
            ));
        }
        Ok(Self {
            artifact_url_template,
            signature_url_template,
            token_file,
            timeout: Duration::from_secs(15),
        })
    }

    /// Recomputes `sha256(manifest_bytes)` against `rolloutId`; agent verifies signature.
    pub async fn fetch_pair(&self, rollout_id: &str) -> Result<(Vec<u8>, Vec<u8>)> {
        let artifact_url = self
            .artifact_url_template
            .replace(ROLLOUT_ID_PLACEHOLDER, rollout_id);
        let signature_url = self
            .signature_url_template
            .replace(ROLLOUT_ID_PLACEHOLDER, rollout_id);

        let token = signed_fetch::read_token(self.token_file.as_deref())?;
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(self.timeout)
            .build()
            .context("build rollouts-source client")?;

        let (manifest_bytes, signature_bytes) = signed_fetch::fetch_signed_pair(
            &client,
            &artifact_url,
            &signature_url,
            token.as_deref(),
        )
        .await
        .with_context(|| format!("fetch rollout pair for {rollout_id}"))?;

        let mut hasher = Sha256::new();
        hasher.update(&manifest_bytes);
        let computed = format!("{:x}", hasher.finalize());
        if computed != rollout_id {
            return Err(anyhow!(
                "rollouts source: content-address mismatch - \
                 url claimed {rollout_id} but sha256(bytes) = {computed}",
            ));
        }

        Ok((manifest_bytes, signature_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_template_without_placeholder() {
        let err = RolloutsSource::new(
            "https://example/no-placeholder.json".to_string(),
            "https://example/no-placeholder.json.sig".to_string(),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains(ROLLOUT_ID_PLACEHOLDER));
    }

    #[test]
    fn new_rejects_signature_template_without_placeholder() {
        let err = RolloutsSource::new(
            format!("https://example/{ROLLOUT_ID_PLACEHOLDER}.json"),
            "https://example/no-placeholder.json.sig".to_string(),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("signature_url_template"));
    }

    #[test]
    fn new_accepts_valid_templates() {
        let s = RolloutsSource::new(
            format!("https://example/rollouts/{ROLLOUT_ID_PLACEHOLDER}.json"),
            format!("https://example/rollouts/{ROLLOUT_ID_PLACEHOLDER}.json.sig"),
            Some(PathBuf::from("/run/agenix/token")),
        )
        .unwrap();
        assert!(s.artifact_url_template.contains(ROLLOUT_ID_PLACEHOLDER));
        assert!(s.signature_url_template.contains(ROLLOUT_ID_PLACEHOLDER));
        assert_eq!(
            s.token_file.as_deref(),
            Some(std::path::Path::new("/run/agenix/token"))
        );
    }
}
