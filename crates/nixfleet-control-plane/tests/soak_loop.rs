//! End-to-end soak-loop test composing record/confirm, snapshot, project, reconcile, and transition.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use nixfleet_control_plane::db::{Db, DispatchInsert};
use nixfleet_control_plane::observed_projection;
use nixfleet_control_plane::state::{HealthyMarker, HostRolloutState};
use nixfleet_proto::FleetResolved;
use nixfleet_proto::fleet_resolved::Meta;
use nixfleet_proto::testing::FleetBuilder;
use nixfleet_reconciler::{Action, reconcile};
use tempfile::TempDir;

fn fleet_with_single_wave_host(hostname: &str, closure: &str, soak_minutes: u32) -> FleetResolved {
    let mut f = FleetBuilder::new()
        .channel("stable", "default")
        .host(hostname, "stable")
        .host_closure(hostname, closure)
        .wave_with_soak("stable", &[hostname], soak_minutes)
        .meta(Meta {
            schema_version: 1,
            signed_at: Some(Utc::now()),
            ci_commit: Some("abc12345".to_string()),
            signature_algorithm: Some("ed25519".into()),
        })
        .build();
    // Tweak: channel was first declared with a default, but host("...","stable")
    // (idempotent) keeps it. Tweak the per-channel reconcile/freshness/signing
    // intervals to match the original fixture.
    let c = f.channels.get_mut("stable").unwrap();
    c.reconcile_interval_minutes = 5;
    c.freshness_window = 60;
    c.signing_interval_minutes = 30;
    // Original fixture left rollout_policies empty (channel references
    // "default" but no policy registered) - reconcile() doesn't need it
    // here. Drop the auto-inserted "default" policy.
    f.rollout_policies = HashMap::new();
    f
}

#[test]
fn soak_loop_end_to_end_healthy_to_soaked_to_converged() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(Db::open(&tmp.path().join("state.db")).unwrap());
    db.migrate().unwrap();

    let host = "host-02";
    let rollout_id = "stable@abc12345";
    let target_closure = "deadbeef-system";
    let confirm_deadline = Utc::now() + chrono::Duration::seconds(120);
    let healthy_at = Utc::now() - chrono::Duration::minutes(10);
    let now = Utc::now();

    db.host_dispatch_state()
        .record_dispatch(&DispatchInsert {
            hostname: host,
            channel: "stable",
            rollout_id,
            wave: 0,
            target_closure_hash: target_closure,
            target_channel_ref: rollout_id,
            confirm_deadline,
        })
        .unwrap();
    let n = db.host_dispatch_state().confirm(host, rollout_id).unwrap();
    assert_eq!(n, 1, "confirm must flip the operational row");
    db.rollout_state()
        .transition_host_state(
            host,
            rollout_id,
            HostRolloutState::Healthy,
            HealthyMarker::Set(healthy_at),
            None,
        )
        .unwrap();

    let rollouts = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
    assert_eq!(rollouts.len(), 1, "snapshot must surface the rollout");
    assert_eq!(
        rollouts[0].host_states.get(host).map(String::as_str),
        Some("Healthy"),
        "host should be Healthy in the snapshot",
    );
    assert!(
        rollouts[0].last_healthy_since.contains_key(host),
        "soak marker must surface for projection",
    );

    let observed = observed_projection::project(
        &HashMap::new(),
        &HashMap::new(),
        &rollouts,
        HashMap::new(),
        HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(observed.active_rollouts.len(), 1);

    let fleet = fleet_with_single_wave_host(host, target_closure, 5);
    let actions = reconcile(&fleet, &observed, now);
    assert_eq!(actions.len(), 1, "expected exactly one action: {actions:?}");
    match &actions[0] {
        Action::SoakHost {
            rollout: r,
            host: h,
        } => {
            assert_eq!(r, rollout_id);
            assert_eq!(h, host);
        }
        other => panic!("expected Action::SoakHost, got {other:?}"),
    }

    let n = db
        .rollout_state()
        .transition_host_state(
            host,
            rollout_id,
            HostRolloutState::Soaked,
            HealthyMarker::Untouched,
            Some(HostRolloutState::Healthy),
        )
        .unwrap();
    assert_eq!(n, 1, "transition Healthy -> Soaked must update one row");

    let rollouts2 = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
    assert_eq!(
        rollouts2[0].host_states.get(host).map(String::as_str),
        Some("Soaked"),
        "host must surface as Soaked after the action processor",
    );
    let observed2 = observed_projection::project(
        &HashMap::new(),
        &HashMap::new(),
        &rollouts2,
        HashMap::new(),
        HashMap::new(),
        &HashMap::new(),
    );
    let actions2 = reconcile(&fleet, &observed2, now);
    assert!(
        actions2
            .iter()
            .any(|a| matches!(a, Action::ConvergeRollout { rollout } if rollout == rollout_id)),
        "single-wave Soaked host must promote to ConvergeRollout: {actions2:?}",
    );
}

#[test]
fn soak_loop_skips_when_window_not_elapsed() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(Db::open(&tmp.path().join("state.db")).unwrap());
    db.migrate().unwrap();

    let host = "host-02";
    let rollout_id = "stable@abc12345";
    let target_closure = "deadbeef-system";
    let healthy_at = Utc::now() - chrono::Duration::minutes(1);
    let now = Utc::now();

    db.host_dispatch_state()
        .record_dispatch(&DispatchInsert {
            hostname: host,
            channel: "stable",
            rollout_id,
            wave: 0,
            target_closure_hash: target_closure,
            target_channel_ref: rollout_id,
            confirm_deadline: Utc::now() + chrono::Duration::seconds(120),
        })
        .unwrap();
    db.host_dispatch_state().confirm(host, rollout_id).unwrap();
    db.rollout_state()
        .transition_host_state(
            host,
            rollout_id,
            HostRolloutState::Healthy,
            HealthyMarker::Set(healthy_at),
            None,
        )
        .unwrap();

    let rollouts = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
    let observed = observed_projection::project(
        &HashMap::new(),
        &HashMap::new(),
        &rollouts,
        HashMap::new(),
        HashMap::new(),
        &HashMap::new(),
    );
    let fleet = fleet_with_single_wave_host(host, target_closure, 5);
    let actions = reconcile(&fleet, &observed, now);
    assert!(
        actions.is_empty(),
        "soak window not elapsed; reconciler must defer: {actions:?}",
    );
}
