//! Prometheus metrics for the NixFleet control plane.
//!
//! Provides:
//! - `init()`: installs the global Prometheus recorder and returns a handle
//! - `metrics_handler`: Axum handler for GET /metrics
//! - `http_metrics_layer`: middleware recording HTTP request count and latency
//! - `update_fleet_gauges`: recalculates fleet-wide gauges from in-memory state

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::middleware::Next;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use nixfleet_types::metrics as m;
use std::sync::Arc;
use std::time::Instant;

use crate::state::FleetState;

/// Install the Prometheus recorder globally and return a handle to it.
///
/// Call once at startup, before building the Axum router. Subsequent
/// calls are no-ops that return a fresh (unwired) handle — installing
/// twice would panic inside `metrics_exporter_prometheus`, but we
/// tolerate it so in-process tests and supervisors can launch the
/// server more than once in the same process.
pub fn init() -> PrometheusHandle {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("failed to install Prometheus recorder")
        })
        .clone()
}

/// GET /metrics
///
/// Renders current metrics in Prometheus text exposition format.
pub async fn metrics_handler(State(handle): State<Arc<PrometheusHandle>>) -> (StatusCode, String) {
    (StatusCode::OK, handle.render())
}

/// Axum middleware that records HTTP request count and latency for every route.
///
/// Skips /metrics and /health to avoid self-referential noise.
pub async fn http_metrics_layer(request: Request<Body>, next: Next) -> Response<Body> {
    let path = request.uri().path().to_owned();
    let method = request.method().to_string();

    // Skip self-referential endpoints
    if path == "/metrics" || path == "/health" {
        return next.run(request).await;
    }

    let normalized = normalize_path(&path);
    let start = Instant::now();
    let response = next.run(request).await;
    let elapsed = start.elapsed().as_secs_f64();

    let status = response.status().as_u16().to_string();

    metrics::counter!(
        m::HTTP_REQUESTS_TOTAL,
        "method" => method.clone(),
        "path" => normalized.clone(),
        "status" => status
    )
    .increment(1);

    metrics::histogram!(
        m::HTTP_REQUEST_DURATION_SECONDS,
        "method" => method,
        "path" => normalized
    )
    .record(elapsed);

    response
}

/// Recalculate fleet-wide gauges from the current in-memory state.
///
/// Call after any state-mutating operation (register, report, lifecycle change).
pub fn update_fleet_gauges(state: &FleetState) {
    use nixfleet_types::MachineLifecycle;

    let total = state.machines.len() as f64;
    metrics::gauge!(m::FLEET_SIZE).set(total);

    let mut pending = 0u32;
    let mut provisioning = 0u32;
    let mut active = 0u32;
    let mut maintenance = 0u32;
    let mut decommissioned = 0u32;

    for (machine_id, machine) in &state.machines {
        match machine.lifecycle {
            MachineLifecycle::Pending => pending += 1,
            MachineLifecycle::Provisioning => provisioning += 1,
            MachineLifecycle::Active => active += 1,
            MachineLifecycle::Maintenance => maintenance += 1,
            MachineLifecycle::Decommissioned => decommissioned += 1,
            _ => {}
        }

        if let Some(last_seen) = machine.last_seen {
            let ts = last_seen.timestamp() as f64;
            metrics::gauge!(
                m::MACHINE_LAST_SEEN_TIMESTAMP,
                "machine_id" => machine_id.clone()
            )
            .set(ts);
        }
    }

    metrics::gauge!(m::MACHINES_BY_LIFECYCLE, "lifecycle" => "pending").set(pending as f64);
    metrics::gauge!(m::MACHINES_BY_LIFECYCLE, "lifecycle" => "provisioning")
        .set(provisioning as f64);
    metrics::gauge!(m::MACHINES_BY_LIFECYCLE, "lifecycle" => "active").set(active as f64);
    metrics::gauge!(m::MACHINES_BY_LIFECYCLE, "lifecycle" => "maintenance").set(maintenance as f64);
    metrics::gauge!(m::MACHINES_BY_LIFECYCLE, "lifecycle" => "decommissioned")
        .set(decommissioned as f64);
}

