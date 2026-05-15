//! Parity tests for the gates registry. Every gate gets a positive + negative
//! case against `evaluate_for_host` - adding a gate requires adding a parity
//! test here. CP-side parity lives in nixfleet-control-plane integration tests.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use nixfleet_proto::testing::FleetBuilder;
use nixfleet_proto::{Edge, FleetResolved, Host, RolloutBudget, Selector, Wave};

use crate::host_state::HostRolloutState;
use crate::observed::{Observed, Rollout};
use crate::rollout_state::RolloutState;

use super::{GateBlock, GateInput, evaluate_for_host};

fn empty_set() -> HashSet<String> {
    HashSet::new()
}

fn fleet_two_channels() -> FleetResolved {
    FleetBuilder::new()
        .host("host-05", "edge")
        .host_tag("host-05", "server")
        .host("host-01", "stable")
        .host_tag("host-01", "dev")
        .host("host-03", "stable")
        .host_system("host-03", "aarch64-darwin")
        .host_tag("host-03", "dev")
        .policy_strategy("p", "staged")
        .policy_wave(
            "p",
            Selector {
                tags: vec!["dev".into()],
                ..Default::default()
            },
            5,
        )
        .channel_edge("edge", "stable")
        .wave("stable", &["host-01", "host-03"])
        .build()
}

fn rollout(channel: &str, host_states: Vec<(&str, HostRolloutState)>) -> Rollout {
    rollout_with_terminal(channel, host_states, None)
}

fn rollout_with_terminal(
    channel: &str,
    host_states: Vec<(&str, HostRolloutState)>,
    terminal_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Rollout {
    Rollout {
        id: format!("rid-{channel}"),
        channel: channel.into(),
        target_ref: "ref".into(),
        state: RolloutState::Executing,
        current_wave: 0,
        host_states: host_states
            .into_iter()
            .map(|(h, s)| (h.to_string(), s))
            .collect(),
        last_healthy_since: HashMap::new(),
        budgets: vec![],
        terminal_at,
    }
}

#[test]
fn channel_edges_blocks_when_predecessor_active() {
    let fleet = fleet_two_channels();
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("host-05", HostRolloutState::Activating)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: None,
        host: "host-01",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(
        evaluate_for_host(&input),
        Some(GateBlock::ChannelEdges {
            predecessor_channel: "edge".into(),
        }),
    );
}

#[test]
fn channel_edges_passes_when_predecessor_converged() {
    let fleet = fleet_two_channels();
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("host-05", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: None,
        host: "host-01",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(evaluate_for_host(&input), None);
}

/// Regression guard: dispatch/reconciler asymmetry on terminal predecessors.
/// Filtering converged predecessors out of `observed.active_rollouts` caused
/// `Reconcile` to release while `Dispatch` blocked - hosts stuck looping.
/// Both modes must return `None` (release) when the predecessor is terminal.
/// If anyone reverts to filtering terminal rollouts out of the observed
/// builder, the conservative arm here re-fires and catches the regression.
#[test]
fn channel_edges_releases_on_terminal_predecessor_in_both_modes() {
    let fleet = fleet_two_channels();
    let now = Utc::now();
    let observed = Observed {
        active_rollouts: vec![rollout_with_terminal(
            "edge",
            vec![("host-05", HostRolloutState::Converged)],
            Some(now),
        )],
        ..Default::default()
    };
    let empty = empty_set();

    for conservative in [false, true] {
        let input = GateInput {
            fleet: &fleet,
            observed: &observed,
            rollout: None,
            host: "host-01",
            now,
            emitted_opens_in_tick: &empty,
            mode: if conservative {
                super::GateMode::Dispatch
            } else {
                super::GateMode::Reconcile
            },
        };
        assert_eq!(
            evaluate_for_host(&input),
            None,
            "channel_edges must release successor when predecessor is terminal - \
             mode_dispatch={conservative}. If this fails, terminal rollouts \
             are again being filtered out of observed.active_rollouts and \
             the dispatch/reconciler asymmetry has been re-introduced."
        );
    }
}

/// Companion: predecessor truly missing from observed (fresh-boot) is the
/// only place the two modes legitimately diverge. Together these two tests
/// pin the decision table: terminal-in-observed symmetric, missing-from-
/// observed the only asymmetry.
#[test]
fn channel_edges_diverges_only_on_truly_missing_predecessor() {
    let fleet = fleet_two_channels();
    let now = Utc::now();
    let observed = Observed::default();
    let empty = empty_set();

    let mk_input = |conservative| GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: None,
        host: "host-01",
        now,
        emitted_opens_in_tick: &empty,
        mode: if conservative {
            super::GateMode::Dispatch
        } else {
            super::GateMode::Reconcile
        },
    };

    // Conservative blocks (fresh-boot protection).
    assert_eq!(
        evaluate_for_host(&mk_input(true)),
        Some(GateBlock::ChannelEdges {
            predecessor_channel: "edge".into(),
        }),
    );
    // Reconciler allows (trusts emitted_opens_in_tick as the in-tick authority).
    assert_eq!(evaluate_for_host(&mk_input(false)), None);
}

#[test]
fn channel_edges_conservative_blocks_on_missing_predecessor_with_hosts() {
    let fleet = fleet_two_channels();
    let observed = Observed::default();
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: None,
        host: "host-01",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Dispatch,
    };
    assert_eq!(
        evaluate_for_host(&input),
        Some(GateBlock::ChannelEdges {
            predecessor_channel: "edge".into(),
        }),
    );
}

