//! Phase 4 § 5 #2 — HTTP route coverage for the releases family.
//!
//! Routes covered:
//!   GET    /api/v1/releases
//!   POST   /api/v1/releases
//!   GET    /api/v1/releases/{id}
//!   DELETE /api/v1/releases/{id}
//!   GET    /api/v1/releases/{id}/diff/{other_id}
//!
//! Pattern: happy + error + auth (where applicable). Skips slots
//! already covered by Phase 3 R3/R4/R5/R6.

#[path = "harness.rs"]
mod harness;

use harness::{client_anonymous, client_with_key, TEST_DEPLOY_KEY, TEST_READONLY_KEY};
use serde_json::json;

// =====================================================================
// GET /api/v1/releases
// =====================================================================

#[tokio::test]
async fn list_releases_empty_returns_empty_array() {
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
async fn list_releases_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .get(format!("{}/api/v1/releases", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn list_releases_readonly_role_succeeds() {
    let cp = harness::spawn_cp().await;
    let resp = client_with_key(TEST_READONLY_KEY)
        .get(format!("{}/api/v1/releases", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// =====================================================================
// POST /api/v1/releases
// =====================================================================

#[tokio::test]
async fn create_release_empty_entries_returns_400() {
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
async fn create_release_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let body = json!({
        "flake_ref": "test",
        "flake_rev": "deadbeef",
        "cache_url": null,
        "entries": [{
            "hostname": "web-01",
            "store_path": "/nix/store/x",
            "platform": "x86_64-linux",
            "tags": []
        }]
    });
    let resp = client_anonymous()
        .post(format!("{}/api/v1/releases", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn create_release_readonly_returns_403() {
    let cp = harness::spawn_cp().await;
    let body = json!({
        "flake_ref": "test",
        "flake_rev": "deadbeef",
        "cache_url": null,
        "entries": [{
            "hostname": "web-01",
            "store_path": "/nix/store/x",
            "platform": "x86_64-linux",
            "tags": []
        }]
    });
    let resp = client_with_key(TEST_READONLY_KEY)
        .post(format!("{}/api/v1/releases", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

// =====================================================================
// GET /api/v1/releases/{id}
// =====================================================================

#[tokio::test]
async fn get_release_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .get(format!("{}/api/v1/releases/rel-ghost", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn get_release_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .get(format!("{}/api/v1/releases/rel-anything", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn get_release_happy_returns_release_with_entries() {
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
// DELETE /api/v1/releases/{id}
// =====================================================================

#[tokio::test]
async fn delete_release_unknown_id_returns_404() {
    let cp = harness::spawn_cp().await;
    let resp = cp
        .admin
        .delete(format!("{}/api/v1/releases/rel-ghost", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn delete_release_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .delete(format!("{}/api/v1/releases/rel-anything", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn delete_release_deploy_role_returns_403() {
    // delete_release requires admin; deploy must be rejected.
    let cp = harness::spawn_cp().await;
    let id = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let resp = client_with_key(TEST_DEPLOY_KEY)
        .delete(format!("{}/api/v1/releases/{}", cp.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

// =====================================================================
// GET /api/v1/releases/{id}/diff/{other_id}
// =====================================================================

#[tokio::test]
async fn diff_releases_unknown_a_returns_404() {
    let cp = harness::spawn_cp().await;
    let id_b = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let resp = cp
        .admin
        .get(format!(
            "{}/api/v1/releases/rel-ghost/diff/{}",
            cp.base, id_b
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn diff_releases_unknown_b_returns_404() {
    let cp = harness::spawn_cp().await;
    let id_a = harness::create_release(&cp, &[("web-01", "/nix/store/x")]).await;
    let resp = cp
        .admin
        .get(format!(
            "{}/api/v1/releases/{}/diff/rel-ghost",
            cp.base, id_a
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn diff_releases_anonymous_returns_401() {
    let cp = harness::spawn_cp().await;
    let resp = client_anonymous()
        .get(format!(
            "{}/api/v1/releases/rel-a/diff/rel-b",
            cp.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}
