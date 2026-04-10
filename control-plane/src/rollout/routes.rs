use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{NaiveDateTime, TimeZone, Utc};
use nixfleet_types::rollout::{
    BatchDetail, BatchStatus, BatchSummary, CreateRolloutRequest, CreateRolloutResponse,
    MachineHealthStatus, OnFailure, RolloutDetail, RolloutEvent, RolloutStatus, RolloutStrategy,
    RolloutTarget,
};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::Actor;
use crate::db::{RolloutBatchRow, RolloutRow};
use crate::rollout::batch;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ListRolloutsQuery {
    pub status: Option<String>,
}

/// POST /api/v1/rollouts
///
/// Create a new rollout targeting machines by tags or hosts.
pub async fn create_rollout(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Json(req): Json<CreateRolloutRequest>,
) -> Result<(StatusCode, Json<CreateRolloutResponse>), (StatusCode, String)> {
    if !actor.has_role(&["deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "deploy or admin role required".to_string(),
        ));
    }

    // Resolve target machines
    let mut machine_ids = match &req.target {
        RolloutTarget::Tags(tags) => db.get_machines_by_tags(tags).map_err(|e| {
            tracing::error!(error = %e, "Failed to resolve machines by tags");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to resolve machines".to_string(),
            )
        })?,
        RolloutTarget::Hosts(hosts) => hosts.clone(),
    };

    if machine_ids.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "no machines match the target".to_string(),
        ));
    }

    // Load release and intersect with target machines
    let release = db
        .get_release(&req.release_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("release {} not found", req.release_id),
            )
        })?;
    let release_entries = db
        .get_release_entries(&req.release_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let release_hosts: std::collections::HashSet<String> =
        release_entries.iter().map(|e| e.hostname.clone()).collect();
    let original_count = machine_ids.len();
    machine_ids.retain(|id| release_hosts.contains(id));
    if machine_ids.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "no machines match both the target and release {}",
                req.release_id
            ),
        ));
    }
    if original_count > machine_ids.len() {
        tracing::warn!(
            skipped = original_count - machine_ids.len(),
            "machines in target but not in release"
        );
    }
    let cache_url = req.cache_url.or(release.cache_url);

    // Check for active rollout conflicts
    for machine_id in &machine_ids {
        if let Ok(Some(rollout_id)) = db.machine_in_active_rollout(machine_id) {
            return Err((
                StatusCode::CONFLICT,
                format!("machine {machine_id} is in active rollout {rollout_id}"),
            ));
        }
    }

    // Build batches
    let effective_sizes = batch::effective_batch_sizes(&req.strategy, &req.batch_sizes);
    let batches = batch::build_batches(&machine_ids, &effective_sizes);

    // Generate rollout ID
    let rollout_id = format!("r-{}", uuid::Uuid::new_v4());

    // Persist rollout
    let target_tags = match &req.target {
        RolloutTarget::Tags(tags) => Some(serde_json::to_string(tags).unwrap_or_default()),
        RolloutTarget::Hosts(_) => None,
    };
    let target_hosts = match &req.target {
        RolloutTarget::Tags(_) => None,
        RolloutTarget::Hosts(hosts) => Some(serde_json::to_string(hosts).unwrap_or_default()),
    };
    let batch_sizes_json = serde_json::to_string(&effective_sizes).unwrap_or_default();
    let health_timeout = req.health_timeout.unwrap_or(300) as i64;

    let actor_id = actor.identifier();

    db.create_rollout(
        &rollout_id,
        &req.release_id,
        cache_url.as_deref(),
        &req.strategy.to_string(),
        &batch_sizes_json,
        &req.failure_threshold,
        &req.on_failure.to_string(),
        health_timeout,
        target_tags.as_deref(),
        target_hosts.as_deref(),
        &actor_id,
    )
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create rollout");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create rollout".to_string(),
        )
    })?;

    // Persist batches
    let mut batch_summaries = Vec::new();
    for (i, batch_machines) in batches.iter().enumerate() {
        let batch_id = format!("{}-b{}", rollout_id, i);
        let machine_ids_json = serde_json::to_string(batch_machines).unwrap_or_default();
        db.create_rollout_batch(&batch_id, &rollout_id, i as i64, &machine_ids_json)
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to create rollout batch");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create rollout batch".to_string(),
                )
            })?;

        batch_summaries.push(BatchSummary {
            batch_index: i as u32,
            machine_ids: batch_machines.clone(),
            status: BatchStatus::Pending,
        });
    }

    // Set rollout status to running
    db.update_rollout_status(&rollout_id, "running")
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update rollout status");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update rollout status".to_string(),
            )
        })?;

    // Emit rollout events
    let _ = db.insert_rollout_event(
        &rollout_id,
        "status_change",
        &format!(
            "{{\"from\":\"created\",\"to\":\"running\",\"strategy\":\"{}\"}}",
            req.strategy
        ),
        &actor_id,
    );

    // Audit log
    let total_machines = machine_ids.len();
    let _ = db.insert_audit_event(
        &actor_id,
        "rollout.created",
        &rollout_id,
        Some(&format!(
            "strategy={} machines={} batches={}",
            req.strategy,
            total_machines,
            batches.len()
        )),
    );

    tracing::info!(
        rollout_id = %rollout_id,
        strategy = %req.strategy,
        machines = total_machines,
        batches = batches.len(),
        "Rollout created"
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateRolloutResponse {
            rollout_id,
            batches: batch_summaries,
            total_machines,
        }),
    ))
}