#[test]
fn wave_promotion_blocks_wave_one_when_current_is_zero() {
    let mut fleet = fleet_two_channels();
    fleet.waves.insert(
        "stable".into(),
        vec![
            Wave {
                hosts: vec!["host-01".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["host-03".into()],
                soak_minutes: 60,
            },
        ],
    );
    let r = rollout("stable", vec![]);
    assert_eq!(r.current_wave, 0);
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("host-05", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-03",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(
        evaluate_for_host(&input),
        Some(GateBlock::WavePromotion {
            host_wave: 1,
            current_wave: 0,
        }),
    );
}

#[test]
fn host_edges_blocks_until_gating_host_converges() {
    let mut fleet = fleet_two_channels();
    fleet.edges = vec![Edge {
        gated: "host-01".into(),
        gates: "host-03".into(),
        reason: None,
    }];
    let r = rollout("stable", vec![("host-03", HostRolloutState::Activating)]);
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("host-05", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-01",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(
        evaluate_for_host(&input),
        Some(GateBlock::HostEdge {
            gating_host: "host-03".into(),
        }),
    );
}

/// Regression: host-edges must fire on the FIRST checkin of a freshly-opened
/// channel. An earlier dispatch path built `Observed.active_rollouts` from
/// dispatch rows, so a brand-new rollout with empty host_states was invisible
/// and `input.rollout` collapsed to None. Empty host_states defaults peers to
/// Queued (not terminal-for-ordering), so the gate must fire even then.
#[test]
fn host_edges_fires_on_freshly_opened_rollout_with_empty_host_states() {
    let mut fleet = fleet_two_channels();
    fleet.edges = vec![Edge {
        gated: "host-01".into(),
        gates: "host-03".into(),
        reason: None,
    }];
    let r = rollout("stable", vec![]);
    let observed = Observed {
        active_rollouts: vec![r.clone()],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-01",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(
        evaluate_for_host(&input),
        Some(GateBlock::HostEdge {
            gating_host: "host-03".into(),
        }),
        "host-edges must enforce ordering even when no host has dispatched yet - \
         empty host_states defaults peers to Queued (not terminal-for-ordering)",
    );
}

#[test]
fn disruption_budget_blocks_when_at_max_in_flight() {
    let fleet = fleet_two_channels();
    let dev_selector = Selector {
        tags: vec!["dev".into()],
        ..Default::default()
    };
    let budgets = vec![RolloutBudget {
        selector: dev_selector,
        hosts: vec!["host-01".into(), "host-03".into()],
        max_in_flight: Some(1),
        max_in_flight_pct: None,
    }];
    let mut r = rollout("stable", vec![("host-01", HostRolloutState::Healthy)]);
    r.budgets = budgets.clone();
    let observed = Observed {
        active_rollouts: vec![r.clone()],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-03",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    let block = evaluate_for_host(&input);
    match block {
        Some(GateBlock::DisruptionBudget { in_flight, max, .. }) => {
            assert_eq!(in_flight, 1);
            assert_eq!(max, 1);
        }
        other => panic!("expected DisruptionBudget block, got {other:?}"),
    }
}

#[test]
fn disruption_budget_passes_when_under_max() {
    let fleet = fleet_two_channels();
    let dev_selector = Selector {
        tags: vec!["dev".into()],
        ..Default::default()
    };
    let budgets = vec![RolloutBudget {
        selector: dev_selector,
        hosts: vec!["host-01".into(), "host-03".into()],
        max_in_flight: Some(2),
        max_in_flight_pct: None,
    }];
    let mut r = rollout("stable", vec![("host-01", HostRolloutState::Healthy)]);
    r.budgets = budgets;
    let observed = Observed {
        active_rollouts: vec![r.clone()],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-03",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(evaluate_for_host(&input), None);
}

#[test]
fn host_edges_skips_cross_channel_edges() {
    // Regression: cross-channel edge would look up the peer in a different
    // rollout's host_states, default to Queued, and block forever. Cross-
    // channel edges must be no-ops at the host-edges gate.
    let mut fleet = fleet_two_channels();
    fleet.edges = vec![Edge {
        gated: "host-01".into(),
        gates: "host-05".into(),
        reason: None,
    }];
    let r = rollout("stable", vec![]);
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("host-05", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-01",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(
        evaluate_for_host(&input),
        None,
        "cross-channel host edge must NOT block",
    );
}

#[test]
fn compliance_wave_blocks_when_earlier_wave_has_failures_under_enforce() {
    let mut fleet = fleet_two_channels();
    fleet.channels.get_mut("stable").unwrap().compliance.mode = "enforce".into();
    fleet.waves.insert(
        "stable".into(),
        vec![
            Wave {
                hosts: vec!["host-01".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["host-03".into()],
                soak_minutes: 60,
            },
        ],
    );

    let mut r = rollout("stable", vec![]);
    r.current_wave = 1;
    let mut compliance_failures = HashMap::new();
    let mut by_host = HashMap::new();
    by_host.insert("host-01".to_string(), 2usize);
    compliance_failures.insert(r.id.clone(), by_host);

    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("host-05", HostRolloutState::Converged)],
        )],
        outstanding_compliance_events_by_rollout: compliance_failures,
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-03",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    let block = evaluate_for_host(&input);
    match block {
        Some(GateBlock::ComplianceWave {
            failing_events_count,
            host_wave,
        }) => {
            assert_eq!(failing_events_count, 2);
            assert_eq!(host_wave, 1);
        }
        other => panic!("expected ComplianceWave block, got {other:?}"),
    }
}

#[test]
fn compliance_wave_passes_under_permissive_mode() {
    let mut fleet = fleet_two_channels();
    fleet.channels.get_mut("stable").unwrap().compliance.mode = "permissive".into();
    fleet.waves.insert(
        "stable".into(),
        vec![
            Wave {
                hosts: vec!["host-01".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["host-03".into()],
                soak_minutes: 60,
            },
        ],
    );

    let mut r = rollout("stable", vec![]);
    r.current_wave = 1;
    let mut compliance_failures = HashMap::new();
    let mut by_host = HashMap::new();
    by_host.insert("host-01".to_string(), 5usize);
    compliance_failures.insert(r.id.clone(), by_host);

    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("host-05", HostRolloutState::Converged)],
        )],
        outstanding_compliance_events_by_rollout: compliance_failures,
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-03",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(
        evaluate_for_host(&input),
        None,
        "permissive mode must not block",
    );
}

