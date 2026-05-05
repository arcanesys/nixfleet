//! CP-startup invariant pass: reset host_rollout_state rows whose
//! state was written under gate semantics that no longer hold.
//!
//! ## Why
//!
//! Two failure modes can leave `host_rollout_state` in a state the
//! current gate semantics would not have produced:
//!
//!   1. **Recovery partial-write history.** Before the recovery.rs
//!      atomic txn fix, `synthesise_orphan_confirm_rows` could
//!      commit a `confirmed` operational row but fail to write the
//!      Healthy marker. The next reconcile saw the host as Healthy
//!      with NULL `last_healthy_since`; the soak timer never fired.
//!      Existing dirty rows from that era persist on lab today.
//!
//!   2. **Pre-fix race-bug dispatches.** Hosts that dispatched
//!      under earlier gate code (when host_edges didn't fire on
//!      freshly-opened rollouts, or when channel_edges' None-arm
//!      was asymmetric across modes) wrote `Healthy` / `Soaked` /
//!      `Converged` rows that the post-fix gates would have
//!      blocked. The dispatch already happened; the row records it
//!      faithfully — but the row's existence implies a sequence
//!      that the live gate semantics no longer permit.
//!
//! ## What
//!
//! On CP boot, after the verified-fleet snapshot is primed and
//! before the reconcile loop's first tick:
//!
//!   For every non-Queued `host_rollout_state` row whose rollout is
//!   still in flight (`list_in_flight()` — excludes superseded AND
//!   terminal), run the gate registry against the host in `Dispatch`
//!   mode. If the gates emit a block, the row is dirty: reset to
//!   `Queued` and log a `WARN` so the operator sees the recovery.
//!
//! `Dispatch` mode is the right choice: we're asking "would the
//! current gates have ALLOWED the dispatch that produced this row?"
//! `Reconcile` mode would over-permit (it trusts emitted_opens_in_tick,
//! which is empty here) and under-block valid resets.
//!
//! ## What this is NOT
//!
//! Not a periodic sweep. Runs once per CP startup. The reconcile
//! loop's normal action stream + the recovery.rs atomic txn fix
//! prevent NEW dirty rows from being written; this pass cleans up
//! HISTORICAL ones. After the next clean release cycle, nothing
//! should ever require this pass to act — but it stays as a safety
//! net for future gate-semantics-changing fixes.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use nixfleet_reconciler::observed::{Observed, Rollout};
use nixfleet_reconciler::{HostRolloutState, RolloutState};

use crate::state::HealthyMarker;

use super::state::AppState;

