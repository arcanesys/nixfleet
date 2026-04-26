//! Live `Observed` projection from in-memory checkin state.
//!
//! Replaces Phase 2's hand-written `observed.json` as the default
//! source of truth for the reconcile loop. The file-backed input
//! stays as `--observed` for offline-replay debugging (operator
//! dumps in-memory state, reproduces a tick) and as a dev/test
//! fallback when no agents are checking in yet.
//!
//! For now this is intentionally a dumb projection — every host
//! that has ever checked in shows up as `online`, with its most
//! recent `currentGeneration.closureHash` as the
//! `current_generation` field. Phase 4 introduces staleness
//! detection (host with no checkin in N intervals → online: false)
//! and active-rollout tracking; this module's signature stays the
//! same so PR-4's logic plugs in cleanly.

use std::collections::HashMap;

use nixfleet_reconciler::observed::{HostState, Observed};

use crate::server::HostCheckinRecord;

/// Build an `Observed` from the in-memory checkin records and the
/// channel-refs cache. Pure function — caller takes the read locks.
pub fn project(
    host_checkins: &HashMap<String, HostCheckinRecord>,
    channel_refs: &HashMap<String, String>,
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

    Observed {
        channel_refs: channel_refs.clone(),
        // PR-4 doesn't yet track these; reconcile against the empty
        // case is fine — Phase 4's dispatch loop is what populates
        // active rollouts and last-rolled-refs.
        last_rolled_refs: HashMap::new(),
        host_state,
        active_rollouts: Vec::new(),
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
            },
        }
    }

    #[test]
    fn projection_reflects_each_host_checkin() {
        let mut checkins = HashMap::new();
        checkins.insert("krach".to_string(), checkin_for("krach", "abc"));
        checkins.insert("ohm".to_string(), checkin_for("ohm", "def"));

        let channel_refs = HashMap::from([("dev".to_string(), "deadbeef".to_string())]);
        let observed = project(&checkins, &channel_refs);

        assert_eq!(observed.host_state.len(), 2);
        assert_eq!(
            observed.host_state["krach"].current_generation.as_deref(),
            Some("abc")
        );
        assert!(observed.host_state["krach"].online);
        assert_eq!(observed.channel_refs["dev"], "deadbeef");
    }

    #[test]
    fn projection_with_no_checkins_yields_empty_host_state() {
        let observed = project(&HashMap::new(), &HashMap::new());
        assert!(observed.host_state.is_empty());
        assert!(observed.channel_refs.is_empty());
    }
}
