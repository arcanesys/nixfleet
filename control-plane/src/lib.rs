use axum::middleware;
use axum::routing::{get, patch, post};
use axum::Router;
use std::sync::Arc;
use tokio::sync::RwLock;

pub mod audit;
pub mod auth;
pub mod db;
pub mod routes;
pub mod state;
pub mod tls;

pub type AppState = (Arc<RwLock<state::FleetState>>, Arc<db::Db>);

/// Build the Axum router with the given shared state.
///
/// Extracted so integration tests can construct the app without binding a port.
pub fn build_app(fleet_state: Arc<RwLock<state::FleetState>>, db: Arc<db::Db>) -> Router {
    let db_for_auth = db.clone();

    let api_routes = Router::new()
        .route("/api/v1/machines", get(routes::list_machines))
        .route(
            "/api/v1/machines/{id}/desired-generation",
            get(routes::get_desired_generation),
        )
        .route("/api/v1/machines/{id}/report", post(routes::post_report))
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
        .route("/api/v1/audit", get(audit::list_audit_events))
        .route("/api/v1/audit/export", get(audit::export_audit_csv))
        .layer(middleware::from_fn(move |headers, request, next| {
            let db = db_for_auth.clone();
            auth::require_api_key(headers, db, request, next)
        }));

    Router::new()
        .merge(api_routes)
        .route("/health", get(|| async { "ok" }))
        .with_state((fleet_state, db))
}
