use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{NaiveDateTime, TimeZone, Utc};
use nixfleet_types::rollout::{
    CreateScheduleRequest, OnFailure, RolloutStrategy, RolloutTarget, ScheduleStatus,
    ScheduledRollout,
};
use serde::Deserialize;

use crate::auth::Actor;
use crate::db::ScheduledRolloutRow;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ListSchedulesQuery {
    pub status: Option<String>,
}

/// POST /api/v1/schedules
pub async fn create_schedule(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Json(req): Json<CreateScheduleRequest>,
) -> Result<(StatusCode, Json<ScheduledRollout>), (StatusCode, String)> {
    if !actor.has_role(&["deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "deploy or admin role required".to_string(),
        ));
    }

    // Validate policy exists if specified
    let policy_id = if let Some(ref policy_name) = req.policy {
        let policy = db
            .get_policy_by_name(policy_name)
            .map_err(internal)?
            .ok_or((
                StatusCode::BAD_REQUEST,
                format!("policy not found: {policy_name}"),
            ))?;
        Some(policy.id)
    } else {
        None
    };

    // Must have either a policy or explicit strategy
    if policy_id.is_none() && req.strategy.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "either --policy or --strategy is required".to_string(),
        ));
    }

    let id = format!("sched-{}", uuid::Uuid::new_v4());
    let scheduled_at = req.scheduled_at.format("%Y-%m-%d %H:%M:%S").to_string();

    let target_tags = match &req.target {
        RolloutTarget::Tags(tags) => Some(serde_json::to_string(tags).unwrap_or_default()),
        RolloutTarget::Hosts(_) => None,
    };
    let target_hosts = match &req.target {
        RolloutTarget::Tags(_) => None,
        RolloutTarget::Hosts(hosts) => Some(serde_json::to_string(hosts).unwrap_or_default()),
    };

    let batch_sizes_json = req
        .batch_sizes
        .as_ref()
        .map(|bs| serde_json::to_string(bs).unwrap_or_default());

    db.create_scheduled_rollout(
        &id,
        &scheduled_at,
        policy_id.as_deref(),
        &req.generation_hash,
        req.cache_url.as_deref(),
        req.strategy.as_ref().map(|s| s.to_string()).as_deref(),
        batch_sizes_json.as_deref(),
        req.failure_threshold.as_deref(),
        req.on_failure.as_ref().map(|o| o.to_string()).as_deref(),
        req.health_timeout_secs.map(|h| h as i64),
        target_tags.as_deref(),
        target_hosts.as_deref(),
        &actor.identifier(),
    )
    .map_err(internal)?;

    let _ = db.insert_audit_event(
        &actor.identifier(),
        "schedule.created",
        &id,
        Some(&format!("scheduled_at={}", scheduled_at)),
    );

    let row = db
        .get_scheduled_rollout(&id)
        .map_err(internal)?
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "schedule not found after creation".to_string()))?;

    tracing::info!(schedule_id = %id, scheduled_at = %scheduled_at, "Scheduled rollout created");
    Ok((StatusCode::CREATED, Json(row_to_schedule(&row))))
}

/// GET /api/v1/schedules
pub async fn list_schedules(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Query(query): Query<ListSchedulesQuery>,
) -> Result<Json<Vec<ScheduledRollout>>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "readonly, deploy, or admin role required".to_string(),
        ));
    }

    let rows = db
        .list_scheduled_rollouts(query.status.as_deref(), 100)
        .map_err(internal)?;

    Ok(Json(rows.iter().map(row_to_schedule).collect()))
}

/// GET /api/v1/schedules/{id}
pub async fn get_schedule(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<String>,
) -> Result<Json<ScheduledRollout>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "readonly, deploy, or admin role required".to_string(),
        ));
    }

    let row = db
        .get_scheduled_rollout(&id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, format!("schedule not found: {id}")))?;

    Ok(Json(row_to_schedule(&row)))
}

/// POST /api/v1/schedules/{id}/cancel
pub async fn cancel_schedule(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !actor.has_role(&["deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "deploy or admin role required".to_string(),
        ));
    }

    let row = db
        .get_scheduled_rollout(&id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, format!("schedule not found: {id}")))?;

    if row.status != "pending" {
        return Err((
            StatusCode::CONFLICT,
            format!("schedule is {}, not pending", row.status),
        ));
    }

    db.update_scheduled_rollout_status(&id, "cancelled", None)
        .map_err(internal)?;

    let _ = db.insert_audit_event(&actor.identifier(), "schedule.cancelled", &id, None);

    tracing::info!(schedule_id = %id, "Scheduled rollout cancelled");
    Ok(StatusCode::OK)
}

fn internal(e: anyhow::Error) -> (StatusCode, String) {
    tracing::error!(error = %e, "Internal error");
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn row_to_schedule(row: &ScheduledRolloutRow) -> ScheduledRollout {
    let scheduled_at = parse_datetime(&row.scheduled_at);
    let created_at = parse_datetime(&row.created_at);

    let strategy = row
        .strategy
        .as_ref()
        .and_then(|s| serde_json::from_str::<RolloutStrategy>(&format!("\"{s}\"")).ok());

    let on_failure = row
        .on_failure
        .as_ref()
        .and_then(|s| serde_json::from_str::<OnFailure>(&format!("\"{s}\"")).ok());

    let batch_sizes = row
        .batch_sizes
        .as_ref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok());

    let target_tags = row
        .target_tags
        .as_ref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok());

    let target_hosts = row
        .target_hosts
        .as_ref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok());

    let status = match row.status.as_str() {
        "triggered" => ScheduleStatus::Triggered,
        "cancelled" => ScheduleStatus::Cancelled,
        _ => ScheduleStatus::Pending,
    };

    ScheduledRollout {
        id: row.id.clone(),
        scheduled_at,
        policy_id: row.policy_id.clone(),
        generation_hash: row.generation_hash.clone(),
        cache_url: row.cache_url.clone(),
        strategy,
        batch_sizes,
        failure_threshold: row.failure_threshold.clone(),
        on_failure,
        health_timeout_secs: row.health_timeout_secs.map(|h| h as u64),
        target_tags,
        target_hosts,
        status,
        rollout_id: row.rollout_id.clone(),
        created_at,
        created_by: row.created_by.clone(),
    }
}

fn parse_datetime(s: &str) -> chrono::DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|dt| Utc.from_utc_datetime(&dt))
        .unwrap_or_else(|_| Utc::now())
}
