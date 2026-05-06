//! `/metrics` — Prometheus text format. mTLS-protected like `/v1/*`;
//! lab Prometheus scrapes with the same agent identity it presents to
//! `/v1/hosts` (see fleet's `monitoring-prometheus.nix nixfleet-cp`
//! job). Renders the global recorder; emits build_info each render so
//! the gauge is always present.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::metrics::{install_recorder, record_build_info};

/// `GET /metrics` — render counter state. No state-derived gauges —
/// see `metrics.rs` doc.
pub(in crate::server) async fn metrics_handler() -> Result<Response, StatusCode> {
    record_build_info();
    let body = install_recorder().render();
    Ok((
        [("content-type", "text/plain; version=0.0.4")],
        body,
    )
        .into_response())
}
