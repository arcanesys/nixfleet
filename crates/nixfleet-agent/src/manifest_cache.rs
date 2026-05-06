//! Rollout manifest fetch + verify + cache. Before the agent consumes any
//! field of a target it must verify the manifest, recompute its hash equals
//! the advertised rolloutId, and assert (hostname, wave_index) ∈ host_set.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use nixfleet_proto::{RolloutManifest, TrustConfig};

#[derive(Debug)]
pub enum ManifestError {
    Missing(String),
    VerifyFailed(String),
    Mismatch(String),
}

impl ManifestError {
    pub fn reason(&self) -> &str {
        match self {
            ManifestError::Missing(s) => s,
            ManifestError::VerifyFailed(s) => s,
            ManifestError::Mismatch(s) => s,
        }
    }
}

pub struct ManifestCache {
    rollouts_dir: PathBuf,
    trust_path: PathBuf,
}

impl ManifestCache {
    pub fn new(state_dir: &Path, trust_path: &Path) -> Self {
        Self {
            rollouts_dir: state_dir.join("rollouts"),
            trust_path: trust_path.to_path_buf(),
        }
    }

    fn manifest_path(&self, rollout_id: &str) -> PathBuf {
        self.rollouts_dir.join(format!("{rollout_id}.json"))
    }

    fn signature_path(&self, rollout_id: &str) -> PathBuf {
        self.rollouts_dir.join(format!("{rollout_id}.json.sig"))
    }

    /// Reads (manifest, sig) bytes if both exist; does NOT verify.
    pub fn read_cached_bytes(&self, rollout_id: &str) -> Option<(Vec<u8>, Vec<u8>)> {
        let manifest = std::fs::read(self.manifest_path(rollout_id)).ok()?;
        let sig = std::fs::read(self.signature_path(rollout_id)).ok()?;
        Some((manifest, sig))
    }

    fn load_trust_roots(
        &self,
        now: chrono::DateTime<Utc>,
    ) -> Result<(
        Vec<nixfleet_proto::TrustedPubkey>,
        Option<chrono::DateTime<Utc>>,
    )> {
        let raw = std::fs::read_to_string(&self.trust_path)
            .with_context(|| format!("read trust file {}", self.trust_path.display()))?;
        let trust: TrustConfig = serde_json::from_str(&raw).context("parse trust file")?;
        Ok((
            trust.ci_release_key.active_keys_at(now),
            trust.ci_release_key.reject_before,
        ))
    }

    fn verify_bytes(
        &self,
        manifest_bytes: &[u8],
        signature_bytes: &[u8],
        advertised_rollout_id: &str,
    ) -> Result<RolloutManifest, ManifestError> {
        // 1h window matches channel-refs poll posture.
        let now = Utc::now();
        let (trusted_keys, reject_before) = self
            .load_trust_roots(now)
            .map_err(|err| ManifestError::VerifyFailed(format!("load trust roots: {err:#}")))?;
        let window = std::time::Duration::from_secs(3600);
        let manifest = nixfleet_reconciler::verify_rollout_manifest(
            manifest_bytes,
            signature_bytes,
            &trusted_keys,
            now,
            window,
            reject_before,
        )
        .map_err(|err| ManifestError::VerifyFailed(format!("{err:?}")))?;

        // LOADBEARING: hash the bytes we received, NOT a re-serialised parsed
        // struct. The agent's RolloutManifest proto may lag the producer's
        // when the schema is in mid-cutover (CONTRACTS §V Pattern A says
        // additive changes are safe - but only if the verify path is
        // byte-faithful). Re-serialising drops fields the agent's struct
        // doesn't know about, producing a different canonical hash and
        // bricking auto-upgrade across schema versions.
        let recomputed = nixfleet_reconciler::rollout_id_from_bytes(manifest_bytes)
            .map_err(|err| ManifestError::Mismatch(format!("rollout_id_from_bytes: {err:?}")))?;
        if recomputed != advertised_rollout_id {
            return Err(ManifestError::Mismatch(format!(
                "advertised rolloutId {advertised} ≠ recomputed sha256 {recomputed}",
                advertised = advertised_rollout_id
            )));
        }
        Ok(manifest)
    }

    fn assert_membership(
        manifest: &RolloutManifest,
        hostname: &str,
        wave_index: u32,
    ) -> Result<(), ManifestError> {
        let in_set = manifest
            .host_set
            .iter()
            .any(|h| h.hostname == hostname && h.wave_index == wave_index);
        if !in_set {
            return Err(ManifestError::Mismatch(format!(
                "(hostname={hostname}, wave_index={wave_index}) not in manifest.host_set"
            )));
        }
        Ok(())
    }

    fn write_cache(&self, rollout_id: &str, manifest_bytes: &[u8], sig_bytes: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.rollouts_dir).with_context(|| {
            format!("create rollouts cache dir {}", self.rollouts_dir.display())
        })?;
        std::fs::write(self.manifest_path(rollout_id), manifest_bytes)
            .with_context(|| format!("write {}", self.manifest_path(rollout_id).display()))?;
        std::fs::write(self.signature_path(rollout_id), sig_bytes)
            .with_context(|| format!("write {}", self.signature_path(rollout_id).display()))?;
        Ok(())
    }

    /// Cache hit re-verifies bytes (defense in depth); miss fetches + writes through.
    pub async fn ensure(
        &self,
        client: &reqwest::Client,
        cp_url: &str,
        rollout_id: &str,
        hostname: &str,
        wave_index: u32,
    ) -> Result<RolloutManifest, ManifestError> {
        if let Some((manifest_bytes, sig_bytes)) = self.read_cached_bytes(rollout_id) {
            let manifest = self.verify_bytes(&manifest_bytes, &sig_bytes, rollout_id)?;
            Self::assert_membership(&manifest, hostname, wave_index)?;
            return Ok(manifest);
        }

        let base = cp_url.trim_end_matches('/');
        let manifest_url = format!("{base}/v1/rollouts/{rollout_id}");
        let sig_url = format!("{base}/v1/rollouts/{rollout_id}/sig");

        let manifest_bytes = fetch(client, &manifest_url).await?;
        let sig_bytes = fetch(client, &sig_url).await?;

        let manifest = self.verify_bytes(&manifest_bytes, &sig_bytes, rollout_id)?;
        Self::assert_membership(&manifest, hostname, wave_index)?;

        if let Err(err) = self.write_cache(rollout_id, &manifest_bytes, &sig_bytes) {
            // Cache-write failure non-fatal: in-memory manifest is verified.
            tracing::warn!(
                rollout_id = %rollout_id,
                error = %err,
                "manifest cache: write-through failed (will refetch next checkin)",
            );
        }

        Ok(manifest)
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, ManifestError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|err| ManifestError::Missing(format!("GET {url}: {err}")))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(ManifestError::Missing(format!("404 from {url}")));
    }
    if !status.is_success() {
        return Err(ManifestError::Missing(format!("{url}: {status}")));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|err| ManifestError::Missing(format!("read body {url}: {err}")))?;
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_error_variants_distinct_on_debug() {
        let outcomes = [
            format!("{:?}", ManifestError::Missing("x".into())),
            format!("{:?}", ManifestError::VerifyFailed("x".into())),
            format!("{:?}", ManifestError::Mismatch("x".into())),
        ];
        let unique: std::collections::HashSet<_> = outcomes.iter().collect();
        assert_eq!(unique.len(), outcomes.len());
    }
}
