//! Per-checkin host-state hygiene running alongside dispatch.

use nixfleet_proto::agent_wire::CheckinRequest;

use super::super::state::AppState;

/// Emit `RollbackSignal` for hosts in `Failed` under `rollback-and-halt`.
pub(super) async fn rollback_signal_for_checkin(
    state: &AppState,
    req: &CheckinRequest,
) -> Option<nixfleet_proto::agent_wire::RollbackSignal> {
    let db = state.db.as_ref()?;
    let fleet = state.verified_fleet.read().await.clone()?.fleet;
    let failed = match db.rollout_state().failed_rollouts_for_host(&req.hostname) {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(
                hostname = %req.hostname,
                error = %err,
                "rollback-signal: failed_rollouts_for_host query failed",
            );
            return None;
        }
    };
    let signal = compute_rollback_signal(&fleet, &req.hostname, &failed)?;
    tracing::info!(
        target: "rollback-signal",
        hostname = %req.hostname,
        rollout = %signal.rollout,
        target_ref = %signal.target_ref,
        "rollback-signal: emitting RollbackSignal (policy: rollback-and-halt, host: Failed)",
    );
    Some(signal)
}

fn compute_rollback_signal(
    fleet: &nixfleet_proto::FleetResolved,
    hostname: &str,
    failed_rollouts: &[(String, String)],
) -> Option<nixfleet_proto::agent_wire::RollbackSignal> {
    let host = fleet.hosts.get(hostname)?;
    let channel = fleet.channels.get(&host.channel)?;
    let policy = fleet.rollout_policies.get(&channel.rollout_policy)?;
    if !matches!(
        policy.on_health_failure,
        nixfleet_proto::OnHealthFailure::RollbackAndHalt
    ) {
        return None;
    }
    let (rollout_id, target_ref) = failed_rollouts.first()?;
    Some(nixfleet_proto::agent_wire::RollbackSignal {
        rollout: rollout_id.clone(),
        target_ref: target_ref.clone(),
        reason: format!("policy: rollback-and-halt; host {} is Failed", hostname,),
    })
}

/// Clear Healthy marker when the host's reported closure no longer matches.
// LOADBEARING: must run before dispatch_target_for_checkin so soak-state hygiene is in place.
pub(super) async fn clear_left_healthy_for_checkin(state: &AppState, req: &CheckinRequest) {
    let Some(db) = state.db.as_ref() else {
        return;
    };
    let healthy = match db.rollout_state().healthy_rollouts_for_host(&req.hostname) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                hostname = %req.hostname,
                error = %err,
                "checkin: healthy_rollouts_for_host query failed",
            );
            return;
        }
    };
    for (rollout_id, target_closure) in healthy {
        if req.current_generation.closure_hash == target_closure {
            continue;
        }
        match db
            .rollout_state()
            .clear_healthy_marker(&req.hostname, &rollout_id)
        {
            Ok(n) if n > 0 => {
                tracing::info!(
                    target: "soak",
                    hostname = %req.hostname,
                    rollout = %rollout_id,
                    target_closure = %target_closure,
                    current_closure = %req.current_generation.closure_hash,
                    "checkin: host left Healthy (closure mismatch); cleared soak timer",
                );
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    hostname = %req.hostname,
                    rollout = %rollout_id,
                    error = %err,
                    "checkin: clear_healthy_marker failed",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::fleet_with_host;
    use super::*;

    fn with_policy(
        mut fleet: nixfleet_proto::FleetResolved,
        policy: nixfleet_proto::OnHealthFailure,
    ) -> nixfleet_proto::FleetResolved {
        if let Some(p) = fleet.rollout_policies.get_mut("default") {
            p.on_health_failure = policy;
        }
        fleet
    }

    #[test]
    fn compute_rollback_signal_emits_under_rollback_and_halt() {
        let fleet = with_policy(
            fleet_with_host("test-host", Some("system-r1")),
            nixfleet_proto::OnHealthFailure::RollbackAndHalt,
        );
        let failed = vec![("stable@abc12345".to_string(), "ref-r1".to_string())];
        let signal =
            compute_rollback_signal(&fleet, "test-host", &failed).expect("signal expected");
        assert_eq!(signal.rollout, "stable@abc12345");
        assert_eq!(signal.target_ref, "ref-r1");
        assert!(
            signal.reason.contains("rollback-and-halt"),
            "reason should name the policy: {}",
            signal.reason,
        );
    }

    #[test]
    fn compute_rollback_signal_returns_none_under_halt() {
        let fleet = with_policy(
            fleet_with_host("test-host", Some("system-r1")),
            nixfleet_proto::OnHealthFailure::Halt,
        );
        let failed = vec![("stable@abc12345".to_string(), "ref-r1".to_string())];
        assert!(compute_rollback_signal(&fleet, "test-host", &failed).is_none());
    }

    #[test]
    fn compute_rollback_signal_returns_none_when_no_failed_rollouts() {
        let fleet = with_policy(
            fleet_with_host("test-host", Some("system-r1")),
            nixfleet_proto::OnHealthFailure::RollbackAndHalt,
        );
        assert!(compute_rollback_signal(&fleet, "test-host", &[]).is_none());
    }

    #[test]
    fn compute_rollback_signal_returns_none_when_host_unknown() {
        let fleet = with_policy(
            fleet_with_host("test-host", Some("system-r1")),
            nixfleet_proto::OnHealthFailure::RollbackAndHalt,
        );
        let failed = vec![("stable@abc12345".to_string(), "ref-r1".to_string())];
        assert!(compute_rollback_signal(&fleet, "ghost-host", &failed).is_none());
    }
}
