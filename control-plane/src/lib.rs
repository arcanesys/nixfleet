use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use std::sync::Arc;
use tokio::sync::RwLock;

pub mod audit;
pub mod auth;
pub mod auth_cn;
pub mod db;
pub mod metrics;
pub mod release;
pub mod rollout;
pub mod routes;
pub mod state;
pub mod tls;

pub type AppState = (Arc<RwLock<state::FleetState>>, Arc<db::Db>);

/// Maximum number of items any paginated admin endpoint will return in
/// a single response. Ceiling exists to cap DB work and response size
/// regardless of what a client passes in `?limit=…`.
pub const MAX_PAGE_SIZE: i64 = 500;

/// Maximum length of a caller-supplied identifier (machine_id,
/// rollout_id, release_id) that will be accepted by any handler. This
/// is a defensive bound to prevent pathologically large strings from
/// flowing into DB queries. Real IDs are under 64 chars.
pub const MAX_ID_LEN: usize = 128;

/// Unified error type for Axum handlers.
///
/// Previously every handler returned `Result<_, (StatusCode, String)>`
/// and constructed tuples by hand, producing inconsistent mapping
/// between `anyhow::Error`, `rusqlite::Error`, and HTTP status codes.
/// This type centralizes the translation so each handler can use `?`
/// against infallible boilerplate.
#[derive(Debug)]
pub enum ControlPlaneError {
    /// 400 — client supplied malformed or invalid input.
    BadRequest(String),
    /// 401 — missing or invalid authentication.
    Unauthorized(String),
    /// 403 — authenticated but not allowed to perform this action.
    Forbidden(String),
    /// 404 — target resource does not exist.
    NotFound(String),
    /// 409 — request conflicts with current state (duplicate, dependency).
    Conflict(String),
    /// 500 — unexpected internal error. Message is logged but not
    /// leaked to the client verbatim.
    Internal(anyhow::Error),
}

impl ControlPlaneError {
    /// Convenience: wrap any error as an Internal variant.
    pub fn internal<E: Into<anyhow::Error>>(err: E) -> Self {
        Self::Internal(err.into())
    }
}

impl std::fmt::Display for ControlPlaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(m) => write!(f, "bad request: {m}"),
            Self::Unauthorized(m) => write!(f, "unauthorized: {m}"),
            Self::Forbidden(m) => write!(f, "forbidden: {m}"),
            Self::NotFound(m) => write!(f, "not found: {m}"),
            Self::Conflict(m) => write!(f, "conflict: {m}"),
            Self::Internal(e) => write!(f, "internal error: {e}"),
        }
    }
}

impl std::error::Error for ControlPlaneError {}

impl From<anyhow::Error> for ControlPlaneError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err)
    }
}

impl From<rusqlite::Error> for ControlPlaneError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Internal(anyhow::Error::from(err))
    }
}

impl From<serde_json::Error> for ControlPlaneError {
    fn from(err: serde_json::Error) -> Self {
        Self::Internal(anyhow::Error::from(err))
    }
}

impl IntoResponse for ControlPlaneError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m),
            Self::Conflict(m) => (StatusCode::CONFLICT, m),
            Self::Internal(e) => {
                // Log the full chain internally; return a generic
                // message to the client. We do not leak raw rusqlite
                // or anyhow error messages across a trust boundary.
                tracing::error!(error = %e, "control-plane internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, body).into_response()
    }
}

/// Log an error from a non-essential event/audit insert without
/// aborting the caller. Rollout and audit events are diagnostic: an
/// insert failure must not crash a live rollout, but it must not
/// silently disappear either. Accepts any `Result<T, E: Display>`
/// so the call site doesn't need to discard a success value first.
pub fn log_insert_err<T, E: std::fmt::Display>(kind: &str, result: Result<T, E>) {
    if let Err(e) = result {
        tracing::warn!(event = kind, error = %e, "failed to record audit/event");
    }
}

/// Build the Axum router with the given shared state.
///
/// Extracted so integration tests can construct the app without binding a port.
pub fn build_app(
    fleet_state: Arc<RwLock<state::FleetState>>,
    db: Arc<db::Db>,
    metrics_handle: Arc<PrometheusHandle>,
) -> Router {
    let db_for_auth = db.clone();

    // Agent-facing endpoints: authenticated via mTLS at the transport layer.
    // No API key middleware — agents don't carry bearer tokens.
    //
    // Defense-in-depth: cn_matches_path_machine_id rejects with 403
    // when the peer cert's CN does not match the {id} path segment, so
    // a leaked agent cert cannot impersonate a different agent. The
    // middleware is a no-op when no peer cert is present (raw HTTP
    // test harness or mTLS not configured), so existing tests are
    // unaffected.
    let agent_routes = Router::new()
        .route(
            "/api/v1/machines/{id}/desired-generation",
            get(routes::get_desired_generation),
        )
        .route("/api/v1/machines/{id}/report", post(routes::post_report))
        .layer(middleware::from_fn(auth_cn::cn_matches_path_machine_id));

    // Admin/operator endpoints: authenticated via API key (Bearer token).
    let admin_routes = Router::new()
        .route("/api/v1/machines", get(routes::list_machines))
        .route(
            "/api/v1/machines/{id}/register",
            post(routes::register_machine),
        )
        .route(
            "/api/v1/machines/{id}/lifecycle",
            patch(routes::update_lifecycle),
        )
        .route(
            "/api/v1/machines/{id}/desired-generation",
            delete(routes::clear_desired_generation),
        )
        .route(
            "/api/v1/machines/{id}/notify-deploy",
            post(routes::notify_deploy),
        )
        .route("/api/v1/rollouts", post(rollout::routes::create_rollout))
        .route("/api/v1/rollouts", get(rollout::routes::list_rollouts))
        .route(
            "/api/v1/rollouts/{id}",
            get(rollout::routes::get_rollout).delete(rollout::routes::delete_rollout),
        )
        .route(
            "/api/v1/rollouts/{id}/resume",
            post(rollout::routes::resume_rollout),
        )
        .route(
            "/api/v1/rollouts/{id}/cancel",
            post(rollout::routes::cancel_rollout),
        )
        .route(
            "/api/v1/releases",
            axum::routing::get(release::routes::list_releases)
                .post(release::routes::create_release),
        )
        .route(
            "/api/v1/releases/{id}",
            axum::routing::get(release::routes::get_release)
                .delete(release::routes::delete_release),
        )
        .route(
            "/api/v1/releases/{id}/diff/{other_id}",
            axum::routing::get(release::routes::diff_releases),
        )
        .route("/api/v1/audit", get(audit::list_audit_events))
        .route("/api/v1/audit/export", get(audit::export_audit_csv))
        .layer(middleware::from_fn(move |headers, request, next| {
            let db = db_for_auth.clone();
            auth::require_api_key(headers, db, request, next)
        }));

    // Bootstrap route: no auth required (only works when no keys exist).
    let bootstrap_routes =
        Router::new().route("/api/v1/keys/bootstrap", post(routes::bootstrap_api_key));

    Router::new()
        .merge(agent_routes)
        .merge(admin_routes)
        .merge(bootstrap_routes)
        .route("/health", get(|| async { "ok" }))
        .route(
            "/metrics",
            get(metrics::metrics_handler).with_state(metrics_handle),
        )
        .layer(middleware::from_fn(metrics::http_metrics_layer))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state((fleet_state, db))
}
