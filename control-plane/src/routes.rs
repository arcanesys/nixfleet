use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use nixfleet_types::{DesiredGeneration, MachineLifecycle, MachineStatus, Report};
use serde::{Deserialize, Serialize};

use crate::auth::Actor;
use crate::{log_insert_err, AppState, MAX_ID_LEN};

/// GET /api/v1/machines/{id}/desired-generation
///
/// Returns the desired generation for a machine, or 404 if not set.
pub async fn get_desired_generation(
    State((state, db)): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DesiredGeneration>, StatusCode> {
    if id.len() > MAX_ID_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    let fleet = state.read().await;
    let mut gen = fleet
        .machines
        .get(&id)
        .and_then(|m| m.desired_generation.clone())
        .ok_or(StatusCode::NOT_FOUND)?;

    // Hint faster polling when the machine is part of an active rollout
    if let Ok(Some(_)) = db.machine_in_active_rollout(&id) {
        gen.poll_hint = Some(5);
    }

    Ok(Json(gen))
}

/// POST /api/v1/machines/{id}/report
///
/// Receives a status report from an agent.
///
/// This is an agent-facing endpoint, authenticated via mTLS at the transport layer.
/// No bearer token is required. The actor is derived from the machine ID in the path.
pub async fn post_report(
    State((state, db)): State<AppState>,
    Path(id): Path<String>,
    Json(report): Json<Report>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Validate path and report fields before any DB writes
    if id.len() > MAX_ID_LEN {
        return Err((StatusCode::BAD_REQUEST, "machine_id too long".to_string()));
    }
    if report.current_generation.len() > 512 {
        return Err((
            StatusCode::BAD_REQUEST,
            "current_generation too long".to_string(),
        ));
    }
    if report.message.len() > 4096 {
        return Err((StatusCode::BAD_REQUEST, "message too long".to_string()));
    }

    let report_success = report.success;

    // Persist to database
    db.insert_report(
        &id,
        &report.current_generation,
        report.success,
        &report.message,
    )
    .map_err(|e| {
        tracing::error!(error = %e, machine_id = %id, "Failed to persist report");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to persist report".to_string(),
        )
    })?;

    // Persist health report if present. Serialization of a well-typed
    // Vec<HealthCheckResult> cannot realistically fail, but we still
    // propagate the error as a 500 rather than silently storing "".
    if let Some(ref health) = report.health {
        let results_json = serde_json::to_string(&health.results).map_err(|e| {
            tracing::error!(error = %e, machine_id = %id, "Failed to serialize health results");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to serialize health results".to_string(),
            )
        })?;
        log_insert_err(
            "health_report",
            db.insert_health_report(&id, &results_json, health.all_passed),
        );
    }

    // Update in-memory state — auto-register unknown machines on first report
    let mut fleet = state.write().await;
    let is_new = !fleet.machines.contains_key(&id);
    let machine = fleet.get_or_create(&id);
    machine.last_received = Some(chrono::Utc::now());
    machine.last_report = Some(report);
    machine.agent_version = machine
        .last_report
        .as_ref()
        .map(|r| r.agent_version.clone())
        .unwrap_or_default();
    machine.uptime_seconds = machine
        .last_report
        .as_ref()
        .map(|r| r.uptime_seconds)
        .unwrap_or(0);

    // Auto-register: persist to DB on first report from unknown machine
    if is_new {
        log_insert_err("register_machine", db.register_machine(&id, "active"));
        machine.lifecycle = MachineLifecycle::Active;
        machine.registered_at = Some(chrono::Utc::now());
        tracing::info!(machine_id = %id, "Auto-registered on first report");
    }

    // Sync tags from agent report if they changed
    if let Some(ref report) = machine.last_report {
        if !report.tags.is_empty() && machine.tags != report.tags {
            machine.tags = report.tags.clone();
            log_insert_err("set_machine_tags", db.set_machine_tags(&id, &report.tags));
            tracing::info!(machine_id = %id, tags = ?report.tags, "Tags synced from report");
        }
    }

    // Auto-transition Pending/Provisioning -> Active on first report
    if machine.lifecycle == MachineLifecycle::Pending
        || machine.lifecycle == MachineLifecycle::Provisioning
    {
        machine.lifecycle = MachineLifecycle::Active;
        log_insert_err(
            "set_machine_lifecycle",
            db.set_machine_lifecycle(&id, "active"),
        );
        tracing::info!(machine_id = %id, "Auto-activated on first report");
    }

    // Persist runtime state so it survives CP restarts.
    // Placed after auto-register so the machine row exists before the upsert
    // (first-report INSERT race: the machine must be in machines table first).
    // Read current_generation from machine.last_report (report was moved above).
    let health_status = if report_success { "ok" } else { "error" };
    if let Some(ref last) = machine.last_report {
        crate::log_insert_err(
            "machine_state",
            db.upsert_machine_state(&id, &last.current_generation, health_status),
        );
    }

    let actor_id = format!("machine:{id}");
    let detail = if report_success { "success" } else { "failure" };
    log_insert_err(
        "audit_event",
        db.insert_audit_event(&actor_id, "report", &id, Some(detail)),
    );

    // Update fleet gauges while we still hold the write guard — avoids
    // racing with another writer and skips a second lock acquisition.
    // `update_fleet_gauges` takes `&FleetState`, which auto-derefs from
    // the write guard.
    crate::metrics::update_fleet_gauges(&fleet);
    drop(fleet);

    tracing::info!(machine_id = %id, "Report received");

    Ok(StatusCode::OK)
}

