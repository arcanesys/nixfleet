use axum::extract::{Query, State};
use axum::http::header;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use nixfleet_types::AuditEvent;
use serde::Deserialize;

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub actor: Option<String>,
    pub action: Option<String>,
    pub target: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// GET /api/v1/audit
pub async fn list_audit_events(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<crate::auth::Actor>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEvent>>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((StatusCode::FORBIDDEN, "insufficient role".to_string()));
    }
    let events = db
        .query_audit_events(
            query.actor.as_deref(),
            query.action.as_deref(),
            query.target.as_deref(),
            query.limit,
        )
        .unwrap_or_default();
    Ok(Json(events))
}

/// Escape a CSV field to prevent formula injection in spreadsheet software.
fn escape_csv_field(field: &str) -> String {
    if field.starts_with('=')
        || field.starts_with('+')
        || field.starts_with('-')
        || field.starts_with('@')
        || field.starts_with('\t')
        || field.starts_with('\r')
    {
        format!("'{field}")
    } else {
        field.to_string()
    }
}

/// GET /api/v1/audit/export
pub async fn export_audit_csv(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<crate::auth::Actor>,
    Query(query): Query<AuditQuery>,
) -> Response {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return (StatusCode::FORBIDDEN, "insufficient role").into_response();
    }
    let events = match db.query_audit_events(
        query.actor.as_deref(),
        query.action.as_deref(),
        query.target.as_deref(),
        query.limit,
    ) {
        Ok(e) => e,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let mut csv = String::from("timestamp,actor,action,target,detail\n");
    for event in &events {
        csv.push_str(&format!(
            "{},{},{},{},{}\n",
            event.timestamp,
            escape_csv_field(&event.actor),
            escape_csv_field(&event.action),
            escape_csv_field(&event.target),
            escape_csv_field(event.detail.as_deref().unwrap_or("")),
        ));
    }

    (
        [
            (header::CONTENT_TYPE, "text/csv"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"audit.csv\"",
            ),
        ],
        csv,
    )
        .into_response()
}
