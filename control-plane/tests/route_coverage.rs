//! HTTP route happy / error / auth coverage for every admin route.
//!
//! Tests are grouped into `// =====` sections below so a failure can
//! be attributed at a glance, and filtered from the CLI via
//! `cargo test -p nixfleet-control-plane --test route_coverage <substring>`.
//!
//! Routes covered:
//!
//!   Machines family
//!     GET    /api/v1/machines
//!     POST   /api/v1/machines/{id}/register
//!     PATCH  /api/v1/machines/{id}/lifecycle
//!     DELETE /api/v1/machines/{id}/tags/{tag}
//!     GET    /api/v1/machines/{id}/desired-generation
//!     POST   /api/v1/machines/{id}/report
//!
//!   Rollouts family
//!     POST /api/v1/rollouts
//!     GET  /api/v1/rollouts
//!     GET  /api/v1/rollouts/{id}
//!     POST /api/v1/rollouts/{id}/resume
//!     POST /api/v1/rollouts/{id}/cancel
//!
//!   Releases family
//!     GET    /api/v1/releases
//!     POST   /api/v1/releases
//!     GET    /api/v1/releases/{id}
//!     DELETE /api/v1/releases/{id}
//!     GET    /api/v1/releases/{id}/diff/{other_id}
//!
//!   Audit + bootstrap + public
//!     GET    /api/v1/audit
//!     GET    /api/v1/audit/export
//!     POST   /api/v1/keys/bootstrap
//!     GET    /health
//!     GET    /metrics
//!
//! Pattern per route: happy path + error path + auth path (where
//! applicable). Cases that a domain-specific scenario file already
//! pins end-to-end (e.g. failure thresholds in `failure_scenarios.rs`,
//! rollout strategies in `deploy_scenarios.rs`) are skipped here to
//! avoid duplication.

#[path = "harness.rs"]
mod harness;

use harness::{client_anonymous, client_with_key, TEST_DEPLOY_KEY, TEST_READONLY_KEY};
use nixfleet_types::rollout::{
    CreateRolloutRequest, OnFailure, RolloutStatus, RolloutStrategy, RolloutTarget,
};
use nixfleet_types::Report;
use serde_json::json;

// =====================================================================
// Machines — GET /api/v1/machines
// =====================================================================

#[tokio::test]
async fn machines_list_returns_empty_when_none_registered() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn machines_list_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().get(format!("{}/api/v1/machines", cp.base)),
        401,
    )
    .await;
}

// =====================================================================
// Machines — POST /api/v1/machines/{id}/register
// =====================================================================

#[tokio::test]
async fn machines_register_creates_machine_and_returns_201() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .post(format!("{}/api/v1/machines/web-01/register", cp.base))
        .json(&json!({"tags": ["web"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["machine_id"], "web-01");
    assert_eq!(body["lifecycle"], "active");

    // Side effect: machine appears in subsequent list.
    let list: serde_json::Value = cp
        .admin
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["machine_id"] == "web-01"));
}

#[tokio::test]
async fn machines_register_invalid_lifecycle_returns_400() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/machines/web-01/register", cp.base))
            .json(&json!({"lifecycle": "not-a-real-state", "tags": []})),
        400,
    )
    .await;
}

#[tokio::test]
async fn machines_register_readonly_role_returns_403() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_with_key(TEST_READONLY_KEY)
            .post(format!("{}/api/v1/machines/web-01/register", cp.base))
            .json(&json!({"tags": []})),
        403,
    )
    .await;
}

// =====================================================================
// Machines — PATCH /api/v1/machines/{id}/lifecycle
// =====================================================================

#[tokio::test]
async fn machines_lifecycle_invalid_transition_returns_409() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;

    // The harness registers the machine as "active". Decommissioned is
    // a terminal state — once there, the only valid target is itself.
    // Move web-01 to decommissioned via a valid transition first.
    let move_to_decom = cp
        .admin
        .patch(format!("{}/api/v1/machines/web-01/lifecycle", cp.base))
        .json(&json!({"lifecycle": "decommissioned"}))
        .send()
        .await
        .unwrap();
    assert_eq!(move_to_decom.status(), 200);

    // Now try to move it back to active — invalid transition.
    let resp = cp
        .admin
        .patch(format!("{}/api/v1/machines/web-01/lifecycle", cp.base))
        .json(&json!({"lifecycle": "active"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn machines_lifecycle_invalid_state_string_returns_400() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    harness::assert_status(
        cp.admin
            .patch(format!("{}/api/v1/machines/web-01/lifecycle", cp.base))
            .json(&json!({"lifecycle": "imaginary"})),
        400,
    )
    .await;
}

#[tokio::test]
async fn machines_lifecycle_unknown_machine_returns_404() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        cp.admin
            .patch(format!("{}/api/v1/machines/ghost/lifecycle", cp.base))
            .json(&json!({"lifecycle": "decommissioned"})),
        404,
    )
    .await;
}

