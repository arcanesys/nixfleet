//! `GET /v1/deferrals` - channels currently held by `channelEdges`.
//!
//! Thin wrapper around `crate::state_view::compute_channel_deferrals`
//! so the route handler and the Prometheus exporter read the same
//! domain truth.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;

use super::super::state::AppState;
use crate::state_view::compute_channel_deferrals;

pub(in crate::server) async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    let deferrals = compute_channel_deferrals(&state).await;
    let json: Vec<serde_json::Value> = deferrals
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "channel": d.channel,
                "targetRef": d.target_ref,
                "blockedBy": d.blocked_by,
                "reason": d.reason,
            })
        })
        .collect();
    let body = serde_json::json!({ "deferrals": json }).to_string();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok((headers, body))
}
