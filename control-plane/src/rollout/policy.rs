use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{NaiveDateTime, TimeZone, Utc};
use nixfleet_types::rollout::{OnFailure, PolicyRequest, RolloutPolicy, RolloutStrategy};

use crate::auth::Actor;
use crate::AppState;

/// POST /api/v1/policies
pub async fn create_policy(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Json(req): Json<PolicyRequest>,
) -> Result<(StatusCode, Json<RolloutPolicy>), (StatusCode, String)> {
    if !actor.has_role(&["deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "deploy or admin role required".to_string(),
        ));
    }

    // Check for duplicate name
    if db.get_policy_by_name(&req.name).map_err(internal)?.is_some() {
        return Err((
            StatusCode::CONFLICT,
            format!("policy '{}' already exists", req.name),
        ));
    }

    let id = format!("pol-{}", uuid::Uuid::new_v4());
    let batch_sizes_json = serde_json::to_string(&req.batch_sizes).unwrap_or_default();

    db.create_policy(
        &id,
        &req.name,
        &req.strategy.to_string(),
        &batch_sizes_json,
        &req.failure_threshold,
        &req.on_failure.to_string(),
        req.health_timeout_secs as i64,
    )
    .map_err(internal)?;

    let _ = db.insert_audit_event(
        &actor.identifier(),
        "policy.created",
        &req.name,
        Some(&format!("strategy={} id={}", req.strategy, id)),
    );

    let policy = row_to_policy(
        &db.get_policy_by_name(&req.name)
            .map_err(internal)?
            .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "policy not found after creation".to_string()))?,
    );

    tracing::info!(policy_name = %req.name, "Policy created");
    Ok((StatusCode::CREATED, Json(policy)))
}

/// GET /api/v1/policies
pub async fn list_policies(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
) -> Result<Json<Vec<RolloutPolicy>>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "readonly, deploy, or admin role required".to_string(),
        ));
    }

    let rows = db.list_policies().map_err(internal)?;
    Ok(Json(rows.iter().map(row_to_policy).collect()))
}

/// GET /api/v1/policies/{name}
pub async fn get_policy(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(name): Path<String>,
) -> Result<Json<RolloutPolicy>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "readonly, deploy, or admin role required".to_string(),
        ));
    }

    let row = db
        .get_policy_by_name(&name)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, format!("policy not found: {name}")))?;

    Ok(Json(row_to_policy(&row)))
}

/// PUT /api/v1/policies/{name}
pub async fn update_policy(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(name): Path<String>,
    Json(req): Json<PolicyRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !actor.has_role(&["deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "deploy or admin role required".to_string(),
        ));
    }

    let batch_sizes_json = serde_json::to_string(&req.batch_sizes).unwrap_or_default();

    let updated = db
        .update_policy(
            &name,
            &req.strategy.to_string(),
            &batch_sizes_json,
            &req.failure_threshold,
            &req.on_failure.to_string(),
            req.health_timeout_secs as i64,
        )
        .map_err(internal)?;

    if !updated {
        return Err((StatusCode::NOT_FOUND, format!("policy not found: {name}")));
    }

    let _ = db.insert_audit_event(
        &actor.identifier(),
        "policy.updated",
        &name,
        Some(&format!("strategy={}", req.strategy)),
    );

    tracing::info!(policy_name = %name, "Policy updated");
    Ok(StatusCode::OK)
}

/// DELETE /api/v1/policies/{name}
pub async fn delete_policy(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !actor.has_role(&["admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "admin role required".to_string(),
        ));
    }

    let deleted = db.delete_policy(&name).map_err(internal)?;
    if !deleted {
        return Err((StatusCode::NOT_FOUND, format!("policy not found: {name}")));
    }

    let _ = db.insert_audit_event(&actor.identifier(), "policy.deleted", &name, None);

    tracing::info!(policy_name = %name, "Policy deleted");
    Ok(StatusCode::OK)
}

fn internal(e: anyhow::Error) -> (StatusCode, String) {
    tracing::error!(error = %e, "Internal error");
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn row_to_policy(row: &crate::db::PolicyRow) -> RolloutPolicy {
    let strategy: RolloutStrategy = serde_json::from_str(&format!("\"{}\"", row.strategy))
        .unwrap_or(RolloutStrategy::Staged);
    let on_failure: OnFailure =
        serde_json::from_str(&format!("\"{}\"", row.on_failure)).unwrap_or_default();
    let batch_sizes: Vec<String> =
        serde_json::from_str(&row.batch_sizes).unwrap_or_else(|_| vec!["100%".to_string()]);

    let created_at = parse_datetime(&row.created_at);
    let updated_at = parse_datetime(&row.updated_at);

    RolloutPolicy {
        id: row.id.clone(),
        name: row.name.clone(),
        strategy,
        batch_sizes,
        failure_threshold: row.failure_threshold.clone(),
        on_failure,
        health_timeout_secs: row.health_timeout_secs as u64,
        created_at,
        updated_at,
    }
}

fn parse_datetime(s: &str) -> chrono::DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|dt| Utc.from_utc_datetime(&dt))
        .unwrap_or_else(|_| Utc::now())
}
