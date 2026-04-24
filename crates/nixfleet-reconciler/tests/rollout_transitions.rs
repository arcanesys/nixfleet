//! Rollout-level state-machine transitions from RFC-0002 §3.1.

#[path = "common/mod.rs"]
mod common;

#[test]
fn pending_to_planning() {
    let (actual, expected) = common::run("rollout/pending_to_planning");
    common::assert_matches(&actual, &expected);
}
