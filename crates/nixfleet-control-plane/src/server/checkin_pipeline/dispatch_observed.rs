//! Shared `Observed` builder for the dispatch endpoint's gate evaluation.
//!
//! Every gate sees identical inputs at the dispatch endpoint, and a
//! single fix to filtering covers all of them.
//!
//! LOADBEARING: source `active_rollouts` from `db.rollouts().list_active()`
//! (the canonical "in-flight" list, filters BOTH superseded_at AND
//! terminal_at). Then LEFT JOIN host states from
//! `host_dispatch_state.active_rollouts_snapshot()` for the per-host
//! observable state. The opposite ordering — building from
//! host_dispatch_state and excluding superseded — fails for freshly-
//! opened channels: when channelEdges releases a new rollout, the
//! rollouts table has the row but no host has been dispatched yet, so
//! host_dispatch_state has nothing for that rollout. The dispatch path
//! then sees `input.rollout = None`, the host-edges / budget /
//! compliance gates short-circuit, and the first checkin on the new
//! channel bypasses ordering. Reading from list_active first guarantees
//! the rollout is visible to gates with `host_states = empty`, which
//! correctly defaults all peers to `Queued` (not terminal-for-ordering).
//!
//! `current_rollout_ids` filter still applies — a rollout can be in
//! flight in the table but not in the *current* fleet snapshot (e.g.,
//! mid-release race where the table caught up before the verified-
//! fleet swap). Same filter as `record_rollouts_gated_by_channel_edges`
//! in the polling layer.
use std::collections::HashMap;
use std::path::Path;

use nixfleet_proto::FleetResolved;
use nixfleet_reconciler::observed::{Observed, Rollout};
use nixfleet_reconciler::{HostRolloutState, RolloutState};

use super::super::state::AppState;

/// Build the per-checkin `Observed` for dispatch-time gate evaluation.
///
/// `rollouts_dir` is `state.rollouts_dir` — the directory CI writes
/// signed rollout manifests into. When `Some`, each active rollout's
/// `disruption_budgets` snapshot is loaded so the budget gate has the
/// frozen membership the reconciler also sees. When `None` (test
/// fixtures, CP without artifact dir), budgets are empty and the
/// budget gate no-ops — same permissive behaviour as
/// `server::reconcile::load_rollout_budgets`.
///
/// Returns a default-empty `Observed` if any DB read fails; callers
/// already handle the "no DB" / "no fleet" cases gracefully.
pub(super) async fn build_observed_for_gates(
    db: &crate::db::Db,
    fleet: &FleetResolved,
    fleet_resolved_hash: &str,
    rollouts_dir: Option<&Path>,
) -> Observed {
    let current_rollout_ids: std::collections::HashSet<String> =
        nixfleet_reconciler::current_rollout_ids(fleet, fleet_resolved_hash);

    // Canonical source of truth for "what's in flight" — rollouts table.
    // Filters superseded AND terminal in one query; gates see freshly-
    // opened rollouts (no dispatches yet) too.
    let in_flight = match db.rollouts().list_active() {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "dispatch_observed: list_active failed; gates fall back to permissive");
            return Observed::default();
        }
    };

    // Per-host observable state, keyed by rollout_id. Empty for
    // rollouts that exist in the table but haven't had a host
    // dispatch yet — gates see Queued defaults, which is correct
    // for ordering enforcement on fresh channels.
    let host_state_by_rollout: HashMap<String, crate::db::RolloutDbSnapshot> =
        match db.host_dispatch_state().active_rollouts_snapshot() {
            Ok(v) => v.into_iter().map(|s| (s.rollout_id.clone(), s)).collect(),
            Err(err) => {
                tracing::warn!(error = %err, "dispatch_observed: active_rollouts_snapshot failed; merging with empty host states");
                HashMap::new()
            }
        };

    let mut active_rollouts: Vec<Rollout> = in_flight
        .into_iter()
        .filter(|r| current_rollout_ids.contains(&r.rollout_id))
        .map(|in_flight_r| {
            let host_snap = host_state_by_rollout.get(&in_flight_r.rollout_id);
            // target_ref defaults to the rollout_id (content-addressed
            // by closure hash; the rollout_id IS the channel_ref by
            // convention). Host_dispatch_state's value wins when
            // present (preserves any consumer that distinguishes the
            // two strings; current code treats them equivalently).
            let target_ref = host_snap
                .map(|s| s.target_channel_ref.clone())
                .unwrap_or_else(|| in_flight_r.rollout_id.clone());
            let host_states = host_snap
                .map(|s| {
                    s.host_states
                        .iter()
                        .filter_map(|(h, st)| {
                            HostRolloutState::from_db_str(st)
                                .ok()
                                .map(|parsed| (h.clone(), parsed))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let last_healthy_since = host_snap
                .map(|s| s.last_healthy_since.clone())
                .unwrap_or_default();
            Rollout {
                id: in_flight_r.rollout_id,
                channel: in_flight_r.channel,
                target_ref,
                state: RolloutState::Executing,
                current_wave: in_flight_r.current_wave as usize,
                host_states,
                last_healthy_since,
                budgets: vec![],
                terminal_at: in_flight_r.terminal_at,
            }
        })
        .collect();

    if let Some(dir) = rollouts_dir {
        for r in active_rollouts.iter_mut() {
            r.budgets = load_budgets_from_manifest(dir, &r.id).await;
        }
    }

    // Compliance failures aggregated by (rollout, host). Same DB query
    // the reconciler tick uses, so the compliance_wave gate sees the
    // same input at both call sites. Permissive on read failure: the
    // gate then no-ops which is the same behaviour as the disabled
    // mode, preserving "missing data is silent" rather than surprising
    // the operator with a hard block.
    let compliance_failures_by_rollout = match db.reports().outstanding_compliance_events_by_rollout() {
        Ok(m) => m,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "dispatch_observed: outstanding_compliance_events_by_rollout failed; compliance gate no-ops",
            );
            std::collections::HashMap::new()
        }
    };

    Observed {
        active_rollouts,
        compliance_failures_by_rollout,
        ..Default::default()
    }
}

/// Wrapper that pulls the manifest dir from `AppState`. Most callers
/// have AppState handy and shouldn't have to thread the path manually.
pub(super) async fn build_observed_for_gates_from_state(
    state: &AppState,
    fleet: &FleetResolved,
    fleet_resolved_hash: &str,
) -> Observed {
    build_observed_for_gates(
        state
            .db
            .as_ref()
            .expect("dispatch_observed: caller already verified db.is_some()"),
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
                "dispatch_observed: manifest unavailable; budget gate no-ops",
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
                "dispatch_observed: manifest parse failed; budget gate no-ops",
            );
            Vec::new()
        }
    }
}