/// Query parameters for listing machines.
#[derive(Debug, Deserialize)]
pub struct ListMachinesQuery {
    #[serde(default)]
    pub tag: Vec<String>,
}

/// GET /api/v1/machines
///
/// List all known machines with their current status.
/// Supports optional `?tag=X&tag=Y` filtering (AND logic).
pub async fn list_machines(
    State((state, _db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Query(query): Query<ListMachinesQuery>,
) -> Result<Json<Vec<MachineStatus>>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "readonly, deploy, or admin role required".to_string(),
        ));
    }

    let fleet = state.read().await;
    let machines: Vec<MachineStatus> = fleet
        .machines
        .iter()
        .map(|(id, m)| MachineStatus {
            machine_id: id.clone(),
            current_generation: m
                .last_report
                .as_ref()
                .map(|r| r.current_generation.clone())
                .unwrap_or_default(),
            desired_generation: m.desired_generation.as_ref().map(|d| d.hash.clone()),
            agent_version: m.agent_version.clone(),
            system_state: m
                .last_report
                .as_ref()
                .map(|r| {
                    if r.success {
                        "ok".to_string()
                    } else {
                        "error".to_string()
                    }
                })
                .unwrap_or_else(|| "unknown".to_string()),
            uptime_seconds: m.uptime_seconds,
            last_report: m.last_report.as_ref().map(|r| r.timestamp),
            lifecycle: m.lifecycle.clone(),
            tags: m.tags.clone(),
            seconds_since_last_report: m
                .last_received
                .map(|t| (chrono::Utc::now() - t).num_seconds().max(0) as u64),
        })
        .filter(|m| {
            if query.tag.is_empty() {
                true
            } else {
                query.tag.iter().all(|t| m.tags.contains(t))
            }
        })
        .collect();

    Ok(Json(machines))
}

/// Request body for registering a machine.
#[derive(Debug, Deserialize)]
pub struct RegisterMachineRequest {
    /// Optional initial lifecycle state. Defaults to "active" because the
    /// common case for operator-driven registration is onboarding a
    /// known-good machine into the fleet. Callers who want to reserve
    /// an identifier for hardware that is not yet online can pass
    /// `lifecycle: "pending"` explicitly.
    #[serde(default = "default_active")]
    pub lifecycle: String,
    /// Optional initial tags for the machine.
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_active() -> String {
    "active".to_string()
}

/// Response for registration.
#[derive(Debug, Serialize)]
pub struct RegisterMachineResponse {
    pub machine_id: String,
    pub lifecycle: String,
}

/// POST /api/v1/machines/{id}/register
///
/// Pre-register a machine in the fleet (admin endpoint).
pub async fn register_machine(
    State((state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<String>,
    Json(req): Json<RegisterMachineRequest>,
) -> Result<(StatusCode, Json<RegisterMachineResponse>), (StatusCode, String)> {
    if id.len() > MAX_ID_LEN {
        return Err((StatusCode::BAD_REQUEST, "machine ID too long".to_string()));
    }
    if !actor.has_role(&["admin"]) {
        return Err((StatusCode::FORBIDDEN, "admin role required".to_string()));
    }

    let lifecycle = MachineLifecycle::from_str_lc(&req.lifecycle).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid lifecycle state: {}", req.lifecycle),
        )
    })?;

