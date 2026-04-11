//! Phase 4 § 5 #4 — Executor state transition coverage.
//!
//! For each transition the executor can make, this file pins:
//!   1. Positive: the documented inputs cause the transition.
//!   2. Negative: the transition does NOT fire when the conditions
//!      are not met.
//!
//! The transitions covered:
//!
//!   Created → Running          (rollout creation; route handler, not executor)
//!   Running → Paused (failure threshold)   ← already pinned by F5
//!   Running → Paused (operator pause)      ← N/A: no operator-pause route exists
//!   Running → Completed                    ← all batches succeeded
//!   Paused → Running           (resume)    ← already pinned by F1 + Task 8
//!   Running → Cancelled                    ← admin cancel
//!   Paused → Cancelled                     ← admin cancel from paused
//!   Cancelled → (terminal)                 ← cannot transition
//!
//! Slots already covered by Phase 3 scenarios are referenced in
//! comments and not re-tested here. New tests fill the gaps.

#[path = "harness.rs"]
mod harness;

use nixfleet_types::rollout::{OnFailure, RolloutStatus, RolloutStrategy};

// =====================================================================
// Created → Running
// =====================================================================

/// POST /api/v1/rollouts creates a rollout already in `running` state
/// (the CP does not have a separate Created state at the route level —
/// the enum variant exists for completeness but the create handler
/// inserts directly into running). This pins that contract.
#[tokio::test]
async fn created_to_running_happens_on_create() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    let detail: nixfleet_types::rollout::RolloutDetail = serde_json::from_str(
        &cp.admin
            .get(format!("{}/api/v1/rollouts/{}", cp.base, id))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        detail.status,
        RolloutStatus::Running,
        "create_rollout must initialise status=running"
    );
}

// =====================================================================
// Running → Completed
// =====================================================================

/// Pins that a rollout transitions to Completed when all batches
/// succeed. The executor needs:
///   tick A: batch pending → deploying (deploy_batch sets started_at)
///   tick B: batch deploying → succeeded (evaluate_batch sees healthy)
///   tick C: rollout running → completed (process_rollout sees no
///           current batch and all batches succeeded)
#[tokio::test]
async fn running_to_completed_when_all_batches_succeed() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id =
        harness::create_release(&cp, &[("web-01", "/nix/store/r2c-web-01")]).await;
    let id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    harness::tick_once(&cp).await; // pending → deploying
    harness::fake_agent_report(
        &cp,
        "web-01",
        "/nix/store/r2c-web-01",
        true,
        "ok",
        &["web"],
    )
    .await;
    cp.db.insert_health_report("web-01", "{}", true).unwrap();
    harness::tick_once(&cp).await; // deploying → succeeded
    harness::tick_once(&cp).await; // running → completed

    let detail = harness::wait_rollout_status(
        &cp,
        &id,
        RolloutStatus::Completed,
        std::time::Duration::from_secs(2),
    )
    .await;
    assert_eq!(detail.status, RolloutStatus::Completed);
}

/// Negative companion: a rollout with a still-pending batch (no agent
/// report) does NOT transition to Completed within the observation
/// window.
#[tokio::test]
async fn running_does_not_complete_while_batch_pending() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id =
        harness::create_release(&cp, &[("web-01", "/nix/store/np-web-01")]).await;
    let id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    // Tick a few times without ever sending an agent report.
    harness::tick_once(&cp).await;
    harness::tick_once(&cp).await;
    harness::tick_once(&cp).await;

    let detail: nixfleet_types::rollout::RolloutDetail = serde_json::from_str(
        &cp.admin
            .get(format!("{}/api/v1/rollouts/{}", cp.base, id))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_ne!(
        detail.status,
        RolloutStatus::Completed,
        "rollout must NOT complete while a batch is still pending"
    );
}

// =====================================================================
// Running → Cancelled
// =====================================================================

/// Pins that POST /cancel from Running transitions directly to
/// Cancelled (not via Paused). Already lightly covered by route
/// tests in route_coverage_rollouts.rs; this version asserts the
/// state-transition explicitly via the executor tick path.
#[tokio::test]
async fn running_to_cancelled_via_admin_cancel() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Tick the executor — it must NOT advance a cancelled rollout.
    harness::tick_once(&cp).await;
    let detail = harness::wait_rollout_status(
        &cp,
        &id,
        RolloutStatus::Cancelled,
        std::time::Duration::from_secs(2),
    )
    .await;
    assert_eq!(detail.status, RolloutStatus::Cancelled);
}

// =====================================================================
// Paused → Cancelled
// =====================================================================

/// Cancel must work from Paused state too (an operator may cancel a
/// paused rollout instead of resuming).
#[tokio::test]
async fn paused_to_cancelled_via_admin_cancel() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id =
        harness::create_release(&cp, &[("web-01", "/nix/store/p2c-web-01")]).await;
    let id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    // Pause via failure: insert an unhealthy report on the desired gen.
    harness::tick_once(&cp).await;
    harness::fake_agent_report(
        &cp,
        "web-01",
        "/nix/store/p2c-web-01",
        false,
        "boom",
        &["web"],
    )
    .await;
    cp.db
        .insert_health_report("web-01", "{\"fail\":true}", false)
        .unwrap();
    harness::tick_once(&cp).await;
    let _ = harness::wait_rollout_status(
        &cp,
        &id,
        RolloutStatus::Paused,
        std::time::Duration::from_secs(2),
    )
    .await;

    // Now cancel the paused rollout.
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let detail = harness::wait_rollout_status(
        &cp,
        &id,
        RolloutStatus::Cancelled,
        std::time::Duration::from_secs(2),
    )
    .await;
    assert_eq!(detail.status, RolloutStatus::Cancelled);
}

// =====================================================================
// Cancelled → (terminal)
// =====================================================================

/// Pins that a Cancelled rollout cannot be cancelled again (409),
/// resumed (409 since it's not paused), and is not advanced by the
/// executor tick.
#[tokio::test]
async fn cancelled_is_terminal_no_further_transitions() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    // Cancel.
    cp.admin
        .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id))
        .send()
        .await
        .unwrap();

    // Second cancel → 409.
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    // Resume on cancelled → 409 (resume only works from paused).
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts/{}/resume", cp.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    // Executor tick must leave it cancelled.
    harness::tick_once(&cp).await;
    let detail: nixfleet_types::rollout::RolloutDetail = serde_json::from_str(
        &cp.admin
            .get(format!("{}/api/v1/rollouts/{}", cp.base, id))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(detail.status, RolloutStatus::Cancelled);
}
