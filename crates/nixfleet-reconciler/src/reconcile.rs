//! Top-level `reconcile` orchestration.

use std::collections::{HashMap, HashSet};

use crate::observed::DeferralRecord;
use crate::rollout_state::{self, RolloutState};
use crate::{Action, Observed};
use chrono::{DateTime, Utc};
use nixfleet_proto::FleetResolved;

/// Topological order over `channel_edges`: predecessors first, ties broken
/// alphabetically for tick-to-tick determinism. Edges referencing channels
/// not in the input set are ignored. LOADBEARING: reconcile loop and the
/// polling layer share this order so `emitted_opens` records a predecessor
/// before the successor's gate check runs. Cycle handling is defensive only -
/// mkFleet validates `channelEdges` is a DAG.
pub fn topological_channel_order(fleet: &FleetResolved, channels: &[String]) -> Vec<String> {
    let channel_set: std::collections::HashSet<&str> =
        channels.iter().map(|s| s.as_str()).collect();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut successors: HashMap<String, Vec<String>> = HashMap::new();
    for ch in channels {
        in_degree.insert(ch.clone(), 0);
        successors.insert(ch.clone(), Vec::new());
    }
    for edge in &fleet.channel_edges {
        if !channel_set.contains(edge.gates.as_str()) || !channel_set.contains(edge.gated.as_str())
        {
            continue;
        }
        successors
            .entry(edge.gates.clone())
            .or_default()
            .push(edge.gated.clone());
        *in_degree.entry(edge.gated.clone()).or_insert(0) += 1;
    }
    let mut frontier: Vec<String> = in_degree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| k.clone())
        .collect();
    frontier.sort();
    let mut order: Vec<String> = Vec::with_capacity(channels.len());
    while let Some(node) = frontier.first().cloned() {
        frontier.remove(0);
        order.push(node.clone());
        if let Some(succs) = successors.get(&node) {
            let mut newly_zero: Vec<String> = Vec::new();
            for s in succs {
                if let Some(d) = in_degree.get_mut(s) {
                    *d -= 1;
                    if *d == 0 {
                        newly_zero.push(s.clone());
                    }
                }
            }
            newly_zero.sort();
            frontier.extend(newly_zero);
            frontier.sort();
            frontier.dedup();
        }
    }
    // Cycle defensive: stray channels (mkFleet bug) appended alphabetically
    // so the tick makes progress instead of silently dropping them.
    let mut leftover: Vec<String> = channels
        .iter()
        .filter(|k| !order.contains(k))
        .cloned()
        .collect();
    leftover.sort();
    order.extend(leftover);
    order
}

pub fn reconcile(fleet: &FleetResolved, observed: &Observed, now: DateTime<Utc>) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut emitted_opens: HashSet<String> = HashSet::new();

    // Open rollouts for changed refs in topological order so a predecessor's
    // OpenRollout is seen by its successor's gate within the same tick.
    let channel_names: Vec<String> = observed.channel_refs.keys().cloned().collect();
    for channel in topological_channel_order(fleet, &channel_names) {
        let current_ref = match observed.channel_refs.get(&channel) {
            Some(r) => r,
            None => continue,
        };
        if observed.last_rolled_refs.get(&channel) == Some(current_ref) {
            continue;
        }
        let has_active = observed.active_rollouts.iter().any(|r| {
            r.channel == channel
                && matches!(r.state, RolloutState::Executing | RolloutState::Planning)
        });
        if has_active || !fleet.channels.contains_key(&channel) {
            continue;
        }
        if let Some(blocker) = crate::gates::channel_edges::check_for_channel(
            fleet,
            observed,
            &emitted_opens,
            &channel,
            crate::gates::GateMode::Reconcile,
        ) {
            // Debounce: only emit when (target_ref, blocked_by) changes.
            let proposed = DeferralRecord {
                target_ref: current_ref.clone(),
                blocked_by: blocker.clone(),
            };
            if observed.last_deferrals.get(&channel) != Some(&proposed) {
                actions.push(Action::RolloutDeferred {
                    channel: channel.clone(),
                    target_ref: current_ref.clone(),
                    blocked_by: blocker.clone(),
                    reason: format!(
                        "channel '{blocker}' has an active rollout - channelEdges holds OpenRollout until predecessor converges",
                    ),
                });
            }
            continue;
        }
        actions.push(Action::OpenRollout {
            channel: channel.clone(),
            target_ref: current_ref.clone(),
        });
        emitted_opens.insert(channel);
    }

    // Advance each Executing rollout; channel-removed rollouts emit
    // ChannelUnknown before silent-continue.
    for rollout in &observed.active_rollouts {
        if !fleet.channels.contains_key(&rollout.channel) {
            actions.push(Action::ChannelUnknown {
                channel: rollout.channel.clone(),
            });
            continue;
        }
        actions.extend(rollout_state::advance_rollout(
            fleet, observed, rollout, now,
        ));
    }

    actions
}

