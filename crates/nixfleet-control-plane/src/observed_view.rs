//! Canonical `Observed` builder for fleet-level gate evaluation.
//!
//! Sibling of `state_view` (per-host status view) and `deferrals_view`
//! (per-channel deferral view). Three callers consume it:
//!
//!   - `server::checkin_pipeline::dispatch_target` — per-checkin gate
//!     evaluation (calls `build_for_gates`).
//!   - `server::reconcile` — per-tick gate evaluation (calls
//!     `list_active_rollouts` and feeds the result through
//!     `observed_projection::project`).
//!   - `metrics::record_disruption_budgets` — uses the same Observed
//!     for `in_flight_count` so the metric and the gate verdict
//!     never disagree.
//!
//! LOADBEARING source ordering: `db.rollouts().list_active()` first
//! (filters superseded only; terminal rollouts stay visible so gates
//! can detect predecessor convergence), then LEFT JOIN host states
//! from `host_dispatch_state.active_rollouts_snapshot()`. The
//! opposite ordering — building from host_dispatch_state — drops
//! freshly-opened rollouts that have no dispatched hosts yet, and
//! the first checkin on a new channel bypasses host-edges / budget
//! / compliance because `input.rollout = None` short-circuits.
//!
//! `current_rollout_ids` filter applies asymmetrically: dispatch
//! callers filter (gates only enforce on the active dispatch target),
//! the reconciler caller does not (it must see non-current in-flight
//! rollouts so `sweep_terminal_orphans` and ConvergeRollout fire on
//! stragglers).

use std::collections::HashMap;
use std::path::Path;

use nixfleet_proto::FleetResolved;
use nixfleet_reconciler::observed::{Observed, Rollout};
use nixfleet_reconciler::{HostRolloutState, RolloutState};

use crate::db::{Db, RolloutDbSnapshot};
use crate::server::AppState;

/// `rollouts.list_active()` LEFT JOINed with `host_dispatch_state.active_rollouts_snapshot()`.
///
/// Canonical "what's in flight" view shared by reconciler and dispatch. Rows
/// without operational state get an empty-host_states snapshot (correct for
/// freshly-opened rollouts that haven't dispatched yet — peers default Queued,
/// gates fire correctly). Empty vec on DB read failure (caller's permissive
/// path: gates no-op rather than hard-blocking).
pub fn list_active_rollouts(db: &Db) -> Vec<RolloutDbSnapshot> {
    let in_flight = match db.rollouts().list_active() {
        Ok(v) => v.into_inner(),
        Err(err) => {
            tracing::warn!(error = %err, "observed_view: list_active failed; treating as empty");
            return Vec::new();
        }
    };

    let host_state_by_rollout: HashMap<String, RolloutDbSnapshot> =
        match db.host_dispatch_state().active_rollouts_snapshot() {
            Ok(v) => v.into_iter().map(|s| (s.rollout_id.clone(), s)).collect(),
            Err(err) => {
                tracing::warn!(error = %err, "observed_view: active_rollouts_snapshot failed; merging with empty host states");
                HashMap::new()
            }
        };

    in_flight
        .into_iter()
        .map(|r| match host_state_by_rollout.get(&r.rollout_id) {
            Some(snap) => RolloutDbSnapshot {
                rollout_id: r.rollout_id,
                channel: r.channel,
                target_closure_hash: snap.target_closure_hash.clone(),
                target_channel_ref: snap.target_channel_ref.clone(),
                host_states: snap.host_states.clone(),
                last_healthy_since: snap.last_healthy_since.clone(),
                current_wave: r.current_wave,
                terminal_at: r.terminal_at,
            },
            None => RolloutDbSnapshot {
                rollout_id: r.rollout_id.clone(),
                channel: r.channel,
                target_closure_hash: String::new(),
                target_channel_ref: r.rollout_id,
                host_states: HashMap::new(),
                last_healthy_since: HashMap::new(),
                current_wave: r.current_wave,
                terminal_at: r.terminal_at,
            },
        })
        .collect()
}

