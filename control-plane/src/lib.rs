use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, patch, post};
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use std::sync::Arc;
use tokio::sync::RwLock;

pub mod audit;
pub mod auth;
pub mod db;
pub mod metrics;
pub mod rollout;
pub mod routes;
pub mod state;
pub mod tls;

pub type AppState = (Arc<RwLock<state::FleetState>>, Arc<db::Db>);

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
    let agent_routes = Router::new()
        .route(
            "/api/v1/machines/{id}/desired-generation",
            get(routes::get_desired_generation),
        )
        .route("/api/v1/machines/{id}/report", post(routes::post_report));

    // Admin/operator endpoints: authenticated via API key (Bearer token).
    let admin_routes = Router::new()
        .route("/api/v1/machines", get(routes::list_machines))
        .route(
            "/api/v1/machines/{id}/set-generation",
            post(routes::set_desired_generation),
        )
        .route(
            "/api/v1/machines/{id}/register",
            post(routes::register_machine),
        )
        .route(
            "/api/v1/machines/{id}/lifecycle",
            patch(routes::update_lifecycle),
        )
        .route("/api/v1/machines/{id}/tags", post(routes::set_tags))
        .route(
            "/api/v1/machines/{id}/tags/{tag}",
            axum::routing::delete(routes::remove_tag),
        )
        .route("/api/v1/rollouts", post(rollout::routes::create_rollout))
        .route("/api/v1/rollouts", get(rollout::routes::list_rollouts))
        .route("/api/v1/rollouts/{id}", get(rollout::routes::get_rollout))
        .route(
            "/api/v1/rollouts/{id}/resume",
            post(rollout::routes::resume_rollout),
        )
        .route(
            "/api/v1/rollouts/{id}/cancel",
            post(rollout::routes::cancel_rollout),
        )
        .route("/api/v1/audit", get(audit::list_audit_events))
        .route("/api/v1/audit/export", get(audit::export_audit_csv))
        .layer(middleware::from_fn(move |headers, request, next| {
            let db = db_for_auth.clone();
            auth::require_api_key(headers, db, request, next)
        }));

    // Bootstrap route: no auth required (only works when no keys exist).
    let bootstrap_routes = Router::new()
        .route("/api/v1/keys/bootstrap", post(routes::bootstrap_api_key));

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