#[tokio::test]
async fn machines_lifecycle_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    harness::assert_status(
        client_anonymous()
            .patch(format!("{}/api/v1/machines/web-01/lifecycle", cp.base))
            .json(&json!({"lifecycle": "decommissioned"})),
        401,
    )
    .await;
}

// =====================================================================
// Machines — DELETE /api/v1/machines/{id}/tags/{tag}
// =====================================================================

#[tokio::test]
async fn machines_remove_tag_succeeds_and_drops_from_list() {
    let cp = harness::spawn_cp().await;

    // Use the HTTP register endpoint so the in-memory fleet state is
    // populated with the tags. The harness::register_machine helper
    // only writes to the DB and creates an empty fleet entry, so the
    // list_machines handler (which reads from fleet state) would see
    // an empty tags vec.
    cp.admin
        .post(format!("{}/api/v1/machines/web-01/register", cp.base))
        .json(&json!({"tags": ["web", "us-west"]}))
        .send()
        .await
        .unwrap();

    let resp = cp
        .admin
        .delete(format!("{}/api/v1/machines/web-01/tags/web", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Side effect: machine no longer has the "web" tag.
    let list: serde_json::Value = cp
        .admin
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let m = list
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["machine_id"] == "web-01")
        .expect("web-01 must still be listed");
    let tags = m["tags"].as_array().unwrap();
    assert!(!tags.iter().any(|t| t == "web"));
    assert!(tags.iter().any(|t| t == "us-west"));
}

#[tokio::test]
async fn machines_remove_tag_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    harness::assert_status(
        client_anonymous().delete(format!("{}/api/v1/machines/web-01/tags/web", cp.base)),
        401,
    )
    .await;
}

#[tokio::test]
async fn machines_remove_tag_readonly_returns_403() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    harness::assert_status(
        client_with_key(TEST_READONLY_KEY)
            .delete(format!("{}/api/v1/machines/web-01/tags/web", cp.base)),
        403,
    )
    .await;
}

// =====================================================================
// Machines — GET /api/v1/machines/{id}/desired-generation
// =====================================================================

#[tokio::test]
async fn machines_desired_generation_returns_404_when_not_set() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    harness::assert_status(
        cp.admin.get(format!(
            "{}/api/v1/machines/web-01/desired-generation",
            cp.base
        )),
        404,
    )
    .await;
}

#[tokio::test]
async fn machines_desired_generation_returns_400_when_id_too_long() {
    let cp = harness::spawn_cp().await;
    let long_id = "x".repeat(257);
    harness::assert_status(
        cp.admin.get(format!(
            "{}/api/v1/machines/{long_id}/desired-generation",
            cp.base
        )),
        400,
    )
    .await;
}

// =====================================================================
// Machines — POST /api/v1/machines/{id}/report
// =====================================================================

/// Baseline valid `Report` body. Tests override one field to hit a
/// specific size-validation branch.
fn valid_report(machine_id: &str) -> Report {
    Report {
        machine_id: machine_id.to_string(),
        current_generation: "/nix/store/abc".to_string(),
        success: true,
        message: "ok".to_string(),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    }
}

#[tokio::test]
async fn machines_report_rejects_oversized_current_generation() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let mut report = valid_report("web-01");
    report.current_generation = "/nix/store/".to_string() + &"x".repeat(600);
    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/machines/web-01/report", cp.base))
            .json(&report),
        400,
    )
    .await;
}

#[tokio::test]
async fn machines_report_rejects_oversized_message() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let mut report = valid_report("web-01");
    report.message = "x".repeat(5000);
    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/machines/web-01/report", cp.base))
            .json(&report),
        400,
    )
    .await;
}

