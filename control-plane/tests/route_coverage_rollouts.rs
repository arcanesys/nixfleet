//! Phase 4 § 5 #2 — HTTP route coverage for the rollouts family.
//!
//! Routes covered:
//!   POST /api/v1/rollouts
//!   GET  /api/v1/rollouts
//!   GET  /api/v1/rollouts/{id}
//!   POST /api/v1/rollouts/{id}/resume
//!   POST /api/v1/rollouts/{id}/cancel
//!
//! Pattern: happy + error + auth (where applicable). Skips slots
//! already covered by Phase 3 scenarios.

#[path = "harness.rs"]
mod harness;

use harness::{client_anonymous, client_with_key, TEST_READONLY_KEY};
use nixfleet_types::rollout::{
    CreateRolloutRequest, OnFailure, RolloutStatus, RolloutStrategy, RolloutTarget,
};

// =====================================================================
// POST /api/v1/rollouts
// =====================================================================

#[tokio::test]
async fn create_rollout_unknown_release_returns_404_or_400() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;

    let body = CreateRolloutRequest {
        release_id: "rel-does-not-exist".to_string(),
        cache_url: None,
        strategy: RolloutStrategy::AllAtOnce,
        batch_sizes: None,
        failure_threshold: "0".to_string(),
        on_failure: OnFailure::Pause,
        health_timeout: Some(60),
        target: RolloutTarget::Tags(vec!["web".to_string()]),
        policy: None,
    };
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    assert!(
        status == 404 || status == 400,
        "expected 404 or 400 for unknown release, got {status}"
    );
}

#[tokio::test]
async fn create_rollout_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let release_id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let body = CreateRolloutRequest {
        release_id,
        cache_url: None,
        strategy: RolloutStrategy::AllAtOnce,
        batch_sizes: None,
        failure_threshold: "0".to_string(),
        on_failure: OnFailure::Pause,
        health_timeout: Some(60),
        target: RolloutTarget::Tags(vec!["web".to_string()]),
        policy: None,
    };
    let resp = client_anonymous()
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn create_rollout_readonly_returns_403() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let body = CreateRolloutRequest {
        release_id,
        cache_url: None,
        strategy: RolloutStrategy::AllAtOnce,
        batch_sizes: None,
        failure_threshold: "0".to_string(),
        on_failure: OnFailure::Pause,
        health_timeout: Some(60),
        target: RolloutTarget::Tags(vec!["web".to_string()]),
        policy: None,
    };
    let resp = client_with_key(TEST_READONLY_KEY)
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

// =====================================================================
// GET /api/v1/rollouts
// =====================================================================

#[tokio::test]
async fn list_rollouts_empty_returns_empty_array() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .get(format!("{}/api/v1/rollouts", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_rollouts_with_status_filter() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let _id = harness::create_rollout_for_tag(
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

    // Filter for "running" — the rollout is running by default after create.
    let resp = cp
        .admin
        .get(format!("{}/api/v1/rollouts?status=running", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert!(!arr.is_empty(), "running filter must match at least the new rollout");
    assert!(arr.iter().all(|r| r["status"] == "running"));
}

#[tokio::test]
async fn list_rollouts_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .get(format!("{}/api/v1/rollouts", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// =====================================================================
// GET /api/v1/rollouts/{id}
// =====================================================================

#[tokio::test]
async fn get_rollout_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .get(format!("{}/api/v1/rollouts/r-ghost", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn get_rollout_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .get(format!("{}/api/v1/rollouts/r-anything", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// =====================================================================
// POST /api/v1/rollouts/{id}/resume
// =====================================================================

#[tokio::test]
async fn resume_rollout_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts/r-ghost/resume", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn resume_rollout_when_running_returns_409() {
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

    // Rollout is created in `running` state — resume must 409 because
    // there's nothing to resume.
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts/{}/resume", cp.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn resume_rollout_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .post(format!("{}/api/v1/rollouts/r-anything/resume", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// =====================================================================
// POST /api/v1/rollouts/{id}/cancel
// =====================================================================

#[tokio::test]
async fn cancel_rollout_running_succeeds_and_state_changes() {
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

    // Side effect: rollout reaches Cancelled.
    let detail = harness::wait_rollout_status(
        &cp,
        &id,
        RolloutStatus::Cancelled,
        std::time::Duration::from_secs(2),
    )
    .await;
    assert!(matches!(detail.status, RolloutStatus::Cancelled));
}

#[tokio::test]
async fn cancel_rollout_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts/r-ghost/cancel", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn cancel_rollout_already_cancelled_returns_409() {
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
    // First cancel succeeds.
    cp.admin
        .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id))
        .send()
        .await
        .unwrap();
    // Second cancel must 409 — the rollout is no longer active.
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn cancel_rollout_readonly_returns_403() {
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
    let resp = client_with_key(TEST_READONLY_KEY)
        .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}
