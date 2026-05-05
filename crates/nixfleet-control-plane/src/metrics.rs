//! Prometheus metrics surface — minimum needed for the operator dashboard.
//!
//! Three info gauges drive the bulk of the dashboard:
//!   - `nixfleet_host_info{host, channel, state, convergence,
//!                         current_closure, declared_closure, rollout_id}=1`
//!     One series per declared host. Labels carry the operator-visible state
//!     (which closure is running, which rollout it's targeting, whether it
//!     matches the declared closure, current state machine position).
//!     Cardinality bound: O(hosts) — each host has exactly one active series;
//!     stale series age out via Prometheus staleness when any label changes.
//!
//!   - `nixfleet_active_rollout_info{rollout_id, channel,
//!                                   target_ref, current_wave}=1`
//!     One series per in-flight rollout (excludes superseded + terminal).
//!     Cardinality bound: O(active rollouts) — small per channel.
//!
//!   - `nixfleet_fleet_meta_info{ci_commit, signed_at, signature_algorithm,
//!                               schema_version}=1`
//!     One series — the verified fleet snapshot's metadata.
//!
//! Plus per-host numeric gauges for the table's right columns
//! (`outstanding_total`, `last_checkin_seconds`, `uptime_seconds`,
//! `converged`) and the per-state distribution gauge
//! (`host_rollout_state{host, channel, state}=1` — drives the bargauge
//! `count by (state) (... == 1)`).
//!
//! Plus pre-existing alert-source counters: `compliance_failure_events_total`,
//! `runtime_gate_error_events_total`, `gate_block_total`. These don't drive
//! the dashboard but power alerts and historical investigation.
//!
//! Cardinality discipline: closure paths and rollout IDs ARE on info-gauge
//! labels. They're bounded by the *current* set (one per host, one per
//! active rollout). Counters with closure_hash labels would grow without
//! bound; info gauges with closure_hash labels age out as soon as the host
//! moves to a new closure. Standard Prometheus pattern (cf. `kube_pod_info`,
//! `node_uname_info`).
//!
//! Init pattern: `install_recorder()` is idempotent via OnceLock — first
//! call installs the global recorder; subsequent calls return the same
//! handle. Tests can spin multiple test servers without colliding.

use std::sync::OnceLock;

use chrono::Utc;
use metrics::{counter, gauge};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use nixfleet_proto::HostStatusEntry;

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

/// Increment on `ComplianceFailure` event arrival in `/v1/agent/report`.
/// Bounded labels: hosts × controls (closed compliance set ≈ 16).
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

/// Increment when `gates::evaluate_for_host` returns `Some(GateBlock)` at
/// the dispatch endpoint. `gate_kind` is the kebab-case discriminator
/// (channel-edges / wave-promotion / host-edge / disruption-budget /
/// compliance-wave). Drives `rate(...{gate="compliance-wave"}[5m]) > 0`
/// alerts.
pub fn record_gate_block(gate_kind: &str) {
    counter!(
        "nixfleet_gate_block_total",
        "gate" => gate_kind.to_string(),
    )
    .increment(1);
}

/// Set once at server boot. `cp_build_info{version,git_commit}=1` is the
/// standard Prometheus pattern for tracking running version across scrapes.
pub fn record_build_info(version: &str, git_commit: Option<&str>) {
    gauge!(
        "nixfleet_cp_build_info",
        "version" => version.to_string(),
        "git_commit" => git_commit.unwrap_or("unknown").to_string(),
    )
    .set(1.0);
}

/// Refresh per-host + per-channel + per-rollout gauges from in-memory
/// state. Called by the `/metrics` HTTP handler on every scrape.
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
        record_host_info(view);
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

    record_fleet_meta_info(&snapshot.fleet);
    record_active_rollouts_info(state, now).await;

    Ok(())
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
    gauge!("nixfleet_host_outstanding_total", &labels[..]).set(
        (view.outstanding_compliance_failures + view.outstanding_runtime_gate_errors) as f64,
    );

    if let Some(last) = view.last_checkin_at {
        let age = now.signed_duration_since(last).num_seconds().max(0);
        gauge!("nixfleet_host_last_checkin_seconds", &labels[..]).set(age as f64);
    }
    if let Some(uptime) = view.last_uptime_secs {
        gauge!("nixfleet_host_uptime_seconds", &labels[..]).set(uptime as f64);
    }

    // One-of-N categorical gauge driving the `Hosts by Rollout State`
    // bargauge: `count by (state) (... == 1)`. Distinct from the
    // info-gauge surface, which carries state on a label for the
    // per-host table; both come from the same `view.rollout_state`.
    if let Some(state) = view.rollout_state {
        gauge!(
            "nixfleet_host_rollout_state",
            "host" => view.hostname.clone(),
            "channel" => view.channel.clone(),
            "state" => state.as_db_str().to_string(),
        )
        .set(1.0);
    }
}

