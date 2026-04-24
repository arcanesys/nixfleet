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