/// GET /api/v1/rollouts?status=running
///
/// List rollouts with optional status filter.
pub async fn list_rollouts(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Query(query): Query<ListRolloutsQuery>,
) -> Result<Json<Vec<RolloutDetail>>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "readonly, deploy, or admin role required".to_string(),
        ));
    }

    let rollouts = db
        .list_rollouts_by_status(query.status.as_deref(), 100)
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list rollouts");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list rollouts".to_string(),
            )
        })?;

    let mut details = Vec::new();
    for rollout in rollouts {
        let batches = db.get_rollout_batches(&rollout.id).map_err(|e| {
            tracing::error!(error = %e, rollout_id = %rollout.id, "Failed to get batches");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get rollout batches".to_string(),
            )
        })?;
        details.push(row_to_detail(&rollout, &batches));
    }

    Ok(Json(details))
}

/// GET /api/v1/rollouts/{id}
///
/// Get a single rollout by ID.
pub async fn get_rollout(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<String>,
) -> Result<Json<RolloutDetail>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "readonly, deploy, or admin role required".to_string(),
        ));
    }

    let rollout = db
        .get_rollout(&id)
        .map_err(|e| {
            tracing::error!(error = %e, rollout_id = %id, "Failed to get rollout");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get rollout".to_string(),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("rollout not found: {id}")))?;

    let batches = db.get_rollout_batches(&rollout.id).map_err(|e| {
        tracing::error!(error = %e, rollout_id = %id, "Failed to get batches");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to get rollout batches".to_string(),
        )
    })?;

    let events = db.get_rollout_events(&rollout.id).unwrap_or_default();

    Ok(Json(row_to_detail_with_events(
        &rollout, &batches, &events,
    )))
}

/// POST /api/v1/rollouts/{id}/resume
///
/// Resume a paused rollout by resetting the failed batch to pending.
pub async fn resume_rollout(
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

    let rollout = db
        .get_rollout(&id)
        .map_err(|e| {
            tracing::error!(error = %e, rollout_id = %id, "Failed to get rollout");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get rollout".to_string(),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("rollout not found: {id}")))?;

    if rollout.status != "paused" {
        return Err((
            StatusCode::CONFLICT,
            format!("rollout is {}, not paused", rollout.status),
        ));
    }

    // Reset the failed batch to pending
    let batches = db.get_rollout_batches(&id).map_err(|e| {
        tracing::error!(error = %e, rollout_id = %id, "Failed to get batches");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to get rollout batches".to_string(),
        )
    })?;

    for batch in &batches {
        if batch.status == "failed" {
            db.update_batch_status(&batch.id, "pending").map_err(|e| {
                tracing::error!(error = %e, batch_id = %batch.id, "Failed to reset batch");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to reset batch".to_string(),
                )
            })?;
        }
    }

    // Set rollout to running
    db.update_rollout_status(&id, "running").map_err(|e| {
        tracing::error!(error = %e, rollout_id = %id, "Failed to update rollout status");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update rollout status".to_string(),
        )
    })?;

    let _ = db.insert_rollout_event(
        &id,
        "status_change",
        "{\"from\":\"paused\",\"to\":\"running\"}",
        &actor.identifier(),
    );
    let _ = db.insert_audit_event(&actor.identifier(), "rollout.resumed", &id, None);

    tracing::info!(rollout_id = %id, "Rollout resumed");
    Ok(StatusCode::OK)
}

