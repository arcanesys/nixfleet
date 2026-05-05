//! Prometheus metrics surface. Recorder is process-global; one
//! `PrometheusHandle` per process renders the text format on demand.
//!
//! Cardinality discipline (load-bearing): label sets carry only
//! values bounded by the verified fleet snapshot or the closed compliance
//! control set. Never label by `closure_hash`, `rollout_id`, or
//! `evidence_snippet` — those grow without bound and would blow up the
//! TSDB. Hostnames + channels + control IDs are the only safe labels.
//!
//! Scrape contract: `/metrics` calls `record_fleet_metrics` first to
//! refresh gauges from in-memory state, then renders. Counters
//! (compliance_failure_events_total) increment on event arrival in
//! `/v1/agent/report`, not on scrape.
//!
//! Init pattern: `install_recorder()` is idempotent via OnceLock. First
//! call installs the global; subsequent calls return the same handle.
//! Tests can therefore spin multiple test servers without colliding.

use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use metrics::{counter, gauge};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use nixfleet_proto::HostStatusEntry;

use crate::deferrals_view::compute_channel_deferrals;
use crate::server::AppState;
use crate::state_view::{fleet_state_view, StateViewError};

static METRICS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the process-global Prometheus recorder. Idempotent — safe to
/// call from each test's server-spawn helper.
pub fn install_recorder() -> &'static PrometheusHandle {
    METRICS_HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .install_recorder()
            .expect("install Prometheus recorder")
    })
}

/// Counter increment hook for `/v1/agent/report` to call when a
/// `ComplianceFailure` event arrives. `control_id` is bounded by the
/// closed compliance crate's control set (currently 16); `host` is
/// bounded by the verified-fleet snapshot. Cardinality: hosts ×
/// controls — within budget for fleets up to a few hundred hosts.
pub fn record_compliance_event(control_id: &str, host: &str) {
    counter!(
        "nixfleet_compliance_failure_events_total",
        "control_id" => control_id.to_string(),
        "host" => host.to_string(),
    )
    .increment(1);
}

/// Counter increment hook for `RuntimeGateError` events.
pub fn record_runtime_gate_error() {
    counter!("nixfleet_runtime_gate_error_events_total").increment(1);
}

/// Increment when `gates::evaluate_for_host` returns `Some(GateBlock)`
/// at the dispatch endpoint. `gate_kind` is the kebab-case discriminator
/// (channel-edges / wave-promotion / host-edge / disruption-budget /
/// compliance-wave) — bounded set, safe label. Operators alert on
/// `rate(nixfleet_gate_block_total{gate="compliance-wave"}[5m]) > 0` or
/// similar to surface "enforce mode is actively holding hosts".
pub fn record_gate_block(gate_kind: &str) {
    counter!(
        "nixfleet_gate_block_total",
        "gate" => gate_kind.to_string(),
    )
    .increment(1);
}

/// Refresh per-host + per-channel gauges from the current fleet state
/// view. Called by the `/metrics` handler on every scrape. The DB-backed
/// reads (active rollouts, deferrals) are bounded by the fleet size and
/// fast enough to run on the scrape path; if a future fleet outgrows
/// that, move them to the reconcile-tick instead.
pub async fn record_fleet_metrics(state: &AppState) -> Result<(), StateViewError> {
    let views = fleet_state_view(state).await?;
    let snapshot = state
        .verified_fleet
        .read()
        .await
        .clone()
        .ok_or(StateViewError::FleetNotPrimed)?;

    let now = Utc::now();
    for view in &views {
        record_host_gauges(view, now);
    }

    for (name, channel) in &snapshot.fleet.channels {
        gauge!(
            "nixfleet_channel_freshness_window_minutes",
            "channel" => name.clone(),
        )
        .set(f64::from(channel.freshness_window));
    }
    if let Some(signed_at) = snapshot.fleet.meta.signed_at {
        let age = now.signed_duration_since(signed_at).num_seconds().max(0);
        gauge!("nixfleet_fleet_signed_age_seconds").set(age as f64);
    }

    record_active_rollouts(state, &snapshot.fleet.channels.keys().cloned().collect::<Vec<_>>(), now);
    record_channel_deferrals(state, &snapshot.fleet.channels.keys().cloned().collect::<Vec<_>>()).await;

    Ok(())
}

