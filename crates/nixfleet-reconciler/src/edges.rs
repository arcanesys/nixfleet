//! Edge predecessor ordering check (RFC-0002 §4.1).

use crate::observed::Rollout;
use nixfleet_proto::FleetResolved;

/// If `host`'s in-wave predecessors are NOT all Soaked/Converged, return
/// the name of the first incomplete predecessor. Otherwise `None`.
pub(crate) fn predecessor_blocking(
    fleet: &FleetResolved,
    rollout: &Rollout,
    host: &str,
) -> Option<String> {
    fleet
        .edges
        .iter()
        .filter(|e| e.before == host)
        .find_map(|e| {
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
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observed::Rollout;
    use nixfleet_proto::{Edge, FleetResolved, Meta};
    use std::collections::HashMap;

    fn fleet_with_edges(edges: Vec<Edge>) -> FleetResolved {
        FleetResolved {
            schema_version: 1,
            hosts: HashMap::new(),
            channels: HashMap::new(),
            rollout_policies: HashMap::new(),
            waves: HashMap::new(),
            edges,
            disruption_budgets: Vec::new(),
            meta: Meta {
                schema_version: 1,
                signed_at: None,
                ci_commit: None,
                signature_algorithm: None,
            },
        }
    }

    fn rollout_with_states(states: Vec<(&str, &str)>) -> Rollout {
        let mut host_states = HashMap::new();
        for (h, s) in states {
            host_states.insert(h.to_string(), s.to_string());
        }
        Rollout {
            id: "r".into(),
            channel: "c".into(),
            target_ref: "ref".into(),
            state: "Executing".into(),
            current_wave: 0,
            host_states,
        }
    }

    #[test]
    fn no_edges_means_no_block() {
        let fleet = fleet_with_edges(vec![]);
        let rollout = rollout_with_states(vec![]);
        assert!(predecessor_blocking(&fleet, &rollout, "h1").is_none());
    }

    #[test]
    fn predecessor_done_is_not_blocking() {
        let fleet = fleet_with_edges(vec![Edge {
            before: "h1".into(),
            after: "h2".into(),
            reason: None,
        }]);
        let rollout = rollout_with_states(vec![("h2", "Soaked")]);
        assert!(predecessor_blocking(&fleet, &rollout, "h1").is_none());
    }

    #[test]
    fn predecessor_queued_is_blocking() {
        let fleet = fleet_with_edges(vec![Edge {
            before: "h1".into(),
            after: "h2".into(),
            reason: None,
        }]);
        let rollout = rollout_with_states(vec![("h2", "Queued")]);
        let blocker = predecessor_blocking(&fleet, &rollout, "h1");
        assert_eq!(blocker.as_deref(), Some("h2"));
    }
}