/// Walk in-flight rollouts; reset any non-Queued host_rollout_state
/// row that the current gates (in `Dispatch` mode) would have
/// blocked. Logs WARN per reset.
///
/// Idempotent: safe to call multiple times. Skips silently if the
/// verified-fleet snapshot isn't loaded yet (caller should run
/// after `verify_fleet_only` prime succeeds; if prime failed the
/// pass is a no-op until the next CP boot).
pub(super) async fn reset_dirty_host_rollout_state(state: &Arc<AppState>) {
    let snapshot = match state.verified_fleet.read().await.clone() {
        Some(s) => s,
        None => {
            tracing::debug!(
                target: "startup",
                "startup invariant pass: verified-fleet snapshot not primed; skipping",
            );
            return;
        }
    };
    let Some(db) = state.db.as_deref() else {
        return;
    };

    let observed = match build_observed_for_invariant_check(db).await {
        Some(o) => o,
        None => {
            tracing::debug!(
                target: "startup",
                "startup invariant pass: observed builder returned None (DB read failed); skipping",
            );
            return;
        }
    };

    let in_flight = match db.rollouts().list_in_flight() {
        Ok(v) => v.into_inner(),
        Err(err) => {
            tracing::warn!(
                target: "startup",
                error = %err,
                "startup invariant pass: list_in_flight failed; skipping",
            );
            return;
        }
    };

    let now = Utc::now();
    let empty_emitted_opens: HashSet<String> = HashSet::new();
    let mut reset_count: usize = 0;

    for in_flight_r in in_flight {
        let Some(rollout) = observed
            .active_rollouts
            .iter()
            .find(|r| r.id == in_flight_r.rollout_id)
        else {
            continue;
        };

        // Snapshot the host_states we want to check — non-Queued
        // entries on this rollout. Cloned so we don't hold a borrow
        // across the reset write.
        let dirty_candidates: Vec<(String, HostRolloutState)> = rollout
            .host_states
            .iter()
            .filter(|(_, s)| **s != HostRolloutState::Queued)
            .map(|(h, s)| (h.clone(), *s))
            .collect();

        for (host, prev_state) in dirty_candidates {
            let input = nixfleet_reconciler::gates::GateInput {
                fleet: &snapshot.fleet,
                observed: &observed,
                rollout: Some(rollout),
                host: &host,
                now,
                emitted_opens_in_tick: &empty_emitted_opens,
                // Dispatch mode = "would the current gates have
                // allowed the dispatch behind this row?". Reconcile
                // mode would over-permit on missing-predecessor.
                mode: nixfleet_reconciler::gates::GateMode::Dispatch,
            };
            let Some(block) = nixfleet_reconciler::gates::evaluate_for_host(&input) else {
                continue;
            };

            // Gate would block. The row's existence implies a
            // dispatch the current semantics forbid. Reset to Queued.
            match db.rollout_state().transition_host_state(
                &host,
                &in_flight_r.rollout_id,
                HostRolloutState::Queued,
                HealthyMarker::Untouched,
                None,
            ) {
                Ok(0) => {
                    tracing::debug!(
                        target: "startup",
                        host = %host,
                        rollout = %in_flight_r.rollout_id,
                        "startup invariant pass: reset query matched 0 rows (concurrent reconciler write?)",
                    );
                }
                Ok(_) => {
                    reset_count += 1;
                    tracing::warn!(
                        target: "startup",
                        host = %host,
                        rollout = %in_flight_r.rollout_id,
                        prev_state = %prev_state.as_db_str(),
                        gate = %block.discriminator(),
                        reason = %block.reason(),
                        "startup invariant pass: reset host_rollout_state to Queued — \
                         current gate semantics would have blocked the dispatch behind this row",
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        target: "startup",
                        host = %host,
                        rollout = %in_flight_r.rollout_id,
                        error = %err,
                        "startup invariant pass: reset failed",
                    );
                }
            }
        }
    }

    if reset_count > 0 {
        tracing::warn!(
            target: "startup",
            reset_count,
            "startup invariant pass complete — reset N dirty host_rollout_state rows",
        );
    } else {
        tracing::info!(
            target: "startup",
            "startup invariant pass complete — no dirty rows found",
        );
    }
}

