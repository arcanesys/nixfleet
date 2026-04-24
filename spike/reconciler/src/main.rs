use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

// ---------- fleet.resolved schema (RFC-0001 §4.1) ----------

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Fleet {
    schema_version: u32,
    hosts: HashMap<String, Host>,
    channels: HashMap<String, Channel>,
    waves: HashMap<String, Vec<Wave>>,
    edges: Vec<Edge>,
    disruption_budgets: Vec<Budget>,
    #[serde(default)]
    rollout_policies: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Host { system: String, tags: Vec<String>, channel: String, closure_hash: Option<String> }

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Channel { rollout_policy: String, reconcile_interval_minutes: u32 }

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Wave { hosts: Vec<String>, soak_minutes: u32 }

#[derive(Deserialize, Debug)]
struct Edge { before: String, after: String, #[serde(default)] reason: String }

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Budget { hosts: Vec<String>, max_in_flight: Option<u32> }

// ---------- observed state ----------

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Observed {
    channel_refs: HashMap<String, String>,
    last_rolled_refs: HashMap<String, String>,
    host_state: HashMap<String, HostState>,
    active_rollouts: Vec<Rollout>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct HostState { online: bool, current_generation: Option<String> }

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Rollout {
    id: String,
    channel: String,
    target_ref: String,
    state: String,
    current_wave: usize,
    host_states: HashMap<String, String>,
}

// ---------- action plan ----------

#[derive(Serialize, Debug)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Action {
    OpenRollout   { channel: String, target_ref: String },
    DispatchHost  { rollout: String, host: String, target_ref: String },
    PromoteWave   { rollout: String, new_wave: usize },
    ConvergeRollout { rollout: String },
    HaltRollout   { rollout: String, reason: String },
    Skip          { host: String, reason: String },
}

// ---------- reconciler ----------

fn reconcile(fleet: &Fleet, observed: &Observed) -> Vec<Action> {
    let mut actions = Vec::new();

    // Step 2 (RFC-0002 §4): open rollouts for channels whose ref changed.
    for (ch, current_ref) in &observed.channel_refs {
        if observed.last_rolled_refs.get(ch) == Some(current_ref) { continue; }
        let has_active = observed.active_rollouts.iter()
            .any(|r| r.channel == *ch && (r.state == "Executing" || r.state == "Planning"));
        if !has_active {
            actions.push(Action::OpenRollout { channel: ch.clone(), target_ref: current_ref.clone() });
        }
    }

    // Step 4: advance each Executing rollout.
    for r in &observed.active_rollouts {
        if r.state != "Executing" { continue; }
        let waves = match fleet.waves.get(&r.channel) { Some(w) => w, None => continue };
        let wave = match waves.get(r.current_wave) {
            Some(w) => w,
            None => { actions.push(Action::ConvergeRollout { rollout: r.id.clone() }); continue; }
        };

        let budget_for = |h: &str| -> Option<u32> {
            fleet.disruption_budgets.iter()
                .filter(|b| b.hosts.iter().any(|bh| bh == h))
                .filter_map(|b| b.max_in_flight)
                .min()
        };
        let mut in_flight = count_in_flight(r);
        let mut wave_all_soaked = true;

        for host in &wave.hosts {
            let state = r.host_states.get(host).map(String::as_str).unwrap_or("Queued");
            match state {
                "Queued" => {
                    wave_all_soaked = false;
                    let hs = observed.host_state.get(host);
                    if hs.map(|h| !h.online).unwrap_or(true) {
                        actions.push(Action::Skip { host: host.clone(), reason: "offline".into() });
                        continue;
                    }
                    if !predecessors_done(host, &fleet.edges, r) {
                        actions.push(Action::Skip { host: host.clone(), reason: "edge predecessor incomplete".into() });
                        continue;
                    }
                    if let Some(max) = budget_for(host) {
                        if in_flight >= max {
                            actions.push(Action::Skip {
                                host: host.clone(),
                                reason: format!("disruption budget ({}/{} in flight)", in_flight, max),
                            });
                            continue;
                        }
                    }
                    actions.push(Action::DispatchHost {
                        rollout: r.id.clone(), host: host.clone(), target_ref: r.target_ref.clone(),
                    });
                    in_flight += 1;
                }
                "Dispatched" | "Activating" | "ConfirmWindow" | "Healthy" => { wave_all_soaked = false; }
                "Soaked" | "Converged" => { /* done */ }
                "Failed" => {
                    actions.push(Action::HaltRollout {
                        rollout: r.id.clone(),
                        reason: format!("host {} failed", host),
                    });
                    wave_all_soaked = false;
                }
                _ => {}
            }
        }

        if wave_all_soaked {
            if r.current_wave + 1 >= waves.len() {
                actions.push(Action::ConvergeRollout { rollout: r.id.clone() });
            } else {
                actions.push(Action::PromoteWave { rollout: r.id.clone(), new_wave: r.current_wave + 1 });
            }
        }
    }

    actions
}

fn count_in_flight(r: &Rollout) -> u32 {
    r.host_states.values()
        .filter(|s| matches!(s.as_str(), "Dispatched" | "Activating" | "ConfirmWindow" | "Healthy"))
        .count() as u32
}

fn predecessors_done(host: &str, edges: &[Edge], r: &Rollout) -> bool {
    edges.iter()
        .filter(|e| e.before == host)
        .all(|e| {
            let s = r.host_states.get(&e.after).map(String::as_str).unwrap_or("Queued");
            matches!(s, "Soaked" | "Converged")
        })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: {} <fleet.resolved.json> <observed.json>", args[0]);
        std::process::exit(2);
    }
    let fleet: Fleet = serde_json::from_str(&fs::read_to_string(&args[1]).expect("read fleet")).expect("parse fleet");
    let observed: Observed = serde_json::from_str(&fs::read_to_string(&args[2]).expect("read observed")).expect("parse observed");

    let plan = reconcile(&fleet, &observed);
    println!("# reconcile tick: {} action(s)", plan.len());
    for (i, a) in plan.iter().enumerate() {
        println!("{:2}. {}", i + 1, serde_json::to_string(a).unwrap());
    }
}