    // Persist to database
    db.register_machine(&id, &lifecycle.to_string())
        .map_err(|e| {
            tracing::error!(error = %e, machine_id = %id, "Failed to register machine");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to register machine".to_string(),
            )
        })?;

    // Persist tags if provided
    if !req.tags.is_empty() {
        db.set_machine_tags(&id, &req.tags).map_err(|e| {
            tracing::error!(error = %e, machine_id = %id, "Failed to set tags on register");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set tags".to_string(),
            )
        })?;
    }

    // Update in-memory state
    let mut fleet = state.write().await;
    let machine = fleet.get_or_create(&id);
    machine.lifecycle = lifecycle.clone();
    if machine.registered_at.is_none() {
        machine.registered_at = Some(chrono::Utc::now());
    }
    if !req.tags.is_empty() {
        machine.tags = req.tags;
    }

    log_insert_err(
        "audit_event",
        db.insert_audit_event(
            &actor.identifier(),
            "register",
            &id,
            Some(&lifecycle.to_string()),
        ),
    );

    tracing::info!(machine_id = %id, lifecycle = %lifecycle, "Machine registered");

    // Update fleet gauges while still under the write guard.
    crate::metrics::update_fleet_gauges(&fleet);
    drop(fleet);

    Ok((
        StatusCode::CREATED,
        Json(RegisterMachineResponse {
            machine_id: id,
            lifecycle: lifecycle.to_string(),
        }),
    ))
}

/// Request body for changing lifecycle state.
#[derive(Debug, Deserialize)]
pub struct UpdateLifecycleRequest {
    pub lifecycle: String,
}

/// PATCH /api/v1/machines/{id}/lifecycle
///
/// Change a machine's lifecycle state (admin endpoint).
pub async fn update_lifecycle(
    State((state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<String>,
    Json(req): Json<UpdateLifecycleRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if id.len() > MAX_ID_LEN {
        return Err((StatusCode::BAD_REQUEST, "machine ID too long".to_string()));
    }
    if !actor.has_role(&["admin"]) {
        return Err((StatusCode::FORBIDDEN, "admin role required".to_string()));
    }

    let target = MachineLifecycle::from_str_lc(&req.lifecycle).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid lifecycle state: {}", req.lifecycle),
        )
    })?;

    let mut fleet = state.write().await;
    let machine = fleet
        .machines
        .get_mut(&id)
        .ok_or((StatusCode::NOT_FOUND, format!("machine not found: {id}")))?;

    if !machine.lifecycle.can_transition_to(&target) {
        return Err((
            StatusCode::CONFLICT,
            format!("invalid transition: {} -> {}", machine.lifecycle, target),
        ));
    }

    // Persist to database
    db.set_machine_lifecycle(&id, &target.to_string())
        .map_err(|e| {
            tracing::error!(error = %e, machine_id = %id, "Failed to update lifecycle");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update lifecycle".to_string(),
            )
        })?;

    let from = machine.lifecycle.to_string();
    machine.lifecycle = target.clone();

    log_insert_err(
        "audit_event",
        db.insert_audit_event(
            &actor.identifier(),
            "update_lifecycle",
            &id,
            Some(&format!("{from} -> {target}")),
        ),
    );

    tracing::info!(
        machine_id = %id,
        from = %from,
        to = %target,
        "Lifecycle state changed"
    );

    // Update fleet gauges while still under the write guard.
    crate::metrics::update_fleet_gauges(&fleet);
    drop(fleet);

    Ok(StatusCode::OK)
}

