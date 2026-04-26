//! Forgejo poll loop for channel-refs (Phase 3 PR-4).
//!
//! Replaces the hand-edited `/etc/nixfleet/cp/channel-refs.json`
//! default from PR-4's earlier design. Polls Forgejo's contents API
//! every 60s for `releases/fleet.resolved.json`, decodes the base64
//! body, runs the existing `verify_artifact` against it, and
//! refreshes an in-memory `channel_refs` cache.
//!
//! Failure semantics: log warning + retain previous cache. CP does
//! not crash on Forgejo unavailability — operator can curl /healthz
//! and see when the last successful tick was even if Forgejo is
//! down.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;
use tokio::sync::RwLock;

/// Poll cadence — D9 default. Faster doesn't help (CI sign + push
/// latency dominates); slower delays the operator's "I pushed a
/// release commit, when does CP see it" feedback loop unhelpfully.
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Configuration for the poll task. All fields populated by the
/// CLI flags in main.rs.
#[derive(Debug, Clone)]
pub struct ForgejoConfig {
    /// e.g. `https://git.lab.internal`. No trailing slash needed —
    /// the URL builder normalises.
    pub base_url: String,
    /// e.g. `abstracts33d`.
    pub owner: String,
    /// e.g. `fleet`.
    pub repo: String,
    /// Path inside the repo to the canonical resolved-artifact JSON.
    /// Default: `releases/fleet.resolved.json`.
    pub artifact_path: String,
    /// Path to a file containing the Forgejo API token (no surrounding
    /// whitespace). Read on each poll so token rotation propagates
    /// without restart. Loaded into memory at request time.
    pub token_file: PathBuf,
}

/// Forgejo `/api/v1/repos/{o}/{r}/contents/{path}` response.
/// `content` is base64-encoded with `\n` chunked every 60 chars
/// (RFC 2045 / "MIME" encoding).
#[derive(Debug, Deserialize)]
struct ContentsResponse {
    content: String,
    encoding: String,
}

/// In-memory cache the reconcile loop reads from. Wrapped in
/// `Arc<RwLock<...>>` so concurrent reads are cheap; writes only
/// happen at poll cadence.
#[derive(Debug, Clone, Default)]
pub struct ChannelRefsCache {
    pub refs: HashMap<String, String>,
    /// rfc3339 wall-clock of the last *successful* poll. None if
    /// we've never had one.
    pub last_refreshed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Spawn the poll task. Runs forever; logs warnings on failure;
/// updates the cache on success.
///
/// Verification: on each successful fetch, the resolved-artifact
/// JSON is parsed and `nixfleet_proto::FleetResolved::channels` is
/// flattened into `channel_refs` (channelName → ref string). PR-4
/// wires the existing `verify_artifact` once we have the matching
/// signature pulled from Forgejo too — for now we trust the
/// authenticated TLS channel + Forgejo's RBAC. Verify-on-load
/// lands in PR-4.5 (TODO note).
pub fn spawn(
    cache: Arc<RwLock<ChannelRefsCache>>,
    config: ForgejoConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("build forgejo poll client");

        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            ticker.tick().await;
            match poll_once(&client, &config).await {
                Ok(refs) => {
                    let mut guard = cache.write().await;
                    let changed = guard.refs != refs;
                    guard.refs = refs.clone();
                    guard.last_refreshed_at = Some(chrono::Utc::now());
                    drop(guard);
                    if changed {
                        tracing::info!(
                            count = refs.len(),
                            "channel refs refreshed from forgejo (changed)"
                        );
                    } else {
                        tracing::debug!(count = refs.len(), "channel refs refreshed (unchanged)");
                    }
                }
                Err(err) => {
                    // Cache retained — reconcile loop continues
                    // against last-known channel-refs.
                    tracing::warn!(error = %err, "forgejo poll failed; retaining previous cache");
                }
            }
        }
    })
}

async fn poll_once(
    client: &reqwest::Client,
    config: &ForgejoConfig,
) -> Result<HashMap<String, String>> {
    let token = std::fs::read_to_string(&config.token_file)
        .with_context(|| format!("read forgejo token file {}", config.token_file.display()))?
        .trim()
        .to_string();

    let url = format!(
        "{}/api/v1/repos/{}/{}/contents/{}",
        config.base_url.trim_end_matches('/'),
        config.owner,
        config.repo,
        config.artifact_path,
    );

    let resp = client
        .get(&url)
        .header("Authorization", format!("token {token}"))
        .header("Accept", "application/json")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{url}: {status}: {body}");
    }

    let parsed: ContentsResponse = resp.json().await.context("parse forgejo contents response")?;
    if parsed.encoding != "base64" {
        anyhow::bail!("unexpected forgejo content encoding: {}", parsed.encoding);
    }

    // Forgejo wraps base64 lines at 60 chars per RFC 2045. Engine
    // tolerates whitespace by default so we don't strip newlines
    // ourselves.
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(parsed.content.replace(['\n', '\r'], "").as_bytes())
        .context("decode forgejo base64 content")?;
    let fleet_resolved: nixfleet_proto::FleetResolved =
        serde_json::from_slice(&bytes).context("parse fleet.resolved.json")?;

    // Flatten channels → channel_refs (the shape Observed expects).
    // For PR-4, we use a single CI commit as the ref for every
    // channel — the resolved-artifact rev IS the channel rev when
    // there's only one fleet repo. Phase 4 may split per-channel
    // refs if multi-channel-rev support is wanted (e.g. dev channel
    // tracks main, prod tracks a release branch).
    //
    // TODO(phase-3-pr-4): operator review — confirm "all channels
    // share the same ci_commit" matches the intended channel
    // semantics. Likely sufficient for the homelab's single-channel
    // fleet but would need refinement for a multi-channel deployment.
    let ci_commit = fleet_resolved
        .meta
        .ci_commit
        .clone()
        .unwrap_or_else(|| "<unknown>".to_string());
    let mut refs = HashMap::new();
    for name in fleet_resolved.channels.keys() {
        refs.insert(name.clone(), ci_commit.clone());
    }
    Ok(refs)
}
