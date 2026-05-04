//! `GET /healthz` - outside `/v1/*` so it bypasses the protocol-version middleware.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::super::state::AppState;

#[derive(Debug, Serialize)]
pub(in crate::server) struct HealthzResponse {
    ok: bool,
    version: &'static str,
    /// RFC3339 UTC; `null` until the reconcile loop ticks once.
    last_tick_at: Option<String>,
}

pub(in crate::server) async fn healthz(state: State<Arc<AppState>>) -> Json<HealthzResponse> {
    let last = *state.last_tick_at.read().await;
    Json(HealthzResponse {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        last_tick_at: last.map(|t| t.to_rfc3339()),
    })
}