/// Normalize a URL path to a route template to prevent label cardinality explosion.
///
/// Replaces dynamic path segments with placeholder tokens:
/// - Nix store paths and UUID-like machine IDs → `{id}`
/// - Rollout UUIDs → `{id}`
/// - Tag names in /tags/{tag} → `{tag}`
pub fn normalize_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    let mut result = Vec::with_capacity(segments.len());
    let mut i = 0;

    while i < segments.len() {
        let segment = segments[i];

        // /tags/{tag} — segment after "tags" is a tag name
        if segment == "tags" {
            result.push("tags");
            if i + 1 < segments.len() && !segments[i + 1].is_empty() {
                result.push("{tag}");
                i += 2;
                continue;
            }
        }
        // Nix store paths: start with /nix/store/
        else if segment == "nix" && i + 1 < segments.len() && segments[i + 1] == "store" {
            result.push("nix");
            result.push("store");
            result.push("{id}");
            i += 3;
            continue;
        }
        // UUID-like segments (contains hyphens, length 36) or rollout IDs
        else if is_dynamic_segment(segment) {
            result.push("{id}");
        } else {
            result.push(segment);
        }

        i += 1;
    }

    result.join("/")
}

/// Returns true if a path segment looks like a dynamic identifier (UUID, hash, machine ID
/// that is not a known static route keyword).
fn is_dynamic_segment(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }

    // Skip known static API segments
    let static_segments = [
        "api",
        "v1",
        "machines",
        "rollouts",
        "releases",
        "keys",
        "audit",
        "export",
        "bootstrap",
        "desired-generation",
        "register",
        "lifecycle",
        "tags",
        "report",
        "resume",
        "cancel",
        "diff",
        "health",
        "metrics",
    ];
    if static_segments.contains(&segment) {
        return false;
    }

    // UUID format: 8-4-4-4-12
    let parts: Vec<&str> = segment.split('-').collect();
    if parts.len() == 5
        && parts[0].len() == 8
        && parts[1].len() == 4
        && parts[2].len() == 4
        && parts[3].len() == 4
        && parts[4].len() == 12
    {
        return true;
    }

    // Anything containing a hyphen that doesn't match known static names is likely a
    // machine ID (e.g., "web-01", "dev-node-3") or rollout reference
    if segment.contains('-') {
        return true;
    }

    // Long hex-like strings (nix hashes, etc.)
    if segment.len() > 20 && segment.chars().all(|c| c.is_ascii_alphanumeric()) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_machine_id_paths() {
        assert_eq!(
            normalize_path("/api/v1/machines/web-01/desired-generation"),
            "/api/v1/machines/{id}/desired-generation"
        );
        assert_eq!(
            normalize_path("/api/v1/machines/dev-node-3/report"),
            "/api/v1/machines/{id}/report"
        );
        assert_eq!(
            normalize_path("/api/v1/machines/my-host/register"),
            "/api/v1/machines/{id}/register"
        );
        assert_eq!(
            normalize_path("/api/v1/machines/web-01/lifecycle"),
            "/api/v1/machines/{id}/lifecycle"
        );
    }

    #[test]
    fn test_normalize_rollout_id_paths() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            normalize_path(&format!("/api/v1/rollouts/{uuid}")),
            "/api/v1/rollouts/{id}"
        );
        assert_eq!(
            normalize_path(&format!("/api/v1/rollouts/{uuid}/resume")),
            "/api/v1/rollouts/{id}/resume"
        );
        assert_eq!(
            normalize_path(&format!("/api/v1/rollouts/{uuid}/cancel")),
            "/api/v1/rollouts/{id}/cancel"
        );
    }

    #[test]
    fn test_normalize_tag_paths() {
        assert_eq!(
            normalize_path("/api/v1/machines/web-01/tags"),
            "/api/v1/machines/{id}/tags"
        );
        assert_eq!(
            normalize_path("/api/v1/machines/web-01/tags/production"),
            "/api/v1/machines/{id}/tags/{tag}"
        );
        assert_eq!(
            normalize_path("/api/v1/machines/web-01/tags/env:prod"),
            "/api/v1/machines/{id}/tags/{tag}"
        );
    }

    #[test]
    fn test_normalize_release_paths() {
        assert_eq!(normalize_path("/api/v1/releases"), "/api/v1/releases");
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            normalize_path(&format!("/api/v1/releases/{uuid}")),
            "/api/v1/releases/{id}"
        );
        let uuid2 = "660e8400-e29b-41d4-a716-446655440001";
        assert_eq!(
            normalize_path(&format!("/api/v1/releases/{uuid}/diff/{uuid2}")),
            "/api/v1/releases/{id}/diff/{id}"
        );
    }

    #[test]
    fn test_normalize_bootstrap_path() {
        assert_eq!(
            normalize_path("/api/v1/keys/bootstrap"),
            "/api/v1/keys/bootstrap"
        );
    }

    #[test]
    fn test_normalize_preserves_empty_root() {
        // Root path — no dynamic segment replacement
        assert_eq!(normalize_path("/"), "/");
    }
}
