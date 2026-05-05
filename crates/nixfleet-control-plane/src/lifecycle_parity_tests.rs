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

use crate::db::{Db, DispatchInsert};
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

/// Test-side `Observed` constructor — delegates to the production
/// `list_active_rollouts` helper, then maps each `RolloutDbSnapshot`
/// into a `Rollout` with the same shape the dispatch path uses.
///
/// Skips the `current_rollout_ids` filter and the polling-race
/// synthesis: tests in this file set up arbitrary rids (`R-edge`,
/// `R-stable`) that don't match a `compute_rollout_id_for_channel`
/// output. Those concerns have dedicated coverage —
/// `polling_race_window_observed_view_synthesizes_placeholder` and
/// the helper-direct test below.
fn build_observed(db: &Db) -> Observed {
    let active_rollouts: Vec<Rollout> = crate::observed_view::list_active_rollouts(db)
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

/// **Regression guard for the polling-race window.**
///
/// Lab observed live (2026-05-05): a fresh fleet release came in
/// while the previous edge rollout was still soaking. When edge
/// Soaked → channelEdges released → the new stable rollout was
/// expected to open. There's a ~30-60 s window between channelEdges
/// release and the next channel-refs poll recording the new
/// successor in the rollouts table. During that window:
///
///   - `list_active` does NOT include the new successor rollout
///     (polling hasn't recorded it yet)
///   - `current_rollout_ids` derived from the live fleet snapshot
///     DOES include it
///   - dispatch endpoint's gate evaluation found no rollout for
///     the host's channel → `host_edges::check`'s `let rollout =
///     input.rollout?` returned None → gate skipped
///   - first checkin slipped through, dispatched on the new
///     rollout via `decide_target` (which uses
///     `compute_rollout_id_for_channel`, not list_active)
///   - host_edges' ordering invariant violated for the
///     freshly-opened channel
///
/// The fix synthesizes empty Rollout placeholders for current-
/// fleet rollouts not yet in `list_active`. Empty `host_states`
/// → peer states default to `Queued` → host_edges fires correctly.
///
/// This test pins the synthesis. If a future commit reverts the
/// placeholder loop in `observed_view::build_for_gates` (or the
/// reconciler's equivalent), this test fails because aether's
/// dispatch on a freshly-opened stable channel passes when it
/// should be blocked by host-edges.
#[test]
fn polling_race_window_observed_view_synthesizes_placeholder() {
    use nixfleet_proto::Edge;

    let db = fresh_db();
    let mut fleet = fleet();
    fleet.meta.signed_at = Some(Utc::now());
    fleet.meta.ci_commit = Some("test-ci-commit".into());
    fleet.edges = vec![Edge {
        gated: "aether".into(),
        gates: "krach".into(),
        reason: None,
    }];
    // Add aether and krach to the fleet (the existing fixture only
    // has lab + krach). aether on stable; krach on stable; lab on
    // edge as before.
    fleet.hosts.insert(
        "aether".into(),
        nixfleet_proto::Host {
            system: "aarch64-darwin".into(),
            tags: vec!["dev".into()],
            channel: "stable".into(),
            closure_hash: Some("aether-closure".into()),
            pubkey: None,
        },
    );
    fleet.waves.insert(
        "stable".into(),
        vec![nixfleet_proto::Wave {
            hosts: vec!["krach".into(), "aether".into()],
            soak_minutes: 5,
        }],
    );
    let fleet_resolved_hash = "test-fleet-hash";

    // Set up the polling-race window:
    //   - lab on edge has reached Soaked (predecessor terminal-
    //     for-ordering, so channel_edges releases stable)
    //   - stable's current rollout (per fleet snapshot) is NOT
    //     yet in the rollouts table — channel-refs poll hasn't run
    let edge_rid = nixfleet_reconciler::compute_rollout_id_for_channel(
        &fleet,
        fleet_resolved_hash,
        "edge",
    )
    .unwrap()
    .unwrap();
    db.rollouts()
        .record_active_rollout(&edge_rid, "edge")
        .unwrap();
    db.host_dispatch_state()
        .record_dispatch(&crate::db::DispatchInsert {
            hostname: "lab",
            rollout_id: &edge_rid,
            channel: "edge",
            wave: 0,
            target_closure_hash: "lab-closure",
            target_channel_ref: &edge_rid,
            confirm_deadline: Utc::now() + chrono::Duration::minutes(10),
        })
        .unwrap();
    db.rollout_state()
        .transition_host_state(
            "lab",
            &edge_rid,
            HostRolloutState::Soaked,
            crate::state::HealthyMarker::Set(Utc::now()),
            None,
        )
        .unwrap();

    // The stable rollout the fleet snapshot expects:
    let stable_rid = nixfleet_reconciler::compute_rollout_id_for_channel(
        &fleet,
        fleet_resolved_hash,
        "stable",
    )
    .unwrap()
    .unwrap();
    // CRITICAL: stable_rid is NOT in the rollouts table — that's
    // the race window. list_active won't return it.
    assert_eq!(
        db.rollouts().list_active().unwrap().len(),
        1,
        "fixture: only edge rollout recorded; stable is the race-window unrecorded one",
    );

    // Build observed via the production builder (synchronous wrapper).
    let observed = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(crate::observed_view::build_for_gates(
            &db,
            &fleet,
            fleet_resolved_hash,
            None, // no rollouts_dir → no budgets, fine for this test
        ));

    // Synthesis assertion: stable_rid must appear in active_rollouts
    // even though it's not in list_active.
    let stable_in_observed = observed
        .active_rollouts
        .iter()
        .find(|r| r.id == stable_rid)
        .expect(
            "stable rollout MUST be synthesized as a placeholder when missing \
             from list_active — that's the polling-race-window fix",
        );
    assert_eq!(stable_in_observed.channel, "stable");
    assert!(
        stable_in_observed.host_states.is_empty(),
        "synthesized placeholder must have empty host_states so peers default to Queued",
    );
    assert!(stable_in_observed.terminal_at.is_none());

    // Now run the host_edges gate against this observed for aether.
    // Without the placeholder: input.rollout=None → gate short-
    // circuits → aether dispatches (the bug). With the placeholder:
    // input.rollout=Some(stable_rid_with_empty_host_states) → krach
    // defaults Queued → host_edges fires. This is the load-bearing
    // assertion — the entire fix exists to make this verdict stable.
    let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
    let input = nixfleet_reconciler::gates::GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(stable_in_observed),
        host: "aether",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: nixfleet_reconciler::gates::GateMode::Dispatch,
    };
    let verdict = nixfleet_reconciler::gates::evaluate_for_host(&input);
    match verdict {
        Some(nixfleet_reconciler::gates::GateBlock::HostEdge { gating_host }) => {
            assert_eq!(gating_host, "krach");
        }
        other => panic!(
            "expected HostEdge {{ gating_host: \"krach\" }} block on the synthesized \
             placeholder; got {other:?}. If this fails, the polling-race window has \
             re-opened — host_edges is bypassed for freshly-opened rollouts.",
        ),
    }
}

