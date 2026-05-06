//! Prometheus metrics surface — minimum needed for the operator dashboard.
//!
//! Four info-gauge surfaces drive the bulk of the dashboard:
//!   - `nixfleet_host_info{host, channel, state, convergence,
//!                         current_closure, declared_closure, rollout_id}=1`
//!     One series per declared host. Labels carry the operator-visible state
//!     (which closure is running, which rollout it's targeting, whether it
//!     matches the declared closure, current state machine position).
//!     Cardinality bound: O(hosts) — each host has exactly one active series;
//!     stale series age out via Prometheus staleness when any label changes.
//!
//!   - `nixfleet_rollout_view{kind, rollout_id, channel, display_name,
//!                            target_ref, current_wave, wave_summary,
//!                            fleet_anchor, health_max_failures,
//!                            compliance_required, compliance_frameworks,
//!                            host, wave, state, target_closure,
//!                            soak_window_minutes}` (0..100)
//!     and `nixfleet_rollout_view_time_seconds{...same labels...}` (seconds).
//!     Unified master/detail surface — one row per rollout summary
//!     (`kind=rollout`) and one row per host within each in-flight
//!     rollout (`kind=host`, including pre-dispatch hosts of late waves
//!     so the full wave plan renders even before its dispatch). Source
//!     filtering excludes terminal + superseded rollouts entirely.
//!     Cardinality bound: O(in-flight rollouts × hosts in those
//!     rollouts' manifests).
//!
//!     Manifest-derived rollout fields (`display_name`, `fleet_anchor`,
//!     `wave_summary`, `health_max_failures`, `compliance_required`,
//!     `compliance_frameworks`) come from the signed
//!     `{rollout_id}.json` on local disk. Empty when the manifest hasn't
//!     reached the artifact dir yet — the rollout still gets a summary
//!     row with the DB-only fields.
//!
//!     Value semantics:
//!       - kind=rollout: `view` = (converged+soaked) / total dispatched × 100;
//!         `time_seconds` = wall-clock age.
//!       - kind=host: `view` = soak progress 0..100;
//!         `time_seconds` = soak remaining (0 once past window).
//!
//!   - `nixfleet_channel_status{channel, status, rollout_id, target_ref,
//!                              blocked_by, reason}=1`
//!     One row per declared channel. `status="active"` for channels with
//!     an in-flight rollout (then `rollout_id` carries it); `status="deferred"`
//!     when held by a fleet-level gate (then `blocked_by` + `reason` carry
//!     the operator-visible explanation); `status="idle"` for converged
//!     channels with no work. Cardinality bound: O(channels).
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
use std::time::Duration;

use chrono::Utc;
use metrics::{counter, gauge};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use metrics_util::MetricKindMask;
use nixfleet_proto::HostStatusEntry;

use crate::server::AppState;
use crate::state_view::{fleet_state_view, StateViewError};

static METRICS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Idle timeout for gauges. Series that aren't re-emitted within this
/// window get dropped from the registry — eliminates stale label sets
/// after host state / closure / rollout-id transitions. Counters are
/// excluded (cumulative; mustn't be reset).
///
/// Tuned ≥ 2× scrape interval (15s on lab) so a transient scrape gap
/// doesn't drop the metric. Every gauge is refreshed by
/// `record_fleet_metrics` each scrape, so steady-state series stay
/// alive indefinitely.
const GAUGE_IDLE_TIMEOUT: Duration = Duration::from_secs(45);

/// Install the process-global Prometheus recorder. Idempotent — safe to
/// call from each test's server-spawn helper.
pub fn install_recorder() -> &'static PrometheusHandle {
    METRICS_HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .idle_timeout(MetricKindMask::GAUGE, Some(GAUGE_IDLE_TIMEOUT))
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