#[tokio::test]
async fn machines_report_rejects_oversized_machine_id_in_path() {
    let cp = harness::spawn_cp().await;
    let long_id = "x".repeat(257);
    let report = valid_report(&long_id);
    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/machines/{long_id}/report", cp.base))
            .json(&report),
        400,
    )
    .await;
}

// =====================================================================
// Rollouts — POST /api/v1/rollouts
// =====================================================================

/// Build a valid `CreateRolloutRequest` with an all-at-once,
/// zero-tolerance, pause-on-failure shape. Shared by every POST
/// /rollouts test below.
fn valid_rollout_body(release_id: String, tag: &str) -> CreateRolloutRequest {
    CreateRolloutRequest {
        release_id,
        cache_url: None,
        strategy: RolloutStrategy::AllAtOnce,
        batch_sizes: None,
        failure_threshold: "0".to_string(),
        on_failure: OnFailure::Pause,
        health_timeout: Some(60),
        target: RolloutTarget::Tags(vec![tag.to_string()]),
    }
}

#[tokio::test]
async fn rollouts_create_unknown_release_returns_404_or_400() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let body = valid_rollout_body("rel-does-not-exist".to_string(), "web");
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
async fn rollouts_create_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let release_id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let body = valid_rollout_body(release_id, "web");
    harness::assert_status(
        client_anonymous()
            .post(format!("{}/api/v1/rollouts", cp.base))
            .json(&body),
        401,
    )
    .await;
}

#[tokio::test]
async fn rollouts_create_readonly_returns_403() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let body = valid_rollout_body(release_id, "web");
    harness::assert_status(
        client_with_key(TEST_READONLY_KEY)
            .post(format!("{}/api/v1/rollouts", cp.base))
            .json(&body),
        403,
    )
    .await;
}

// =====================================================================
// Rollouts — GET /api/v1/rollouts
// =====================================================================

#[tokio::test]
async fn rollouts_list_empty_returns_empty_array() {
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
async fn rollouts_list_with_status_filter() {
    let (cp, _, _) = harness::spawn_cp_with_rollout("/nix/store/x").await;

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
    assert!(
        !arr.is_empty(),
        "running filter must match at least the new rollout"
    );
    assert!(arr.iter().all(|r| r["status"] == "running"));
}

#[tokio::test]
async fn rollouts_list_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().get(format!("{}/api/v1/rollouts", cp.base)),
        401,
    )
    .await;
}

// =====================================================================
// Rollouts — GET /api/v1/rollouts/{id}
// =====================================================================

#[tokio::test]
async fn rollouts_get_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        cp.admin.get(format!("{}/api/v1/rollouts/r-ghost", cp.base)),
        404,
    )
    .await;
}

#[tokio::test]
async fn rollouts_get_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().get(format!("{}/api/v1/rollouts/r-anything", cp.base)),
        401,
    )
    .await;
}

// =====================================================================
// Rollouts — POST /api/v1/rollouts/{id}/resume
// =====================================================================

#[tokio::test]
async fn rollouts_resume_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/rollouts/r-ghost/resume", cp.base)),
        404,
    )
    .await;
}

#[tokio::test]
async fn rollouts_resume_when_running_returns_409() {
    let (cp, _, id) = harness::spawn_cp_with_rollout("/nix/store/x").await;

    // Rollout is created in `running` state — resume must 409 because
    // there's nothing to resume.
    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/rollouts/{}/resume", cp.base, id)),
        409,
    )
    .await;
}

#[tokio::test]
async fn rollouts_resume_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().post(format!("{}/api/v1/rollouts/r-anything/resume", cp.base)),
        401,
    )
    .await;
}

// =====================================================================
// Rollouts — POST /api/v1/rollouts/{id}/cancel
// =====================================================================

#[tokio::test]
async fn rollouts_cancel_running_succeeds_and_state_changes() {
    let (cp, _, id) = harness::spawn_cp_with_rollout("/nix/store/x").await;

    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id)),
        200,
    )
    .await;

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
async fn rollouts_cancel_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/rollouts/r-ghost/cancel", cp.base)),
        404,
    )
    .await;
}

#[tokio::test]
async fn rollouts_cancel_already_cancelled_returns_409() {
    let (cp, _, id) = harness::spawn_cp_with_rollout("/nix/store/x").await;
    // First cancel succeeds.
    cp.admin
        .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id))
        .send()
        .await
        .unwrap();
    // Second cancel must 409 — the rollout is no longer active.
    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id)),
        409,
    )
    .await;
}

