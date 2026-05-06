//! Prometheus counters surface — minimum viable set for alerting.
//!
//! Three counters (auto-emit on event, monotonic) + one info gauge for
//! `cp_build_info`. No state-as-label gauges, no master/detail panel
//! drivers — those proved unworkable in Grafana's table model and were
//! stripped (see fleet's `nixfleet-events.json` which now reads from
//! Loki instead). What stays is the alerting surface:
//!
//!   - `nixfleet_compliance_failure_events_total{control_id, host}` —
//!     per-control, per-host. Cardinality bounded by the closed
//!     compliance set (~16 controls) × hosts.
//!   - `nixfleet_runtime_gate_error_events_total` — unlabeled. One
//!     global counter for the "agent couldn't measure compliance"
//!     class.
//!   - `nixfleet_gate_block_total{gate}` — one increment per
//!     `gates::evaluate_for_host` block. `gate` discriminator is one
//!     of the kebab-case gate kinds (channel-edges / wave-promotion /
//!     host-edge / disruption-budget / compliance-wave). Drives
//!     `rate(...{gate="compliance-wave"}[5m]) > 0` style alerts.
//!   - `nixfleet_cp_build_info{version, git_commit}=1` — one series.
//!     Standard pattern (cf. `kube_pod_info`) for tracking the
//!     deployed CP version across scrapes. Re-emitted every render
//!     since the values are compile-time constants.
//!
//! The exporter recorder is process-global and idempotent — first
//! `install_recorder()` wins. Tests can spin multiple test servers
//! without colliding.
//!
//! `idle_timeout` deliberately NOT set: counters are cumulative and
//! must NEVER reset; the previous version applied idle eviction to
//! gauges, but with no gauges in this slim surface, idle eviction is
//! moot. `cp_build_info` is the only gauge and it's re-emitted every
//! scrape via `record_build_info()`.

use std::sync::OnceLock;

use metrics::{counter, gauge};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

static METRICS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the process-global Prometheus recorder. Idempotent — safe
/// to call from each test's server-spawn helper.
pub fn install_recorder() -> &'static PrometheusHandle {
    METRICS_HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .install_recorder()
            .expect("install Prometheus recorder")
    })
}

/// Increment on `ComplianceFailure` event arrival in `/v1/agent/report`.
/// Bounded labels: hosts × controls.
pub fn record_compliance_event(control_id: &str, host: &str) {
    counter!(
        "nixfleet_compliance_failure_events_total",
        "control_id" => control_id.to_string(),
        "host" => host.to_string(),
    )
    .increment(1);
}

/// Increment on `RuntimeGateError` event arrival in `/v1/agent/report`.
pub fn record_runtime_gate_error() {
    counter!("nixfleet_runtime_gate_error_events_total").increment(1);
}

/// Increment when `gates::evaluate_for_host` returns `Some(GateBlock)`
/// at the dispatch endpoint. `gate_kind` is the kebab-case
/// discriminator (channel-edges / wave-promotion / host-edge /
/// disruption-budget / compliance-wave).
pub fn record_gate_block(gate_kind: &str) {
    counter!(
        "nixfleet_gate_block_total",
        "gate" => gate_kind.to_string(),
    )
    .increment(1);
}

/// `cp_build_info{version, git_commit}=1` — the deployed CP version.
/// Constants resolve at compile time; re-emit each scrape so it always
/// renders.
pub fn record_build_info() {
    gauge!(
        "nixfleet_cp_build_info",
        "version" => env!("CARGO_PKG_VERSION").to_string(),
        "git_commit" => option_env!("GIT_COMMIT").unwrap_or("unknown").to_string(),
    )
    .set(1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_recorder_is_idempotent() {
        let h1 = install_recorder();
        let h2 = install_recorder();
        assert!(std::ptr::eq(h1, h2), "recorder must be process-global");
    }
}
