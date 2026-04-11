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

#[cfg(test)]
mod tests {
    //! Spec § 5 #6 — every metric in `shared/src/metrics.rs` must
    //! have an emission assertion. The 7 CP-side constants are
    //! covered by `control-plane/tests/metrics_scenarios.rs::ME1`;
    //! the 6 agent-side constants are covered here using a thread-
    //! local `DebuggingRecorder` so the test does not fight with the
    //! global metrics recorder installed by `metrics::init()`.
    //!
    //! Source of truth: importing `nixfleet_types::metrics as m`
    //! means renaming a constant in shared/src/metrics.rs will
    //! refuse to compile here rather than silently skipping a check.

    use super::*;
    use metrics_util::debugging::DebuggingRecorder;
    use metrics_util::MetricKind;
    use std::time::Duration;

    /// Snapshot the current state of the recorder and return a vec of
    /// (kind, name) tuples for assertion.
    fn observed(snapshotter: &metrics_util::debugging::Snapshotter) -> Vec<(MetricKind, String)> {
        snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .map(|(key, _, _, _)| (key.kind(), key.key().name().to_string()))
            .collect()
    }

    /// Helper: did we record at least one observation with the given
    /// kind+name pair?
    fn observed_has(obs: &[(MetricKind, String)], kind: MetricKind, name: &str) -> bool {
        obs.iter().any(|(k, n)| *k == kind && n == name)
    }

    #[test]
    fn record_state_transition_emits_agent_state_gauge() {
        let recorder = DebuggingRecorder::default();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            record_state_transition("idle", "checking");
        });
        let obs = observed(&snap);
        assert!(
            observed_has(&obs, MetricKind::Gauge, m::AGENT_STATE),
            "AGENT_STATE gauge not observed; saw: {obs:?}"
        );
    }

    #[test]
    fn record_poll_emits_duration_histogram_and_timestamp_gauge() {
        let recorder = DebuggingRecorder::default();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            record_poll(Duration::from_millis(42));
        });
        let obs = observed(&snap);
        assert!(
            observed_has(&obs, MetricKind::Histogram, m::AGENT_POLL_DURATION_SECONDS),
            "AGENT_POLL_DURATION_SECONDS not observed; saw: {obs:?}"
        );
        assert!(
            observed_has(&obs, MetricKind::Gauge, m::AGENT_LAST_POLL_TIMESTAMP),
            "AGENT_LAST_POLL_TIMESTAMP not observed; saw: {obs:?}"
        );
    }

    #[test]
    fn record_health_check_emits_duration_histogram_and_status_gauge() {
        let recorder = DebuggingRecorder::default();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            record_health_check("disk", "command", 12, true);
        });
        let obs = observed(&snap);
        assert!(
            observed_has(
                &obs,
                MetricKind::Histogram,
                m::AGENT_HEALTH_CHECK_DURATION_SECONDS
            ),
            "AGENT_HEALTH_CHECK_DURATION_SECONDS not observed; saw: {obs:?}"
        );
        assert!(
            observed_has(&obs, MetricKind::Gauge, m::AGENT_HEALTH_CHECK_STATUS),
            "AGENT_HEALTH_CHECK_STATUS not observed; saw: {obs:?}"
        );
    }

    #[test]
    fn record_generation_emits_generation_info_gauge() {
        let recorder = DebuggingRecorder::default();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            record_generation("/nix/store/abc-system");
        });
        let obs = observed(&snap);
        assert!(
            observed_has(&obs, MetricKind::Gauge, m::AGENT_GENERATION_INFO),
            "AGENT_GENERATION_INFO not observed; saw: {obs:?}"
        );
    }

    /// Coverage check: every constant declared in
    /// `shared/src/metrics.rs` for the agent side has at least one
    /// emission test above. This list compiles only if all the names
    /// still exist in shared.
    #[test]
    fn all_six_agent_metrics_have_test_coverage() {
        // If a contributor adds a new agent metric without an
        // emission test, this list goes stale; the comment in the
        // file's #[cfg(test)] block above is the contract.
        let _ = (
            m::AGENT_STATE,
            m::AGENT_POLL_DURATION_SECONDS,
            m::AGENT_LAST_POLL_TIMESTAMP,
            m::AGENT_HEALTH_CHECK_DURATION_SECONDS,
            m::AGENT_HEALTH_CHECK_STATUS,
            m::AGENT_GENERATION_INFO,
        );
    }
}