/// `cp_build_info{version,git_commit}=1` — standard Prometheus pattern
/// for tracking running version across scrapes. Re-emitted every scrape
/// (constants resolve at compile time) so the idle-timeout doesn't evict
/// it.
fn record_build_info() {
    gauge!(
        "nixfleet_cp_build_info",
        "version" => env!("CARGO_PKG_VERSION").to_string(),
        "git_commit" => option_env!("GIT_COMMIT").unwrap_or("unknown").to_string(),
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
    record_build_info();
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
    record_rollout_overview(state, &snapshot.fleet, now).await;
    record_channel_status(state, &snapshot.fleet).await;

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

/// Unified rollout/host overview — drives the dashboard's master/detail
/// Rollouts panel. Two metrics emitted per row, identical label schemas:
///
///   - `nixfleet_rollout_view{...}` = progress percent (0..100)
///   - `nixfleet_rollout_view_time_seconds{...}` = age (rollout) / remaining (host)
///
/// Source filtering excludes terminal + superseded rollouts entirely:
/// rows are emitted only for `db.rollouts().list_in_flight()` — no
/// zombie host rows from rollouts whose host_dispatch_state hasn't been
/// orphan-swept yet. Same data path the `/v1/rollouts` HTTP route uses.
///
/// For each in-flight rollout R, the loop reads R's signed manifest from
/// `state.rollouts_dir/{rolloutId}.json` (already on local disk — the
/// channel-refs poll downloads them). Manifest parse failure is
/// permissive: rollout still gets a summary row, host detail just won't
/// see pre-dispatch hosts (rare, only matters for freshly-opened
/// rollouts). The manifest enriches:
///   - rollout summary labels: `display_name`, `fleet_anchor`,
///     `health_max_failures`, `compliance_required`,
///     `compliance_frameworks`, `wave_summary`
///   - host rows for hosts in `manifest.host_set` that haven't been
///     dispatched yet — they appear with `state="Queued"` so the operator
///     sees the full wave plan, not just hosts already in motion.
///
/// Sort `[rollout_id ASC, kind DESC]` in the dashboard puts each rollout
/// summary above its host detail rows.
///
/// Soak math mirrors fleet-status's `soak_status_for_host` (render.sh):
/// elapsed from `host_rollout_state.last_healthy_since`, soak window
/// from `policy.waves[wave].soak_minutes`. Soaked/Converged pin to
/// progress=100 / remaining=0; pre-dispatch (Queued) pins to 0/0.
async fn record_rollout_overview(
    state: &AppState,
    fleet: &nixfleet_proto::FleetResolved,
    now: chrono::DateTime<Utc>,
) {
    let Some(db) = state.db.as_deref() else {
        return;
    };
    let rollouts = match db.rollouts().list_in_flight() {
        Ok(rs) => rs,
        Err(err) => {
            tracing::warn!(error = %err, "metrics: rollout_overview list_in_flight failed");
            return;
        }
    };
    let snap_by_id: std::collections::HashMap<String, _> = match db
        .host_dispatch_state()
        .active_rollouts_snapshot()
    {
        Ok(v) => v.into_iter().map(|s| (s.rollout_id.clone(), s)).collect(),
        Err(err) => {
            tracing::warn!(error = %err, "metrics: rollout_overview snapshot failed");
            std::collections::HashMap::new()
        }
    };

    for r in rollouts.iter() {
        let snap = snap_by_id.get(&r.rollout_id);
        let manifest = load_rollout_manifest(state.rollouts_dir.as_deref(), &r.rollout_id).await;
        let target_ref = snap.map(|s| s.target_channel_ref.clone()).unwrap_or_default();

        // Build the wave breakdown from the manifest's host_set joined
        // with snap.host_states. "wave 0: 1/2 · wave 1: 0/2" form; same
        // shape as fleet-status's wave_summary. Empty manifest → empty
        // string — the operator sees the rollout but loses the breakdown
        // until the manifest reaches /var/lib/nixfleet-cp/rollouts/.
        let wave_summary = manifest
            .as_ref()
            .map(|m| compute_wave_summary(m, snap))
            .unwrap_or_default();

        // Progress = (converged + soaked) / total dispatched. The denominator
        // is dispatched (not manifest size) because pre-dispatch hosts of
        // late waves shouldn't dilute the progress signal — when wave 0
        // converges the rollout shows 50% (wave 1 not yet dispatched), then
        // climbs as wave 1 dispatches and converges.
        let (done, total) = snap
            .map(|s| {
                let total = s.host_states.len();
                let done = s
                    .host_states
                    .values()
                    .filter(|st| matches!(st.as_str(), "Soaked" | "Converged"))
                    .count();
                (done, total)
            })
            .unwrap_or((0, 0));
        let rollout_progress = if total > 0 {
            done as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        let rollout_age = chrono::DateTime::parse_from_rfc3339(&r.created_at)
            .map(|ts| {
                now.signed_duration_since(ts.with_timezone(&Utc))
                    .num_seconds()
                    .max(0)
            })
            .unwrap_or(0);

        let display_name = manifest
            .as_ref()
            .map(|m| m.display_name.clone())
            .unwrap_or_default();
        let fleet_anchor = manifest
            .as_ref()
            .map(|m| short_id(Some(&m.fleet_resolved_hash)))
            .unwrap_or_default();
        let health_max_failures = manifest
            .as_ref()
            .and_then(|m| m.health_gate.systemd_failed_units.as_ref())
            .map(|s| s.max.to_string())
            .unwrap_or_else(|| "—".to_string());
        let compliance_required = manifest
            .as_ref()
            .and_then(|m| m.health_gate.compliance_probes.as_ref())
            .map(|c| {
                if c.required {
                    "required".to_string()
                } else {
                    "optional".to_string()
                }
            })
            .unwrap_or_default();
        let compliance_frameworks = manifest
            .as_ref()
            .map(|m| m.compliance_frameworks.join(", "))
            .unwrap_or_default();

        let rollout_row_key = format!("rollout:{}:", short_id(Some(&r.rollout_id)));
        let rollout_labels = [
            ("row_key", rollout_row_key),
            ("kind", "rollout".to_string()),
            ("rollout_id", short_id(Some(&r.rollout_id))),
            ("channel", r.channel.clone()),
            ("display_name", display_name),
            ("target_ref", short_id(Some(&target_ref))),
            ("current_wave", r.current_wave.to_string()),
            ("wave_summary", wave_summary),
            ("fleet_anchor", fleet_anchor),
            ("health_max_failures", health_max_failures),
            ("compliance_required", compliance_required),
            ("compliance_frameworks", compliance_frameworks),
            ("host", String::new()),
            ("wave", String::new()),
            ("state", String::new()),
            ("target_closure", String::new()),
            ("soak_window_minutes", String::new()),
        ];
        gauge!("nixfleet_rollout_view", &rollout_labels[..]).set(rollout_progress);
        gauge!(
            "nixfleet_rollout_view_time_seconds",
            &rollout_labels[..]
        )
        .set(rollout_age as f64);

        // Per-host detail rows. Source-of-truth precedence:
        //   1. manifest.host_set if available — full wave plan, including
        //      pre-dispatch hosts that don't have host_dispatch_state rows
        //      yet (so wave 1 hosts show "Queued" while wave 0 is in
        //      flight).
        //   2. snap.host_states fallback when no manifest — same set as
        //      `/v1/rollouts.hostStates`, but misses pre-dispatch hosts.
        let policy = fleet
            .channels
            .get(&r.channel)
            .and_then(|c| fleet.rollout_policies.get(&c.rollout_policy));
        let host_iter: Vec<(String, u32, String)> = if let Some(m) = manifest.as_ref() {
            m.host_set
                .iter()
                .map(|h| {
                    (
                        h.hostname.clone(),
                        h.wave_index,
                        h.target_closure.clone(),
                    )
                })
                .collect()
        } else if let Some(s) = snap {
            s.host_states
                .keys()
                .map(|h| {
                    (
                        h.clone(),
                        s.host_waves.get(h).copied().unwrap_or(r.current_wave),
                        s.target_closure_hash.clone(),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };

        for (host, wave_idx, target_closure) in &host_iter {
            let host_state = snap
                .and_then(|s| s.host_states.get(host).cloned())
                .unwrap_or_else(|| "Queued".to_string());
            let wave_def = policy.and_then(|p| p.waves.get(*wave_idx as usize));
            let window = wave_def
                .map(|w| i64::from(w.soak_minutes) * 60)
                .unwrap_or(0);
            let elapsed = match host_state.as_str() {
                "Soaked" | "Converged" => window,
                "Healthy" => snap
                    .and_then(|s| s.last_healthy_since.get(host))
                    .map(|t| now.signed_duration_since(*t).num_seconds().clamp(0, window))
                    .unwrap_or(0),
                _ => 0,
            };
            let host_progress = if window > 0 {
                elapsed as f64 / window as f64 * 100.0
            } else if matches!(host_state.as_str(), "Soaked" | "Converged") {
                100.0
            } else {
                0.0
            };
            let remaining = (window - elapsed).max(0);
            let host_row_key = format!("host:{}:{}", short_id(Some(&r.rollout_id)), host);
            let host_labels = [
                ("row_key", host_row_key),
                ("kind", "host".to_string()),
                ("rollout_id", short_id(Some(&r.rollout_id))),
                ("channel", r.channel.clone()),
                ("display_name", String::new()),
                ("target_ref", String::new()),
                ("current_wave", String::new()),
                ("wave_summary", String::new()),
                ("fleet_anchor", String::new()),
                ("health_max_failures", String::new()),
                ("compliance_required", String::new()),
                ("compliance_frameworks", String::new()),
                ("host", host.clone()),
                ("wave", wave_idx.to_string()),
                ("state", host_state),
                ("target_closure", short_id(Some(target_closure))),
                (
                    "soak_window_minutes",
                    wave_def
                        .map(|w| w.soak_minutes.to_string())
                        .unwrap_or_default(),
                ),
            ];
            gauge!("nixfleet_rollout_view", &host_labels[..]).set(host_progress);
            gauge!("nixfleet_rollout_view_time_seconds", &host_labels[..])
                .set(remaining as f64);
        }
    }
}

/// Read + parse `{rollout_id}.json` from the rollouts artifact dir.
/// Permissive on every failure mode (no dir configured, file absent,
/// parse error) — caller falls back to host_dispatch_state-only data.
/// Mirrors `observed_view::load_budgets_from_manifest` minus the
/// budgets-only narrowing.
async fn load_rollout_manifest(
    dir: Option<&std::path::Path>,
    rollout_id: &str,
) -> Option<nixfleet_proto::RolloutManifest> {
    let dir = dir?;
    let path = dir.join(format!("{rollout_id}.json"));
    let bytes = tokio::fs::read(&path).await.ok()?;
    match serde_json::from_slice::<nixfleet_proto::RolloutManifest>(&bytes) {
        Ok(m) => Some(m),
        Err(err) => {
            tracing::warn!(
                rollout = %rollout_id,
                error = %err,
                "metrics: rollout manifest parse failed; rollout summary loses display_name + wave_summary",
            );
            None
        }
    }
}

/// `wave 0: 1/2 · wave 1: 0/2` — total per wave from manifest, done count
/// per wave from `snap.host_states` (Soaked + Converged). Returns empty
/// string for an empty manifest host_set.
fn compute_wave_summary(
    manifest: &nixfleet_proto::RolloutManifest,
    snap: Option<&crate::db::RolloutDbSnapshot>,
) -> String {
    use std::collections::BTreeMap;
    let mut by_wave: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    for h in &manifest.host_set {
        let entry = by_wave.entry(h.wave_index).or_insert((0, 0));
        entry.1 += 1;
        if let Some(s) = snap {
            if matches!(
                s.host_states.get(&h.hostname).map(String::as_str),
                Some("Soaked") | Some("Converged"),
            ) {
                entry.0 += 1;
            }
        }
    }
    by_wave
        .iter()
        .map(|(w, (done, total))| format!("wave {w}: {done}/{total}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Per-channel info gauge merging "running a rollout" and "deferred by
/// a fleet-level gate" into one row per channel. Drives the dashboard's
/// channel-status table; one source of truth for both `/v1/rollouts`
/// (active) and `/v1/deferrals` (deferred) operator surfaces.
///
/// `status="active"`: channel has an in-flight rollout; `rollout_id` +
/// `target_ref` carry it; `blocked_by` + `reason` are empty.
/// `status="deferred"`: channel is held by `channel_edges`; `blocked_by`
/// + `reason` carry the predecessor + operator-visible explanation;
/// `rollout_id` is empty.
/// `status="idle"`: declared channel with no active rollout and no
/// deferral — the steady-state of a converged channel.
async fn record_channel_status(state: &AppState, fleet: &nixfleet_proto::FleetResolved) {
    let Some(db) = state.db.as_deref() else {
        return;
    };
    let in_flight = match db.rollouts().list_in_flight() {
        Ok(rs) => rs,
        Err(err) => {
            tracing::warn!(error = %err, "metrics: channel_status list_in_flight failed");
            return;
        }
    };
    let mut active_by_channel: std::collections::HashMap<String, &crate::db::rollouts::ActiveRollout> =
        std::collections::HashMap::new();
    for r in in_flight.iter() {
        active_by_channel.insert(r.channel.clone(), r);
    }
    let deferrals = crate::deferrals_view::compute_channel_deferrals(state).await;
    let deferred_by_channel: std::collections::HashMap<String, &crate::deferrals_view::ChannelDeferral> =
        deferrals.iter().map(|d| (d.channel.clone(), d)).collect();

    for channel_name in fleet.channels.keys() {
        if let Some(r) = active_by_channel.get(channel_name) {
            let target_ref = match db.host_dispatch_state().active_rollouts_snapshot() {
                Ok(v) => v
                    .iter()
                    .find(|s| s.rollout_id == r.rollout_id)
                    .map(|s| s.target_channel_ref.clone())
                    .unwrap_or_default(),
                Err(_) => String::new(),
            };
            gauge!(
                "nixfleet_channel_status",
                "channel" => channel_name.clone(),
                "status" => "active".to_string(),
                "rollout_id" => short_id(Some(&r.rollout_id)),
                "target_ref" => short_id(Some(&target_ref)),
                "blocked_by" => String::new(),
                "reason" => String::new(),
            )
            .set(1.0);
        } else if let Some(d) = deferred_by_channel.get(channel_name) {
            gauge!(
                "nixfleet_channel_status",
                "channel" => channel_name.clone(),
                "status" => "deferred".to_string(),
                "rollout_id" => String::new(),
                "target_ref" => short_id(Some(&d.target_ref)),
                "blocked_by" => d.blocked_by.clone(),
                "reason" => d.reason.clone(),
            )
            .set(1.0);
        } else {
            gauge!(
                "nixfleet_channel_status",
                "channel" => channel_name.clone(),
                "status" => "idle".to_string(),
                "rollout_id" => String::new(),
                "target_ref" => String::new(),
                "blocked_by" => String::new(),
                "reason" => String::new(),
            )
            .set(1.0);
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
