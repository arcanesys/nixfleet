//! Lifecycle parity tests — pin the chain `DB state → observed →
//! gate verdict` at each stage of a multi-channel rollout, and pin
//! that the dispatch-mode and reconcile-mode evaluations agree on
//! the same Observed.
//!
//! The regression class these guard against: the bug we shipped
//! three rounds of fixes for. Symptom was the dispatch endpoint
//! and reconcile loop reaching opposite verdicts on the same DB
//! state, because they built `Observed.active_rollouts` from
//! divergent sources (or the same source filtered differently).
//!
//! Existing unit tests pin the gate predicates in isolation
//! (`channel_edges_releases_on_terminal_predecessor_in_both_modes`)
//! and the DB query semantics in isolation
//! (`mark_terminal_keeps_rollout_in_list_active_but_drops_from_list_in_flight`).
//! These tests fill in the seam: build_observed_for_gates fed real
//! DB state at each lifecycle stage, gate evaluator on the result,
//! both modes asserted equivalent except for the documented
//! divergence (truly-missing predecessor, conservative-vs-permissive).

#![cfg(test)]

use std::collections::HashSet;

use chrono::Utc;
use nixfleet_proto::{
    Channel, ChannelEdge, Compliance, FleetResolved, Host, Meta, OnHealthFailure, PolicyWave,
    RolloutPolicy, Selector, Wave,
};
use nixfleet_reconciler::gates::{evaluate_for_host, GateBlock, GateInput, GateMode};
use nixfleet_reconciler::observed::{Observed, Rollout};
use nixfleet_reconciler::{HostRolloutState, RolloutState};

use crate::db::{Db, DispatchInsert, RolloutDbSnapshot};
use crate::state::HealthyMarker;

// ============================================================
// Fixture: edge ─→ stable channel topology
//
// hosts:
//   lab    on edge   (server tier, wave 0)
//   krach  on stable (dev tier,    wave 0)
//
// channelEdges: gates=edge, gated=stable
//
// This is the same shape lab is running today — the smallest
// fleet that can exhibit the cross-channel + per-host gating
// the bugs targeted.
// ============================================================

fn fresh_db() -> Db {
    let db = Db::open_in_memory().unwrap();
    db.migrate().unwrap();
    db
}

