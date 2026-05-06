//! Canonical `Observed` builder for fleet-level gate evaluation.
//! Consumed by dispatch checkin, reconcile tick, and disruption-budget metrics.
//!
//! LOADBEARING source ordering: `rollouts.list_active()` (keeps terminal — gates
//! need them to detect predecessor convergence) LEFT JOINed with
//! `host_dispatch_state.active_rollouts_snapshot()`. Opposite ordering would
//! drop freshly-opened rollouts and bypass host-edges/budget/compliance gates.
//!
//! `current_rollout_ids` filter: dispatch callers apply it (gates enforce on
//! active dispatch target only), reconciler does not (needs non-current
//! in-flight rollouts for `sweep_terminal_orphans` + `ConvergeRollout`).

use std::collections::HashMap;
use std::path::Path;

use nixfleet_proto::{FleetResolved, RolloutBudget};
use nixfleet_reconciler::observed::{Observed, Rollout};
use nixfleet_reconciler::{HostRolloutState, RolloutState};

use crate::db::{Db, RolloutDbSnapshot};
use crate::server::AppState;

/// What to do when a SQL `host_state` string isn't in `HostRolloutState`.
/// Gate path uses `Drop` (default-permissive: gate no-ops on bad data);
/// reconciler projection uses `Halt` (default-conservative: warn + halt
/// the rollout via Failed fallback). The variance is intentional.
#[derive(Debug, Clone, Copy)]
pub enum ParseUnknown {
    Drop,
    Halt,
}

/// `RolloutDbSnapshot` → `Rollout`. Shared between the gate observed
/// builder (`build_for_gates`) and the reconciler projection
/// (`observed_projection::project`); they differ only in `parse` and
/// in how budgets are sourced.
pub fn snapshot_to_rollout(
    snap: &RolloutDbSnapshot,
    budgets: Vec<RolloutBudget>,
    parse: ParseUnknown,
) -> Rollout {
    let host_states = snap
        .host_states
        .iter()
        .filter_map(|(h, s)| match HostRolloutState::from_db_str(s) {
            Ok(parsed) => Some((h.clone(), parsed)),
            Err(_) => match parse {
                ParseUnknown::Drop => None,
                ParseUnknown::Halt => {
                    tracing::warn!(
                        rollout = %snap.rollout_id,
                        hostname = %h,
                        unknown_state = %s,
                        "host_rollout_state value not in HostRolloutState enum — \
                         halting rollout (Failed fallback). Likely a SQL CHECK \
                         extension that wasn't propagated to the typed enum.",
                    );
                    Some((h.clone(), HostRolloutState::Failed))
                }
            },
        })
        .collect();
    Rollout {
        id: snap.rollout_id.clone(),
        channel: snap.channel.clone(),
        target_ref: snap.target_channel_ref.clone(),
        state: RolloutState::Executing,
        current_wave: snap.current_wave as usize,
        host_states,
        last_healthy_since: snap.last_healthy_since.clone(),
        budgets,
        terminal_at: snap.terminal_at,
    }
}

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
                host_waves: snap.host_waves.clone(),
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
                host_waves: HashMap::new(),
                last_healthy_since: HashMap::new(),
                current_wave: r.current_wave,
                terminal_at: r.terminal_at,
            },
        })
        .collect()
}

/// `rollouts_dir = Some(d)` loads frozen `disruption_budgets` per rollout from
/// signed manifests; `None` → budget gate no-ops (same as missing manifest).
/// Permissive on DB read failure: callers handle "no DB" / "no fleet" upstream.
pub async fn build_for_gates(
    db: &Db,
    fleet: &FleetResolved,
    fleet_resolved_hash: &str,
    rollouts_dir: Option<&Path>,
) -> Observed {
    let current_rollout_ids: std::collections::HashSet<String> =
        nixfleet_reconciler::current_rollout_ids(fleet, fleet_resolved_hash);

    // Filter to current rollouts (gates only enforce on the active
    // dispatch target).
    let mut active_rollouts: Vec<Rollout> = list_active_rollouts(db)
        .into_iter()
        .filter(|s| current_rollout_ids.contains(&s.rollout_id))
        .map(|s| snapshot_to_rollout(&s, Vec::new(), ParseUnknown::Drop))
        .collect();

    if let Some(dir) = rollouts_dir {
        for r in active_rollouts.iter_mut() {
            r.budgets = load_budgets_from_manifest(dir, &r.id).await;
        }
    }

    // Same query as the reconciler tick — both call sites see identical input.
    // Permissive on read failure: gate no-ops (matches `disabled` mode).
    let outstanding_compliance_events_by_rollout = db
        .reports()
        .outstanding_compliance_events_by_rollout()
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, "observed_view: outstanding_compliance_events_by_rollout failed; gate no-ops");
            std::collections::HashMap::new()
        });

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
