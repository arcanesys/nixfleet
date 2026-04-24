//! Disruption-budget and edge-ordering fixtures.

#[path = "common/mod.rs"]
mod common;

#[test]
fn budget_exhausted_skip() {
    let (actual, expected) = common::run("budgets_edges/budget_exhausted_skip");
    common::assert_matches(&actual, &expected);
}

#[test]
fn budget_across_rollouts() {
    let (actual, expected) = common::run("budgets_edges/budget_across_rollouts");
    common::assert_matches(&actual, &expected);
}

#[test]
fn edge_predecessor_blocks() {
    let (actual, expected) = common::run("budgets_edges/edge_predecessor_blocks");
    common::assert_matches(&actual, &expected);
}

#[test]
fn edge_predecessor_done_dispatch() {
    let (actual, expected) = common::run("budgets_edges/edge_predecessor_done_dispatch");
    common::assert_matches(&actual, &expected);
}
