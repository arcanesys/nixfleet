//! Live `Observed` projection from in-memory checkin state; pass `&[]` rollouts when no DB.

use std::collections::HashMap;

use nixfleet_proto::RolloutBudget;
use nixfleet_reconciler::observed::{DeferralRecord, HostState, Observed, Rollout};
use nixfleet_reconciler::{HostRolloutState, RolloutState};

use crate::db::RolloutDbSnapshot;
use crate::server::HostCheckinRecord;

/// `rollout_budgets`: per-rollout budget snapshot from the signed
/// manifest. Empty entry when no manifest is loaded yet (CP just primed)
/// — budget gates then no-op, which is correct: a rollout with no known
/// budgets has no constraint until its manifest is verified.
pub fn project(
    host_checkins: &HashMap<String, HostCheckinRecord>,
    channel_refs: &HashMap<String, String>,
    rollouts: &[RolloutDbSnapshot],
    compliance_failures_by_rollout: HashMap<String, HashMap<String, usize>>,
    last_deferrals: HashMap<String, DeferralRecord>,
    rollout_budgets: &HashMap<String, Vec<RolloutBudget>>,
) -> Observed {
    let mut host_state: HashMap<String, HostState> = HashMap::new();
    for (host, record) in host_checkins {
        host_state.insert(
            host.clone(),
            HostState {
                online: true,
                current_generation: Some(record.checkin.current_generation.closure_hash.clone()),
            },
        );
    }

    let active_rollouts: Vec<Rollout> = rollouts
        .iter()
        .map(|snap| Rollout {
            id: snap.rollout_id.clone(),
            channel: snap.channel.clone(),
            target_ref: snap.target_channel_ref.clone(),
            state: RolloutState::Executing,
            current_wave: snap.current_wave as usize,
            // Unknown SQL strings fall back to Failed; Queued would re-dispatch every tick.
            host_states: snap
                .host_states
                .iter()
                .map(|(h, s)| {
                    let parsed = HostRolloutState::from_db_str(s).unwrap_or_else(|_| {
                        tracing::warn!(
                            rollout = %snap.rollout_id,
                            host = %h,
                            unknown_state = %s,
                            "host_rollout_state value not in HostRolloutState enum — \
                             halting rollout (Failed fallback). Likely a SQL CHECK \
                             extension that wasn't propagated to the typed enum.",
                        );
                        HostRolloutState::Failed
                    });
                    (h.clone(), parsed)
                })
                .collect(),
            last_healthy_since: snap.last_healthy_since.clone(),
            budgets: rollout_budgets
                .get(&snap.rollout_id)
                .cloned()
                .unwrap_or_default(),
            terminal_at: snap.terminal_at,
        })
        .collect();

    // last_rolled_refs reflects "what rollout(s) the CP has already
    // recorded for each channel". Source: the rollouts table snapshot,
    // mapping channel → target_channel_ref. Without this, the reconciler
    // re-emits OpenRollout every tick because the channel_refs ↔ last_rolled_refs
    // equality check never matches. Empty hashmap was a load-bearing
    // bug — populating it makes the OpenRollout action fire once per
    // ref change, not once per tick.
    let mut last_rolled_refs: HashMap<String, String> = HashMap::new();
    for snap in rollouts {
        last_rolled_refs.insert(snap.channel.clone(), snap.target_channel_ref.clone());
    }

    Observed {
        channel_refs: channel_refs.clone(),
        last_rolled_refs,
        host_state,
        active_rollouts,
        compliance_failures_by_rollout,
        last_deferrals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nixfleet_proto::agent_wire::{CheckinRequest, GenerationRef};

    fn checkin_for(hostname: &str, closure: &str) -> HostCheckinRecord {
        HostCheckinRecord {
            last_checkin: Utc::now(),
            checkin: CheckinRequest {
                hostname: hostname.to_string(),
                agent_version: "0.2.0".to_string(),
                current_generation: GenerationRef {
                    closure_hash: closure.to_string(),
                    channel_ref: None,
                    boot_id: "boot".to_string(),
                },
                pending_generation: None,
                last_evaluated_target: None,
                last_fetch_outcome: None,
                uptime_secs: Some(1),
                last_confirmed_at: None,
            },
        }
    }

    #[test]
    fn projection_reflects_each_host_checkin() {
        let mut checkins = HashMap::new();
        checkins.insert("test-host".to_string(), checkin_for("test-host", "abc"));
        checkins.insert("ohm".to_string(), checkin_for("ohm", "def"));

        let channel_refs = HashMap::from([("dev".to_string(), "deadbeef".to_string())]);
        let observed = project(&checkins, &channel_refs, &[], HashMap::new(), HashMap::new(), &HashMap::new());

        assert_eq!(observed.host_state.len(), 2);
        assert_eq!(
            observed.host_state["test-host"]
                .current_generation
                .as_deref(),
            Some("abc")
        );
        assert!(observed.host_state["test-host"].online);
        assert_eq!(observed.channel_refs["dev"], "deadbeef");
    }

    #[test]
    fn projection_with_no_checkins_yields_empty_host_state() {
        let observed = project(&HashMap::new(), &HashMap::new(), &[], HashMap::new(), HashMap::new(), &HashMap::new());
        assert!(observed.host_state.is_empty());
        assert!(observed.channel_refs.is_empty());
        assert!(observed.active_rollouts.is_empty());
    }

    #[test]
    fn host_rollout_state_check_matches_enum() {
        let schema = include_str!("../migrations/V001__schema.sql");
        let needle = "host_state IN (";
        let start = schema.find(needle).expect("CHECK clause present");
        let after = &schema[start + needle.len()..];
        let end = after.find(')').expect("CHECK clause closes");
        let list = &after[..end];
        let values: Vec<&str> = list
            .split(',')
            .map(|s: &str| s.trim().trim_matches('\'').trim())
            .filter(|s: &&str| !s.is_empty())
            .collect();
        assert!(!values.is_empty(), "expected ≥1 value in CHECK clause");
        for v in &values {
            HostRolloutState::from_db_str(v).unwrap_or_else(|_| {
                panic!(
                    "V001 CHECK list value {v:?} is not in HostRolloutState. \
                     Either extend the enum or remove the value from the CHECK."
                )
            });
        }
    }

    #[test]
    fn projection_falls_back_to_failed_on_unknown_host_state() {
        let mut host_states = HashMap::new();
        host_states.insert("ohm".to_string(), "TotallyBogus".to_string());
        let snap = RolloutDbSnapshot {
            rollout_id: "stable@deadbeef".to_string(),
            channel: "stable".to_string(),
            target_closure_hash: "system-r1".to_string(),
            target_channel_ref: "stable@deadbeef".to_string(),
            host_states,
            last_healthy_since: HashMap::new(),
            current_wave: 0,
        terminal_at: None,
        };
        let observed = project(
            &HashMap::new(),
            &HashMap::new(),
            std::slice::from_ref(&snap),
            HashMap::new(),
            HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            observed.active_rollouts[0].host_states.get("ohm").copied(),
            Some(HostRolloutState::Failed),
        );
    }

    #[test]
    fn projection_round_trips_reverted_state() {
        let mut host_states = HashMap::new();
        host_states.insert("ohm".to_string(), "Reverted".to_string());
        let snap = RolloutDbSnapshot {
            rollout_id: "stable@deadbeef".to_string(),
            channel: "stable".to_string(),
            target_closure_hash: "system-r1".to_string(),
            target_channel_ref: "stable@deadbeef".to_string(),
            host_states,
            last_healthy_since: HashMap::new(),
            current_wave: 0,
        terminal_at: None,
        };
        let observed = project(
            &HashMap::new(),
            &HashMap::new(),
            std::slice::from_ref(&snap),
            HashMap::new(),
            HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            observed.active_rollouts[0].host_states.get("ohm").copied(),
            Some(HostRolloutState::Reverted),
        );
    }

    #[test]
    fn projection_surfaces_active_rollouts_from_snapshot() {
        let now = Utc::now();
        let mut host_states = HashMap::new();
        host_states.insert("ohm".to_string(), "Healthy".to_string());
        host_states.insert("krach".to_string(), "ConfirmWindow".to_string());
        let mut last_healthy = HashMap::new();
        last_healthy.insert("ohm".to_string(), now);

        let snap = RolloutDbSnapshot {
            rollout_id: "stable@abc12345".to_string(),
            channel: "stable".to_string(),
            target_closure_hash: "system-r1".to_string(),
            target_channel_ref: "stable@abc12345".to_string(),
            host_states,
            last_healthy_since: last_healthy,
            current_wave: 0,
        terminal_at: None,
        };
        let observed = project(
            &HashMap::new(),
            &HashMap::new(),
            std::slice::from_ref(&snap),
            HashMap::new(),
            HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(observed.active_rollouts.len(), 1);
        let r = &observed.active_rollouts[0];
        assert_eq!(r.id, "stable@abc12345");
        assert_eq!(r.channel, "stable");
        assert_eq!(r.target_ref, "stable@abc12345");
        assert_eq!(r.state, RolloutState::Executing);
        assert_eq!(r.current_wave, 0);
        assert_eq!(
            r.host_states.get("ohm").copied(),
            Some(HostRolloutState::Healthy),
        );
        assert_eq!(
            r.host_states.get("krach").copied(),
            Some(HostRolloutState::ConfirmWindow),
        );
        assert_eq!(r.last_healthy_since.len(), 1);
        assert_eq!(r.last_healthy_since["ohm"].timestamp(), now.timestamp());
    }
}
