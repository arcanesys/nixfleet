//! Rollout-level state-machine transitions from RFC-0002 §3.1.

#[path = "common/mod.rs"]
mod common;

#[test]
fn pending_to_planning() {
    let (actual, expected) = common::run("rollout/pending_to_planning");
    common::assert_matches(&actual, &expected);
}

#[test]
fn planning_to_executing() {
    let (actual, expected) = common::run("rollout/planning_to_executing");
    common::assert_matches(&actual, &expected);
}

#[test]
fn wave_active_to_soaking() {
    let (actual, expected) = common::run("rollout/wave_active_to_soaking");
    common::assert_matches(&actual, &expected);
}

#[test]
fn wave_soaking_to_promoted() {
    let (actual, expected) = common::run("rollout/wave_soaking_to_promoted");
    common::assert_matches(&actual, &expected);
}

#[test]
fn all_waves_converged() {
    let (actual, expected) = common::run("rollout/all_waves_converged");
    common::assert_matches(&actual, &expected);
}

#[test]
fn onfailure_rollback_and_halt() {
    let (actual, expected) = common::run("rollout/onfailure_rollback_and_halt");
    common::assert_matches(&actual, &expected);
}

#[test]
fn onfailure_halt() {
    let (actual, expected) = common::run("rollout/onfailure_halt");
    common::assert_matches(&actual, &expected);
}
