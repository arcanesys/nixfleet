//! Auth + protocol middleware for the v1 router.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use nixfleet_proto::agent_wire::{PROTOCOL_MAJOR_VERSION, PROTOCOL_VERSION_HEADER};
use serde_json::json;

use crate::auth::auth_cn::PeerCertificates;

use super::state::AppState;

/// `Retry-After` hint advertised on 503 not-ready responses. Tracks
/// `channel_refs_poll::POLL_INTERVAL` (60 s) loosely - agents spread
/// their retries across the hint so the next poll cycle has time to
/// complete before they all reconnect.
const NOT_READY_RETRY_AFTER_SECS: u32 = 30;

/// 401 on missing/revoked cert; re-enrolled certs (notBefore > revoked_before) pass.
///
/// LOADBEARING: revocation DB rows store the **short** hostname (the
/// operator-declared form from fleet.nix), while the cert's CN is the
/// **canonical** `agent-<machineId>.<suffix>` form. Look up by the
/// canonicalized-down short hostname so the two sides match.
pub(super) async fn require_cn(
    state: &AppState,
    peer_certs: &PeerCertificates,
) -> Result<String, StatusCode> {
    if !peer_certs.is_present() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let cn = peer_certs.leaf_cn().ok_or(StatusCode::UNAUTHORIZED)?;

    if let Some(db) = &state.db {
        let machine_id = crate::auth::issuance::extract_machine_id(&cn, &state.agent_cn_suffix);
        match db.revocations().cert_revoked_before(&machine_id) {
            Ok(Some(revoked_before)) => {
                let cert_nbf = peer_certs
                    .leaf_not_before()
                    .ok_or(StatusCode::UNAUTHORIZED)?;
                if cert_nbf < revoked_before {
                    tracing::warn!(
                        cn = %cn,
                        machine_id = %machine_id,
                        cert_not_before = %cert_nbf.to_rfc3339(),
                        revoked_before = %revoked_before.to_rfc3339(),
                        "rejecting revoked cert"
                    );
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::error!(error = %err, "db cert_revoked_before failed");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }

    Ok(cn)
}

/// Type-system witness that auth ran; private field prevents forgery in handler code.
#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedCn(String);

impl AuthenticatedCn {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

pub(super) async fn require_cn_layer(
    state: Arc<AppState>,
    mut req: HttpRequest<Body>,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    let peer_certs = req
        .extensions()
        .get::<PeerCertificates>()
        .cloned()
        .unwrap_or_default();
    let cn = require_cn(&state, &peer_certs).await?;
    req.extensions_mut().insert(AuthenticatedCn(cn));
    Ok(next.run(req).await)
}

/// 503 with `Retry-After: 30` until `AppState::is_ready()` returns true.
/// Applied to every `/v1/*` route so agents see a deterministic "come
/// back later" signal instead of partial behaviour driven by stale or
/// missing trust state. Health/version/metrics are routed outside
/// `/v1/*` and stay unguarded so operators can scrape them while the
/// daemon is still priming.
pub(super) async fn require_ready_layer(
    state: Arc<AppState>,
    req: HttpRequest<Body>,
    next: Next,
) -> Response {
    if state.is_ready() {
        return next.run(req).await;
    }

    let body = Json(json!({
        "error": "control plane not ready",
        "reason": "awaiting first signed artifact",
    }));
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
    if let Ok(value) = NOT_READY_RETRY_AFTER_SECS.to_string().parse() {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// Forward-compat: missing header accepted; mismatched major -> 426. Strict mode rejects missing.
pub(super) async fn protocol_version_middleware(
    strict: bool,
    req: HttpRequest<Body>,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    if let Some(value) = req.headers().get(PROTOCOL_VERSION_HEADER) {
        match value.to_str().ok().and_then(|s| s.parse::<u32>().ok()) {
            Some(v) if v == PROTOCOL_MAJOR_VERSION => Ok(next.run(req).await),
            Some(v) => {
                tracing::warn!(
                    sent = v,
                    expected = PROTOCOL_MAJOR_VERSION,
                    "rejecting request with mismatched protocol major version"
                );
                Err(StatusCode::UPGRADE_REQUIRED)
            }
            None => {
                tracing::warn!(
                    raw = ?value,
                    "X-Nixfleet-Protocol header malformed"
                );
                Err(StatusCode::BAD_REQUEST)
            }
        }
    } else if strict {
        tracing::warn!("rejecting request without X-Nixfleet-Protocol (strict mode)");
        Err(StatusCode::BAD_REQUEST)
    } else {
        tracing::debug!("request without X-Nixfleet-Protocol - accepting (forward-compat)");
        Ok(next.run(req).await)
    }
}
