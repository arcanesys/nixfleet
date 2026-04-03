use metrics::{gauge, histogram};
use nixfleet_types::metrics as m;
use std::time::Duration;

/// Initialize the Prometheus metrics exporter with an HTTP listener.
/// Call only when --metrics-port is set.
pub fn init(port: u16) {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], port))
        .install()
        .expect("failed to install metrics recorder");
    tracing::info!(port, "Prometheus metrics listener started");
}

/// Record a state machine transition.
pub fn record_state_transition(from: &str, to: &str) {
    gauge!(m::AGENT_STATE, "state" => from.to_string()).set(0.0);
    gauge!(m::AGENT_STATE, "state" => to.to_string()).set(1.0);
}

/// Record a CP poll duration and update the last-poll timestamp.
pub fn record_poll(duration: Duration) {
    histogram!(m::AGENT_POLL_DURATION_SECONDS).record(duration.as_secs_f64());
    gauge!(m::AGENT_LAST_POLL_TIMESTAMP).set(chrono::Utc::now().timestamp() as f64);
}

/// Record a health check result.
pub fn record_health_check(name: &str, check_type: &str, duration_ms: u64, passed: bool) {
    histogram!(
        m::AGENT_HEALTH_CHECK_DURATION_SECONDS,
        "check_name" => name.to_string(),
        "check_type" => check_type.to_string()
    )
    .record(duration_ms as f64 / 1000.0);
    gauge!(
        m::AGENT_HEALTH_CHECK_STATUS,
        "check_name" => name.to_string()
    )
    .set(if passed { 1.0 } else { 0.0 });
}

/// Update the current generation info metric.
pub fn record_generation(hash: &str) {
    gauge!(m::AGENT_GENERATION_INFO, "generation" => hash.to_string()).set(1.0);
}