#[tokio::test]
async fn rollouts_cancel_readonly_returns_403() {
    let (cp, _, id) = harness::spawn_cp_with_rollout("/nix/store/x").await;
    harness::assert_status(
        client_with_key(TEST_READONLY_KEY)
            .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, id)),
        403,
    )
    .await;
}

// =====================================================================
// Releases — GET /api/v1/releases
// =====================================================================

#[tokio::test]
async fn releases_list_empty_returns_empty_array() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .get(format!("{}/api/v1/releases", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn releases_list_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().get(format!("{}/api/v1/releases", cp.base)),
        401,
    )
    .await;
}

#[tokio::test]
async fn releases_list_readonly_role_succeeds() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_with_key(TEST_READONLY_KEY).get(format!("{}/api/v1/releases", cp.base)),
        200,
    )
    .await;
}

// =====================================================================
// Releases — POST /api/v1/releases
// =====================================================================

/// A minimal valid release body with a single entry. Used as a base
/// for the POST /releases tests that aren't about the entries shape.
fn minimal_release_body() -> serde_json::Value {
    json!({
        "flake_ref": "test",
        "flake_rev": "deadbeef",
        "cache_url": null,
        "entries": [{
            "hostname": "web-01",
            "store_path": "/nix/store/x",
            "platform": "x86_64-linux",
            "tags": []
        }]
    })
}

#[tokio::test]
async fn releases_create_empty_entries_returns_400() {
    let cp = harness::spawn_cp().await;
    let body = json!({
        "flake_ref": "test",
        "flake_rev": "deadbeef",
        "cache_url": null,
        "entries": []
    });
    let resp = cp
        .admin
        .post(format!("{}/api/v1/releases", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "expected 400/422 for empty entries, got {status}"
    );
}

#[tokio::test]
async fn releases_create_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous()
            .post(format!("{}/api/v1/releases", cp.base))
            .json(&minimal_release_body()),
        401,
    )
    .await;
}

#[tokio::test]
async fn releases_create_readonly_returns_403() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_with_key(TEST_READONLY_KEY)
            .post(format!("{}/api/v1/releases", cp.base))
            .json(&minimal_release_body()),
        403,
    )
    .await;
}

// =====================================================================
// Releases — GET /api/v1/releases/{id}
// =====================================================================

#[tokio::test]
async fn releases_get_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        cp.admin
            .get(format!("{}/api/v1/releases/rel-ghost", cp.base)),
        404,
    )
    .await;
}

#[tokio::test]
async fn releases_get_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().get(format!("{}/api/v1/releases/rel-anything", cp.base)),
        401,
    )
    .await;
}

#[tokio::test]
async fn releases_get_happy_returns_release_with_entries() {
    let cp = harness::spawn_cp().await;
    let id = harness::create_release(
        &cp,
        &[
            ("web-01", "/nix/store/aaa-web-01"),
            ("web-02", "/nix/store/bbb-web-02"),
        ],
    )
    .await;

    let resp = cp
        .admin
        .get(format!("{}/api/v1/releases/{}", cp.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], id);
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
}

// =====================================================================
// Releases — DELETE /api/v1/releases/{id}
// =====================================================================

#[tokio::test]
async fn releases_delete_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        cp.admin
            .delete(format!("{}/api/v1/releases/rel-ghost", cp.base)),
        404,
    )
    .await;
}

#[tokio::test]
async fn releases_delete_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().delete(format!("{}/api/v1/releases/rel-anything", cp.base)),
        401,
    )
    .await;
}

#[tokio::test]
async fn releases_delete_deploy_role_returns_403() {
    // delete_release requires admin; deploy must be rejected.
    let cp = harness::spawn_cp().await;
    let id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    harness::assert_status(
        client_with_key(TEST_DEPLOY_KEY)
            .delete(format!("{}/api/v1/releases/{}", cp.base, id)),
        403,
    )
    .await;
}

// =====================================================================
// Releases — GET /api/v1/releases/{id}/diff/{other_id}
// =====================================================================

