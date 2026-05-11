// Budget enforcement moved to per-rollout snapshots (see RFC-0001 §2.6
// rollout-manifest budget snapshot). Fixture-based budget tests retired
// in favour of in-line unit tests in `nixfleet-reconciler/src/host_state.rs`
// `budgets::tests` - those exercise the snapshot semantics directly,
// including cross-rollout selector-identity counting and the frozen-
// against-mid-rollout-retag invariant.

#[path = "common/mod.rs"]
mod common;

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
