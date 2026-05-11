//! Per-host state machine. Emits actions and tracks wave soaked-ness.

use crate::observed::{Observed, Rollout};
use crate::Action;
use chrono::{DateTime, Utc};
use nixfleet_proto::{FleetResolved, Wave};

pub use nixfleet_proto::HostRolloutState;

/// Defaults absent hosts to [`HostRolloutState::Queued`].
pub fn lookup_host_state(rollout: &Rollout, host: &str) -> HostRolloutState {
    rollout
        .host_states
        .get(host)
        .copied()
        .unwrap_or(HostRolloutState::Queued)
}

pub(crate) struct WaveOutcome {
    pub actions: Vec<Action>,
    pub wave_all_soaked: bool,
}

pub(crate) fn handle_wave(
    fleet: &FleetResolved,
    observed: &Observed,
    rollout: &Rollout,
    wave: &Wave,
    now: DateTime<Utc>,
) -> WaveOutcome {
    let mut out = WaveOutcome {
        actions: Vec::new(),
        wave_all_soaked: true,
    };

    for host in &wave.hosts {
        let state = lookup_host_state(rollout, host);
        match state {
            HostRolloutState::Queued => {
                out.wave_all_soaked = false;
                let online = observed
                    .host_state
                    .get(host)
                    .map(|h| h.online)
                    .unwrap_or(false);
                if !online {
                    out.actions.push(Action::Skip {
                        host: host.clone(),
                        reason: "offline".into(),
                    });
                    continue;
                }
                // Fleet-level gates (channelEdges, wave-promotion, host-edges,
                // disruption-budget). Same evaluator the dispatch endpoint
                // calls; if Skip is emitted here, the agent's checkin path
                // returns None for the same reason. Reconciler runs after
                // polling, so missing predecessor state is genuinely "not
                // declared" and shouldn't pre-block every channel.
                let empty_emitted_opens = std::collections::HashSet::new();
                let gate_input = crate::gates::GateInput {
                    fleet,
                    observed,
                    rollout: Some(rollout),
                    host,
                    now,
                    emitted_opens_in_tick: &empty_emitted_opens,
                    mode: crate::gates::GateMode::Reconcile,
                };
                if let Some(block) = crate::gates::evaluate_for_host(&gate_input) {
                    out.actions.push(Action::Skip {
                        host: host.clone(),
                        reason: block.reason(),
                    });
                    continue;
                }
                out.actions.push(Action::DispatchHost {
                    rollout: rollout.id.clone(),
                    host: host.clone(),
                    target_ref: rollout.target_ref.clone(),
                });
            }
            HostRolloutState::Dispatched
            | HostRolloutState::Activating
            | HostRolloutState::ConfirmWindow => {
                out.wave_all_soaked = false;
            }
            HostRolloutState::Healthy => {
                // Healthy → Soaked requires both `wave.soak_minutes` elapsed
                // AND probes passing. Missing `last_healthy_since` keeps the
                // gate closed (wait rather than promote on missing data).
                // Probes-passing absent from the map defaults to true so a
                // missing projection can't stall every wave (fail-open).
                out.wave_all_soaked = false;
                let soak_window = chrono::Duration::minutes(wave.soak_minutes as i64);
                if let Some(since) = rollout.last_healthy_since.get(host) {
                    let soak_elapsed = now.signed_duration_since(*since) >= soak_window;
                    let probes_pass = observed
                        .host_probes_passing
                        .get(host)
                        .copied()
                        .unwrap_or(true);
                    if soak_elapsed && probes_pass {
                        out.actions.push(Action::SoakHost {
                            rollout: rollout.id.clone(),
                            host: host.clone(),
                        });
                    }
                }
            }
            HostRolloutState::Soaked | HostRolloutState::Converged => {}
            HostRolloutState::Failed | HostRolloutState::Reverted => {
                // Both halt; only `Failed` triggers a fresh RollbackHost
                // (Reverted is already rolled back).
                out.wave_all_soaked = false;
                if let Some(chan) = fleet.channels.get(&rollout.channel) {
                    if let Some(policy) = fleet.rollout_policies.get(&chan.rollout_policy) {
                        out.actions.push(Action::HaltRollout {
                            rollout: rollout.id.clone(),
                            reason: format!(
                                "host {host} {} (policy: {})",
                                state.as_db_str().to_lowercase(),
                                policy.on_health_failure
                            ),
                        });
                        if matches!(
                            policy.on_health_failure,
                            nixfleet_proto::OnHealthFailure::RollbackAndHalt
                        ) && matches!(state, HostRolloutState::Failed)
                        {
                            out.actions.push(Action::RollbackHost {
                                rollout: rollout.id.clone(),
                                host: host.clone(),
                                target_ref: rollout.target_ref.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_defaults_absent_to_queued() {
        use crate::rollout_state::RolloutState;
        let rollout = Rollout {
            id: "r".into(),
            channel: "c".into(),
            target_ref: "ref".into(),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states: std::collections::HashMap::new(),
            last_healthy_since: std::collections::HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        };
        assert_eq!(
            lookup_host_state(&rollout, "missing"),
            HostRolloutState::Queued
        );
    }

    fn fleet_with_policy(on_health_failure: nixfleet_proto::OnHealthFailure) -> FleetResolved {
        use nixfleet_proto::testing::FleetBuilder;
        use nixfleet_proto::Selector;

        FleetBuilder::new()
            .host("host-a", "stable")
            .host_no_closure("host-a")
            .channel_compliance("stable", "permissive", &[])
            .policy_wave(
                "p",
                Selector {
                    all: true,
                    ..Default::default()
                },
                0,
            )
            .policy_on_failure("p", on_health_failure)
            .build()
    }

    fn rollout_with_state(host: &str, state: HostRolloutState) -> Rollout {
        use crate::rollout_state::RolloutState;
        let mut host_states = std::collections::HashMap::new();
        host_states.insert(host.into(), state);
        Rollout {
            id: "stable@abc12345".into(),
            channel: "stable".into(),
            target_ref: "ref-xyz".into(),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states,
            last_healthy_since: std::collections::HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        }
    }

    fn observed_online(host: &str) -> Observed {
        observed_with_probes(host, true)
    }

    fn observed_with_probes(host: &str, probes_passing: bool) -> Observed {
        use crate::observed::HostState;
        let mut host_state = std::collections::HashMap::new();
        host_state.insert(
            host.into(),
            HostState {
                online: true,
                current_generation: None,
            },
        );
        let mut probes = std::collections::HashMap::new();
        probes.insert(host.to_string(), probes_passing);
        Observed {
            channel_refs: std::collections::HashMap::new(),
            last_rolled_refs: std::collections::HashMap::new(),
            host_state,
            active_rollouts: vec![],
            outstanding_compliance_events_by_rollout: std::collections::HashMap::new(),
            last_deferrals: std::collections::HashMap::new(),
            host_probes_passing: probes,
        }
    }

    fn rollout_healthy_for(host: &str, since_minutes_ago: i64) -> Rollout {
        use crate::rollout_state::RolloutState;
        let mut host_states = std::collections::HashMap::new();
        host_states.insert(host.into(), HostRolloutState::Healthy);
        let mut last_healthy = std::collections::HashMap::new();
        last_healthy.insert(
            host.into(),
            chrono::Utc::now() - chrono::Duration::minutes(since_minutes_ago),
        );
        Rollout {
            id: "stable@abc12345".into(),
            channel: "stable".into(),
            target_ref: "ref-xyz".into(),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states,
            last_healthy_since: last_healthy,
            budgets: vec![],
            terminal_at: None,
        }
    }

    #[test]
    fn soak_promotes_when_probes_pass_and_window_elapsed() {
        let fleet = fleet_with_policy(nixfleet_proto::OnHealthFailure::Halt);
        let rollout = rollout_healthy_for("host-a", 10);
        let observed = observed_with_probes("host-a", true);
        let wave = Wave {
            hosts: vec!["host-a".into()],
            soak_minutes: 5,
        };
        let outcome = handle_wave(&fleet, &observed, &rollout, &wave, chrono::Utc::now());
        let soak_count = outcome
            .actions
            .iter()
            .filter(|a| matches!(a, Action::SoakHost { .. }))
            .count();
        assert_eq!(soak_count, 1, "actions: {:?}", outcome.actions);
    }

    /// Regression: a failing probe must block Healthy → Soaked regardless
    /// of soak elapsed.
    #[test]
    fn soak_holds_when_probes_failing_even_if_window_elapsed() {
        let fleet = fleet_with_policy(nixfleet_proto::OnHealthFailure::Halt);
        let rollout = rollout_healthy_for("host-a", 10);
        let observed = observed_with_probes("host-a", false);
        let wave = Wave {
            hosts: vec!["host-a".into()],
            soak_minutes: 5,
        };
        let outcome = handle_wave(&fleet, &observed, &rollout, &wave, chrono::Utc::now());
        let soak_count = outcome
            .actions
            .iter()
            .filter(|a| matches!(a, Action::SoakHost { .. }))
            .count();
        assert_eq!(
            soak_count, 0,
            "probe-failing host must not promote: actions={:?}",
            outcome.actions
        );
        assert!(!outcome.wave_all_soaked);
    }

    #[test]
    fn soak_holds_when_window_not_elapsed_even_if_probes_pass() {
        let fleet = fleet_with_policy(nixfleet_proto::OnHealthFailure::Halt);
        let rollout = rollout_healthy_for("host-a", 1);
        let observed = observed_with_probes("host-a", true);
        let wave = Wave {
            hosts: vec!["host-a".into()],
            soak_minutes: 5,
        };
        let outcome = handle_wave(&fleet, &observed, &rollout, &wave, chrono::Utc::now());
        assert!(
            outcome
                .actions
                .iter()
                .all(|a| !matches!(a, Action::SoakHost { .. })),
            "soak window NOT elapsed must hold even with probes passing",
        );
    }

    /// Fail-open: hosts absent from the probes map default to passing.
    #[test]
    fn soak_promotes_when_probes_absent_from_map_fail_open() {
        let fleet = fleet_with_policy(nixfleet_proto::OnHealthFailure::Halt);
        let rollout = rollout_healthy_for("host-a", 10);
        let observed = observed_online("host-a");
        let mut observed = observed;
        observed.host_probes_passing.clear();
        let wave = Wave {
            hosts: vec!["host-a".into()],
            soak_minutes: 5,
        };
        let outcome = handle_wave(&fleet, &observed, &rollout, &wave, chrono::Utc::now());
        assert!(
            outcome
                .actions
                .iter()
                .any(|a| matches!(a, Action::SoakHost { .. })),
            "absent map entry must default to passing, not block",
        );
    }

    #[test]
    fn failed_under_halt_emits_only_halt_rollout() {
        let fleet = fleet_with_policy(nixfleet_proto::OnHealthFailure::Halt);
        let rollout = rollout_with_state("host-a", HostRolloutState::Failed);
        let observed = observed_online("host-a");
        let wave = Wave {
            hosts: vec!["host-a".into()],
            soak_minutes: 0,
        };
        let outcome = handle_wave(&fleet, &observed, &rollout, &wave, Utc::now());
        let halt_count = outcome
            .actions
            .iter()
            .filter(|a| matches!(a, Action::HaltRollout { .. }))
            .count();
        let rollback_count = outcome
            .actions
            .iter()
            .filter(|a| matches!(a, Action::RollbackHost { .. }))
            .count();
        assert_eq!(
            halt_count, 1,
            "halt expected; actions={:?}",
            outcome.actions
        );
        assert_eq!(
            rollback_count, 0,
            "no RollbackHost under `halt`; actions={:?}",
            outcome.actions
        );
    }

    #[test]
    fn failed_under_rollback_and_halt_emits_both_actions() {
        let fleet = fleet_with_policy(nixfleet_proto::OnHealthFailure::RollbackAndHalt);
        let rollout = rollout_with_state("host-a", HostRolloutState::Failed);
        let observed = observed_online("host-a");
        let wave = Wave {
            hosts: vec!["host-a".into()],
            soak_minutes: 0,
        };
        let outcome = handle_wave(&fleet, &observed, &rollout, &wave, Utc::now());
        let halt = outcome
            .actions
            .iter()
            .find(|a| matches!(a, Action::HaltRollout { .. }))
            .expect("HaltRollout still emitted");
        let rb = outcome
            .actions
            .iter()
            .find_map(|a| match a {
                Action::RollbackHost {
                    rollout,
                    host,
                    target_ref,
                } => Some((rollout.clone(), host.clone(), target_ref.clone())),
                _ => None,
            })
            .expect("RollbackHost emitted under rollback-and-halt + Failed");
        let _ = halt;
        assert_eq!(rb.0, "stable@abc12345");
        assert_eq!(rb.1, "host-a");
        assert_eq!(rb.2, "ref-xyz");
    }

    #[test]
    fn reverted_under_rollback_and_halt_does_not_re_emit_rollback() {
        let fleet = fleet_with_policy(nixfleet_proto::OnHealthFailure::RollbackAndHalt);
        let rollout = rollout_with_state("host-a", HostRolloutState::Reverted);
        let observed = observed_online("host-a");
        let wave = Wave {
            hosts: vec!["host-a".into()],
            soak_minutes: 0,
        };
        let outcome = handle_wave(&fleet, &observed, &rollout, &wave, Utc::now());
        let rollback_count = outcome
            .actions
            .iter()
            .filter(|a| matches!(a, Action::RollbackHost { .. }))
            .count();
        assert_eq!(
            rollback_count, 0,
            "Reverted suppresses RollbackHost emission; actions={:?}",
            outcome.actions
        );
    }
}
