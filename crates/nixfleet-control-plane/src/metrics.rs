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

/// Increment for every `Action` the reconciler emits per tick, labelled
/// by its snake_case discriminator (`open_rollout`, `dispatch_host`,
/// `promote_wave`, `converge_rollout`, `halt_rollout`, `rollback_host`,
/// `soak_host`, `channel_unknown`, `skip`, `wave_blocked`,
/// `rollout_deferred`). The dashboard decomposes the action stream
/// into per-kind rate panels (reconciler decisions, soak transitions,
/// wave promotions, convergence, rollback) — all sourced from this
/// single counter. Bounded label set: 11 action kinds.
pub fn record_reconciler_action(action_kind: &'static str) {
    counter!(
        "nixfleet_reconciler_action_total",
        "action_kind" => action_kind,
    )
    .increment(1);
}

/// snake_case discriminator matching the `Action` enum's serde tag.
/// Kept here instead of on the reconciler crate so the latter stays
/// dependency-light (no metrics crate pull).
pub fn action_kind_label(action: &nixfleet_reconciler::Action) -> &'static str {
    use nixfleet_reconciler::Action;
    match action {
        Action::OpenRollout { .. } => "open_rollout",
        Action::DispatchHost { .. } => "dispatch_host",
        Action::PromoteWave { .. } => "promote_wave",
        Action::ConvergeRollout { .. } => "converge_rollout",
        Action::HaltRollout { .. } => "halt_rollout",
        Action::RollbackHost { .. } => "rollback_host",
        Action::SoakHost { .. } => "soak_host",
        Action::ChannelUnknown { .. } => "channel_unknown",
        Action::Skip { .. } => "skip",
        Action::WaveBlocked { .. } => "wave_blocked",
        Action::RolloutDeferred { .. } => "rollout_deferred",
    }
}

/// Increment per `/v1/agent/checkin` once `decide_target` has produced
/// its `Decision`. `decision` is the kebab-case discriminator
/// (dispatch / converged / unmanaged / no-declaration / in-flight /
/// hold-after-failure / wave-not-reached) — bounded set. `host` lets
/// the dashboard show "is this host actively being responded to?".
/// Cardinality: hosts × 7 decision kinds, bounded.
pub fn record_checkin_decision(host: &str, decision: &'static str) {
    counter!(
        "nixfleet_checkin_decision_total",
        "host" => host.to_string(),
        "decision" => decision,
    )
    .increment(1);
}

