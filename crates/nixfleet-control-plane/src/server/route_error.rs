//! Shared route-handler error helpers. Each returns a `FnOnce(E) -> StatusCode`
//! that logs and converts - paired with `?` to drop the 4-line map_err blocks.

use axum::http::StatusCode;
use std::fmt::Display;

/// `.map_err(internal("label: detail"))` -> log at error + 500.
pub(crate) fn internal<E: Display>(label: &'static str) -> impl FnOnce(E) -> StatusCode {
    move |err| {
        tracing::error!(error = %err, "{label}");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// `.map_err(bad_request("label: detail"))` -> log at warn + 400.
pub(crate) fn bad_request<E: Display>(label: &'static str) -> impl FnOnce(E) -> StatusCode {
    move |err| {
        tracing::warn!(error = %err, "{label}");
        StatusCode::BAD_REQUEST
    }
}

/// `.map_err(bad_request_error("label: detail"))` - log level escalated to
/// error (handler-side reasons indicate a CP bug, not a malformed request).
pub(crate) fn bad_request_error<E: Display>(label: &'static str) -> impl FnOnce(E) -> StatusCode {
    move |err| {
        tracing::error!(error = %err, "{label}");
        StatusCode::BAD_REQUEST
    }
}

/// `.map_err(internal_warn("label: detail"))` - same as `internal` but at
/// warn level. For DB-side queries where a transient miss isn't a CP bug.
pub(crate) fn internal_warn<E: Display>(label: &'static str) -> impl FnOnce(E) -> StatusCode {
    move |err| {
        tracing::warn!(error = %err, "{label}");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}
