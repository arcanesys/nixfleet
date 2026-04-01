use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use nixfleet_types::{DesiredGeneration, MachineLifecycle, MachineStatus, Report};
use serde::{Deserialize, Serialize};

use crate::auth::Actor;
use crate::AppState;

/// GET /api/v1/machines/{id}/desired-generation
///
/// Returns the desired generation for a machine, or 404 if not set.
pub async fn get_desired_generation(
    State((state, _db)): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DesiredGeneration>, StatusCode> {
    let fleet = state.read().await;
    fleet
        .machines
        .get(&id)
        .and_then(|m| m.desired_generation.clone())
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// POST /api/v1/machines/{id}/report
///
/// Receives a status report from an agent.
pub async fn post_report(
    State((state, db)): State<AppState>,
    actor: Option<Extension<Actor>>,
    Path(id): Path<String>,
    Json(report): Json<Report>,
) -> Result<StatusCode, (StatusCode, String)> {
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

    // Update in-memory state
    let mut fleet = state.write().await;
    let machine = fleet.get_or_create(&id);
    machine.last_seen = Some(report.timestamp);
    machine.last_report = Some(report);

    // Auto-transition Pending/Provisioning -> Active on first report
    if machine.lifecycle == MachineLifecycle::Pending
        || machine.lifecycle == MachineLifecycle::Provisioning
    {
        machine.lifecycle = MachineLifecycle::Active;
        // Persist lifecycle change — best-effort (report already saved)
        let _ = db.set_machine_lifecycle(&id, "active");
        tracing::info!(machine_id = %id, "Auto-activated on first report");
    }

    let actor_id = actor
        .map(|Extension(a)| a.identifier())
        .unwrap_or_else(|| format!("machine:{id}"));
    let detail = if report_success { "success" } else { "failure" };
    let _ = db.insert_audit_event(&actor_id, "report", &id, Some(detail));

    tracing::info!(machine_id = %id, "Report received");
    Ok(StatusCode::OK)
}

/// Request body for setting a desired generation.
#[derive(Debug, Deserialize)]
pub struct SetGenerationRequest {
    pub hash: String,
    #[serde(default)]
    pub cache_url: Option<String>,
}

/// POST /api/v1/machines/{id}/set-generation
///
/// Admin endpoint to set the desired generation for a machine.
pub async fn set_desired_generation(
    State((state, db)): State<AppState>,
    actor: Option<Extension<Actor>>,
    Path(id): Path<String>,
    Json(req): Json<SetGenerationRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Persist to database
    db.set_desired_generation(&id, &req.hash).map_err(|e| {
        tracing::error!(error = %e, machine_id = %id, "Failed to persist generation");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to persist generation".to_string(),
        )
    })?;

    // Update in-memory state
    let mut fleet = state.write().await;
    let machine = fleet.get_or_create(&id);
    machine.desired_generation = Some(DesiredGeneration {
        hash: req.hash.clone(),
        cache_url: req.cache_url,
    });

    let actor_id = actor
        .map(|Extension(a)| a.identifier())
        .unwrap_or_else(|| "unknown".to_string());
    let _ = db.insert_audit_event(
        &actor_id,
        "set_generation",
        &id,
        Some(&format!("hash={}", req.hash)),
    );

    tracing::info!(
        machine_id = %id,
        hash = %req.hash,
        "Desired generation set"
    );
    Ok(StatusCode::OK)
}

/// GET /api/v1/machines
///
/// List all known machines with their current status.
pub async fn list_machines(State((state, _db)): State<AppState>) -> Json<Vec<MachineStatus>> {
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
            agent_version: String::new(),
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
            uptime_seconds: 0,
            last_report: m.last_report.as_ref().map(|r| r.timestamp),
            lifecycle: m.lifecycle.clone(),
        })
        .collect();

    Json(machines)
}

/// Request body for registering a machine.
#[derive(Debug, Deserialize)]
pub struct RegisterMachineRequest {
    /// Optional initial lifecycle state (defaults to "pending").
    #[serde(default = "default_pending")]
    pub lifecycle: String,
}

fn default_pending() -> String {
    "pending".to_string()
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
    actor: Option<Extension<Actor>>,
    Path(id): Path<String>,
    Json(req): Json<RegisterMachineRequest>,
) -> Result<(StatusCode, Json<RegisterMachineResponse>), (StatusCode, String)> {
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

    // Update in-memory state
    let mut fleet = state.write().await;
    let machine = fleet.get_or_create(&id);
    machine.lifecycle = lifecycle.clone();
    if machine.registered_at.is_none() {
        machine.registered_at = Some(chrono::Utc::now());
    }

    let actor_id = actor
        .map(|Extension(a)| a.identifier())
        .unwrap_or_else(|| "unknown".to_string());
    let _ = db.insert_audit_event(&actor_id, "register", &id, Some(&lifecycle.to_string()));

    tracing::info!(machine_id = %id, lifecycle = %lifecycle, "Machine registered");
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
    actor: Option<Extension<Actor>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateLifecycleRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
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

    let actor_id = actor
        .map(|Extension(a)| a.identifier())
        .unwrap_or_else(|| "unknown".to_string());
    let _ = db.insert_audit_event(
        &actor_id,
        "update_lifecycle",
        &id,
        Some(&format!("{from} -> {target}")),
    );

    tracing::info!(
        machine_id = %id,
        from = %from,
        to = %target,
        "Lifecycle state changed"
    );
    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_generation_request_deserialization() {
        let json = r#"{"hash": "/nix/store/abc123-nixos-system"}"#;
        let req: SetGenerationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.hash, "/nix/store/abc123-nixos-system");
        assert!(req.cache_url.is_none());
    }

    #[test]
    fn test_set_generation_request_with_cache_url() {
        let json = r#"{"hash": "/nix/store/abc123", "cache_url": "https://cache.example.com"}"#;
        let req: SetGenerationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.hash, "/nix/store/abc123");
        assert_eq!(req.cache_url, Some("https://cache.example.com".to_string()));
    }
}