/// Per-channel rollout activity. Pulls from the rollouts table directly
/// (same source `/v1/rollouts` consumes), so the dashboard reflects the
/// CP's authoritative view, not an agent-breadcrumb summary.
///
/// Emits, per channel that has at least one active rollout:
///   nixfleet_channel_active_rollouts_total{channel}     — count
///   nixfleet_channel_max_current_wave{channel}          — highest wave
///   nixfleet_channel_oldest_active_rollout_age_seconds{channel}
///
/// Channels with no active rollouts get a zero `_total` gauge so PromQL
/// `sum by (channel) (nixfleet_channel_active_rollouts_total)` is
/// well-defined fleet-wide. Cardinality bounded by #channels.
fn record_active_rollouts(state: &AppState, all_channels: &[String], now: DateTime<Utc>) {
    let Some(db) = state.db.as_deref() else {
        return;
    };
    let rollouts = match db.rollouts().list_active() {
        Ok(rs) => rs,
        Err(err) => {
            tracing::warn!(error = %err, "metrics: list_active rollouts failed");
            return;
        }
    };

    // Per-channel aggregates.
    let mut count_by_channel: HashMap<String, u32> = HashMap::new();
    let mut max_wave: HashMap<String, u32> = HashMap::new();
    let mut oldest: HashMap<String, DateTime<Utc>> = HashMap::new();
    for r in &rollouts {
        *count_by_channel.entry(r.channel.clone()).or_insert(0) += 1;
        let entry = max_wave.entry(r.channel.clone()).or_insert(0);
        if r.current_wave > *entry {
            *entry = r.current_wave;
        }
        if let Ok(ts) = DateTime::parse_from_rfc3339(&r.created_at) {
            let ts = ts.with_timezone(&Utc);
            let cur = oldest.entry(r.channel.clone()).or_insert(ts);
            if ts < *cur {
                *cur = ts;
            }
        }
    }

    // Emit for every declared channel — zero where nothing's active —
    // so a panel filtered to "active>0" is a stable expression.
    for channel in all_channels {
        let count = count_by_channel.get(channel).copied().unwrap_or(0);
        gauge!(
            "nixfleet_channel_active_rollouts_total",
            "channel" => channel.clone(),
        )
        .set(f64::from(count));
        let wave = max_wave.get(channel).copied().unwrap_or(0);
        gauge!(
            "nixfleet_channel_max_current_wave",
            "channel" => channel.clone(),
        )
        .set(f64::from(wave));
        let age = oldest
            .get(channel)
            .map(|ts| now.signed_duration_since(*ts).num_seconds().max(0))
            .unwrap_or(0);
        gauge!(
            "nixfleet_channel_oldest_active_rollout_age_seconds",
            "channel" => channel.clone(),
        )
        .set(age as f64);
    }
}

/// Per-channel deferral state. Reuses the same `compute_channel_deferrals`
/// helper as `GET /v1/deferrals` so the metric and the API never diverge.
///
///   nixfleet_channel_deferred{channel,blocked_by} = 1 when held;
///   the gauge is set to 0 for declared channels not currently deferred,
///   so `sum(nixfleet_channel_deferred)` is the fleet-wide deferred count.
async fn record_channel_deferrals(state: &AppState, all_channels: &[String]) {
    let deferrals = compute_channel_deferrals(state).await;
    let mut blockers: HashMap<String, String> = HashMap::new();
    for d in &deferrals {
        blockers.insert(d.channel.clone(), d.blocked_by.clone());
    }
    for channel in all_channels {
        match blockers.get(channel) {
            Some(blocked_by) => {
                gauge!(
                    "nixfleet_channel_deferred",
                    "channel" => channel.clone(),
                    "blocked_by" => blocked_by.clone(),
                )
                .set(1.0);
            }
            None => {
                // Reset any prior series to 0 — but only when there's no
                // blocker label to attach. Using `blocked_by="none"` keeps
                // the series rendered (PromQL `== 1` filters cleanly) and
                // bounds cardinality to (#channels × ≤2) since each
                // channel is either deferred-by-X or not-deferred.
                gauge!(
                    "nixfleet_channel_deferred",
                    "channel" => channel.clone(),
                    "blocked_by" => "none".to_string(),
                )
                .set(0.0);
            }
        }
    }
}

fn record_host_gauges(view: &HostStatusEntry, now: chrono::DateTime<Utc>) {
    let labels = [
        ("host", view.hostname.clone()),
        ("channel", view.channel.clone()),
    ];
    gauge!("nixfleet_host_converged", &labels[..]).set(if view.converged { 1.0 } else { 0.0 });
    gauge!(
        "nixfleet_host_outstanding_compliance_failures",
        &labels[..]
    )
    .set(view.outstanding_compliance_failures as f64);
    gauge!("nixfleet_host_outstanding_runtime_gate_errors", &labels[..])
        .set(view.outstanding_runtime_gate_errors as f64);
    // Pre-summed outstanding total — the dashboard's Fleet Status table
    // joins this directly instead of computing the sum via PromQL
    // arithmetic (which drops `__name__` and breaks joinByLabels).
    gauge!("nixfleet_host_outstanding_total", &labels[..]).set(
        (view.outstanding_compliance_failures + view.outstanding_runtime_gate_errors) as f64,
    );
    gauge!("nixfleet_host_verified_event_count", &labels[..])
        .set(view.verified_event_count as f64);

    if let Some(last) = view.last_checkin_at {
        let age = now.signed_duration_since(last).num_seconds().max(0);
        gauge!(
            "nixfleet_host_last_checkin_seconds",
            "host" => view.hostname.clone(),
        )
        .set(age as f64);
    }

    if let Some(uptime) = view.last_uptime_secs {
        gauge!(
            "nixfleet_host_uptime_seconds",
            "host" => view.hostname.clone(),
        )
        .set(uptime as f64);
    }

    // Two parallel state surfaces, by design:
    //
    // 1. `nixfleet_host_rollout_state{host,channel,state}` — one-of-N
    //    label-style gauge. Used by `count by (state) (... == 1)` to
    //    drive the per-state distribution bargauge. The state name is
    //    in the LABEL.
    //
    // 2. `nixfleet_host_state_code{host,channel}` — numeric encoding
    //    of the same enum. Used by tabular dashboards that join
    //    multiple metrics by (host,channel) via `joinByLabels` —
    //    those joins discard non-key labels, so a state-via-label
    //    metric can't carry the state name through. Numeric value +
    //    Grafana value mappings recovers the name.
    //
    // Cardinality cost of the second surface: +1 series per (host,channel),
    // bounded by the verified-fleet snapshot.
    if let Some(state) = view.rollout_state {
        gauge!(
            "nixfleet_host_rollout_state",
            "host" => view.hostname.clone(),
            "channel" => view.channel.clone(),
            "state" => state.as_db_str().to_string(),
        )
        .set(1.0);
        gauge!("nixfleet_host_state_code", &labels[..]).set(f64::from(state.state_code()));
    }
}

