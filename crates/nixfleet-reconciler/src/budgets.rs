//! Disruption budget evaluation (RFC-0002 §4.2).

use crate::observed::Observed;
use nixfleet_proto::FleetResolved;

/// Count hosts currently in-flight across all active rollouts.
pub(crate) fn in_flight_count(observed: &Observed, budget_hosts: &[String]) -> u32 {
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
}

/// For a given host, return the tightest (in_flight, max_in_flight) across
/// all budgets that include the host.
pub(crate) fn budget_max(
    fleet: &FleetResolved,
    observed: &Observed,
    host: &str,
) -> Option<(u32, u32)> {
    fleet
        .disruption_budgets
        .iter()
        .filter(|b| b.hosts.iter().any(|bh| bh == host))
        .filter_map(|b| {
            b.max_in_flight
                .map(|max| (in_flight_count(observed, &b.hosts), max))
        })
        .min_by_key(|(_, max)| *max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observed::Rollout;
    use std::collections::HashMap;

    fn observed_with(rollout_hosts: Vec<(String, String)>) -> Observed {
        let mut host_states = HashMap::new();
        for (h, s) in rollout_hosts {
            host_states.insert(h, s);
        }
        Observed {
            channel_refs: HashMap::new(),
            last_rolled_refs: HashMap::new(),
            host_state: HashMap::new(),
            active_rollouts: vec![Rollout {
                id: "r".into(),
                channel: "c".into(),
                target_ref: "ref".into(),
                state: "Executing".into(),
                current_wave: 0,
                host_states,
            }],
        }
    }

    #[test]
    fn in_flight_count_empty() {
        let obs = observed_with(vec![]);
        assert_eq!(in_flight_count(&obs, &["a".into(), "b".into()]), 0);
    }

    #[test]
    fn in_flight_count_counts_only_in_flight_states() {
        let obs = observed_with(vec![
            ("a".into(), "Queued".into()),
            ("b".into(), "Dispatched".into()),
            ("c".into(), "Activating".into()),
            ("d".into(), "Soaked".into()),
            ("e".into(), "Healthy".into()),
        ]);
        let budget = vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()];
        assert_eq!(in_flight_count(&obs, &budget), 3);
    }

    #[test]
    fn in_flight_count_filters_by_budget_hosts() {
        let obs = observed_with(vec![
            ("a".into(), "Dispatched".into()),
            ("b".into(), "Dispatched".into()),
        ]);
        assert_eq!(in_flight_count(&obs, &["a".into()]), 1);
    }
}