/// POST /api/v1/keys/bootstrap
///
/// Create the first admin API key. Only works when no keys exist (first-time setup).
/// Returns 409 Conflict if keys already exist.
pub async fn bootstrap_api_key(
    State((_, db)): State<AppState>,
    Json(req): Json<BootstrapKeyRequest>,
) -> Result<Json<BootstrapKeyResponse>, (StatusCode, String)> {
    if db.has_api_keys().map_err(|e| {
        tracing::error!(error = %e, "failed to check existing API keys");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?
    {
        return Err((
            StatusCode::CONFLICT,
            "API keys already exist. Use an admin key to create more.".to_string(),
        ));
    }

    let raw_key = format!("nfk-{}", generate_random_key());
    let key_hash = crate::auth::hash_key(&raw_key);

    let name = if req.name.is_empty() {
        "admin"
    } else {
        &req.name
    };

    db.insert_api_key(&key_hash, name, "admin").map_err(|e| {
        tracing::error!(error = %e, "failed to create bootstrap API key");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create key".to_string(),
        )
    })?;

    log_insert_err(
        "audit_event",
        db.insert_audit_event(
            "system:bootstrap",
            "bootstrap",
            name,
            Some("first admin key created"),
        ),
    );

    tracing::info!(name = %name, "Bootstrap API key created");

    Ok(Json(BootstrapKeyResponse {
        key: raw_key,
        name: name.to_string(),
        role: "admin".to_string(),
    }))
}

fn generate_random_key() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    hex::encode(bytes)
}

#[derive(serde::Deserialize)]
pub struct BootstrapKeyRequest {
    #[serde(default)]
    pub name: String,
}

#[derive(serde::Serialize)]
pub struct BootstrapKeyResponse {
    pub key: String,
    pub name: String,
    pub role: String,
}

/// Request body for notifying an SSH deploy.
#[derive(Debug, Deserialize)]
pub struct NotifyDeployRequest {
    pub store_path: String,
}

/// POST /api/v1/machines/{id}/notify-deploy — notify the CP of an SSH deploy.
///
/// Sets both desired_generation (DB + fleet state) and current_generation
/// (fleet state only — the agent will confirm on its next report).
/// Used by `nixfleet deploy --ssh` to keep the CP in sync.
pub async fn notify_deploy(
    State((state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<String>,
    Json(body): Json<NotifyDeployRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if id.len() > MAX_ID_LEN {
        return Err((StatusCode::BAD_REQUEST, "machine ID too long".to_string()));
    }
    if !actor.has_role(&["admin", "deploy"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "admin or deploy role required".to_string(),
        ));
    }

    db.set_desired_generation(&id, &body.store_path)
        .map_err(|e| {
            tracing::error!(error = %e, machine_id = %id, "failed to set desired generation");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            )
        })?;

    let mut fleet = state.write().await;
    let machine = fleet.get_or_create(&id);
    machine.desired_generation = Some(DesiredGeneration {
        hash: body.store_path.clone(),
        cache_url: None,
        poll_hint: None,
    });

    crate::log_insert_err(
        "audit_event",
        db.insert_audit_event(
            &actor.identifier(),
            "notify_deploy",
            &id,
            Some(&body.store_path),
        ),
    );
    drop(fleet);

    tracing::info!(machine_id = %id, store_path = %body.store_path, "SSH deploy notified");
    Ok(StatusCode::OK)
}

/// DELETE /api/v1/machines/{id}/desired-generation
pub async fn clear_desired_generation(
    State((state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if id.len() > MAX_ID_LEN {
        return Err((StatusCode::BAD_REQUEST, "machine ID too long".to_string()));
    }
    if !actor.has_role(&["admin"]) {
        return Err((StatusCode::FORBIDDEN, "admin role required".to_string()));
    }

    // Check machine exists via in-memory fleet state (O(1) lookup)
    {
        let fleet = state.read().await;
        if !fleet.machines.contains_key(&id) {
            return Err((StatusCode::NOT_FOUND, format!("machine {id} not found")));
        }
    }

    db.clear_desired_generation(&id).map_err(|e| {
        tracing::error!(error = %e, machine_id = %id, "Failed to clear desired generation");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?;

    // Update in-memory state
    let mut fleet = state.write().await;
    let machine = fleet.machines.get_mut(&id).ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "machine state inconsistency".to_string(),
        )
    })?;
    machine.desired_generation = None;
    crate::log_insert_err(
        "audit_event",
        db.insert_audit_event(&actor.identifier(), "clear_desired", &id, None),
    );
    drop(fleet);

    tracing::info!(machine_id = %id, "Desired generation cleared");
    Ok(StatusCode::NO_CONTENT)
}