/// Increment when `transition_host_state` actually flips a row
/// (DB returned >0 affected). `from`/`to` are the canonical SQL
/// literal names (`Healthy`, `Soaked`, `Converged`, `Failed`, ...).
/// NO host label — cardinality is 9² ≤ 81 even if every transition
/// edge is exercised, vs. hosts × 9² which blows up. Per-host
/// transitions are visible in the journal stream by design.
pub fn record_state_transition(from: &str, to: &str) {
    counter!(
        "nixfleet_host_state_transition_total",
        "from_state" => from.to_string(),
        "to_state" => to.to_string(),
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
    record_declarative_info(&snapshot.fleet);
    record_disruption_budgets(&snapshot.fleet, state).await;
    record_agent_versions(state).await;

    Ok(())
}

/// Declarative shape of the fleet — channels, edges, wave plan. All
/// derived from the verified-fleet snapshot at scrape time. Cardinality
/// is bounded by fleet config size: hosts + channels + edges + waves.
/// Operator dashboards consume these as info-style gauges (`== 1` filter
/// + label projection) — the value is always 1 except for wave_size
/// which carries the host count.
fn record_declarative_info(fleet: &nixfleet_proto::FleetResolved) {
    for (name, channel) in &fleet.channels {
        // Info gauge: only stable categorical labels. Numeric values
        // (freshness window, signing interval) live as their own
        // gauges so they series-flip cleanly when reconfigured —
        // putting them on labels would leave stale series for the
        // ~staleness window every time the operator bumps a value.
        gauge!(
            "nixfleet_channel_info",
            "channel" => name.clone(),
            "rollout_policy" => channel.rollout_policy.clone(),
            "compliance_mode" => channel.compliance.mode.clone(),
        )
        .set(1.0);
        gauge!(
            "nixfleet_channel_signing_interval_minutes",
            "channel" => name.clone(),
        )
        .set(f64::from(channel.signing_interval_minutes));
        // freshness_window_minutes already its own gauge — emitted in
        // record_fleet_metrics' channels loop above.
    }
    for edge in &fleet.channel_edges {
        gauge!(
            "nixfleet_channel_edge_info",
            "gates" => edge.gates.clone(),
            "gated" => edge.gated.clone(),
        )
        .set(1.0);
    }
    for edge in &fleet.edges {
        gauge!(
            "nixfleet_host_edge_info",
            "gates" => edge.gates.clone(),
            "gated" => edge.gated.clone(),
        )
        .set(1.0);
    }
    // Per-channel wave plan — one series per (channel, wave) carrying
    // the host count. Reads `fleet.waves[channel]` (the resolved wave
    // assignment, not the `rolloutPolicies.waves` template — the latter
    // is selector-driven and not host-resolved at this layer).
    for (channel, waves) in &fleet.waves {
        for (idx, wave) in waves.iter().enumerate() {
            gauge!(
                "nixfleet_channel_wave_size",
                "channel" => channel.clone(),
                "wave" => idx.to_string(),
            )
            .set(wave.hosts.len() as f64);
        }
    }
}

/// Per-disruption-budget headroom. Cardinality bound: number of declared
/// `disruption_budgets` entries (typically ≤ a handful).
///
/// Two metrics, both labelled by `Selector::summary()`:
///
///   nixfleet_disruption_budget_max{selector}        — declared cap
///   nixfleet_disruption_budget_in_flight{selector}  — current count
///
/// The in-flight count uses `gates::disruption_budget::in_flight_count`
/// against the same `Observed` view the gates evaluate against —
/// ONE source of truth for "how many slots are taken". Drift between
/// metric and gate is structurally impossible.
///
/// Max is the declared `max_in_flight`, or `floor(pct/100 * matched_hosts)`
/// when only `max_in_flight_pct` is set. `Selector::resolve` provides
/// the matched-host count from the live fleet snapshot.
async fn record_disruption_budgets(
    fleet: &nixfleet_proto::FleetResolved,
    state: &AppState,
) {
    if state.db.is_none() {
        return;
    }
    let observed = crate::observed_view::build_for_gates_from_state(
        state,
        fleet,
        &state
            .verified_fleet
            .read()
            .await
            .as_ref()
            .map(|s| s.fleet_resolved_hash.clone())
            .unwrap_or_default(),
    )
    .await;

    for budget in &fleet.disruption_budgets {
        let label = budget.selector.summary();
        let max = budget
            .max_in_flight
            .map(f64::from)
            .or_else(|| {
                budget.max_in_flight_pct.and_then(|pct| {
                    let total = budget.selector.resolve(fleet.hosts.iter()).len() as u32;
                    if total == 0 {
                        None
                    } else {
                        Some(((pct as f64 / 100.0) * total as f64).floor())
                    }
                })
            })
            .unwrap_or(0.0);
        gauge!(
            "nixfleet_disruption_budget_max",
            "selector" => label.clone(),
        )
        .set(max);
        let in_flight = nixfleet_reconciler::gates::disruption_budget::in_flight_count(
            &observed,
            &budget.selector,
        );
        gauge!(
            "nixfleet_disruption_budget_in_flight",
            "selector" => label,
        )
        .set(f64::from(in_flight));
    }
}

/// Per-host agent version, info-gauge style. One series per (host,
/// agent_version) pair seen at last checkin. Cardinality bounded:
/// hosts × few-versions-rolled-out-at-once. Operators read the
/// dashboard for "is everyone on the same version?".
async fn record_agent_versions(state: &AppState) {
    let checkins = state.host_checkins.read().await;
    for (hostname, record) in checkins.iter() {
        gauge!(
            "nixfleet_host_agent_version_info",
            "host" => hostname.clone(),
            "agent_version" => record.checkin.agent_version.clone(),
        )
        .set(1.0);
    }
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
    // UI/operator metric — count what's actually pending operator
    // attention. `list_in_flight()` excludes both superseded AND
    // terminal so a converged-and-stamped rollout doesn't keep
    // bumping `nixfleet_active_rollouts` after the work is done.
    // Gates use `list_active()` instead (terminal predecessors stay
    // visible to channel_edges).
    let rollouts = match db.rollouts().list_in_flight() {
        Ok(rs) => rs,
        Err(err) => {
            tracing::warn!(error = %err, "metrics: list_in_flight rollouts failed");
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

    #[test]
    fn reconciler_action_counter_uses_snake_case_kind() {
        use nixfleet_reconciler::Action;
        let handle = install_recorder();
        record_reconciler_action(action_kind_label(&Action::OpenRollout {
            channel: "stable".into(),
            target_ref: "abc".into(),
        }));
        record_reconciler_action(action_kind_label(&Action::DispatchHost {
            rollout: "r1".into(),
            host: "lab".into(),
            target_ref: "abc".into(),
        }));
        record_reconciler_action(action_kind_label(&Action::ConvergeRollout {
            rollout: "r1".into(),
        }));
        let body = handle.render();
        assert!(
            body.contains("action_kind=\"open_rollout\""),
            "missing open_rollout label:\n{body}"
        );
        assert!(
            body.contains("action_kind=\"dispatch_host\""),
            "missing dispatch_host label:\n{body}"
        );
        assert!(
            body.contains("action_kind=\"converge_rollout\""),
            "missing converge_rollout label:\n{body}"
        );
    }

    #[test]
    fn checkin_decision_counter_carries_host_and_decision() {
        let handle = install_recorder();
        record_checkin_decision("test-host-A", "dispatch");
        record_checkin_decision("test-host-B", "converged");
        record_checkin_decision("test-host-A", "in-flight");
        let body = handle.render();
        assert!(
            body.contains("nixfleet_checkin_decision_total"),
            "missing decision counter:\n{body}"
        );
        // Either label-order is fine.
        assert!(
            body.contains("decision=\"dispatch\"") && body.contains("host=\"test-host-A\""),
            "missing decision/host labels:\n{body}"
        );
    }

    #[test]
    fn state_transition_counter_pairs_from_to() {
        let handle = install_recorder();
        record_state_transition("Healthy", "Soaked");
        record_state_transition("Soaked", "Converged");
        let body = handle.render();
        assert!(
            body.contains("nixfleet_host_state_transition_total"),
            "missing transition counter:\n{body}"
        );
        assert!(
            body.contains("from_state=\"Healthy\"") && body.contains("to_state=\"Soaked\""),
            "missing from/to labels:\n{body}"
        );
    }

}
