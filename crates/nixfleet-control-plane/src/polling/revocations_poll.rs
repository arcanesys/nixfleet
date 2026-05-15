//! Revocations poll: fetch+verify signed revocations.json, replay into `cert_revocations`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;

use crate::db::Db;
use crate::polling::poller::SignedArtifactPoller;
use crate::polling::signed_fetch;

pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct RevocationsSource {
    pub artifact_url: String,
    pub signature_url: String,
    pub token_file: Option<PathBuf>,
    pub trust_path: PathBuf,
    pub freshness_window: Duration,
}

/// `revocations_primed` flips to `true` after the first successful verify
/// + apply. The `/v1/*` ready gate (#95) consults this when
/// `revocations_required` is set so the daemon won't serve agents until
/// the full trust footprint is loaded - preventing the rebuild-revives-
/// revoked-certs window noted in #70.
pub fn spawn(
    cancel: tokio_util::sync::CancellationToken,
    db: Arc<Db>,
    config: RevocationsSource,
    revocations_primed: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    SignedArtifactPoller {
        interval: POLL_INTERVAL,
        label: "revocations",
    }
    .spawn(cancel, move |client| {
        let db = Arc::clone(&db);
        let config = config.clone();
        let revocations_primed = Arc::clone(&revocations_primed);
        async move {
            let revs = poll_once(&client, &config).await?;
            apply_verified_revocations(&db, &revs);
            let was_primed = revocations_primed.swap(true, Ordering::AcqRel);
            if !was_primed {
                tracing::info!(
                    target: "revocations",
                    entries = revs.revocations.len(),
                    "revocations primed: first verified list applied",
                );
            }
            Ok(())
        }
    })
}

/// Per-entry write failures log + continue; one bad row mustn't poison the rest.
fn apply_verified_revocations(db: &Db, revs: &nixfleet_proto::Revocations) {
    let n = revs.revocations.len();
    let mut applied = 0usize;
    for entry in &revs.revocations {
        match db.revocations().revoke_cert(
            &entry.hostname,
            entry.not_before,
            entry.reason.as_deref(),
            entry.revoked_by.as_deref(),
        ) {
            Ok(()) => applied += 1,
            Err(err) => tracing::warn!(
                hostname = %entry.hostname,
                error = %err,
                "revocations poll: revoke_cert failed for entry",
            ),
        }
    }
    // Reconcile - delete any DB row whose hostname dropped out of the
    // signed list, so operator-side de-revoke (entry removed from
    // fleet.nix `revocations`) actually un-blocks the host on the next
    // poll tick. Upsert-only would otherwise leave the row stale.
    let keep: Vec<&str> = revs
        .revocations
        .iter()
        .map(|e| e.hostname.as_str())
        .collect();
    let pruned = match db.revocations().retain_only(&keep) {
        Ok(n) => n,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "revocations poll: retain_only failed - DB may keep stale revocations",
            );
            0
        }
    };
    tracing::info!(
        target: "revocations",
        entries = n,
        applied = applied,
        pruned = pruned,
        signed_at = ?revs.meta.signed_at,
        ci_commit = ?revs.meta.ci_commit,
        "revocations poll: list verified",
    );
}

async fn poll_once(
    client: &reqwest::Client,
    config: &RevocationsSource,
) -> Result<nixfleet_proto::Revocations> {
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

    nixfleet_reconciler::verify_revocations(
        &artifact_bytes,
        &signature_bytes,
        &trusted_keys,
        chrono::Utc::now(),
        config.freshness_window,
        reject_before,
    )
    .map_err(|e| anyhow::anyhow!("verify_revocations (revocations poll): {e:?}"))
}