/// POST /api/v1/rollouts/{id}/cancel
///
/// Cancel an active rollout.
pub async fn cancel_rollout(
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

    let rollout = db
        .get_rollout(&id)
        .map_err(|e| {
            tracing::error!(error = %e, rollout_id = %id, "Failed to get rollout");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get rollout".to_string(),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("rollout not found: {id}")))?;

    let status = RolloutStatus::from_str_lc(&rollout.status).ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("invalid rollout status: {}", rollout.status),
    ))?;

    if !status.is_active() {
        return Err((
            StatusCode::CONFLICT,
            format!("rollout is {}, cannot cancel", rollout.status),
        ));
    }

    db.update_rollout_status(&id, "cancelled").map_err(|e| {
        tracing::error!(error = %e, rollout_id = %id, "Failed to cancel rollout");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to cancel rollout".to_string(),
        )
    })?;

    let _ = db.insert_rollout_event(
        &id,
        "status_change",
        &format!("{{\"from\":\"{}\",\"to\":\"cancelled\"}}", rollout.status),
        &actor.identifier(),
    );
    let _ = db.insert_audit_event(&actor.identifier(), "rollout.cancelled", &id, None);

    tracing::info!(rollout_id = %id, "Rollout cancelled");
    Ok(StatusCode::OK)
}

/// Convert database rows into a RolloutDetail response type.
fn row_to_detail(rollout: &RolloutRow, batch_rows: &[RolloutBatchRow]) -> RolloutDetail {
    row_to_detail_with_events(rollout, batch_rows, &[])
}

/// Convert database rows into a RolloutDetail response type, with events.
fn row_to_detail_with_events(
    rollout: &RolloutRow,
    batch_rows: &[RolloutBatchRow],
    event_rows: &[crate::db::RolloutEventRow],
) -> RolloutDetail {
    let strategy: RolloutStrategy = serde_json::from_str(&format!("\"{}\"", rollout.strategy))
        .unwrap_or(RolloutStrategy::Staged);

    let on_failure: OnFailure =
        serde_json::from_str(&format!("\"{}\"", rollout.on_failure)).unwrap_or_default();

    let status: RolloutStatus =
        serde_json::from_str(&format!("\"{}\"", rollout.status)).unwrap_or(RolloutStatus::Created);

    let created_at = parse_datetime(&rollout.created_at);
    let updated_at = parse_datetime(&rollout.updated_at);

    let batches = batch_rows
        .iter()
        .map(|b| {
            let machine_ids: Vec<String> = serde_json::from_str(&b.machine_ids).unwrap_or_default();

            let batch_status: BatchStatus =
                serde_json::from_str(&format!("\"{}\"", b.status)).unwrap_or(BatchStatus::Pending);

            let inferred_status = match batch_status {
                BatchStatus::Succeeded => MachineHealthStatus::Healthy,
                BatchStatus::Failed => MachineHealthStatus::Unhealthy("batch failed".to_string()),
                _ => MachineHealthStatus::Pending,
            };
            let machine_health: HashMap<String, MachineHealthStatus> = machine_ids
                .iter()
                .map(|id| (id.clone(), inferred_status.clone()))
                .collect();

            BatchDetail {
                batch_index: b.batch_index as u32,
                machine_ids,
                status: batch_status,
                machine_health,
                started_at: b.started_at.as_ref().map(|s| parse_datetime(s)),
                completed_at: b.completed_at.as_ref().map(|s| parse_datetime(s)),
            }
        })
        .collect();

    let events: Vec<RolloutEvent> = event_rows
        .iter()
        .map(|e| RolloutEvent {
            id: e.id,
            rollout_id: e.rollout_id.clone(),
            event_type: e.event_type.clone(),
            detail: e.detail.clone(),
            actor: e.actor.clone(),
            created_at: parse_datetime(&e.created_at),
        })
        .collect();

    RolloutDetail {
        id: rollout.id.clone(),
        status,
        strategy,
        release_id: rollout.release_id.clone(),
        on_failure,
        failure_threshold: rollout.failure_threshold.clone(),
        health_timeout: rollout.health_timeout as u64,
        batches,
        created_at,
        updated_at,
        created_by: rollout.created_by.clone(),
        events,
    }
}

/// Parse a SQLite datetime string into a chrono DateTime<Utc>.
fn parse_datetime(s: &str) -> chrono::DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|dt| Utc.from_utc_datetime(&dt))
        .unwrap_or_else(|e| {
            tracing::warn!(input = %s, error = %e, "Failed to parse datetime, falling back to now");
            Utc::now()
        })
}