/// Set once at server boot. `cp_build_info{version,git_commit}=1` is
/// the standard Prometheus pattern for tracking running version across
/// scrapes — operators alert on `changes(nixfleet_cp_build_info[1h])`.
pub fn record_build_info(version: &str, git_commit: Option<&str>) {
    gauge!(
        "nixfleet_cp_build_info",
        "version" => version.to_string(),
        "git_commit" => git_commit.unwrap_or("unknown").to_string(),
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
        // Same OnceLock cell — pointer equality.
        assert!(std::ptr::eq(h1, h2), "recorder must be process-global");
    }

    #[test]
    fn rendered_output_contains_known_metric_after_increment() {
        let handle = install_recorder();
        record_compliance_event("ANSSI-BP-028", "metrics-test-host");
        let body = handle.render();
        assert!(
            body.contains("nixfleet_compliance_failure_events_total"),
            "missing counter in render output:\n{body}"
        );
        assert!(
            body.contains("ANSSI-BP-028"),
            "missing control_id label:\n{body}"
        );
        assert!(
            body.contains("host=\"metrics-test-host\""),
            "missing host label:\n{body}"
        );
    }

    #[test]
    fn host_state_code_renders_numeric_value() {
        use nixfleet_proto::HostRolloutState;
        let handle = install_recorder();
        let view = HostStatusEntry {
            hostname: "state-code-host".into(),
            channel: "stable".into(),
            declared_closure_hash: None,
            current_closure_hash: None,
            pending_closure_hash: None,
            last_checkin_at: None,
            last_rollout_id: None,
            converged: false,
            outstanding_compliance_failures: 2,
            outstanding_runtime_gate_errors: 1,
            verified_event_count: 0,
            last_uptime_secs: None,
            rollout_state: Some(HostRolloutState::Healthy),
        };
        record_host_gauges(&view, Utc::now());
        let body = handle.render();
        assert!(
            body.contains("nixfleet_host_state_code{channel=\"stable\",host=\"state-code-host\"} 4")
                || body.contains(
                    "nixfleet_host_state_code{host=\"state-code-host\",channel=\"stable\"} 4"
                ),
            "expected state_code=4 (Healthy) for state-code-host:\n{body}"
        );
        // Outstanding total = compliance + runtime = 3.
        assert!(
            body.contains("nixfleet_host_outstanding_total{channel=\"stable\",host=\"state-code-host\"} 3")
                || body.contains(
                    "nixfleet_host_outstanding_total{host=\"state-code-host\",channel=\"stable\"} 3"
                ),
            "expected outstanding_total=3 for state-code-host:\n{body}"
        );
    }

    #[test]
    fn build_info_renders_with_labels() {
        let handle = install_recorder();
        record_build_info("0.2.0-test", Some("abc1234"));
        let body = handle.render();
        assert!(
            body.contains("nixfleet_cp_build_info"),
            "missing build_info gauge:\n{body}"
        );
        assert!(
            body.contains("version=\"0.2.0-test\""),
            "missing version label:\n{body}"
        );
    }

    #[test]
    fn gate_block_counter_renders_with_kebab_label() {
        let handle = install_recorder();
        record_gate_block("compliance-wave");
        record_gate_block("disruption-budget");
        let body = handle.render();
        assert!(
            body.contains("nixfleet_gate_block_total"),
            "missing gate_block counter:\n{body}"
        );
        assert!(
            body.contains("gate=\"compliance-wave\""),
            "missing compliance-wave label:\n{body}"
        );
        assert!(
            body.contains("gate=\"disruption-budget\""),
            "missing disruption-budget label:\n{body}"
        );
    }
}