/// Build the per-checkin / per-scrape `Observed` for fleet-level gate
/// evaluation.
///
/// `rollouts_dir` is `state.rollouts_dir` — the directory CI writes
/// signed rollout manifests into. When `Some`, each active rollout's
/// `disruption_budgets` snapshot is loaded so the budget gate has the
/// frozen membership the reconciler also sees. When `None` (test
/// fixtures, CP without artifact dir, or scrape-time use that doesn't
/// need budgets), budgets are empty and the budget gate no-ops — same
/// permissive behaviour as `server::reconcile::load_rollout_budgets`.
///
/// Returns a default-empty `Observed` if any DB read fails; callers
/// already handle the "no DB" / "no fleet" cases gracefully.
pub async fn build_for_gates(
    db: &Db,
    fleet: &FleetResolved,
    fleet_resolved_hash: &str,
    rollouts_dir: Option<&Path>,
) -> Observed {
    let current_rollout_ids: std::collections::HashSet<String> =
        nixfleet_reconciler::current_rollout_ids(fleet, fleet_resolved_hash);

    // Filter to current rollouts (gates only enforce on the active
    // dispatch target) and convert each snapshot to a typed `Rollout`.
    let mut active_rollouts: Vec<Rollout> = list_active_rollouts(db)
        .into_iter()
        .filter(|s| current_rollout_ids.contains(&s.rollout_id))
        .map(|s| Rollout {
            id: s.rollout_id,
            channel: s.channel,
            target_ref: s.target_channel_ref,
            state: RolloutState::Executing,
            current_wave: s.current_wave as usize,
            // Unknown SQL strings drop silently here (gate-side); the
            // reconciler-side projection logs and falls back to Failed.
            // Same data, different recovery posture: gates default-
            // permissive on parse failure, reconciler default-halt.
            host_states: s
                .host_states
                .iter()
                .filter_map(|(h, st)| {
                    HostRolloutState::from_db_str(st)
                        .ok()
                        .map(|parsed| (h.clone(), parsed))
                })
                .collect(),
            last_healthy_since: s.last_healthy_since,
            budgets: vec![],
            terminal_at: s.terminal_at,
        })
        .collect();

    if let Some(dir) = rollouts_dir {
        for r in active_rollouts.iter_mut() {
            r.budgets = load_budgets_from_manifest(dir, &r.id).await;
        }
    }

    // Outstanding compliance events aggregated by (rollout, host). Same
    // DB query the reconciler tick uses, so the compliance_wave gate
    // sees the same input at both call sites. Aggregates BOTH
    // ComplianceFailure and RuntimeGateError events — kind is the SQL
    // filter, not visible past this seam. Permissive on read failure:
    // the gate then no-ops which matches `disabled` mode, preserving
    // "missing data is silent" rather than surprising the operator
    // with a hard block.
    let outstanding_compliance_events_by_rollout = match db
        .reports()
        .outstanding_compliance_events_by_rollout()
    {
        Ok(m) => m,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "observed_view: outstanding_compliance_events_by_rollout failed; compliance gate no-ops",
            );
            std::collections::HashMap::new()
        }
    };

    Observed {
        active_rollouts,
        outstanding_compliance_events_by_rollout,
        ..Default::default()
    }
}

/// Wrapper that pulls the manifest dir from `AppState`. Most callers
/// have AppState handy and shouldn't have to thread the path manually.
pub async fn build_for_gates_from_state(
    state: &AppState,
    fleet: &FleetResolved,
    fleet_resolved_hash: &str,
) -> Observed {
    build_for_gates(
        state
            .db
            .as_ref()
            .expect("observed_view: caller already verified db.is_some()"),
        fleet,
        fleet_resolved_hash,
        state.rollouts_dir.as_deref(),
    )
    .await
}

/// Load `disruption_budgets` from a single rollout manifest. Permissive on
/// failure: missing/corrupt manifest → empty budgets → budget gate
/// no-ops for this rollout. Mirrors `server::reconcile::load_rollout_budgets`.
async fn load_budgets_from_manifest(
    dir: &Path,
    rollout_id: &str,
) -> Vec<nixfleet_proto::RolloutBudget> {
    let manifest_path = dir.join(format!("{rollout_id}.json"));
    let bytes = match tokio::fs::read(&manifest_path).await {
        Ok(b) => b,
        Err(err) => {
            tracing::debug!(
                rollout = %rollout_id,
                path = %manifest_path.display(),
                error = %err,
                "observed_view: manifest unavailable; budget gate no-ops",
            );
            return Vec::new();
        }
    };
    match serde_json::from_slice::<nixfleet_proto::RolloutManifest>(&bytes) {
        Ok(m) => m.disruption_budgets,
        Err(err) => {
            tracing::warn!(
                rollout = %rollout_id,
                error = %err,
                "observed_view: manifest parse failed; budget gate no-ops",
            );
            Vec::new()
        }
    }
}
