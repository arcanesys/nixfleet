//! Phase 4 § 5 #2 — HTTP route coverage for the machines family.
//!
//! Routes covered:
//!   GET    /api/v1/machines
//!   POST   /api/v1/machines/{id}/register
//!   PATCH  /api/v1/machines/{id}/lifecycle
//!   DELETE /api/v1/machines/{id}/tags/{tag}
//!   GET    /api/v1/machines/{id}/desired-generation
//!   POST   /api/v1/machines/{id}/report
//!
//! Pattern per route: happy path + error path + auth path
//! (where applicable). Skips slots already covered by Phase 3
//! scenarios per the matrix in
//! `docs/superpowers/notes/2026-04-11-phase-4-coverage-matrix.md`.

#[path = "harness.rs"]
mod harness;

use harness::{client_anonymous, client_with_key, TEST_DEPLOY_KEY, TEST_READONLY_KEY};
use nixfleet_types::Report;
use serde_json::json;

// =====================================================================
// GET /api/v1/machines
// =====================================================================

#[tokio::test]
async fn list_machines_returns_empty_when_none_registered() {
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
async fn list_machines_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// =====================================================================
// POST /api/v1/machines/{id}/register
// =====================================================================

#[tokio::test]
async fn register_machine_creates_machine_and_returns_201() {
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
async fn register_machine_invalid_lifecycle_returns_400() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .post(format!("{}/api/v1/machines/web-01/register", cp.base))
        .json(&json!({"lifecycle": "not-a-real-state", "tags": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn register_machine_readonly_role_returns_403() {
    let cp = harness::spawn_cp().await;
    let readonly = client_with_key(TEST_READONLY_KEY);
    let resp = readonly
        .post(format!("{}/api/v1/machines/web-01/register", cp.base))
        .json(&json!({"tags": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

// =====================================================================
// PATCH /api/v1/machines/{id}/lifecycle
// =====================================================================

#[tokio::test]
async fn update_lifecycle_invalid_transition_returns_409() {
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
async fn update_lifecycle_invalid_state_string_returns_400() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let resp = cp
        .admin
        .patch(format!("{}/api/v1/machines/web-01/lifecycle", cp.base))
        .json(&json!({"lifecycle": "imaginary"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn update_lifecycle_unknown_machine_returns_404() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .patch(format!("{}/api/v1/machines/ghost/lifecycle", cp.base))
        .json(&json!({"lifecycle": "decommissioned"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn update_lifecycle_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let resp = client_anonymous()
        .patch(format!("{}/api/v1/machines/web-01/lifecycle", cp.base))
        .json(&json!({"lifecycle": "decommissioned"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// =====================================================================
// DELETE /api/v1/machines/{id}/tags/{tag}
// =====================================================================

#[tokio::test]
async fn remove_tag_succeeds_and_drops_from_list() {
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
async fn remove_tag_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let resp = client_anonymous()
        .delete(format!("{}/api/v1/machines/web-01/tags/web", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn remove_tag_readonly_returns_403() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let resp = client_with_key(TEST_READONLY_KEY)
        .delete(format!("{}/api/v1/machines/web-01/tags/web", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

// =====================================================================
// GET /api/v1/machines/{id}/desired-generation
// =====================================================================

#[tokio::test]
async fn desired_generation_returns_404_when_not_set() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let resp = cp
        .admin
        .get(format!("{}/api/v1/machines/web-01/desired-generation", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn desired_generation_returns_400_when_id_too_long() {
    let cp = harness::spawn_cp().await;
    let long_id = "x".repeat(257);
    let resp = cp
        .admin
        .get(format!("{}/api/v1/machines/{long_id}/desired-generation", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// =====================================================================
// POST /api/v1/machines/{id}/report
// =====================================================================

#[tokio::test]
async fn report_rejects_oversized_current_generation() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let huge = "/nix/store/".to_string() + &"x".repeat(600);
    let report = Report {
        machine_id: "web-01".to_string(),
        current_generation: huge,
        success: true,
        message: "ok".to_string(),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    };
    let resp = cp
        .admin
        .post(format!("{}/api/v1/machines/web-01/report", cp.base))
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn report_rejects_oversized_message() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let report = Report {
        machine_id: "web-01".to_string(),
        current_generation: "/nix/store/abc".to_string(),
        success: true,
        message: "x".repeat(5000),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    };
    let resp = cp
        .admin
        .post(format!("{}/api/v1/machines/web-01/report", cp.base))
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn report_rejects_oversized_machine_id_in_path() {
    let cp = harness::spawn_cp().await;
    let long_id = "x".repeat(257);
    let report = Report {
        machine_id: long_id.clone(),
        current_generation: "/nix/store/abc".to_string(),
        success: true,
        message: "ok".to_string(),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    };
    let resp = cp
        .admin
        .post(format!("{}/api/v1/machines/{long_id}/report", cp.base))
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// Compile-only marker so the unused-import lint stays happy if no test
// references TEST_DEPLOY_KEY.
#[allow(dead_code)]
fn _deploy_key_marker() -> &'static str {
    TEST_DEPLOY_KEY
}