/// Mirror of `dispatch_observed::build_observed_for_gates` at the
/// data shape — sourced from `list_active()` (gate view) merged
/// with `host_dispatch_state.active_rollouts_snapshot()` for host
/// states.
///
/// Skips the rollout-budget manifest read (gate evaluation here
/// only needs channel_edges + host_edges; budget gate would
/// degrade gracefully on empty `budgets`).
async fn build_observed_for_invariant_check(db: &crate::db::Db) -> Option<Observed> {
    let in_flight = db.rollouts().list_active().ok()?;
    let snap = db.host_dispatch_state().active_rollouts_snapshot().ok()?;
    let host_state_by_rollout: std::collections::HashMap<String, crate::db::RolloutDbSnapshot> =
        snap.into_iter()
            .map(|s| (s.rollout_id.clone(), s))
            .collect();

    let active_rollouts: Vec<Rollout> = in_flight
        .into_iter()
        .map(|in_flight_r| {
            let host_snap = host_state_by_rollout.get(&in_flight_r.rollout_id);
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

    Some(Observed {
        active_rollouts,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::state::HealthyMarker;
    use nixfleet_proto::{
        Channel, ChannelEdge, Compliance, FleetResolved, Host, Meta, OnHealthFailure, PolicyWave,
        RolloutPolicy, Selector, Wave,
    };
    use std::collections::HashMap;

    fn fresh_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.migrate().unwrap();
        db
    }

    fn fleet_with_channel_edge() -> FleetResolved {
        let mut hosts = HashMap::new();
        hosts.insert(
            "lab".into(),
            Host {
                system: "x86_64-linux".into(),
                tags: vec!["server".into()],
                channel: "edge".into(),
                closure_hash: Some("lab-closure".into()),
                pubkey: None,
            },
        );
        hosts.insert(
            "krach".into(),
            Host {
                system: "x86_64-linux".into(),
                tags: vec!["dev".into()],
                channel: "stable".into(),
                closure_hash: Some("krach-closure".into()),
                pubkey: None,
            },
        );
        let mut channels = HashMap::new();
        channels.insert(
            "edge".into(),
            Channel {
                rollout_policy: "p".into(),
                reconcile_interval_minutes: 30,
                signing_interval_minutes: 60,
                freshness_window: 1440,
                compliance: Compliance {
                    mode: "disabled".into(),
                    frameworks: vec![],
                },
            },
        );
        channels.insert(
            "stable".into(),
            Channel {
                rollout_policy: "p".into(),
                reconcile_interval_minutes: 30,
                signing_interval_minutes: 60,
                freshness_window: 1440,
                compliance: Compliance {
                    mode: "disabled".into(),
                    frameworks: vec![],
                },
            },
        );
        let mut rollout_policies = HashMap::new();
        rollout_policies.insert(
            "p".into(),
            RolloutPolicy {
                strategy: "all-at-once".into(),
                waves: vec![PolicyWave {
                    selector: Selector::default(),
                    soak_minutes: 5,
                }],
                health_gate: nixfleet_proto::HealthGate::default(),
                on_health_failure: OnHealthFailure::Halt,
            },
        );
        let mut waves = HashMap::new();
        waves.insert(
            "edge".into(),
            vec![Wave {
                hosts: vec!["lab".into()],
                soak_minutes: 5,
            }],
        );
        waves.insert(
            "stable".into(),
            vec![Wave {
                hosts: vec!["krach".into()],
                soak_minutes: 5,
            }],
        );
        FleetResolved {
            schema_version: 1,
            hosts,
            channels,
            rollout_policies,
            waves,
            edges: vec![],
            // krach (stable) gated by lab (edge): lab must converge
            // before stable can dispatch. This is the "dirty row"
            // setup — krach has Healthy state from a pre-fix
            // dispatch, but lab's edge rollout is still in flight,
            // so the gate WOULD now block krach.
            channel_edges: vec![ChannelEdge {
                gates: "edge".into(),
                gated: "stable".into(),
                reason: None,
            }],
            disruption_budgets: vec![],
            meta: Meta {
                schema_version: 1,
                signed_at: None,
                ci_commit: None,
                signature_algorithm: None,
            },
        }
    }

    /// **Regression guard**: dirty `Healthy` row on a successor
    /// channel whose predecessor is still in-flight (channel_edges
    /// would block) gets reset to Queued.
    #[test]
    fn invariant_pass_resets_row_blocked_by_channel_edges() {
        // Build a fresh DB representing the post-race-bug state:
        //   - edge rollout R-edge in flight, lab is Activating
        //     (predecessor not converged — still active for ordering)
        //   - stable rollout R-stable in flight, krach has Healthy
        //     state from a dispatch the post-fix channel_edges
        //     would have blocked
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("R-edge", "edge")
            .unwrap();
        db.rollouts()
            .record_active_rollout("R-stable", "stable")
            .unwrap();
        // Dispatch lab on edge (Activating — not terminal-for-ordering).
        db.host_dispatch_state()
            .record_dispatch(&crate::db::DispatchInsert {
                hostname: "lab",
                rollout_id: "R-edge",
                channel: "edge",
                wave: 0,
                target_closure_hash: "lab-closure",
                target_channel_ref: "R-edge",
                confirm_deadline: chrono::Utc::now() + chrono::Duration::minutes(10),
            })
            .unwrap();
        db.rollout_state()
            .transition_host_state(
                "lab",
                "R-edge",
                HostRolloutState::Activating,
                HealthyMarker::Untouched,
                None,
            )
            .unwrap();
        // Dispatch krach on stable (Healthy from pre-fix race-bug
        // dispatch — current gates would have blocked it because
        // edge is not converged).
        db.host_dispatch_state()
            .record_dispatch(&crate::db::DispatchInsert {
                hostname: "krach",
                rollout_id: "R-stable",
                channel: "stable",
                wave: 0,
                target_closure_hash: "krach-closure",
                target_channel_ref: "R-stable",
                confirm_deadline: chrono::Utc::now() + chrono::Duration::minutes(10),
            })
            .unwrap();
        db.rollout_state()
            .transition_host_state(
                "krach",
                "R-stable",
                HostRolloutState::Healthy,
                HealthyMarker::Set(chrono::Utc::now()),
                None,
            )
            .unwrap();

        // Build a minimal AppState-like setup. We can't easily
        // construct a full AppState in a unit test, so test the
        // build_observed_for_invariant_check + gate-eval path
        // directly.
        let observed =
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(build_observed_for_invariant_check(&db))
                .unwrap();
        let fleet = fleet_with_channel_edge();

        // Sanity: krach has Healthy state in observed.
        let stable = observed
            .active_rollouts
            .iter()
            .find(|r| r.id == "R-stable")
            .expect("stable rollout in observed");
        assert_eq!(
            stable.host_states.get("krach"),
            Some(&HostRolloutState::Healthy),
            "fixture set up krach=Healthy on stable",
        );

        // Gate evaluation in Dispatch mode: should block krach
        // because the edge predecessor is still active.
        let empty: HashSet<String> = HashSet::new();
        let input = nixfleet_reconciler::gates::GateInput {
            fleet: &fleet,
            observed: &observed,
            rollout: Some(stable),
            host: "krach",
            now: chrono::Utc::now(),
            emitted_opens_in_tick: &empty,
            mode: nixfleet_reconciler::gates::GateMode::Dispatch,
        };
        let block = nixfleet_reconciler::gates::evaluate_for_host(&input);
        assert!(
            block.is_some(),
            "channel_edges must block krach: edge predecessor is still Activating",
        );
    }

    /// Negative case: a row whose dispatch the current gates would
    /// allow is left untouched. Pinned so a too-aggressive future
    /// invariant pass doesn't nuke valid Healthy rows.
    #[test]
    fn invariant_pass_leaves_consistent_rows_untouched() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("R-stable-only", "stable")
            .unwrap();
        // krach Healthy — but no edge rollout exists (no predecessor
        // to block on). Channel_edges in Dispatch mode falls into
        // the conservative arm, BUT fleet_with_channel_edge declares
        // edge as a channel with hosts → conservative blocks. To
        // make this test isolate the "valid row" case, use a fleet
        // WITHOUT channel_edges.
        db.host_dispatch_state()
            .record_dispatch(&crate::db::DispatchInsert {
                hostname: "krach",
                rollout_id: "R-stable-only",
                channel: "stable",
                wave: 0,
                target_closure_hash: "krach-closure",
                target_channel_ref: "R-stable-only",
                confirm_deadline: chrono::Utc::now() + chrono::Duration::minutes(10),
            })
            .unwrap();
        db.rollout_state()
            .transition_host_state(
                "krach",
                "R-stable-only",
                HostRolloutState::Healthy,
                HealthyMarker::Set(chrono::Utc::now()),
                None,
            )
            .unwrap();

        let observed =
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(build_observed_for_invariant_check(&db))
                .unwrap();
        let mut fleet = fleet_with_channel_edge();
        // Strip channel_edges — krach is on stable, no predecessor.
        fleet.channel_edges.clear();

        let stable = observed
            .active_rollouts
            .iter()
            .find(|r| r.id == "R-stable-only")
            .unwrap();
        let empty: HashSet<String> = HashSet::new();
        let input = nixfleet_reconciler::gates::GateInput {
            fleet: &fleet,
            observed: &observed,
            rollout: Some(stable),
            host: "krach",
            now: chrono::Utc::now(),
            emitted_opens_in_tick: &empty,
            mode: nixfleet_reconciler::gates::GateMode::Dispatch,
        };
        assert_eq!(
            nixfleet_reconciler::gates::evaluate_for_host(&input),
            None,
            "no channel_edges, no host_edges → krach Healthy is consistent; gate must pass",
        );
    }
}
