//! Per-host state-machine transitions (RFC-0002 §3.2).

#[path = "common/mod.rs"]
mod common;

#[test]
fn queued_to_dispatched() {
    let (actual, expected) = common::run("host/queued_to_dispatched");
    common::assert_matches(&actual, &expected);
}

#[test]
fn healthy_to_soaked() {
    let (actual, expected) = common::run("host/healthy_to_soaked");
    common::assert_matches(&actual, &expected);
}
