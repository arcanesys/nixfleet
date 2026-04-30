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

#[test]
fn healthy_soak_elapsed_emits_soak_host() {
    let (actual, expected) = common::run("host/healthy_soak_elapsed");
    common::assert_matches(&actual, &expected);
}

#[test]
fn healthy_soak_not_elapsed_emits_nothing() {
    let (actual, expected) = common::run("host/healthy_soak_not_elapsed");
    common::assert_matches(&actual, &expected);
}

#[test]
fn confirmwindow_blocks_wave() {
    let (actual, expected) = common::run("host/confirmwindow_timeout_reverted");
    common::assert_matches(&actual, &expected);
}

#[test]
fn host_failed_triggers_halt() {
    let (actual, expected) = common::run("host/host_failed_triggers_halt");
    common::assert_matches(&actual, &expected);
}

#[test]
fn offline_host_skipped() {
    let (actual, expected) = common::run("host/offline_host_skipped");
    common::assert_matches(&actual, &expected);
}
