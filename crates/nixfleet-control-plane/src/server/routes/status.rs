//! Read-only status endpoints and closure proxy fallback.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use chrono::Utc;
use nixfleet_proto::HostsResponse;
use serde::Serialize;

use super::super::middleware::AuthenticatedCn;
use super::super::state::AppState;
use crate::state_view::{StateViewError, fleet_state_view};

#[derive(Debug, Serialize)]
pub(in crate::server) struct WhoamiResponse {
    cn: String,
    /// RFC3339; moment we observed the verified identity, not the cert's notBefore.
    #[serde(rename = "issuedAt")]
    issued_at: String,
}

/// `GET /v1/whoami` - verified mTLS CN of the caller.
pub(in crate::server) async fn whoami(
    Extension(cn): Extension<AuthenticatedCn>,
) -> Json<WhoamiResponse> {
    Json(WhoamiResponse {
        cn: cn.into_string(),
        issued_at: Utc::now().to_rfc3339(),
    })
}

#[derive(Debug, Serialize)]
pub(in crate::server) struct ChannelStatusResponse {
    name: String,
    /// `None` when offline / file-backed deploys leave `meta.ciCommit` unset.
    declared_ci_commit: Option<String>,
    signed_at: Option<String>,
    freshness_window_minutes: u32,
}

/// `GET /v1/channels/{name}` - 503 until verified snapshot primed; 404 if channel undeclared.
pub(in crate::server) async fn channel_status(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<ChannelStatusResponse>, StatusCode> {
    let snapshot = state.verified_fleet.read().await.clone();
    let snap = snapshot.ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let fleet = snap.fleet;
    let channel = fleet.channels.get(&name).ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(ChannelStatusResponse {
        name,
        declared_ci_commit: fleet.meta.ci_commit.clone(),
        signed_at: fleet.meta.signed_at.map(|t| t.to_rfc3339()),
        freshness_window_minutes: channel.freshness_window,
    }))
}

/// `GET /v1/hosts` - joins verified fleet declarations with per-host checkins and report buffers.
pub(in crate::server) async fn hosts_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<HostsResponse>, StatusCode> {
    let hosts = fleet_state_view(&state).await.map_err(|e| match e {
        StateViewError::FleetNotPrimed => StatusCode::SERVICE_UNAVAILABLE,
    })?;
    Ok(Json(HostsResponse { hosts }))
}

/// `GET /v1/agent/closure/{hash}` - narinfo proxy fallback; 501 when no upstream configured.
pub(in crate::server) async fn closure_proxy(
    State(state): State<Arc<AppState>>,
    Extension(cn): Extension<AuthenticatedCn>,
    Path(closure_hash): Path<String>,
) -> Result<Response, StatusCode> {
    let cn = cn.as_str();

    let upstream = match &state.closure_upstream {
        Some(u) => u,
        None => {
            tracing::info!(
                target: "closure_proxy",
                cn = %cn,
                closure = %closure_hash,
                "closure proxy hit but no --closure-upstream configured (501)"
            );
            let body = serde_json::json!({
                "error": "closure proxy not configured",
                "closure": closure_hash,
                "tracking": "set services.nixfleet-control-plane.closureUpstream",
            });
            return Ok(Response::builder()
                .status(StatusCode::NOT_IMPLEMENTED)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("Response::builder with valid status + body is infallible"));
        }
    };

    let url = format!(
        "{}/{}.narinfo",
        upstream.base_url.trim_end_matches('/'),
        closure_hash
    );
    tracing::debug!(target: "closure_proxy", cn = %cn, url = %url, "forwarding");

    let resp = match upstream.client.get(&url).send().await {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(error = %err, "closure proxy: upstream unreachable");
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::from(format!("upstream error: {err}")))
                .expect("Response::builder with valid status + body is infallible"));
        }
    };
    let status = resp.status().as_u16();
    let body = resp.bytes().await.map_err(|err| {
        tracing::warn!(error = %err, "closure proxy: upstream body read failed");
        StatusCode::BAD_GATEWAY
    })?;
    Ok(Response::builder()
        .status(status)
        .header("content-type", "text/x-nix-narinfo")
        .body(Body::from(body))
        .expect("Response::builder with valid status + body is infallible"))
}
