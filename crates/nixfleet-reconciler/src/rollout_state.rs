//! Rollout-level state machine.

use crate::Action;
use crate::host_state::{self, WaveOutcome};
use crate::observed::{Observed, Rollout};
use anyhow::{Error, Result, anyhow};
use chrono::{DateTime, Utc};
use nixfleet_proto::FleetResolved;
use std::str::FromStr;

/// Rollout-level state, wire-formed as a string via serde shim. LOADBEARING:
/// `Halted` requires operator action - the reconciler stops advancing and
/// emits no further actions until the operator flips it back to `Executing`.
/// Don't auto-resume. `Planning` is reserved; current CP transitions inline
/// so callers rarely observe it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RolloutState {
    Planning,
    Executing,
    Halted,
}

impl RolloutState {
    pub fn as_str(&self) -> &'static str {
        match self {
            RolloutState::Planning => "Planning",
            RolloutState::Executing => "Executing",
            RolloutState::Halted => "Halted",
        }
    }
}

impl FromStr for RolloutState {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "Planning" => Ok(RolloutState::Planning),
            "Executing" => Ok(RolloutState::Executing),
            "Halted" => Ok(RolloutState::Halted),
            other => Err(anyhow!("unknown rollout state: {other:?}")),
        }
    }
}

pub(crate) fn advance_rollout(
    fleet: &FleetResolved,
    observed: &Observed,
    rollout: &Rollout,
    now: DateTime<Utc>,
) -> Vec<Action> {
    let mut actions = Vec::new();

    if rollout.state != RolloutState::Executing {
        return actions;
    }

    // Terminal rollouts stay in list_active() so channel_edges can read their
    // host_states (predecessor-converged detection). Short-circuit here so we
    // don't re-emit no-op ConvergeRollout every tick.
    if rollout.terminal_at.is_some() {
        return actions;
    }

    let waves = match fleet.waves.get(&rollout.channel) {
        Some(w) => w,
        None => return actions,
    };
    let wave = match waves.get(rollout.current_wave) {
        Some(w) => w,
        None => {
            // `all-at-once` channels lower to `fleet.waves[ch] = []`, so this
            // arm fires from tick 1 before any host has activated. Gate
            // ConvergeRollout on every declared-channel host's
            // `current_generation` matching its declared `closure_hash`;
            // otherwise the tick stamps `terminal_at`, the
            // Healthy/Soaked sweep updates zero rows, and late-arriving
            // hosts stay at Healthy forever (channelEdges successors
            // blocked indefinitely).
            let all_hosts_on_target = fleet
                .hosts
                .iter()
                .filter(|(_, h)| h.channel == rollout.channel)
                .filter_map(|(name, h)| h.closure_hash.as_deref().map(|c| (name.as_str(), c)))
                .all(|(name, target)| {
                    observed
                        .host_state
                        .get(name)
                        .and_then(|s| s.current_generation.as_deref())
                        == Some(target)
                });
            if all_hosts_on_target {
                actions.push(Action::ConvergeRollout {
                    rollout: rollout.id.clone(),
                });
            }
            return actions;
        }
    };

    let WaveOutcome {
        actions: wave_actions,
        wave_all_soaked,
    } = host_state::handle_wave(fleet, observed, rollout, wave, now);
    actions.extend(wave_actions);

    if wave_all_soaked {
        // Wave-promotion gate. Inclusive range `0..=current_wave`: the current
        // wave's failures hold promotion. The dispatch gate uses the EXCLUSIVE
        // `0..host_wave` (compliance_wave::check).
        let channel_mode = fleet
            .channels
            .get(&rollout.channel)
            .map(|c| nixfleet_proto::compliance::GateMode::from_wire_str(&c.compliance.mode))
            .unwrap_or(nixfleet_proto::compliance::GateMode::Disabled);
        let blocked: Vec<(String, usize)> = if channel_mode.is_enforcing() {
            crate::gates::compliance_wave::outstanding_failures_in_waves(
                observed,
                &rollout.id,
                waves,
                0..(rollout.current_wave + 1),
            )
        } else {
            Vec::new()
        };

        if !blocked.is_empty() {
            let total: usize = blocked.iter().map(|(_, n)| *n).sum();
            let blocked_hosts: Vec<String> = blocked.into_iter().map(|(h, _)| h).collect();
            actions.push(Action::WaveBlocked {
                rollout: rollout.id.clone(),
                blocked_wave: rollout.current_wave + 1,
                failing_hosts: blocked_hosts,
                failing_events_count: total,
            });
        } else if rollout.current_wave + 1 >= waves.len() {
            actions.push(Action::ConvergeRollout {
                rollout: rollout.id.clone(),
            });
        } else {
            actions.push(Action::PromoteWave {
                rollout: rollout.id.clone(),
                new_wave: rollout.current_wave + 1,
            });
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_known_values() {
        for v in [
            RolloutState::Planning,
            RolloutState::Executing,
            RolloutState::Halted,
        ] {
            assert_eq!(RolloutState::from_str(v.as_str()).unwrap(), v);
        }
    }

    #[test]
    fn unknown_strings_error() {
        assert!(RolloutState::from_str("").is_err());
        assert!(RolloutState::from_str("executing").is_err());
        assert!(RolloutState::from_str("garbage").is_err());
    }

    use crate::host_state::HostRolloutState;
    use crate::observed::{Observed, Rollout};
    use chrono::Utc;
    use nixfleet_proto::testing::FleetBuilder;
    use nixfleet_proto::{FleetResolved, Meta, PolicyWave, Selector};
    use std::collections::HashMap;

    fn fleet_two_waves(compliance_mode: &str) -> FleetResolved {
        FleetBuilder::new()
            .host("host-a", "stable")
            .host_closure("host-a", "c-a")
            .host("host-b", "stable")
            .host_closure("host-b", "c-b")
            .channel_compliance("stable", compliance_mode, &[])
            .policy_strategy("p", "staged")
            .policy_waves(
                "p",
                vec![
                    PolicyWave {
                        selector: Selector {
                            hosts: vec!["host-a".into()],
                            ..Default::default()
                        },
                        soak_minutes: 0,
                    },
                    PolicyWave {
                        selector: Selector {
                            hosts: vec!["host-b".into()],
                            ..Default::default()
                        },
                        soak_minutes: 0,
                    },
                ],
            )
            .meta(Meta {
                schema_version: 1,
                signed_at: Some(Utc::now()),
                ci_commit: Some("abc12345".into()),
                signature_algorithm: Some("ed25519".into()),
            })
            .wave_with_soak("stable", &["host-a"], 0)
            .wave_with_soak("stable", &["host-b"], 0)
            .build()
    }

    fn rollout_at_wave_0_soaked(id: &str) -> Rollout {
        let mut host_states = HashMap::new();
        host_states.insert("host-a".into(), HostRolloutState::Soaked);
        Rollout {
            id: id.into(),
            channel: "stable".into(),
            target_ref: id.into(),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states,
            last_healthy_since: HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        }
    }

    fn observed_with_failures(rollout_id: &str, failures: &[(&str, usize)]) -> Observed {
        let mut by_rollout = HashMap::new();
        let mut per_host = HashMap::new();
        for (h, n) in failures {
            per_host.insert(h.to_string(), *n);
        }
        if !per_host.is_empty() {
            by_rollout.insert(rollout_id.to_string(), per_host);
        }
        Observed {
            channel_refs: HashMap::new(),
            last_rolled_refs: HashMap::new(),
            host_state: HashMap::new(),
            active_rollouts: vec![],
            outstanding_compliance_events_by_rollout: by_rollout,
            last_deferrals: HashMap::new(),
            host_probes_passing: HashMap::new(),
            host_probes_observed: HashMap::new(),
            quarantined_closures: HashMap::new(),
        }
    }

    fn extract_action_kind(actions: &[Action]) -> Vec<&'static str> {
        actions
            .iter()
            .map(|a| match a {
                Action::OpenRollout { .. } => "open_rollout",
                Action::DispatchHost { .. } => "dispatch_host",
                Action::PromoteWave { .. } => "promote_wave",
                Action::ConvergeRollout { .. } => "converge_rollout",
                Action::HaltRollout { .. } => "halt_rollout",
                Action::RollbackHost { .. } => "rollback_host",
                Action::SoakHost { .. } => "soak_host",
                Action::ChannelUnknown { .. } => "channel_unknown",
                Action::Skip { .. } => "skip",
                Action::WaveBlocked { .. } => "wave_blocked",
                Action::RolloutDeferred { .. } => "rollout_deferred",
                Action::RotateTrustRoot { .. } => "rotate_trust_root",
            })
            .collect()
    }

    #[test]
    fn wave_blocked_emits_when_enforce_and_outstanding_event_for_this_rollout() {
        let fleet = fleet_two_waves("enforce");
        let rollout = rollout_at_wave_0_soaked("R1");
        let observed = observed_with_failures("R1", &[("host-a", 1)]);
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        let kinds = extract_action_kind(&actions);
        assert!(
            kinds.contains(&"wave_blocked"),
            "expected WaveBlocked, got {kinds:?}",
        );
        assert!(
            !kinds.contains(&"promote_wave"),
            "WaveBlocked must replace PromoteWave",
        );
        let wb = actions
            .iter()
            .find_map(|a| match a {
                Action::WaveBlocked {
                    rollout,
                    blocked_wave,
                    failing_hosts,
                    failing_events_count,
                } => Some((rollout, *blocked_wave, failing_hosts, *failing_events_count)),
                _ => None,
            })
            .expect("WaveBlocked emitted");
        assert_eq!(wb.0, "R1");
        assert_eq!(wb.1, 1);
        assert_eq!(wb.2, &vec!["host-a".to_string()]);
        assert_eq!(wb.3, 1);
    }

    /// Resolution-by-replacement: an R0 event must not block R1.
    #[test]
    fn wave_blocked_does_not_emit_for_event_bound_to_different_rollout() {
        let fleet = fleet_two_waves("enforce");
        let rollout = rollout_at_wave_0_soaked("R1");
        let observed = observed_with_failures("R0", &[("host-a", 1)]);
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        let kinds = extract_action_kind(&actions);
        assert!(
            kinds.contains(&"promote_wave"),
            "expected PromoteWave (R0 events do not block R1), got {kinds:?}",
        );
        assert!(
            !kinds.contains(&"wave_blocked"),
            "stale R0 events must not block R1 - resolution-by-replacement",
        );
    }

    #[test]
    fn wave_blocked_does_not_emit_under_permissive_mode() {
        let fleet = fleet_two_waves("permissive");
        let rollout = rollout_at_wave_0_soaked("R1");
        let observed = observed_with_failures("R1", &[("host-a", 1)]);
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        let kinds = extract_action_kind(&actions);
        assert!(
            kinds.contains(&"promote_wave"),
            "permissive mode advances regardless, got {kinds:?}",
        );
        assert!(!kinds.contains(&"wave_blocked"));
    }

    #[test]
    fn wave_blocked_does_not_emit_under_disabled_mode() {
        let fleet = fleet_two_waves("disabled");
        let rollout = rollout_at_wave_0_soaked("R1");
        let observed = observed_with_failures("R1", &[("host-a", 1)]);
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        let kinds = extract_action_kind(&actions);
        assert!(kinds.contains(&"promote_wave"));
        assert!(!kinds.contains(&"wave_blocked"));
    }

    #[test]
    fn wave_blocked_aggregates_multiple_hosts_in_earlier_waves() {
        let mut fleet = fleet_two_waves("enforce");
        let waves_for_stable = fleet.waves.get_mut("stable").unwrap();
        waves_for_stable[0].hosts = vec!["host-a".into(), "host-b".into()];
        let mut rollout = rollout_at_wave_0_soaked("R1");
        rollout
            .host_states
            .insert("host-b".into(), HostRolloutState::Soaked);
        let observed = observed_with_failures("R1", &[("host-a", 2), ("host-b", 3)]);
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        let wb = actions
            .iter()
            .find_map(|a| match a {
                Action::WaveBlocked {
                    failing_hosts,
                    failing_events_count,
                    ..
                } => Some((failing_hosts, *failing_events_count)),
                _ => None,
            })
            .expect("WaveBlocked emitted");
        assert_eq!(wb.0, &vec!["host-a".to_string(), "host-b".to_string()]);
        assert_eq!(wb.1, 5);
    }

    /// Regression: a rollout with `terminal_at` set must short-circuit -
    /// no actions emitted. Otherwise the reconciler re-emits ConvergeRollout
    /// every tick (functionally a no-op but pollutes the action stream and
    /// wastes DB writes proportional to converged-rollout count). The check
    /// must precede the wave logic so last-wave terminal rollouts don't
    /// re-emit from the `current_wave + 1 >= waves.len()` branch.
    #[test]
    fn advance_rollout_short_circuits_on_terminal_at() {
        let fleet = fleet_two_waves("enforce");
        let mut rollout = rollout_at_wave_0_soaked("R1");
        rollout.terminal_at = Some(Utc::now());
        let observed = observed_with_failures("R1", &[]);
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        assert!(
            actions.is_empty(),
            "advance_rollout must emit zero actions for terminal rollouts; got {actions:?}",
        );
    }

    /// Companion: even at the last wave with all hosts soaked (the natural
    /// ConvergeRollout trigger), a terminal rollout must emit nothing.
    #[test]
    fn advance_rollout_terminal_at_last_wave_emits_no_converge() {
        let fleet = fleet_two_waves("enforce");
        let mut rollout = rollout_at_wave_0_soaked("R1");
        rollout.current_wave = 1;
        rollout
            .host_states
            .insert("host-b".into(), HostRolloutState::Soaked);
        rollout.terminal_at = Some(Utc::now());
        let observed = observed_with_failures("R1", &[]);
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        let kinds = extract_action_kind(&actions);
        assert!(
            !kinds.contains(&"converge_rollout"),
            "terminal rollout at last wave must NOT re-emit ConvergeRollout; got {kinds:?}",
        );
        assert!(
            actions.is_empty(),
            "expected zero actions for terminal rollout, got {actions:?}",
        );
    }

    /// Regression: `all-at-once` channels lower `fleet.waves[ch] = []`. Before
    /// the arrival gate, advance_rollout fired ConvergeRollout on the very
    /// first tick (no wave at current_wave=0), `apply_actions` stamped
    /// `terminal_at`, and the Healthy/Soaked sweep updated zero rows because
    /// no host had reached Healthy yet. Hosts dispatched later stayed at
    /// Healthy forever (channelEdges successors blocked indefinitely).
    fn fleet_all_at_once_one_host(host: &str, channel: &str, closure: &str) -> FleetResolved {
        let mut fleet = FleetBuilder::new()
            .host(host, channel)
            .host_closure(host, closure)
            .build();
        // The Nix lowering populates `fleet.waves[ch] = []` for every channel
        // regardless of strategy. FleetBuilder omits the entry entirely
        // (Some([]) vs None take different early-return paths); insert an
        // empty list so the test exercises the production `Some([])` arm.
        fleet.waves.insert(channel.to_string(), vec![]);
        fleet
    }

    fn observed_with_host_state(host: &str, current: Option<&str>) -> Observed {
        use crate::observed::HostState;
        let mut host_state = HashMap::new();
        host_state.insert(
            host.into(),
            HostState {
                online: true,
                current_generation: current.map(str::to_string),
            },
        );
        Observed {
            channel_refs: HashMap::new(),
            last_rolled_refs: HashMap::new(),
            host_state,
            active_rollouts: vec![],
            outstanding_compliance_events_by_rollout: HashMap::new(),
            last_deferrals: HashMap::new(),
            host_probes_passing: HashMap::new(),
            host_probes_observed: HashMap::new(),
            quarantined_closures: HashMap::new(),
        }
    }

    fn rollout_empty(id: &str, channel: &str) -> Rollout {
        Rollout {
            id: id.into(),
            channel: channel.into(),
            target_ref: id.into(),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states: HashMap::new(),
            last_healthy_since: HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        }
    }

    #[test]
    fn advance_rollout_empty_waves_holds_until_hosts_on_target() {
        let fleet = fleet_all_at_once_one_host("web-02", "edge", "c-new");
        let rollout = rollout_empty("R1", "edge");
        // Host still on the old closure - not yet activated.
        let observed = observed_with_host_state("web-02", Some("c-old"));
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        assert!(
            actions.is_empty(),
            "empty-waves rollout must hold ConvergeRollout until hosts arrive; got {actions:?}",
        );
    }

    #[test]
    fn advance_rollout_empty_waves_holds_when_host_state_missing() {
        let fleet = fleet_all_at_once_one_host("web-02", "edge", "c-new");
        let rollout = rollout_empty("R1", "edge");
        // No checkin yet - host absent from observed.host_state.
        let observed = observed_with_failures("R1", &[]);
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        assert!(
            actions.is_empty(),
            "empty-waves rollout must hold ConvergeRollout when host hasn't checked in; got {actions:?}",
        );
    }

    #[test]
    fn advance_rollout_empty_waves_converges_when_all_hosts_on_target() {
        let fleet = fleet_all_at_once_one_host("web-02", "edge", "c-new");
        let rollout = rollout_empty("R1", "edge");
        let observed = observed_with_host_state("web-02", Some("c-new"));
        let actions = advance_rollout(&fleet, &observed, &rollout, Utc::now());
        let kinds = extract_action_kind(&actions);
        assert!(
            kinds.contains(&"converge_rollout"),
            "ConvergeRollout must emit once every declared-channel host is on target; got {kinds:?}",
        );
    }
}