#[cfg(test)]
mod channel_edge_tests {
    use super::*;
    use crate::host_state::HostRolloutState;
    use crate::observed::{HostState, Rollout};
    use nixfleet_proto::testing::FleetBuilder;
    use nixfleet_proto::{Channel, ChannelEdge, Compliance, FleetResolved, Host};
    use std::collections::HashMap;

    fn fleet_with_channel_edges(edges: Vec<ChannelEdge>) -> FleetResolved {
        let mut f = FleetBuilder::new()
            .channel("db", "p")
            .channel("app", "p")
            .policy_waves("p", vec![])
            .build();
        f.channel_edges = edges;
        f
    }

    fn observed_with_active_rollout_on(channel: &str) -> Observed {
        let mut o = Observed::default();
        o.channel_refs.insert("app".into(), "ref-app-1".into());
        o.active_rollouts.push(Rollout {
            id: format!("{channel}-rollout"),
            channel: channel.into(),
            target_ref: format!("ref-{channel}-active"),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states: HashMap::new(),
            last_healthy_since: HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        });
        o
    }

    #[test]
    fn converged_predecessor_does_not_block_successor() {
        // channelEdges orders *active* rollouts; a fully Converged predecessor
        // stops gating even though the row stays in the DB.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed
            .channel_refs
            .insert("app".into(), "ref-app-1".into());
        observed.active_rollouts.push(Rollout {
            id: "db-rollout".into(),
            channel: "db".into(),
            target_ref: "ref-db-converged".into(),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states: HashMap::from([
                ("db1".to_string(), HostRolloutState::Converged),
                ("db2".to_string(), HostRolloutState::Converged),
            ]),
            last_healthy_since: HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        });
        let actions = reconcile(&fleet, &observed, chrono::Utc::now());
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::OpenRollout { channel, .. } if channel == "app")),
            "app must open once db's rollout is fully converged: {actions:?}",
        );
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::RolloutDeferred { channel, .. } if channel == "app")),
            "no deferral expected when predecessor is converged: {actions:?}",
        );
    }

    #[test]
    fn partially_converged_predecessor_still_blocks_successor() {
        // Any non-terminal host keeps the predecessor active.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed
            .channel_refs
            .insert("app".into(), "ref-app-1".into());
        observed.active_rollouts.push(Rollout {
            id: "db-rollout".into(),
            channel: "db".into(),
            target_ref: "ref-db".into(),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states: HashMap::from([
                ("db1".to_string(), HostRolloutState::Converged),
                ("db2".to_string(), HostRolloutState::Healthy),
            ]),
            last_healthy_since: HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        });
        let actions = reconcile(&fleet, &observed, chrono::Utc::now());
        let blocked = actions.iter().any(
            |a| matches!(a, Action::RolloutDeferred { channel, blocked_by, .. } if channel == "app" && blocked_by == "db"),
        );
        assert!(
            blocked,
            "any non-terminal host keeps the predecessor active: {actions:?}"
        );
    }

    #[test]
    fn all_soaked_predecessor_unblocks_successor() {
        // Soaked counts as terminal-for-ordering, bridging the
        // SoakHost → ConvergeRollout window without holding successors.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed
            .channel_refs
            .insert("app".into(), "ref-app-1".into());
        observed.active_rollouts.push(Rollout {
            id: "db-rollout".into(),
            channel: "db".into(),
            target_ref: "ref-db-soaked".into(),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states: HashMap::from([
                ("db1".to_string(), HostRolloutState::Soaked),
                ("db2".to_string(), HostRolloutState::Soaked),
            ]),
            last_healthy_since: HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        });
        let actions = reconcile(&fleet, &observed, chrono::Utc::now());
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::OpenRollout { channel, .. } if channel == "app")),
            "all-Soaked predecessor must unblock successor: {actions:?}",
        );
    }

    #[test]
    fn mixed_soaked_and_converged_predecessor_unblocks_successor() {
        // Either Soaked or Converged counts as terminal-for-ordering.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed
            .channel_refs
            .insert("app".into(), "ref-app-1".into());
        observed.active_rollouts.push(Rollout {
            id: "db-rollout".into(),
            channel: "db".into(),
            target_ref: "ref-db".into(),
            state: RolloutState::Executing,
            current_wave: 1,
            host_states: HashMap::from([
                ("db-wave0".to_string(), HostRolloutState::Converged),
                ("db-wave1".to_string(), HostRolloutState::Soaked),
            ]),
            last_healthy_since: HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        });
        let actions = reconcile(&fleet, &observed, chrono::Utc::now());
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::OpenRollout { channel, .. } if channel == "app")),
            "mixed Soaked + Converged predecessor must unblock successor: {actions:?}",
        );
    }

    #[test]
    fn fresh_tick_with_edge_holds_after_channel_until_before_opens_first() {
        // Regression: pre-fix, both channels opened in the same tick on a
        // fresh CP. Topological iteration + per-tick `emitted_opens` fixed it.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: Some("schema-migration".into()),
        }]);
        let mut observed = Observed::default();
        observed.channel_refs.insert("db".into(), "ref-db-1".into());
        observed
            .channel_refs
            .insert("app".into(), "ref-app-1".into());

        let actions = reconcile(&fleet, &observed, chrono::Utc::now());

        let opens: Vec<&str> = actions
            .iter()
            .filter_map(|a| match a {
                Action::OpenRollout { channel, .. } => Some(channel.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            opens,
            vec!["db"],
            "exactly one OpenRollout - for `db` (the predecessor) - must be emitted; got {actions:?}",
        );
        let deferred = actions.iter().any(|a| matches!(a, Action::RolloutDeferred { channel, blocked_by, .. } if channel == "app" && blocked_by == "db"));
        assert!(
            deferred,
            "app must be deferred with blocked_by=db within the same tick: {actions:?}",
        );
    }

    #[test]
    fn channel_edge_with_active_predecessor_defers_rather_than_opens() {
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: Some("schema-migration".into()),
        }]);
        let observed = observed_with_active_rollout_on("db");
        let now = chrono::Utc::now();
        let actions = reconcile(&fleet, &observed, now);

        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::OpenRollout { channel, .. } if channel == "app")),
            "app rollout should be held while db has an active rollout: {actions:?}"
        );
        let deferred = actions.iter().find_map(|a| match a {
            Action::RolloutDeferred {
                channel,
                blocked_by,
                ..
            } if channel == "app" => Some(blocked_by.clone()),
            _ => None,
        });
        assert_eq!(
            deferred.as_deref(),
            Some("db"),
            "expected RolloutDeferred(app, blocked_by=db); got {actions:?}",
        );
    }

    #[test]
    fn channel_edge_with_no_predecessor_history_proceeds() {
        // Edges constrain ordering between active rollouts, not a presence
        // requirement - successor opens if predecessor has never had one.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed
            .channel_refs
            .insert("app".into(), "ref-app-1".into());
        let actions = reconcile(&fleet, &observed, chrono::Utc::now());

        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::OpenRollout { channel, .. } if channel == "app")),
            "app should open with no db history: {actions:?}"
        );
    }

    #[test]
    fn rollout_deferred_is_debounced_via_last_deferrals() {
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = observed_with_active_rollout_on("db");
        observed.last_deferrals.insert(
            "app".into(),
            DeferralRecord {
                target_ref: "ref-app-1".into(),
                blocked_by: "db".into(),
            },
        );
        let actions = reconcile(&fleet, &observed, chrono::Utc::now());

        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::RolloutDeferred { .. })),
            "RolloutDeferred must NOT re-fire when last_deferrals already records the same (target_ref, blocked_by): {actions:?}",
        );
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::OpenRollout { channel, .. } if channel == "app")),
            "still blocked, must not open: {actions:?}",
        );
    }

    #[test]
    fn rollout_deferred_re_fires_on_blocker_change() {
        let fleet = fleet_with_channel_edges(vec![
            ChannelEdge {
                gates: "db".into(),
                gated: "app".into(),
                reason: None,
            },
            ChannelEdge {
                gates: "infra".into(),
                gated: "app".into(),
                reason: None,
            },
        ]);
        let mut observed = Observed::default();
        observed
            .channel_refs
            .insert("app".into(), "ref-app-1".into());
        // Active rollout on infra, different blocker from last-emitted (db).
        observed.active_rollouts.push(Rollout {
            id: "infra-rollout".into(),
            channel: "infra".into(),
            target_ref: "ref-infra".into(),
            state: RolloutState::Executing,
            current_wave: 0,
            host_states: HashMap::new(),
            last_healthy_since: HashMap::new(),
            budgets: vec![],
            terminal_at: None,
        });
        observed.last_deferrals.insert(
            "app".into(),
            DeferralRecord {
                target_ref: "ref-app-1".into(),
                blocked_by: "db".into(),
            },
        );
        let mut fleet = fleet;
        fleet.channels.insert(
            "infra".into(),
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
        let actions = reconcile(&fleet, &observed, chrono::Utc::now());

        let deferred_blocker = actions.iter().find_map(|a| match a {
            Action::RolloutDeferred { blocked_by, .. } => Some(blocked_by.clone()),
            _ => None,
        });
        assert_eq!(
            deferred_blocker.as_deref(),
            Some("infra"),
            "blocker changed from db→infra; must re-emit: {actions:?}",
        );
    }

    #[test]
    fn channel_edge_clears_when_predecessor_converges() {
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed
            .channel_refs
            .insert("app".into(), "ref-app-1".into());
        // Stale last_deferral must not hold an unblocked channel.
        observed.last_deferrals.insert(
            "app".into(),
            DeferralRecord {
                target_ref: "ref-app-1".into(),
                blocked_by: "db".into(),
            },
        );
        let _ = HostState {
            online: true,
            current_generation: None,
        };
        let _ = HostRolloutState::Queued;
        let actions = reconcile(&fleet, &observed, chrono::Utc::now());

        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::OpenRollout { channel, .. } if channel == "app")),
            "predecessor no longer active → must open: {actions:?}",
        );
    }

    fn _host(channel: &str, tags: &[&str]) -> Host {
        Host {
            system: "x86_64-linux".into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            channel: channel.into(),
            closure_hash: None,
            pubkey: None,
            pin: None,
        }
    }
}
