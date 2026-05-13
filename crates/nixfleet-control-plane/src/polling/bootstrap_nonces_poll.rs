//! Bootstrap-nonces poll: fetch + verify signed bootstrap-nonces.json,
//! replace the in-memory `AllowedNoncesView` wholesale.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::RwLock;

use crate::db::allowed_nonces::AllowedNoncesView;
use crate::polling::poller::SignedArtifactPoller;
use crate::polling::signed_fetch;

pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct BootstrapNoncesSource {
    pub artifact_url: String,
    pub signature_url: String,
    pub token_file: Option<PathBuf>,
    pub trust_path: PathBuf,
    pub freshness_window: Duration,
}

/// `bootstrap_nonces_primed` flips to `true` after the first successful
/// verify + apply. The `/v1/*` ready gate consults this when
/// `bootstrap_nonces_required` is set so the daemon won't serve agents
/// until the full trust footprint is loaded.
pub fn spawn(
    cancel: tokio_util::sync::CancellationToken,
    allowed_nonces: Arc<RwLock<AllowedNoncesView>>,
    config: BootstrapNoncesSource,
    bootstrap_nonces_primed: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    SignedArtifactPoller {
        interval: POLL_INTERVAL,
        label: "bootstrap_nonces",
    }
    .spawn(cancel, move |client| {
        let allowed_nonces = Arc::clone(&allowed_nonces);
        let config = config.clone();
        let bootstrap_nonces_primed = Arc::clone(&bootstrap_nonces_primed);
        async move {
            let bn = poll_once(&client, &config).await?;
            let entries = bn.bootstrap_nonces.len();
            apply_verified_allowlist(&allowed_nonces, bn).await;
            let was_primed = bootstrap_nonces_primed.swap(true, Ordering::AcqRel);
            if !was_primed {
                tracing::info!(
                    target: "bootstrap_nonces",
                    entries = entries,
                    "bootstrap nonces primed: first verified allowlist applied",
                );
            }
            Ok(())
        }
    })
}

/// Replace the in-memory view wholesale. The previous view is dropped.
async fn apply_verified_allowlist(
    allowed_nonces: &RwLock<AllowedNoncesView>,
    bn: nixfleet_proto::BootstrapNonces,
) {
    let entries = bn.bootstrap_nonces.len();
    let signed_at = bn.meta.signed_at;
    let ci_commit = bn.meta.ci_commit.clone();
    let view = AllowedNoncesView::from_artifact(bn);
    let mut guard = allowed_nonces.write().await;
    *guard = view;
    drop(guard);
    tracing::info!(
        target: "bootstrap_nonces",
        entries = entries,
        signed_at = ?signed_at,
        ci_commit = ?ci_commit,
        "bootstrap-nonces poll: allowlist verified + applied",
    );
}

async fn poll_once(
    client: &reqwest::Client,
    config: &BootstrapNoncesSource,
) -> Result<nixfleet_proto::BootstrapNonces> {
    let token = signed_fetch::read_token(config.token_file.as_deref())?;
    let (artifact_bytes, signature_bytes) = signed_fetch::fetch_signed_pair(
        client,
        &config.artifact_url,
        &config.signature_url,
        token.as_deref(),
    )
    .await?;

    let (trusted_keys, reject_before) =
        signed_fetch::read_trust_roots(&config.trust_path, chrono::Utc::now())?;

    nixfleet_reconciler::verify_bootstrap_nonces(
        &artifact_bytes,
        &signature_bytes,
        &trusted_keys,
        chrono::Utc::now(),
        config.freshness_window,
        reject_before,
    )
    .map_err(|e| anyhow::anyhow!("verify_bootstrap_nonces (bootstrap-nonces poll): {e:?}"))
}
