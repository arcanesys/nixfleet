use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use tracing::info;
use uuid::Uuid;

use crate::auth::Actor;
use crate::{AppState, MAX_ID_LEN, MAX_PAGE_SIZE};
use nixfleet_types::release::{
    CreateReleaseRequest, CreateReleaseResponse, Release, ReleaseDiff, ReleaseDiffEntry,
    ReleaseEntry,
};

pub async fn create_release(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Json(req): Json<CreateReleaseRequest>,
) -> Result<(StatusCode, Json<CreateReleaseResponse>), (StatusCode, String)> {
    if !actor.has_role(&["deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "requires deploy or admin role".into(),
        ));
    }
    if req.entries.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "entries must not be empty".into()));
    }

    let id = format!("rel-{}", Uuid::new_v4());
    let host_count = req.entries.len() as i64;
    let actor_name = actor.identifier();

    // Serialize tags. Vec<String> cannot realistically fail to
    // serialize, but we propagate the error instead of silently
    // coercing to `"[]"` so a corrupted entry surfaces as a 500.
    let entries: Vec<(String, String, String, String)> = req
        .entries
        .iter()
        .map(|e| {
            let tags_json = serde_json::to_string(&e.tags).map_err(|err| {
                tracing::error!(hostname = %e.hostname, error = %err, "failed to serialize release tags");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to serialize release tags".to_string(),
                )
            })?;
            Ok((
                e.hostname.clone(),
                e.store_path.clone(),
                e.platform.clone(),
                tags_json,
            ))
        })
        .collect::<Result<Vec<_>, (StatusCode, String)>>()?;

    db.create_release(
        &id,
        req.flake_ref.as_deref(),
        req.flake_rev.as_deref(),
        req.cache_url.as_deref(),
        host_count,
        &actor_name,
        &entries,
    )
    .map_err(|e| {
        tracing::error!(error = %e, release_id = %id, "failed to create release");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create release".to_string(),
        )
    })?;

    db.insert_audit_event(
        &actor_name,
        "create_release",
        &id,
        Some(&format!("{} hosts", host_count)),
    )
    .map_err(|e| {
        tracing::error!(error = %e, "failed to insert audit event for release creation");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?;

    info!(release_id = %id, host_count, "release created");

    Ok((
        StatusCode::CREATED,
        Json(CreateReleaseResponse {
            id,
            host_count: host_count as usize,
        }),
    ))
}

pub async fn list_releases(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<Release>>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "requires readonly, deploy, or admin role".into(),
        ));
    }
    // Cap the requested page size at MAX_PAGE_SIZE so a rogue client
    // cannot trigger a full-table scan via `?limit=99999999`.
    let limit: i64 = params
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(20)
        .clamp(1, MAX_PAGE_SIZE);

    let rows = if let Some(hostname) = params.get("host") {
        db.list_releases_for_host(hostname, limit).map_err(|e| {
            tracing::error!(error = %e, "failed to list releases for host");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list releases".to_string(),
            )
        })?
    } else {
        db.list_releases(limit).map_err(|e| {
            tracing::error!(error = %e, "failed to list releases");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list releases".to_string(),
            )
        })?
    };

    let mut releases = Vec::with_capacity(rows.len());
    for row in rows {
        let entry_rows = db.get_release_entries(&row.id).map_err(|e| {
            tracing::error!(error = %e, release_id = %row.id, "failed to get release entries");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get release entries".to_string(),
            )
        })?;
        releases.push(row_to_release(row, entry_rows));
    }

    Ok(Json(releases))
}

pub async fn get_release(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<String>,
) -> Result<Json<Release>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "requires readonly, deploy, or admin role".into(),
        ));
    }
    if id.len() > MAX_ID_LEN {
        return Err((StatusCode::BAD_REQUEST, "release id too long".into()));
    }
    let row = db
        .get_release(&id)
        .map_err(|e| {
            tracing::error!(error = %e, release_id = %id, "failed to get release");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get release".to_string(),
            )
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("release {} not found", id)))?;

    let entry_rows = db.get_release_entries(&id).map_err(|e| {
        tracing::error!(error = %e, release_id = %id, "failed to get release entries");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to get release entries".to_string(),
        )
    })?;

    Ok(Json(row_to_release(row, entry_rows)))
}

