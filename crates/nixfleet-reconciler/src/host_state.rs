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
                // calls — if this Skip is emitted here, the agent's
                // checkin path returns None for the same reason.
                let empty_emitted_opens = std::collections::HashSet::new();
                let gate_input = crate::gates::GateInput {
                    fleet,
                    observed,
                    rollout: Some(rollout),
                    host,
                    now,
                    emitted_opens_in_tick: &empty_emitted_opens,
                    // Reconciler runs after polling has populated rollouts
                    // table; missing predecessor state is a real "predecessor
                    // not yet declared" situation and shouldn't pre-block
                    // every channel.
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
                // Healthy → Soaked once Healthy for `wave.soak_minutes`.
                // Without a `last_healthy_since` marker the soak gate
                // stays closed — better to wait than promote on missing data.
                out.wave_all_soaked = false;
                let soak_window = chrono::Duration::minutes(wave.soak_minutes as i64);
                if let Some(since) = rollout.last_healthy_since.get(host) {
                    if now.signed_duration_since(*since) >= soak_window {
                        out.actions.push(Action::SoakHost {
                            rollout: rollout.id.clone(),
                            host: host.clone(),
                        });
                    }
                }
            }
            HostRolloutState::Soaked | HostRolloutState::Converged => {}
            HostRolloutState::Failed | HostRolloutState::Reverted => {
                // `Failed` is reconciler-observed; `Reverted` is
                // agent-attested. Both halt; only `Failed` triggers
                // a fresh RollbackHost (Reverted is already rolled back).
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
        use nixfleet_proto::{
            Channel, Compliance, Host, Meta, PolicyWave, RolloutPolicy, Selector,
        };
        use std::collections::HashMap;

        let mut hosts = HashMap::new();
        hosts.insert(
            "host-a".to_string(),
            Host {
                system: "x86_64-linux".into(),
                tags: vec![],
                channel: "stable".into(),
                closure_hash: None,
                pubkey: None,
            },
        );
        let mut channels = HashMap::new();
        channels.insert(
            "stable".to_string(),
            Channel {
                rollout_policy: "p".into(),
                reconcile_interval_minutes: 30,
                signing_interval_minutes: 60,
                freshness_window: 86400,
                compliance: Compliance {
                    mode: "permissive".into(),
                    frameworks: vec![],
                },
            },
        );
        let mut rollout_policies = HashMap::new();
        rollout_policies.insert(
            "p".to_string(),
            RolloutPolicy {
                strategy: "all-at-once".into(),
                waves: vec![PolicyWave {
                    selector: Selector {
                        tags: vec![],
                        tags_any: vec![],
                        hosts: vec![],
                        channel: None,
                        all: true,
                    },
                    soak_minutes: 0,
                }],
                health_gate: nixfleet_proto::HealthGate::default(),
                on_health_failure,
            },
        );
        FleetResolved {
            schema_version: 1,
            hosts,
            channels,
            rollout_policies,
            waves: HashMap::new(),
            edges: vec![],
            channel_edges: vec![],
            disruption_budgets: vec![],
            meta: Meta {
                schema_version: 1,
                signed_at: None,
                ci_commit: None,
                signature_algorithm: Some("ed25519".into()),
            },
        }
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
        use crate::observed::HostState;
        let mut host_state = std::collections::HashMap::new();
        host_state.insert(
            host.into(),
            HostState {
                online: true,
                current_generation: None,
            },
        );
        Observed {
            channel_refs: std::collections::HashMap::new(),
            last_rolled_refs: std::collections::HashMap::new(),
            host_state,
            active_rollouts: vec![],
            outstanding_compliance_events_by_rollout: std::collections::HashMap::new(),
            last_deferrals: std::collections::HashMap::new(),
        }
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
        assert_eq!(halt_count, 1, "halt expected; actions={:?}", outcome.actions);
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
