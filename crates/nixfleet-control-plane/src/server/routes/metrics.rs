//! `/metrics` - Prometheus text format. mTLS-protected like `/v1/*`;
//! lab Prometheus scrapes with the same agent identity it presents to
//! `/v1/hosts`. Returns 404 when the `metrics` feature is off.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[cfg(feature = "metrics")]
use crate::metrics::{install_recorder, record_build_info};

/// `GET /metrics` - render counter state. 404 when `metrics` feature off.
/// `record_build_info()` is called on every request so `cp_build_info` is
/// always present in the scrape output even when no other events have fired.
#[cfg(feature = "metrics")]
pub(in crate::server) async fn metrics_handler() -> Result<Response, StatusCode> {
    record_build_info();
    let body = install_recorder().render();
    Ok(([("content-type", "text/plain; version=0.0.4")], body).into_response())
}

#[cfg(not(feature = "metrics"))]
pub(in crate::server) async fn metrics_handler() -> Result<Response, StatusCode> {
    Err(StatusCode::NOT_FOUND)
}