#[tokio::test]
async fn releases_diff_unknown_a_returns_404() {
    let cp = harness::spawn_cp().await;
    let id_b = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    harness::assert_status(
        cp.admin.get(format!(
            "{}/api/v1/releases/rel-ghost/diff/{}",
            cp.base, id_b
        )),
        404,
    )
    .await;
}

#[tokio::test]
async fn releases_diff_unknown_b_returns_404() {
    let cp = harness::spawn_cp().await;
    let id_a = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    harness::assert_status(
        cp.admin.get(format!(
            "{}/api/v1/releases/{}/diff/rel-ghost",
            cp.base, id_a
        )),
        404,
    )
    .await;
}

#[tokio::test]
async fn releases_diff_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().get(format!("{}/api/v1/releases/rel-a/diff/rel-b", cp.base)),
        401,
    )
    .await;
}

// =====================================================================
// Audit — GET /api/v1/audit
// =====================================================================

#[tokio::test]
async fn audit_list_events_returns_array() {
    let cp = harness::spawn_cp().await;
    // Trigger one audit event by registering a machine.
    cp.admin
        .post(format!("{}/api/v1/machines/web-01/register", cp.base))
        .json(&json!({"tags": []}))
        .send()
        .await
        .unwrap();

    let resp = cp
        .admin
        .get(format!("{}/api/v1/audit", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let events = body.as_array().unwrap();
    assert!(
        events.iter().any(|e| e["action"] == "register"),
        "audit must contain the register event we just triggered"
    );
}

#[tokio::test]
async fn audit_list_events_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().get(format!("{}/api/v1/audit", cp.base)),
        401,
    )
    .await;
}

#[tokio::test]
async fn audit_list_events_readonly_role_succeeds() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_with_key(TEST_READONLY_KEY).get(format!("{}/api/v1/audit", cp.base)),
        200,
    )
    .await;
}

// =====================================================================
// Audit — GET /api/v1/audit/export
// =====================================================================

#[tokio::test]
async fn audit_export_csv_returns_csv_body() {
    let cp = harness::spawn_cp().await;
    cp.admin
        .post(format!("{}/api/v1/machines/web-01/register", cp.base))
        .json(&json!({"tags": []}))
        .send()
        .await
        .unwrap();

    let resp = cp
        .admin
        .get(format!("{}/api/v1/audit/export", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    assert!(ct.starts_with("text/csv"), "expected text/csv, got {ct:?}");
    let body = resp.text().await.unwrap();
    assert!(body.starts_with("timestamp,actor,action,target,detail"));
    assert!(body.contains("register"));
}

#[tokio::test]
async fn audit_export_csv_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().get(format!("{}/api/v1/audit/export", cp.base)),
        401,
    )
    .await;
}

#[tokio::test]
async fn audit_export_csv_deploy_role_succeeds() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_with_key(TEST_DEPLOY_KEY).get(format!("{}/api/v1/audit/export", cp.base)),
        200,
    )
    .await;
}

// =====================================================================
// Bootstrap — POST /api/v1/keys/bootstrap
// =====================================================================
//
// The harness pre-seeds three API keys via seed_key, so the bootstrap
// route always sees keys-already-exist on the default spawn_cp(). The
// happy first-call path is covered by
// `auth_scenarios.rs::a1_bootstrap_first_admin_then_409` (which wipes
// the seeded keys first); here we only test the 409 conflict from a
// default spawn_cp().

#[tokio::test]
async fn bootstrap_when_keys_exist_returns_409() {
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous()
            .post(format!("{}/api/v1/keys/bootstrap", cp.base))
            .json(&json!({"name": "should-fail"})),
        409,
    )
    .await;
}

// =====================================================================
// Public — GET /health
// =====================================================================

#[tokio::test]
async fn health_returns_ok_without_auth() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .get(format!("{}/health", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "ok");
}

// =====================================================================
// Public — GET /metrics
// =====================================================================

#[tokio::test]
async fn metrics_route_returns_200_without_auth() {
    // The /metrics body content depends on the global prometheus
    // recorder which is process-wide and fragile across test files.
    // metrics_scenarios.rs is the canonical place that asserts the
    // body shape (ME1, ME2). This test only pins the routing
    // contract: /metrics is reachable without an Authorization header
    // and returns 200.
    let cp = harness::spawn_cp().await;
    harness::assert_status(
        client_anonymous().get(format!("{}/metrics", cp.base)),
        200,
    )
    .await;
}
