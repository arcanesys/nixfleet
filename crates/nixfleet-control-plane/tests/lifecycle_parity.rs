//! Pin `DB state → observed → gate verdict` parity across dispatch/reconcile modes.
//! The two modes legitimately diverge only on truly-missing predecessor data
//! (conservative vs permissive); every other stage must agree.

use std::collections::HashSet;

use chrono::Utc;
use nixfleet_proto::testing::FleetBuilder;
use nixfleet_proto::{FleetResolved, Meta};
use nixfleet_reconciler::gates::{evaluate_for_host, GateBlock, GateInput, GateMode};
use nixfleet_reconciler::observed::{Observed, Rollout};
use nixfleet_reconciler::{HostRolloutState, RolloutState};

use nixfleet_control_plane::db::{Db, DispatchInsert};
use nixfleet_control_plane::state::HealthyMarker;

// Fixture: edge ─→ stable. lab on edge (server, wave 0), krach on
// stable (dev, wave 0). Smallest shape exercising channelEdges +
// per-host gating; mirrors lab's topology.

fn fresh_db() -> Db {
    let db = Db::open_in_memory().unwrap();
    db.migrate().unwrap();
    db
}

fn fleet() -> FleetResolved {
    FleetBuilder::new()
        .host("lab", "edge")
        .host_tag("lab", "server")
        .host("krach", "stable")
        .host_tag("krach", "dev")
        .channel_edge("edge", "stable")
        .wave("edge", &["lab"])
        .wave("stable", &["krach"])
        // Match historical fixture: signature_algorithm cleared.
        .meta(Meta {
            schema_version: 1,
            signed_at: None,
            ci_commit: None,
            signature_algorithm: None,
        })
        .build()
}

/// Test-side `Observed` constructor - delegates to the production
/// `list_active_rollouts` helper, then maps each `RolloutDbSnapshot`
/// into a `Rollout` with the same shape the dispatch path uses.
///
/// Skips the `current_rollout_ids` filter: tests in this file use
/// arbitrary rids (`R-edge`, `R-stable`) that don't match a
/// `compute_rollout_id_for_channel` output.
fn build_observed(db: &Db) -> Observed {
    let active_rollouts: Vec<Rollout> =
        nixfleet_control_plane::observed_view::list_active_rollouts(db)
            .into_iter()
            .map(|s| Rollout {
                id: s.rollout_id,
                channel: s.channel,
                target_ref: s.target_channel_ref,
                state: RolloutState::Executing,
                current_wave: s.current_wave as usize,
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

    Observed {
        active_rollouts,
        ..Default::default()
    }
}

/// Eval the gate for `host` against `observed` in the requested mode.
fn gate_for(
    observed: &Observed,
    fleet: &FleetResolved,
    host: &str,
    mode: GateMode,
) -> Option<GateBlock> {
    let empty: HashSet<String> = HashSet::new();
    let rollout = observed.active_rollouts.iter().find(|r| {
        r.channel
            == fleet
                .hosts
                .get(host)
                .map(|h| h.channel.as_str())
                .unwrap_or("")
    });
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
    // Reconcile mode: ALSO blocked - predecessor is in observed and active-for-ordering.
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
        "stage 2 dispatch must release - predecessor in observed with all hosts terminal-for-ordering; got {v_dispatch:?}",
    );
    assert_eq!(
        v_reconcile, None,
        "stage 2 reconcile must release; got {v_reconcile:?}",
    );
}

/// Stage 3: legitimate divergence. Predecessor truly missing  -
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
        "stage 3 dispatch must conservative-block - predecessor channel has hosts but no rollout; got {v_dispatch:?}",
    );
    assert_eq!(
        v_reconcile, None,
        "stage 3 reconcile must permit - emitted_opens_in_tick is the authority; got {v_reconcile:?}",
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

    assert_eq!(
        v_dispatch, None,
        "stage 4 dispatch passes; got {v_dispatch:?}"
    );
    assert_eq!(
        v_reconcile, None,
        "stage 4 reconcile passes; got {v_reconcile:?}"
    );
}

/// Stage 5: list_in_flight (UI view) excludes the terminal edge
/// rollout, but list_active (gate view) keeps it. This is the
/// load-bearing split. Reverting it (filtering terminal out of
/// list_active) would lose the terminal predecessor in gate
/// observed and dispatch-vs-reconcile verdicts would diverge.
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