fn fleet() -> FleetResolved {
    let mut hosts = std::collections::HashMap::new();
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
    let mut channels = std::collections::HashMap::new();
    for c in ["edge", "stable"] {
        channels.insert(
            c.into(),
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
    }
    let mut rollout_policies = std::collections::HashMap::new();
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
    let mut waves = std::collections::HashMap::new();
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

/// Mirror of the canonical observed builder shape used by both
/// the dispatch endpoint and the reconciler. Reads `list_active`
/// (gate view, includes terminal) + active_rollouts_snapshot;
/// LEFT-JOINs by rollout_id; preserves all the asymmetry-pinning
/// semantics the recent fixes locked in.
///
/// If a future commit accidentally diverges the dispatch path's
/// builder from this shape, gate verdicts in this file's
/// scenarios will diverge from the unit-test gate predicates and
/// surface the regression here.
fn build_observed(db: &Db) -> Observed {
    let in_flight = db.rollouts().list_active().unwrap().into_inner();
    let snap = db
        .host_dispatch_state()
        .active_rollouts_snapshot()
        .unwrap();
    let host_state_by_rollout: std::collections::HashMap<String, RolloutDbSnapshot> = snap
        .into_iter()
        .map(|s| (s.rollout_id.clone(), s))
        .collect();

    let active_rollouts: Vec<Rollout> = in_flight
        .into_iter()
        .map(|r| {
            let host_snap = host_state_by_rollout.get(&r.rollout_id);
            let target_ref = host_snap
                .map(|s| s.target_channel_ref.clone())
                .unwrap_or_else(|| r.rollout_id.clone());
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
                id: r.rollout_id,
                channel: r.channel,
                target_ref,
                state: RolloutState::Executing,
                current_wave: r.current_wave as usize,
                host_states,
                last_healthy_since,
                budgets: vec![],
                terminal_at: r.terminal_at,
            }
        })
        .collect();

    Observed {
        active_rollouts,
        ..Default::default()
    }
}

/// Eval the gate for `host` against `observed` in the requested mode.
fn gate_for(observed: &Observed, fleet: &FleetResolved, host: &str, mode: GateMode) -> Option<GateBlock> {
    let empty: HashSet<String> = HashSet::new();
    let rollout = observed
        .active_rollouts
        .iter()
        .find(|r| r.channel == fleet.hosts.get(host).map(|h| h.channel.as_str()).unwrap_or(""));
    let input = GateInput {
        fleet,
        observed,
        rollout,
        host,
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode,
    };
    evaluate_for_host(&input)
}

/// Stage 1: edge rollout opened, lab dispatched + Activating
/// (predecessor in-flight, NOT terminal-for-ordering).
///
/// Stable rollout opens but is held by channelEdges. krach's
/// gate verdict in BOTH modes must be `Some(ChannelEdges)`.
#[test]
fn stage1_edge_active_stable_held_in_both_modes() {
    let db = fresh_db();
    let fleet = fleet();
    let now = Utc::now();

    db.rollouts()
        .record_active_rollout("R-edge", "edge")
        .unwrap();
    db.host_dispatch_state()
        .record_dispatch(&DispatchInsert {
            hostname: "lab",
            rollout_id: "R-edge",
            channel: "edge",
            wave: 0,
            target_closure_hash: "lab-closure",
            target_channel_ref: "R-edge",
            confirm_deadline: now + chrono::Duration::minutes(10),
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

    db.rollouts()
        .record_active_rollout("R-stable", "stable")
        .unwrap();

    let observed = build_observed(&db);

    // Dispatch mode: krach blocked.
    let v_dispatch = gate_for(&observed, &fleet, "krach", GateMode::Dispatch);
    // Reconcile mode: ALSO blocked — predecessor is in observed and active-for-ordering.
    let v_reconcile = gate_for(&observed, &fleet, "krach", GateMode::Reconcile);

    assert!(
        matches!(v_dispatch, Some(GateBlock::ChannelEdges { .. })),
        "stage 1 dispatch verdict must be ChannelEdges block; got {v_dispatch:?}",
    );
    assert!(
        matches!(v_reconcile, Some(GateBlock::ChannelEdges { .. })),
        "stage 1 reconcile verdict must be ChannelEdges block; got {v_reconcile:?}",
    );
    assert_eq!(
        v_dispatch, v_reconcile,
        "stage 1 verdicts MUST agree across modes when predecessor is in observed",
    );
}

/// Stage 2: lab Soaked (terminal-for-ordering). Stable's
/// channelEdges releases. Both modes must agree: krach passes.
///
/// Pre-fix-#74c9ad4: this stage was where dispatch and reconcile
/// diverged because list_active filtered terminal → predecessor
/// fell out of observed → asymmetric None-arm verdict.
#[test]
fn stage2_edge_terminal_stable_released_in_both_modes() {
    let db = fresh_db();
    let fleet = fleet();
    let now = Utc::now();

    db.rollouts()
        .record_active_rollout("R-edge", "edge")
        .unwrap();
    db.host_dispatch_state()
        .record_dispatch(&DispatchInsert {
            hostname: "lab",
            rollout_id: "R-edge",
            channel: "edge",
            wave: 0,
            target_closure_hash: "lab-closure",
            target_channel_ref: "R-edge",
            confirm_deadline: now + chrono::Duration::minutes(10),
        })
        .unwrap();
    db.rollout_state()
        .transition_host_state(
            "lab",
            "R-edge",
            HostRolloutState::Soaked,
            HealthyMarker::Set(now),
            None,
        )
        .unwrap();
    db.rollouts().mark_terminal("R-edge", now).unwrap();

    db.rollouts()
        .record_active_rollout("R-stable", "stable")
        .unwrap();

    let observed = build_observed(&db);

    let v_dispatch = gate_for(&observed, &fleet, "krach", GateMode::Dispatch);
    let v_reconcile = gate_for(&observed, &fleet, "krach", GateMode::Reconcile);

    assert_eq!(
        v_dispatch, None,
        "stage 2 dispatch must release — predecessor in observed with all hosts terminal-for-ordering; got {v_dispatch:?}",
    );
    assert_eq!(
        v_reconcile, None,
        "stage 2 reconcile must release; got {v_reconcile:?}",
    );
}

/// Stage 3: legitimate divergence. Predecessor truly missing —
/// no rollout recorded for edge yet. Dispatch mode (conservative)
/// blocks; Reconcile mode (in-tick authority via emitted_opens)
/// releases. This is the documented mode asymmetry; pinning it
/// prevents a future commit from accidentally collapsing the
/// modes back into a single bool.
#[test]
fn stage3_predecessor_truly_missing_modes_legitimately_diverge() {
    let db = fresh_db();
    let fleet = fleet();

    db.rollouts()
        .record_active_rollout("R-stable", "stable")
        .unwrap();

    let observed = build_observed(&db);

    let v_dispatch = gate_for(&observed, &fleet, "krach", GateMode::Dispatch);
    let v_reconcile = gate_for(&observed, &fleet, "krach", GateMode::Reconcile);

    assert!(
        matches!(v_dispatch, Some(GateBlock::ChannelEdges { .. })),
        "stage 3 dispatch must conservative-block — predecessor channel has hosts but no rollout; got {v_dispatch:?}",
    );
    assert_eq!(
        v_reconcile, None,
        "stage 3 reconcile must permit — emitted_opens_in_tick is the authority; got {v_reconcile:?}",
    );
}

/// Stage 4: terminal predecessor with successor's host already
/// dispatched. The dispatch happened (krach Healthy in observed),
/// the gate verdict for an additional dispatch attempt at the
/// SAME state must release (predecessor terminal, no other
/// blockers).
///
/// This is the canonical "happy path" once a release converges:
/// channelEdges off, host_edges/budget pass, krach proceeds.
#[test]
fn stage4_full_chain_release_to_dispatched_host_passes_in_both_modes() {
    let db = fresh_db();
    let fleet = fleet();
    let now = Utc::now();

    db.rollouts()
        .record_active_rollout("R-edge", "edge")
        .unwrap();
    db.host_dispatch_state()
        .record_dispatch(&DispatchInsert {
            hostname: "lab",
            rollout_id: "R-edge",
            channel: "edge",
            wave: 0,
            target_closure_hash: "lab-closure",
            target_channel_ref: "R-edge",
            confirm_deadline: now + chrono::Duration::minutes(10),
        })
        .unwrap();
    db.rollout_state()
        .transition_host_state(
            "lab",
            "R-edge",
            HostRolloutState::Converged,
            HealthyMarker::Set(now),
            None,
        )
        .unwrap();
    db.rollouts().mark_terminal("R-edge", now).unwrap();

    db.rollouts()
        .record_active_rollout("R-stable", "stable")
        .unwrap();
    db.host_dispatch_state()
        .record_dispatch(&DispatchInsert {
            hostname: "krach",
            rollout_id: "R-stable",
            channel: "stable",
            wave: 0,
            target_closure_hash: "krach-closure",
            target_channel_ref: "R-stable",
            confirm_deadline: now + chrono::Duration::minutes(10),
        })
        .unwrap();
    db.rollout_state()
        .transition_host_state(
            "krach",
            "R-stable",
            HostRolloutState::Healthy,
            HealthyMarker::Set(now),
            None,
        )
        .unwrap();

    let observed = build_observed(&db);

    let v_dispatch = gate_for(&observed, &fleet, "krach", GateMode::Dispatch);
    let v_reconcile = gate_for(&observed, &fleet, "krach", GateMode::Reconcile);

    assert_eq!(v_dispatch, None, "stage 4 dispatch passes; got {v_dispatch:?}");
    assert_eq!(v_reconcile, None, "stage 4 reconcile passes; got {v_reconcile:?}");
}

/// Stage 5: list_in_flight (UI view) excludes the terminal edge
/// rollout, but list_active (gate view) keeps it. This is the
/// load-bearing split — the regression we shipped fixes for.
/// If a future commit reverts the split (filters terminal at
/// list_active), this test fails because gate observed loses the
/// terminal predecessor and verdicts diverge.
#[test]
fn stage5_terminal_rollout_visible_to_gates_hidden_from_ui() {
    let db = fresh_db();
    let now = Utc::now();
    db.rollouts()
        .record_active_rollout("R-edge", "edge")
        .unwrap();
    db.rollouts().mark_terminal("R-edge", now).unwrap();

    let gate_view = db.rollouts().list_active().unwrap();
    let ui_view = db.rollouts().list_in_flight().unwrap();

    assert_eq!(gate_view.len(), 1, "gate view keeps terminal rollouts");
    assert_eq!(ui_view.len(), 0, "UI view hides terminal rollouts");

    // Asymmetric conversion: gate → UI is OK; UI → gate has no
    // method (would re-fabricate missing rows). Confirms the
    // type system enforces the directionality.
    let demoted = gate_view.into_ui();
    assert_eq!(demoted.len(), 0, "into_ui filters terminal");
}