/// 3-wave variant pinning transitivity: a failure in wave 0 must still block
/// wave 2 (the 2-wave variant only proves adjacent wave-0 -> wave-1 blocking).
/// Kind-agnostic at this layer; kind-specific coverage lives in CP integration
/// suite.
#[test]
fn compliance_wave_blocks_transitively_across_three_waves_under_enforce() {
    let mut fleet = fleet_two_channels();
    fleet.hosts.insert(
        "host-04".into(),
        Host {
            system: "x86_64-linux".into(),
            tags: vec!["dev".into()],
            channel: "stable".into(),
            closure_hash: Some("host-04-closure".into()),
            pubkey: None,
            pin: None,
        },
    );
    fleet.channels.get_mut("stable").unwrap().compliance.mode = "enforce".into();
    fleet.waves.insert(
        "stable".into(),
        vec![
            Wave {
                hosts: vec!["host-01".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["host-03".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["host-04".into()],
                soak_minutes: 60,
            },
        ],
    );

    let mut r = rollout("stable", vec![]);
    r.current_wave = 2;
    let mut events = HashMap::new();
    let mut by_host = HashMap::new();
    by_host.insert("host-01".to_string(), 1usize);
    events.insert(r.id.clone(), by_host);

    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("host-05", HostRolloutState::Converged)],
        )],
        outstanding_compliance_events_by_rollout: events,
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-04",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    match evaluate_for_host(&input) {
        Some(GateBlock::ComplianceWave {
            failing_events_count,
            host_wave,
        }) => {
            assert_eq!(failing_events_count, 1);
            assert_eq!(host_wave, 2, "host_wave must reflect host-04's wave-2 slot");
        }
        other => {
            panic!("expected ComplianceWave block from transitive wave-0 failure, got {other:?}",)
        }
    }
}

#[test]
fn empty_input_passes_all_gates() {
    let fleet = fleet_two_channels();
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("host-05", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let r = rollout("stable", vec![]);
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "host-01",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(evaluate_for_host(&input), None);
}
