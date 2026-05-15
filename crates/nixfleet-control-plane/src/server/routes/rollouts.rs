//! Stateless distributor for pre-signed rollout manifests; CP holds no signing key.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;

use super::super::route_error::internal_warn;
use super::super::state::AppState;

// LOADBEARING: 64-char-hex check blocks path-traversal smuggling (`..`, NUL fail the hex check).
fn looks_like_rollout_id(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn manifest_paths(dir: &FsPath, rollout_id: &str) -> (PathBuf, PathBuf) {
    let manifest = dir.join(format!("{rollout_id}.json"));
    let sig = dir.join(format!("{rollout_id}.json.sig"));
    (manifest, sig)
}

type ManifestPair = (Vec<u8>, Vec<u8>);

fn try_load_from_dir(dir: &FsPath, rollout_id: &str) -> Result<Option<ManifestPair>, StatusCode> {
    let (manifest_path, sig_path) = manifest_paths(dir, rollout_id);
    let manifest_bytes = match std::fs::read(&manifest_path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            tracing::warn!(
                rollout_id = %rollout_id,
                path = %manifest_path.display(),
                error = %err,
                "rollouts handler: read manifest failed",
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let sig_bytes = match std::fs::read(&sig_path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // GOTCHA: manifest present but sig missing - refuse rather than serve unverifiable bytes.
            tracing::warn!(
                rollout_id = %rollout_id,
                "rollouts handler: signature file missing for present manifest",
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(err) => {
            tracing::warn!(
                rollout_id = %rollout_id,
                error = %err,
                "rollouts handler: read signature failed",
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    Ok(Some((manifest_bytes, sig_bytes)))
}

/// LOADBEARING: filename IS the sha256; mismatch means corruption or
/// wrong-bytes-for-id. Hashes received bytes directly - never re-serialises
/// a parsed struct (would silently drop fields the CP's proto doesn't know
/// about, breaking content-addressing across additive schema changes).
fn verify_content_address(manifest_bytes: &[u8], rollout_id: &str) -> Result<(), StatusCode> {
    let recomputed = nixfleet_reconciler::rollout_id_from_bytes(manifest_bytes).map_err(|err| {
        tracing::warn!(
            rollout_id = %rollout_id,
            error = ?err,
            "rollouts handler: rollout_id_from_bytes failed",
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    if recomputed != rollout_id {
        tracing::warn!(
            rollout_id = %rollout_id,
            recomputed = %recomputed,
            "rollouts handler: manifest hash does not match path - refusing to serve",
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(())
}

async fn load_pair(state: &AppState, rollout_id: &str) -> Result<ManifestPair, StatusCode> {
    if state.rollouts_dir.is_none() && state.rollouts_source.is_none() {
        tracing::debug!(
            rollout_id = %rollout_id,
            "rollouts handler: neither rollouts_dir nor rollouts_source configured; returning 503",
        );
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    if !looks_like_rollout_id(rollout_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    if let Some(dir) = state.rollouts_dir.as_ref()
        && let Some((manifest_bytes, sig_bytes)) = try_load_from_dir(dir, rollout_id)?
    {
        verify_content_address(&manifest_bytes, rollout_id)?;
        return Ok((manifest_bytes, sig_bytes));
    }

    if let Some(source) = state.rollouts_source.as_ref() {
        match source.fetch_pair(rollout_id).await {
            Ok((manifest_bytes, sig_bytes)) => {
                // Parity with filesystem path: also defends against malformed-but-correctly-hashed payloads.
                verify_content_address(&manifest_bytes, rollout_id)?;
                tracing::info!(
                    rollout_id = %rollout_id,
                    "rollouts handler: fetched manifest pair from upstream source",
                );
                return Ok((manifest_bytes, sig_bytes));
            }
            Err(err) => {
                tracing::warn!(
                    rollout_id = %rollout_id,
                    error = %err,
                    "rollouts handler: upstream fetch failed",
                );
                return Err(StatusCode::BAD_GATEWAY);
            }
        }
    }

    Err(StatusCode::NOT_FOUND)
}

/// `GET /v1/rollouts/{rolloutId}` - manifest bytes; mTLS via router-level `require_cn_layer`.
pub(in crate::server) async fn manifest(
    State(state): State<Arc<AppState>>,
    Path(rollout_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let (manifest_bytes, _sig) = load_pair(&state, &rollout_id).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok((StatusCode::OK, headers, Bytes::from(manifest_bytes)))
}

/// `GET /v1/rollouts/{rolloutId}/sig` - raw signature bytes.
pub(in crate::server) async fn signature(
    State(state): State<Arc<AppState>>,
    Path(rollout_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let (_manifest, sig_bytes) = load_pair(&state, &rollout_id).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    Ok((StatusCode::OK, headers, Bytes::from(sig_bytes)))
}

/// `GET /v1/rollouts` - enumerate active (non-superseded) rollouts with
/// per-host state pulled from `host_rollout_state` (DB-authoritative,
/// independent of the journal event window).
///
/// Operators (status renderers) use this for "what's actually deployed"
/// instead of inferring from journal `target=confirm` events - agent
/// confirms only fire on real dispatches, so converged-at-dispatch hosts
/// would otherwise look unconfirmed forever in journal-derived views.
pub(in crate::server) async fn list_active(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    let db = state.db.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    // UI surface - show only rollouts the operator should still care
    // about (excludes both superseded and terminal). The gate observed
    // builders use `list_active()` directly so converged predecessors
    // stay visible to channel_edges.
    let rollouts_meta = db
        .rollouts()
        .list_in_flight()
        .map_err(internal_warn("list_in_flight rollouts query failed"))?;
    let snap = db
        .host_dispatch_state()
        .active_rollouts_snapshot()
        .map_err(internal_warn("active_rollouts_snapshot query failed"))?;
    // host_states keyed by rollout_id; rollouts not in the snapshot get an
    // empty map (no hosts dispatched yet - happens briefly after a CI sign
    // before agents check in).
    let mut by_rollout: std::collections::HashMap<
        String,
        std::collections::HashMap<String, String>,
    > = std::collections::HashMap::new();
    for r in &snap {
        by_rollout.insert(r.rollout_id.clone(), r.host_states.clone());
    }

    let rollouts: Vec<serde_json::Value> = rollouts_meta
        .into_iter()
        .map(|r| {
            let host_states = by_rollout.remove(&r.rollout_id).unwrap_or_default();
            serde_json::json!({
                "rolloutId": r.rollout_id,
                "channel": r.channel,
                "currentWave": r.current_wave,
                "createdAt": r.created_at,
                "hostStates": host_states,
            })
        })
        .collect();
    let body = serde_json::json!({ "rollouts": rollouts }).to_string();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok((StatusCode::OK, headers, body))
}

/// `GET /v1/rollouts/{rolloutId}/lifecycle` - supersession state for the
/// rollout, sourced solely from the rollouts table. Returns 404 for any
/// rid not tracked there.
///
/// Distinct from the signed manifest endpoint because we can't inject
/// server-derived metadata into the signed bytes.
pub(in crate::server) async fn lifecycle(
    State(state): State<Arc<AppState>>,
    Path(rollout_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    if !looks_like_rollout_id(&rollout_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = state.db.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let status = db
        .rollouts()
        .supersede_status(&rollout_id)
        .map_err(internal_warn("lifecycle: supersede_status query failed"))?;
    let status = status.ok_or(StatusCode::NOT_FOUND)?;
    let body = serde_json::json!({
        "rolloutId": rollout_id,
        "supersededAt": status.superseded_at.map(|t| t.to_rfc3339()),
        "supersededBy": status.superseded_by,
        // Distinct from supersededAt - terminal_at fires on natural
        // convergence (Action::ConvergeRollout) or orphan-sweep retire
        // (channel has no expected hosts). UI consumers use this to
        // gray out finished rollouts; gates ignore it (they read the
        // host_states directly from list_active).
        "terminalAt": status.terminal_at.map(|t| t.to_rfc3339()),
    })
    .to_string();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok((StatusCode::OK, headers, body))
}

/// `GET /v1/rollouts/{rolloutId}/trace` - wave-by-wave dispatch history
/// rendered for `nixfleet rollout trace`. Returns 404 when the rollout
/// has no dispatch_history rows (either never dispatched or pruned past
/// the 90d retention window).
pub(in crate::server) async fn trace(
    State(state): State<Arc<AppState>>,
    Path(rollout_id): Path<String>,
) -> Result<axum::Json<nixfleet_proto::RolloutTrace>, StatusCode> {
    if !looks_like_rollout_id(&rollout_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = state.db.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let rows = db
        .dispatch_history()
        .for_rollout(&rollout_id)
        .map_err(internal_warn("trace: dispatch_history.for_rollout failed"))?;
    if rows.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }
    let events = rows
        .into_iter()
        .map(|r| nixfleet_proto::RolloutTraceEvent {
            host: r.hostname,
            channel: r.channel,
            wave: r.wave,
            target_closure_hash: r.target_closure_hash,
            target_channel_ref: r.target_channel_ref,
            dispatched_at: r.dispatched_at,
            terminal_state: r.terminal_state,
            terminal_at: r.terminal_at,
        })
        .collect();
    Ok(axum::Json(nixfleet_proto::RolloutTrace {
        rollout_id,
        events,
    }))
}
