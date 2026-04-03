//! Prometheus metric name constants shared between agent and control plane.
//!
//! These are string constants only — no `metrics` crate dependency.
//! Each crate imports these and uses them with `metrics::counter!()` etc.

// --- Control Plane metrics ---
pub const FLEET_SIZE: &str = "nixfleet_fleet_size";
pub const MACHINES_BY_LIFECYCLE: &str = "nixfleet_machines_by_lifecycle";
pub const MACHINE_LAST_SEEN_TIMESTAMP: &str = "nixfleet_machine_last_seen_timestamp_seconds";
pub const HTTP_REQUESTS_TOTAL: &str = "nixfleet_http_requests_total";
pub const HTTP_REQUEST_DURATION_SECONDS: &str = "nixfleet_http_request_duration_seconds";
pub const ROLLOUTS_ACTIVE: &str = "nixfleet_rollouts_active";
pub const ROLLOUTS_TOTAL: &str = "nixfleet_rollouts_total";

// --- Agent metrics ---
pub const AGENT_STATE: &str = "nixfleet_agent_state";
pub const AGENT_POLL_DURATION_SECONDS: &str = "nixfleet_agent_poll_duration_seconds";
pub const AGENT_LAST_POLL_TIMESTAMP: &str = "nixfleet_agent_last_poll_timestamp_seconds";
pub const AGENT_HEALTH_CHECK_DURATION_SECONDS: &str =
    "nixfleet_agent_health_check_duration_seconds";
pub const AGENT_HEALTH_CHECK_STATUS: &str = "nixfleet_agent_health_check_status";
pub const AGENT_GENERATION_INFO: &str = "nixfleet_agent_generation_info";