/// Directly pin the split `list_active_rollouts` /
/// `synthesize_polling_race_placeholders` contract.
///
/// The dispatch endpoint and reconciler both feed gates from the same
/// substrate (commit 37e8d07 closed the polling-race window). This
/// test bypasses the gate evaluation and asserts the helpers' shape
/// directly: only-list_active rows from the table, then synthesis
/// adds placeholders for current-fleet rollouts not yet recorded,
/// idempotent on second pass.
///
/// If a future commit reintroduces a parallel inline copy of either
/// helper in `server::reconcile` or `observed_view::build_for_gates`,
/// this test still passes (the helpers are correct in isolation) but
/// the existing `polling_race_window_observed_view_synthesizes_placeholder`
/// will fail, surfacing the drift. Together the two tests pin both
/// halves of the contract.
#[test]
fn helper_split_lists_active_then_synthesizes_only_unknown_current_rollouts() {
    let db = fresh_db();
    let mut fleet = fleet();
    fleet.meta.signed_at = Some(Utc::now());
    let fleet_resolved_hash = "test-fleet-hash";

    // Pre-populate one rollout the table knows about. Use the channel's
    // computed rid so the synthesis sees it as "already known."
    let edge_rid = nixfleet_reconciler::compute_rollout_id_for_channel(
        &fleet,
        fleet_resolved_hash,
        "edge",
    )
    .unwrap()
    .unwrap();
    db.rollouts()
        .record_active_rollout(&edge_rid, "edge")
        .unwrap();
    db.host_dispatch_state()
        .record_dispatch(&DispatchInsert {
            hostname: "lab",
            rollout_id: &edge_rid,
            channel: "edge",
            wave: 0,
            target_closure_hash: "lab-closure",
            target_channel_ref: &edge_rid,
            confirm_deadline: Utc::now() + chrono::Duration::minutes(10),
        })
        .unwrap();

    // Step 1: list_active_rollouts returns the one recorded row, with
    // operational state LEFT-JOINed in. No synthesis yet.
    let mut snapshots = crate::observed_view::list_active_rollouts(&db);
    assert_eq!(snapshots.len(), 1, "list_active_rollouts: only the recorded edge rollout");
    assert_eq!(snapshots[0].rollout_id, edge_rid);
    assert_eq!(snapshots[0].channel, "edge");
    assert_eq!(snapshots[0].target_closure_hash, "lab-closure");
    assert!(
        snapshots[0].host_states.contains_key("lab"),
        "list_active_rollouts must LEFT-JOIN host_dispatch_state",
    );

    // Step 2: synthesize for the fleet's two channels. Edge already
    // present (skipped); stable not present → placeholder added.
    crate::observed_view::synthesize_polling_race_placeholders(
        &mut snapshots,
        &fleet,
        fleet_resolved_hash,
    );
    let stable_rid = nixfleet_reconciler::compute_rollout_id_for_channel(
        &fleet,
        fleet_resolved_hash,
        "stable",
    )
    .unwrap()
    .unwrap();
    assert_eq!(snapshots.len(), 2, "synthesis must add stable placeholder");
    let stable = snapshots
        .iter()
        .find(|s| s.rollout_id == stable_rid)
        .expect("stable placeholder must be synthesized");
    assert_eq!(stable.channel, "stable");
    assert!(
        stable.host_states.is_empty(),
        "synthesized placeholder must have empty host_states (peers default Queued)",
    );
    assert!(stable.terminal_at.is_none());
    // Edge row preserved unchanged.
    let edge = snapshots
        .iter()
        .find(|s| s.rollout_id == edge_rid)
        .expect("edge row must survive synthesis");
    assert_eq!(edge.target_closure_hash, "lab-closure");
    assert!(edge.host_states.contains_key("lab"));

    // Step 3: idempotence — second synthesis pass is a no-op.
    crate::observed_view::synthesize_polling_race_placeholders(
        &mut snapshots,
        &fleet,
        fleet_resolved_hash,
    );
    assert_eq!(
        snapshots.len(),
        2,
        "synthesize_polling_race_placeholders must be idempotent",
    );
}