pub async fn diff_releases(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path((id_a, id_b)): Path<(String, String)>,
) -> Result<Json<ReleaseDiff>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((
            StatusCode::FORBIDDEN,
            "requires readonly, deploy, or admin role".into(),
        ));
    }
    if id_a.len() > MAX_ID_LEN || id_b.len() > MAX_ID_LEN {
        return Err((StatusCode::BAD_REQUEST, "release id too long".into()));
    }
    let entries_a = db.get_release_entries(&id_a).map_err(|e| {
        tracing::error!(error = %e, release_id = %id_a, "failed to get release entries");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to get release entries".to_string(),
        )
    })?;
    let entries_b = db.get_release_entries(&id_b).map_err(|e| {
        tracing::error!(error = %e, release_id = %id_b, "failed to get release entries");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to get release entries".to_string(),
        )
    })?;

    if entries_a.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("release {} not found or empty", id_a),
        ));
    }
    if entries_b.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("release {} not found or empty", id_b),
        ));
    }

    let map_a: std::collections::HashMap<&str, &str> = entries_a
        .iter()
        .map(|e| (e.hostname.as_str(), e.store_path.as_str()))
        .collect();
    let map_b: std::collections::HashMap<&str, &str> = entries_b
        .iter()
        .map(|e| (e.hostname.as_str(), e.store_path.as_str()))
        .collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();

    for (host, path_a) in &map_a {
        match map_b.get(host) {
            Some(path_b) if path_a != path_b => {
                changed.push(ReleaseDiffEntry {
                    hostname: host.to_string(),
                    old_store_path: path_a.to_string(),
                    new_store_path: path_b.to_string(),
                });
            }
            Some(_) => unchanged.push(host.to_string()),
            None => removed.push(host.to_string()),
        }
    }
    for host in map_b.keys() {
        if !map_a.contains_key(host) {
            added.push(host.to_string());
        }
    }

    added.sort();
    removed.sort();
    unchanged.sort();
    changed.sort_by(|a, b| a.hostname.cmp(&b.hostname));

    Ok(Json(ReleaseDiff {
        added,
        removed,
        changed,
        unchanged,
    }))
}

pub async fn delete_release(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !actor.has_role(&["admin"]) {
        return Err((StatusCode::FORBIDDEN, "requires admin role".into()));
    }
    if id.len() > MAX_ID_LEN {
        return Err((StatusCode::BAD_REQUEST, "release id too long".into()));
    }
    let referenced = db.release_referenced_by_rollout(&id).map_err(|e| {
        tracing::error!(error = %e, release_id = %id, "failed to check rollout references");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?;
    if referenced {
        return Err((
            StatusCode::CONFLICT,
            format!("release {} is referenced by a rollout", id),
        ));
    }

    let deleted = db.delete_release(&id).map_err(|e| {
        tracing::error!(error = %e, release_id = %id, "failed to delete release");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to delete release".to_string(),
        )
    })?;
    if !deleted {
        return Err((StatusCode::NOT_FOUND, format!("release {} not found", id)));
    }

    db.insert_audit_event(&actor.identifier(), "delete_release", &id, None)
        .map_err(|e| {
            tracing::error!(error = %e, "failed to insert audit event for release deletion");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            )
        })?;

    info!(release_id = %id, "release deleted");
    Ok(StatusCode::NO_CONTENT)
}

fn parse_datetime(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|dt| chrono::TimeZone::from_utc_datetime(&chrono::Utc, &dt))
}

fn row_to_release(
    row: crate::db::ReleaseRow,
    entry_rows: Vec<crate::db::ReleaseEntryRow>,
) -> Release {
    let entries = entry_rows
        .into_iter()
        .map(|e| ReleaseEntry {
            hostname: e.hostname,
            store_path: e.store_path,
            platform: e.platform,
            tags: serde_json::from_str(&e.tags).unwrap_or_default(),
        })
        .collect();

    Release {
        id: row.id,
        flake_ref: row.flake_ref,
        flake_rev: row.flake_rev,
        cache_url: row.cache_url,
        host_count: row.host_count as usize,
        entries,
        created_at: parse_datetime(&row.created_at).unwrap_or_else(chrono::Utc::now),
        created_by: row.created_by,
    }
}
