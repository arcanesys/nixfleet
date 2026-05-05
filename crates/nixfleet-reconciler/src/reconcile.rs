//! Top-level `reconcile` orchestration.

use std::collections::{HashMap, HashSet};

use crate::observed::DeferralRecord;
use crate::rollout_state::{self, RolloutState};
use crate::{Action, Observed};
use chrono::{DateTime, Utc};
use nixfleet_proto::FleetResolved;

/// Topological order of `channels` with respect to `fleet.channel_edges`.
/// Predecessors first; ties broken alphabetically for tick-to-tick
/// determinism. Edges referencing channels not in the input set are
/// ignored (they can't gate this tick / poll). Channels in `channels`
/// not appearing in any edge are ordered alphabetically among themselves.
///
/// LOADBEARING: the reconcile loop and the polling layer both iterate
/// this order so the in-tick `emitted_opens` set sees a `before` channel
/// recorded before checking the `after` channel's predecessor gate.
/// mkFleet validates `channelEdges` is a DAG, so cycle handling here is
/// defensive only.
pub fn topological_channel_order(
    fleet: &FleetResolved,
    channels: &[String],
) -> Vec<String> {
    let channel_set: std::collections::HashSet<&str> =
        channels.iter().map(|s| s.as_str()).collect();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut successors: HashMap<String, Vec<String>> = HashMap::new();
    for ch in channels {
        in_degree.insert(ch.clone(), 0);
        successors.insert(ch.clone(), Vec::new());
    }
    for edge in &fleet.channel_edges {
        if !channel_set.contains(edge.gates.as_str())
            || !channel_set.contains(edge.gated.as_str())
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
    // Cycle defensive: any channel not yet ordered (cycle, mkFleet bug)
    // gets appended in alphabetical order so the tick still makes
    // progress instead of dropping channels silently.
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

    // Open rollouts for channels whose ref changed, in topological order
    // so a `before` channel's OpenRollout is seen by `after`'s predecessor
    // check within the same tick.
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
            // Debounce: only emit when (target_ref, blocked_by) would change.
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
                        "channel '{blocker}' has an active rollout — channelEdges holds OpenRollout until predecessor converges",
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

    // Advance each Executing rollout. Channel-removed rollouts emit a
    // ChannelUnknown observability event before silent-continue.
    for rollout in &observed.active_rollouts {
        if !fleet.channels.contains_key(&rollout.channel) {
            actions.push(Action::ChannelUnknown {
                channel: rollout.channel.clone(),
            });
            continue;
        }
        actions.extend(rollout_state::advance_rollout(fleet, observed, rollout, now));
    }

    actions
}

#[cfg(test)]
mod channel_edge_tests {
    use super::*;
    use crate::host_state::HostRolloutState;
    use crate::observed::{HostState, Rollout};
    use nixfleet_proto::{
        Channel, ChannelEdge, Compliance, FleetResolved, Host, Meta, OnHealthFailure, RolloutPolicy,
    };
    use std::collections::HashMap;

    fn fleet_with_channel_edges(edges: Vec<ChannelEdge>) -> FleetResolved {
        let mut channels = HashMap::new();
        for ch in ["db", "app"] {
            channels.insert(
                ch.to_string(),
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
                strategy: "all-at-once".into(),
                waves: vec![],
                health_gate: Default::default(),
                on_health_failure: OnHealthFailure::Halt,
            },
        );
        FleetResolved {
            schema_version: 1,
            hosts: HashMap::new(),
            channels,
            rollout_policies,
            waves: HashMap::new(),
            edges: vec![],
            channel_edges: edges,
            disruption_budgets: vec![],
            meta: Meta {
                schema_version: 1,
                signed_at: None,
                ci_commit: None,
                signature_algorithm: Some("ed25519".into()),
            },
        }
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
        // The semantic property channelEdges enforces: ordering between
        // *active* rollouts. Once a predecessor's rollout is fully
        // converged (every host in host_states is Converged), it stops
        // counting as a blocker even though the rollout row remains in
        // the DB until superseded.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed.channel_refs.insert("app".into(), "ref-app-1".into());
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
        // Until ALL hosts in the predecessor reach a terminal state
        // (Soaked or Converged), the predecessor is still active and
        // blocks its successor.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed.channel_refs.insert("app".into(), "ref-app-1".into());
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
        assert!(blocked, "any non-terminal host keeps the predecessor active: {actions:?}");
    }

    #[test]
    fn all_soaked_predecessor_unblocks_successor() {
        // Bridges the SoakHost-to-ConvergeRollout window: once every
        // host of the predecessor reaches Soaked, the rollout has
        // semantically completed wave-staging even though the next
        // reconcile tick hasn't yet emitted ConvergeRollout. Soaked
        // counts as terminal-for-ordering so the successor doesn't
        // get artificially held for one extra tick.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed.channel_refs.insert("app".into(), "ref-app-1".into());
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
        // A multi-wave rollout in the brief window between the last
        // wave reaching Soaked and ConvergeRollout firing: earlier-
        // wave hosts may already be Converged (hands-stamped by a
        // previous ConvergeRollout-equivalent path) while the last
        // wave's hosts are at Soaked. Either is terminal-for-ordering.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed.channel_refs.insert("app".into(), "ref-app-1".into());
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
        // fresh CP because predecessor_channel_blocking only saw
        // active_rollouts (empty post-DB-wipe). The fix iterates channels
        // in topological order and tracks emitted_opens within the tick.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: Some("schema-migration".into()),
        }]);
        let mut observed = Observed::default();
        // Both channels have a fresh ref; no active_rollouts (post-wipe).
        observed.channel_refs.insert("db".into(), "ref-db-1".into());
        observed.channel_refs.insert("app".into(), "ref-app-1".into());

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
            "exactly one OpenRollout — for `db` (the predecessor) — must be emitted; got {actions:?}",
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

        // Must NOT contain OpenRollout for app.
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::OpenRollout { channel, .. } if channel == "app")),
            "app rollout should be held while db has an active rollout: {actions:?}"
        );
        // Must contain RolloutDeferred for app.
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
        // db has never had a rollout — RFC §4.3 punt resolves "predecessor
        // never released" as "proceed" (edges constrain ordering between
        // active rollouts, not a presence requirement).
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed.channel_refs.insert("app".into(), "ref-app-1".into());
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
        // Stamp the same deferral as already-emitted.
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
        observed.channel_refs.insert("app".into(), "ref-app-1".into());
        // Active rollout on infra (not db) — different blocker than the
        // last-emitted record below.
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
        // Need a third channel in fleet for completeness.
        observed.last_deferrals.insert(
            "app".into(),
            DeferralRecord {
                target_ref: "ref-app-1".into(),
                blocked_by: "db".into(),
            },
        );
        // Add infra to channels so the test fleet is consistent.
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
        // No active rollout on db means nothing to wait for.
        let fleet = fleet_with_channel_edges(vec![ChannelEdge {
            gates: "db".into(),
            gated: "app".into(),
            reason: None,
        }]);
        let mut observed = Observed::default();
        observed.channel_refs.insert("app".into(), "ref-app-1".into());
        // Even with a stale last_deferral entry, an unblocked channel opens.
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
        let _ = HostRolloutState::Queued; // import-touch to keep the use clean
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
        }
    }
}
