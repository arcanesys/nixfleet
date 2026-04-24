//! Top-level reconcile. Phase D: everything lives here.

use crate::{Action, Observed};
use chrono::{DateTime, Utc};
use nixfleet_proto::FleetResolved;

pub fn reconcile(
    fleet: &FleetResolved,
    observed: &Observed,
    _now: DateTime<Utc>,
) -> Vec<Action> {
    let mut actions = Vec::new();

    // §4 step 2: open rollouts for channels whose ref changed.
    for (channel, current_ref) in &observed.channel_refs {
        if observed.last_rolled_refs.get(channel) == Some(current_ref) {
            continue;
        }
        let has_active = observed.active_rollouts.iter().any(|r| {
            &r.channel == channel && (r.state == "Executing" || r.state == "Planning")
        });
        if !has_active && fleet.channels.contains_key(channel) {
            actions.push(Action::OpenRollout {
                channel: channel.clone(),
                target_ref: current_ref.clone(),
            });
        }
    }

    // §4 step 4: advance each Executing rollout.
    for rollout in &observed.active_rollouts {
        if rollout.state != "Executing" {
            continue;
        }
        let waves = match fleet.waves.get(&rollout.channel) {
            Some(w) => w,
            None => continue, // missing-channel: silent continue per spec open-q #5
        };
        let wave = match waves.get(rollout.current_wave) {
            Some(w) => w,
            None => {
                actions.push(Action::ConvergeRollout { rollout: rollout.id.clone() });
                continue;
            }
        };

        // In-flight count across all rollouts for this budget's host set.
        let count_in_flight = |budget_hosts: &[String]| -> u32 {
            observed
                .active_rollouts
                .iter()
                .map(|r| {
                    r.host_states
                        .iter()
                        .filter(|(h, st)| {
                            budget_hosts.iter().any(|b| b == *h)
                                && matches!(
                                    st.as_str(),
                                    "Dispatched" | "Activating" | "ConfirmWindow" | "Healthy"
                                )
                        })
                        .count() as u32
                })
                .sum()
        };

        // For a given host, the tightest max_in_flight across all budgets it matches.
        let budget_max = |host: &str| -> Option<(u32, u32)> {
            fleet
                .disruption_budgets
                .iter()
                .filter(|b| b.hosts.iter().any(|bh| bh == host))
                .filter_map(|b| b.max_in_flight.map(|m| (count_in_flight(&b.hosts), m)))
                .min_by_key(|(_, max)| *max)
        };

        let mut wave_all_soaked = true;

        for host in &wave.hosts {
            let state = rollout.host_states.get(host).map(String::as_str).unwrap_or("Queued");
            match state {
                "Queued" => {
                    wave_all_soaked = false;
                    let online = observed.host_state.get(host).map(|h| h.online).unwrap_or(false);
                    if !online {
                        actions.push(Action::Skip {
                            host: host.clone(),
                            reason: "offline".into(),
                        });
                        continue;
                    }
                    // §4.1 edge predecessor check.
                    let incomplete = fleet.edges.iter().find_map(|e| {
                        if e.before != *host {
                            return None;
                        }
                        let s = rollout
                            .host_states
                            .get(&e.after)
                            .map(String::as_str)
                            .unwrap_or("Queued");
                        if matches!(s, "Soaked" | "Converged") {
                            None
                        } else {
                            Some(e.after.clone())
                        }
                    });
                    if let Some(predecessor) = incomplete {
                        actions.push(Action::Skip {
                            host: host.clone(),
                            reason: format!("edge predecessor {predecessor} incomplete"),
                        });
                        continue;
                    }
                    if let Some((in_flight, max)) = budget_max(host) {
                        if in_flight >= max {
                            actions.push(Action::Skip {
                                host: host.clone(),
                                reason: format!("disruption budget ({in_flight}/{max} in flight)"),
                            });
                            continue;
                        }
                    }
                    actions.push(Action::DispatchHost {
                        rollout: rollout.id.clone(),
                        host: host.clone(),
                        target_ref: rollout.target_ref.clone(),
                    });
                }
                "Dispatched" | "Activating" | "ConfirmWindow" | "Healthy" => {
                    wave_all_soaked = false;
                }
                "Soaked" | "Converged" => {}
                "Failed" => {
                    wave_all_soaked = false;
                    if let Some(chan) = fleet.channels.get(&rollout.channel) {
                        if let Some(policy) = fleet.rollout_policies.get(&chan.rollout_policy) {
                            let reason = format!(
                                "host {} failed (policy: {})",
                                host, policy.on_health_failure
                            );
                            actions.push(Action::HaltRollout {
                                rollout: rollout.id.clone(),
                                reason,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        if wave_all_soaked {
            if rollout.current_wave + 1 >= waves.len() {
                actions.push(Action::ConvergeRollout { rollout: rollout.id.clone() });
            } else {
                actions.push(Action::PromoteWave {
                    rollout: rollout.id.clone(),
                    new_wave: rollout.current_wave + 1,
                });
            }
        }
    }

    actions
}