/// Per-host info gauge — single source for the dashboard's per-host
/// table. Labels are derived from in-memory state (the `HostStatusEntry`
/// is what `/v1/hosts` returns); no extra DB queries.
///
/// `convergence` is a derived semantic:
///   - `converged` — current closure equals declared
///   - `diverged` — has a current closure but it's not the declared one
///   - `unknown` — host hasn't checked in (no current closure observed)
fn record_host_info(view: &HostStatusEntry) {
    let convergence = if view.converged {
        "converged"
    } else if view.current_closure_hash.is_some() && view.declared_closure_hash.is_some() {
        "diverged"
    } else {
        "unknown"
    };
    let state_label = view
        .rollout_state
        .map(|s| s.as_db_str().to_string())
        .unwrap_or_else(|| "(none)".to_string());
    gauge!(
        "nixfleet_host_info",
        "host" => view.hostname.clone(),
        "channel" => view.channel.clone(),
        "state" => state_label,
        "convergence" => convergence.to_string(),
        "current_closure" => view.current_closure_hash.clone().unwrap_or_default(),
        "declared_closure" => view.declared_closure_hash.clone().unwrap_or_default(),
        "rollout_id" => short_id(view.last_rollout_id.as_deref()),
    )
    .set(1.0);
}

/// Per-active-rollout info gauge + age. Drives the `Active Rollouts`
/// dashboard table. Reads `db.rollouts().list_in_flight()` (the UI surface
/// that excludes both superseded and terminal rollouts) joined with
/// `host_dispatch_state.active_rollouts_snapshot()` for `target_ref`.
async fn record_active_rollouts_info(state: &AppState, now: chrono::DateTime<Utc>) {
    let Some(db) = state.db.as_deref() else {
        return;
    };
    let rollouts = match db.rollouts().list_in_flight() {
        Ok(rs) => rs,
        Err(err) => {
            tracing::warn!(error = %err, "metrics: list_in_flight failed");
            return;
        }
    };
    let snap_by_id: std::collections::HashMap<String, _> = match db
        .host_dispatch_state()
        .active_rollouts_snapshot()
    {
        Ok(v) => v.into_iter().map(|s| (s.rollout_id.clone(), s)).collect(),
        Err(err) => {
            tracing::warn!(error = %err, "metrics: active_rollouts_snapshot failed");
            std::collections::HashMap::new()
        }
    };
    for r in rollouts.iter() {
        let target_ref = snap_by_id
            .get(&r.rollout_id)
            .map(|s| s.target_channel_ref.clone())
            .unwrap_or_default();
        gauge!(
            "nixfleet_active_rollout_info",
            "rollout_id" => short_id(Some(&r.rollout_id)),
            "channel" => r.channel.clone(),
            "target_ref" => short_id(Some(&target_ref)),
            "current_wave" => r.current_wave.to_string(),
        )
        .set(1.0);
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&r.created_at) {
            let age = now
                .signed_duration_since(ts.with_timezone(&Utc))
                .num_seconds()
                .max(0);
            gauge!(
                "nixfleet_active_rollout_age_seconds",
                "rollout_id" => short_id(Some(&r.rollout_id)),
                "channel" => r.channel.clone(),
            )
            .set(age as f64);
        }
    }
}

/// Verified-fleet snapshot metadata. One series; updates when the
/// snapshot does (CI commit changes, signed_at advances).
fn record_fleet_meta_info(fleet: &nixfleet_proto::FleetResolved) {
    let ci_commit = fleet.meta.ci_commit.clone().unwrap_or_default();
    let signed_at = fleet
        .meta
        .signed_at
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    let algorithm = fleet
        .meta
        .signature_algorithm_or_default()
        .to_string();
    gauge!(
        "nixfleet_fleet_meta_info",
        "ci_commit" => ci_commit,
        "signed_at" => signed_at,
        "signature_algorithm" => algorithm,
        "schema_version" => fleet.meta.schema_version.to_string(),
    )
    .set(1.0);
}

/// Truncate a 64-char hex rollout/closure ID to a 16-char prefix for
/// dashboard readability. Returns "(none)" for absent IDs so the label
/// is still queryable.
fn short_id(id: Option<&str>) -> String {
    match id {
        None => "(none)".to_string(),
        Some(s) if s.len() <= 16 => s.to_string(),
        Some(s) => s.chars().take(16).collect(),
    }
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
    fn short_id_truncates_long_hashes_keeps_short() {
        assert_eq!(short_id(None), "(none)");
        assert_eq!(short_id(Some("abc")), "abc");
        assert_eq!(
            short_id(Some("0123456789abcdef0123456789abcdef0123456789abcdef")),
            "0123456789abcdef"
        );
    }

    #[test]
    fn host_info_renders_with_convergence_label() {
        use nixfleet_proto::HostRolloutState;
        let handle = install_recorder();
        let view = HostStatusEntry {
            hostname: "lab".into(),
            channel: "edge".into(),
            declared_closure_hash: Some("ddddddddddddddddddddddddddddd".into()),
            current_closure_hash: Some("ddddddddddddddddddddddddddddd".into()),
            pending_closure_hash: None,
            last_checkin_at: None,
            last_rollout_id: Some("0123456789abcdef0123456789abcdef".into()),
            converged: true,
            outstanding_compliance_failures: 0,
            outstanding_runtime_gate_errors: 0,
            verified_event_count: 0,
            last_uptime_secs: None,
            rollout_state: Some(HostRolloutState::Soaked),
        };
        record_host_info(&view);
        let body = handle.render();
        assert!(
            body.contains("nixfleet_host_info"),
            "missing host_info gauge:\n{body}"
        );
        assert!(
            body.contains("convergence=\"converged\""),
            "convergence label missing or wrong:\n{body}"
        );
        assert!(
            body.contains("state=\"Soaked\""),
            "state label missing or wrong:\n{body}"
        );
        assert!(
            body.contains("rollout_id=\"0123456789abcdef\""),
            "rollout_id should be 16-char prefix:\n{body}"
        );
    }
}
