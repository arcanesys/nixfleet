use axum::extract::{Query, State};
use axum::http::header;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use nixfleet_types::AuditEvent;
use serde::Deserialize;

use crate::{AppState, MAX_PAGE_SIZE};

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

/// Clamp a caller-supplied limit to the fleet-wide ceiling.
fn clamp_limit(requested: usize) -> usize {
    requested.min(MAX_PAGE_SIZE as usize).max(1)
}

/// GET /api/v1/audit
///
/// Errors from the underlying query propagate as 500 so compliance
/// and forensic consumers see a hard failure rather than an empty
/// result masquerading as "no events".
pub async fn list_audit_events(
    State((_state, db)): State<AppState>,
    Extension(actor): Extension<crate::auth::Actor>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEvent>>, (StatusCode, String)> {
    if !actor.has_role(&["readonly", "deploy", "admin"]) {
        return Err((StatusCode::FORBIDDEN, "insufficient role".to_string()));
    }
    let limit = clamp_limit(query.limit);
    let events = db
        .query_audit_events(
            query.actor.as_deref(),
            query.action.as_deref(),
            query.target.as_deref(),
            limit,
        )
        .map_err(|e| {
            tracing::error!(error = %e, "failed to query audit events");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to query audit events".to_string(),
            )
        })?;
    Ok(Json(events))
}

/// Escape a field for RFC 4180-compliant CSV.
///
/// - Fields containing a comma, double-quote, CR, or LF are wrapped
///   in double quotes with inner quotes doubled.
/// - Fields beginning with a spreadsheet formula character (`=`, `+`,
///   `-`, `@`, `\t`, `\r`) are prefixed with a single quote to block
///   formula injection when opened in Excel/LibreOffice/Sheets.
fn escape_csv_field(field: &str) -> String {
    let formula_prefixed = field
        .chars()
        .next()
        .map(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r'))
        .unwrap_or(false);

    let base = if formula_prefixed {
        format!("'{field}")
    } else {
        field.to_string()
    };

    if base.contains(',') || base.contains('"') || base.contains('\n') || base.contains('\r') {
        let escaped = base.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        base
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
    let limit = clamp_limit(query.limit);
    let events = match db.query_audit_events(
        query.actor.as_deref(),
        query.action.as_deref(),
        query.target.as_deref(),
        limit,
    ) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, "failed to query audit events for CSV export");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut csv = String::from("timestamp,actor,action,target,detail\n");
    for event in &events {
        csv.push_str(&format!(
            "{},{},{},{},{}\n",
            escape_csv_field(&event.timestamp),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_plain_field_unchanged() {
        assert_eq!(escape_csv_field("hello"), "hello");
    }

    #[test]
    fn escape_field_with_comma_is_quoted() {
        assert_eq!(escape_csv_field("a,b"), "\"a,b\"");
    }

    #[test]
    fn escape_field_with_quote_is_doubled_and_wrapped() {
        assert_eq!(escape_csv_field("he said \"hi\""), "\"he said \"\"hi\"\"\"");
    }

    #[test]
    fn escape_formula_prefix_blocked() {
        assert_eq!(escape_csv_field("=SUM(A1:A9)"), "'=SUM(A1:A9)");
    }

    #[test]
    fn escape_formula_plus_comma_combined() {
        // starts with formula char AND contains a comma → quote the
        // prefixed string AND double any quotes.
        assert_eq!(escape_csv_field("=A,B"), "\"'=A,B\"");
    }

    #[test]
    fn escape_empty_field() {
        assert_eq!(escape_csv_field(""), "");
    }

    #[test]
    fn clamp_limit_enforces_ceiling() {
        assert_eq!(clamp_limit(10), 10);
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(10_000), MAX_PAGE_SIZE as usize);
    }
}
