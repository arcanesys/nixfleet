//! Parity tests for the gates registry.
//!
//! Each gate gets a positive case (gate fires) and a negative case (gate
//! passes) verified against `evaluate_for_host`. These prove the
//! behaviour-of-record at the registry level — if you add a gate, you
//! add a parity test here. CP-side parity (reconciler emits Skip,
//! dispatch endpoint returns None for the same Observed) is enforced
//! by integration tests in nixfleet-control-plane.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use nixfleet_proto::{
    Channel, ChannelEdge, Compliance, Edge, FleetResolved, Host, Meta, OnHealthFailure,
    PolicyWave, RolloutBudget, RolloutPolicy, Selector, Wave,
};

use crate::host_state::HostRolloutState;
use crate::observed::{Observed, Rollout};
use crate::rollout_state::RolloutState;

use super::{evaluate_for_host, GateBlock, GateInput};

fn empty_set() -> HashSet<String> {
    HashSet::new()
}

fn fleet_two_channels() -> FleetResolved {
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
    hosts.insert(
        "aether".into(),
        Host {
            system: "aarch64-darwin".into(),
            tags: vec!["dev".into()],
            channel: "stable".into(),
            closure_hash: Some("aether-closure".into()),
            pubkey: None,
        },
    );

    let mut channels = HashMap::new();
    for ch in ["edge", "stable"] {
        channels.insert(
            ch.into(),
            Channel {
                rollout_policy: "p".into(),
                reconcile_interval_minutes: 30,
                freshness_window: 1440,
                signing_interval_minutes: 60,
                compliance: Compliance {
                    frameworks: vec![],
                    mode: "disabled".into(),
                },
            },
        );
    }

    let mut rollout_policies = HashMap::new();
    rollout_policies.insert(
        "p".into(),
        RolloutPolicy {
            strategy: "staged".into(),
            waves: vec![PolicyWave {
                selector: Selector {
                    tags: vec!["dev".into()],
                    ..Default::default()
                },
                soak_minutes: 5,
            }],
            health_gate: Default::default(),
            on_health_failure: OnHealthFailure::Halt,
        },
    );

    let mut waves = HashMap::new();
    waves.insert(
        "stable".into(),
        vec![Wave {
            hosts: vec!["krach".into(), "aether".into()],
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
            signature_algorithm: Some("ed25519".into()),
        },
    }
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
            vec![("lab", HostRolloutState::Activating)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: None,
        host: "krach",
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
            vec![("lab", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: None,
        host: "krach",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(evaluate_for_host(&input), None);
}

/// **Regression guard for the dispatch/reconciler asymmetry that
/// surfaced after the first lifecycle attempt.** When a converged
/// predecessor was filtered out of `observed.active_rollouts`,
/// channel_edges fell into the `None` arm and answered differently:
///
///   - reconciler (`GateMode::Reconcile`) → release
///   - dispatch endpoint (`GateMode::Dispatch`) → block
///
/// The reconciler emitted `DispatchHost`, the dispatch endpoint
/// refused — krach stuck in an infinite loop on lab.
///
/// The fix keeps terminal rollouts visible in observed (filtered
/// only from the UI surface). This test pins both modes returning
/// `None` (release) when the predecessor is terminal — same input,
/// same verdict, regardless of mode.
///
/// If anyone reverts to filtering terminal rollouts at the gate
/// observed builder, the conservative arm of this test will start
/// returning `ChannelEdges` (block) and the test fails — preventing
/// the regression from shipping.
#[test]
fn channel_edges_releases_on_terminal_predecessor_in_both_modes() {
    let fleet = fleet_two_channels();
    let now = Utc::now();
    // Predecessor is in observed, terminal_at stamped, all hosts
    // soaked-or-converged (terminal-for-ordering). The predecessor
    // is functionally done; both modes must agree.
    let observed = Observed {
        active_rollouts: vec![rollout_with_terminal(
            "edge",
            vec![
                ("lab", HostRolloutState::Converged),
            ],
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
            host: "krach",
            now,
            emitted_opens_in_tick: &empty,
            mode: if conservative { super::GateMode::Dispatch } else { super::GateMode::Reconcile },
        };
        assert_eq!(
            evaluate_for_host(&input),
            None,
            "channel_edges must release successor when predecessor is terminal — \
             mode_dispatch={conservative}. If this fails, terminal rollouts \
             are again being filtered out of observed.active_rollouts and \
             the dispatch/reconciler asymmetry has been re-introduced."
        );
    }
}

/// **Companion regression guard.** The asymmetry-permitting case:
/// predecessor genuinely missing from observed (fresh-boot, no
/// rollouts recorded yet). Conservative blocks, non-conservative
/// allows. The two test cases together pin the entire decision
/// table: terminal-in-observed must be symmetric, missing-from-observed
/// is the only place the modes legitimately diverge.
#[test]
fn channel_edges_diverges_only_on_truly_missing_predecessor() {
    let fleet = fleet_two_channels();
    let now = Utc::now();
    // Empty observed = predecessor truly unknown.
    let observed = Observed::default();
    let empty = empty_set();

    let mk_input = |conservative| GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: None,
        host: "krach",
        now,
        emitted_opens_in_tick: &empty,
        mode: if conservative { super::GateMode::Dispatch } else { super::GateMode::Reconcile },
    };

    // Conservative: blocks (fresh-boot protection — predecessor's
    // existence in fleet means dispatch should hold until polling
    // catches up).
    assert_eq!(
        evaluate_for_host(&mk_input(true)),
        Some(GateBlock::ChannelEdges {
            predecessor_channel: "edge".into(),
        }),
    );
    // Non-conservative: allows (reconciler trusts emitted_opens_in_tick
    // as the in-tick authority; absence means "not opened this tick").
    assert_eq!(evaluate_for_host(&mk_input(false)), None);
}

#[test]
fn channel_edges_conservative_blocks_on_missing_predecessor_with_hosts() {
    // Fresh-boot scenario: predecessor channel has hosts in fleet but
    // no rollout recorded yet. Dispatch endpoint sets
    // GateMode::Dispatch to block until polling
    // populates state.
    let fleet = fleet_two_channels();
    let observed = Observed::default();
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: None,
        host: "krach",
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
    // Add a second wave for stable so krach is wave 0, aether is wave 1.
    fleet.waves.insert(
        "stable".into(),
        vec![
            Wave {
                hosts: vec!["krach".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["aether".into()],
                soak_minutes: 60,
            },
        ],
    );
    let r = rollout("stable", vec![]);
    assert_eq!(r.current_wave, 0);
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("lab", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "aether",
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
    // fleet.edges = [{ gated: krach, gates: aether }]
    // krach's dispatch is held until aether reaches Soaked/Converged.
    let mut fleet = fleet_two_channels();
    fleet.edges = vec![Edge {
        gated: "krach".into(),
        gates: "aether".into(),
        reason: None,
    }];
    let r = rollout(
        "stable",
        vec![("aether", HostRolloutState::Activating)], // aether not yet Soaked/Converged
    );
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("lab", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "krach",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(
        evaluate_for_host(&input),
        Some(GateBlock::HostEdge {
            gating_host: "aether".into(),
        }),
    );
}

/// Regression: the host-edges gate must fire on the FIRST checkin
/// of a freshly-opened channel (rollout exists, no host has dispatched
/// yet). Earlier the dispatch path built `Observed.active_rollouts`
/// from `host_dispatch_state.active_rollouts_snapshot()` which is
/// keyed by dispatch rows — a brand-new rollout with empty host_states
/// was invisible, `input.rollout` collapsed to None, and host_edges
/// short-circuited. The new builder reads from `rollouts.list_active()`
/// and merges host_states LEFT-JOIN-style; this test pins the gate's
/// behaviour when host_states is empty: gating peer defaults to
/// Queued, which is NOT terminal-for-ordering, so the gate fires.
#[test]
fn host_edges_fires_on_freshly_opened_rollout_with_empty_host_states() {
    // fleet.edges = [{ gated: krach, gates: aether }]
    let mut fleet = fleet_two_channels();
    fleet.edges = vec![Edge {
        gated: "krach".into(),
        gates: "aether".into(),
        reason: None,
    }];
    // Rollout is "in flight" (visible to gates) but no host has
    // dispatched yet — empty host_states. This is the channelEdges-
    // just-released window.
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
        host: "krach",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(
        evaluate_for_host(&input),
        Some(GateBlock::HostEdge {
            gating_host: "aether".into(),
        }),
        "host-edges must enforce ordering even when no host has dispatched yet — \
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
        hosts: vec!["krach".into(), "aether".into()],
        max_in_flight: Some(1),
        max_in_flight_pct: None,
    }];
    let mut r = rollout("stable", vec![("krach", HostRolloutState::Healthy)]);
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
        host: "aether",
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
        hosts: vec!["krach".into(), "aether".into()],
        max_in_flight: Some(2),
        max_in_flight_pct: None,
    }];
    let mut r = rollout("stable", vec![("krach", HostRolloutState::Healthy)]);
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
        host: "aether",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(evaluate_for_host(&input), None);
}

#[test]
fn host_edges_skips_cross_channel_edges() {
    // Regression: Edge { before: krach (stable), after: lab (edge) }
    // would look up lab in stable's rollout.host_states (always None),
    // default to Queued, and block krach forever. The cross-channel
    // guard treats such edges as no-ops.
    let mut fleet = fleet_two_channels();
    fleet.edges = vec![Edge {
        gated: "krach".into(),
        gates: "lab".into(),
        reason: None,
    }];
    let r = rollout("stable", vec![]);
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("lab", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "krach",
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
    fleet
        .channels
        .get_mut("stable")
        .unwrap()
        .compliance
        .mode = "enforce".into();
    fleet.waves.insert(
        "stable".into(),
        vec![
            Wave {
                hosts: vec!["krach".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["aether".into()],
                soak_minutes: 60,
            },
        ],
    );

    let mut r = rollout("stable", vec![]);
    r.current_wave = 1; // wave_promotion gate must pass for aether (wave 1)
    let mut compliance_failures = HashMap::new();
    let mut by_host = HashMap::new();
    by_host.insert("krach".to_string(), 2usize);
    compliance_failures.insert(r.id.clone(), by_host);

    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("lab", HostRolloutState::Converged)],
        )],
        outstanding_compliance_events_by_rollout: compliance_failures,
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "aether",
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
    fleet
        .channels
        .get_mut("stable")
        .unwrap()
        .compliance
        .mode = "permissive".into();
    fleet.waves.insert(
        "stable".into(),
        vec![
            Wave {
                hosts: vec!["krach".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["aether".into()],
                soak_minutes: 60,
            },
        ],
    );

    let mut r = rollout("stable", vec![]);
    r.current_wave = 1; // wave_promotion gate must pass for aether (wave 1)
    let mut compliance_failures = HashMap::new();
    let mut by_host = HashMap::new();
    by_host.insert("krach".to_string(), 5usize);
    compliance_failures.insert(r.id.clone(), by_host);

    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("lab", HostRolloutState::Converged)],
        )],
        outstanding_compliance_events_by_rollout: compliance_failures,
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "aether",
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

/// 3-wave fleet, wave-0 host has outstanding evidence events, host
/// being evaluated is in wave-2 (NOT immediately adjacent to the
/// failing wave). Proves the gate's range predicate is transitive
/// — a failure two waves back still blocks. The 2-wave variant
/// above only proves wave-0 → wave-1 (adjacent) blocking.
///
/// `outstanding_compliance_events_by_rollout` is kind-agnostic: this
/// test fires identically whether the underlying event is a
/// `ComplianceFailure` or a `RuntimeGateError`. Kind-specific
/// end-to-end coverage lives in the CP integration suite
/// (`tests/wave_gate.rs`).
#[test]
fn compliance_wave_blocks_transitively_across_three_waves_under_enforce() {
    let mut fleet = fleet_two_channels();
    // Add pixel as a third stable-channel host so we can split the
    // stable wave plan into three single-host waves.
    fleet.hosts.insert(
        "pixel".into(),
        Host {
            system: "x86_64-linux".into(),
            tags: vec!["dev".into()],
            channel: "stable".into(),
            closure_hash: Some("pixel-closure".into()),
            pubkey: None,
        },
    );
    fleet
        .channels
        .get_mut("stable")
        .unwrap()
        .compliance
        .mode = "enforce".into();
    fleet.waves.insert(
        "stable".into(),
        vec![
            Wave {
                hosts: vec!["krach".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["aether".into()],
                soak_minutes: 5,
            },
            Wave {
                hosts: vec!["pixel".into()],
                soak_minutes: 60,
            },
        ],
    );

    let mut r = rollout("stable", vec![]);
    r.current_wave = 2; // wave_promotion gate must pass for pixel (wave 2)
    let mut events = HashMap::new();
    let mut by_host = HashMap::new();
    // krach in wave-0 has 1 outstanding event — kind doesn't matter
    // at this layer (DB-side filter is the kind-discriminator).
    by_host.insert("krach".to_string(), 1usize);
    events.insert(r.id.clone(), by_host);

    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("lab", HostRolloutState::Converged)],
        )],
        outstanding_compliance_events_by_rollout: events,
        ..Default::default()
    };
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "pixel",
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
            assert_eq!(host_wave, 2, "host_wave must reflect pixel's wave-2 slot");
        }
        other => panic!(
            "expected ComplianceWave block from transitive wave-0 failure, got {other:?}",
        ),
    }
}

#[test]
fn empty_input_passes_all_gates() {
    let fleet = fleet_two_channels();
    let observed = Observed {
        active_rollouts: vec![rollout(
            "edge",
            vec![("lab", HostRolloutState::Converged)],
        )],
        ..Default::default()
    };
    let r = rollout("stable", vec![]);
    let empty = empty_set();
    let input = GateInput {
        fleet: &fleet,
        observed: &observed,
        rollout: Some(&r),
        host: "krach",
        now: Utc::now(),
        emitted_opens_in_tick: &empty,
        mode: super::GateMode::Reconcile,
    };
    assert_eq!(evaluate_for_host(&input), None);
}
