//! Phase 4 § 5 #2 — HTTP route coverage for the audit, bootstrap,
//! and public route families.
//!
//! Routes covered:
//!   GET    /api/v1/audit
//!   GET    /api/v1/audit/export
//!   POST   /api/v1/keys/bootstrap
//!   GET    /health
//!   GET    /metrics

#[path = "harness.rs"]
mod harness;

use harness::{client_anonymous, client_with_key, TEST_DEPLOY_KEY, TEST_READONLY_KEY};
use serde_json::json;

// =====================================================================
// GET /api/v1/audit
// =====================================================================

#[tokio::test]
async fn list_audit_events_returns_array() {
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
async fn list_audit_events_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .get(format!("{}/api/v1/audit", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn list_audit_events_readonly_role_succeeds() {
    let cp = harness::spawn_cp().await;
    let resp = client_with_key(TEST_READONLY_KEY)
        .get(format!("{}/api/v1/audit", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// =====================================================================
// GET /api/v1/audit/export
// =====================================================================

#[tokio::test]
async fn export_audit_csv_returns_csv_body() {
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
async fn export_audit_csv_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .get(format!("{}/api/v1/audit/export", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn export_audit_csv_deploy_role_succeeds() {
    let cp = harness::spawn_cp().await;
    let resp = client_with_key(TEST_DEPLOY_KEY)
        .get(format!("{}/api/v1/audit/export", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// =====================================================================
// POST /api/v1/keys/bootstrap
// =====================================================================
//
// The harness pre-seeds three API keys via seed_key, so the bootstrap
// route always sees keys-already-exist on the default spawn_cp(). To
// test the happy path we'd need a CP with NO seeded keys; the
// harness doesn't expose that, so we test the 409 conflict path here
// (the happy path is already covered by Phase 3 A1 in
// `auth_scenarios.rs::a1_bootstrap_first_admin_then_409`).

#[tokio::test]
async fn bootstrap_when_keys_exist_returns_409() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .post(format!("{}/api/v1/keys/bootstrap", cp.base))
        .json(&json!({"name": "should-fail"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

// =====================================================================
// GET /health
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

#[tokio::test]
async fn health_works_with_auth_too() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .get(format!("{}/health", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// =====================================================================
// GET /metrics
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
    let resp = client_anonymous()
        .get(format!("{}/metrics", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn metrics_route_works_with_auth_too() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .get(format!("{}/metrics", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
