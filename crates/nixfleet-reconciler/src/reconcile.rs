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
