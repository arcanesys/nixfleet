//! Phase 4 § 5 #8 — auth role × endpoint matrix.
//!
//! Asserts that every admin route returns the right HTTP status for
//! every role tier (admin / deploy / readonly / anonymous). Role gates
//! are scattered across handlers; this file is the single place that
//! exercises every cell.
//!
//! Pre-existing coverage this builds on (not duplicated here):
//!   - `Actor::has_role` unit matrix in `control-plane/src/auth.rs`
//!   - A1 / A2 / A4 in `auth_scenarios.rs` (bootstrap 409, anonymous
//!     admin → 401, readonly cannot POST rollout)
//!   - `route_coverage_{machines,rollouts,releases,misc}.rs` tests for
//!     per-route happy + error paths
//!   - `cn_validation_scenarios.rs` for the agent-route mTLS CN layer
//!
//! What this file adds: explicit 4-role fan-out for every admin route
//! so that "readonly can GET /rollouts" (which route_coverage doesn't
//! test — it only does anonymous→401) and "deploy cannot PATCH a
//! lifecycle" are pinned as first-class assertions.

#[path = "harness.rs"]
mod harness;

use harness::{client_anonymous, client_with_key, TEST_API_KEY, TEST_DEPLOY_KEY, TEST_READONLY_KEY};
use serde_json::json;

/// Run a prepared fan-out: each tuple is (role_label, prepared
/// request builder, expected HTTP status). Used by every fan-out
/// test in this file so the loop shape is written once and each
/// caller only spells out the 4 per-role tuples.
async fn assert_role_responses(cases: Vec<(&'static str, reqwest::RequestBuilder, u16)>) {
    for (label, req, expected) in cases {
        let resp = req.send().await.unwrap();
        assert_eq!(
            resp.status().as_u16(),
            expected,
            "{label}: expected {expected}, got {}",
            resp.status().as_u16()
        );
    }
}

/// Convenience: the 4 role clients in their conventional order.
/// Used by every fan-out test.
fn four_clients() -> (
    reqwest::Client,
    reqwest::Client,
    reqwest::Client,
    reqwest::Client,
) {
    (
        client_with_key(TEST_API_KEY),
        client_with_key(TEST_DEPLOY_KEY),
        client_with_key(TEST_READONLY_KEY),
        client_anonymous(),
    )
}

// =====================================================================
// READ_ONLY routes (admin/deploy/readonly = 200, anon = 401)
// =====================================================================

/// Drive a single GET against all four role clients with the expected
/// READ_ONLY shape (admin/deploy/readonly = 200, anon = 401). Caller
/// supplies only the path.
async fn assert_read_only_get(path_template: &str) {
    let cp = harness::spawn_cp().await;
    let path = path_template.replace("{base}", &cp.base);
    let (admin, deploy, readonly, anon) = four_clients();
    assert_role_responses(vec![
        ("admin GET", admin.get(&path), 200),
        ("deploy GET", deploy.get(&path), 200),
        ("readonly GET", readonly.get(&path), 200),
        ("anon GET", anon.get(&path), 401),
    ])
    .await;
}

#[tokio::test]
async fn matrix_get_machines() {
    assert_read_only_get("{base}/api/v1/machines").await;
}

#[tokio::test]
async fn matrix_get_rollouts() {
    assert_read_only_get("{base}/api/v1/rollouts").await;
}

#[tokio::test]
async fn matrix_get_releases() {
    assert_read_only_get("{base}/api/v1/releases").await;
}

#[tokio::test]
async fn matrix_get_audit() {
    assert_read_only_get("{base}/api/v1/audit").await;
}

#[tokio::test]
async fn matrix_get_audit_export() {
    assert_read_only_get("{base}/api/v1/audit/export").await;
}

// =====================================================================
// DEPLOY_OR_ADMIN routes (readonly = 403, anon = 401)
// =====================================================================
//
// POST /rollouts has two success cases (admin + deploy both 201) and
// needs per-role (machine, tag, release) triples so the already-active
// rollout 409 doesn't mask the auth check.

#[tokio::test]
async fn matrix_post_rollouts_role_check() {
    let cp = harness::spawn_cp().await;

    for (id, tag) in [
        ("ar-admin", "tag-admin"),
        ("ar-deploy", "tag-deploy"),
        ("ar-readonly", "tag-readonly"),
        ("ar-anon", "tag-anon"),
    ] {
        harness::register_machine(&cp, id, &[tag]).await;
    }
    let r_admin = harness::create_release(&cp, &[("ar-admin", "/nix/store/ra")]).await;
    let r_deploy = harness::create_release(&cp, &[("ar-deploy", "/nix/store/rd")]).await;
    let r_readonly = harness::create_release(&cp, &[("ar-readonly", "/nix/store/rr")]).await;
    let r_anon = harness::create_release(&cp, &[("ar-anon", "/nix/store/rn")]).await;

    let mk_body = |release_id: &str, tag: &str| {
        json!({
            "release_id": release_id,
            "cache_url": null,
            "strategy": "all_at_once",
            "batch_sizes": null,
            "failure_threshold": "0",
            "on_failure": "pause",
            "health_timeout": 60,
            "target": {"tags": [tag]},
            "policy": null
        })
    };

    let url = format!("{}/api/v1/rollouts", cp.base);
    let (admin, deploy, readonly, anon) = four_clients();
    assert_role_responses(vec![
        (
            "admin POST /rollouts",
            admin.post(&url).json(&mk_body(&r_admin, "tag-admin")),
            201,
        ),
        (
            "deploy POST /rollouts",
            deploy.post(&url).json(&mk_body(&r_deploy, "tag-deploy")),
            201,
        ),
        (
            "readonly POST /rollouts",
            readonly
                .post(&url)
                .json(&mk_body(&r_readonly, "tag-readonly")),
            403,
        ),
        (
            "anon POST /rollouts",
            anon.post(&url).json(&mk_body(&r_anon, "tag-anon")),
            401,
        ),
    ])
    .await;
}

// =====================================================================
// ADMIN_ONLY routes (admin = 2xx, deploy/readonly = 403, anon = 401)
// =====================================================================

#[tokio::test]
async fn matrix_post_machines_register_admin_only() {
    // Each role hits a different machine id so the admin success case
    // doesn't poison subsequent 403/401 assertions with a 409 conflict.
    let cp = harness::spawn_cp().await;
    let body = json!({"tags": []});
    let (admin, deploy, readonly, anon) = four_clients();
    let mk_url = |id: &str| format!("{}/api/v1/machines/{id}/register", cp.base);
    assert_role_responses(vec![
        ("admin", admin.post(mk_url("m-admin")).json(&body), 201),
        ("deploy", deploy.post(mk_url("m-deploy")).json(&body), 403),
        ("readonly", readonly.post(mk_url("m-readonly")).json(&body), 403),
        ("anon", anon.post(mk_url("m-anon")).json(&body), 401),
    ])
    .await;
}

#[tokio::test]
async fn matrix_patch_machine_lifecycle_admin_only() {
    let cp = harness::spawn_cp().await;
    // Pre-register 4 machines as admin so each role's request hits an
    // existing machine (exercises the auth check, not 404).
    let admin = client_with_key(TEST_API_KEY);
    for id in ["lc-admin", "lc-deploy", "lc-readonly", "lc-anon"] {
        admin
            .post(format!("{}/api/v1/machines/{id}/register", cp.base))
            .json(&json!({"tags": []}))
            .send()
            .await
            .unwrap();
    }

    let body = json!({"lifecycle": "decommissioned"});
    let (admin, deploy, readonly, anon) = four_clients();
    let mk_url = |id: &str| format!("{}/api/v1/machines/{id}/lifecycle", cp.base);
    assert_role_responses(vec![
        ("admin", admin.patch(mk_url("lc-admin")).json(&body), 200),
        ("deploy", deploy.patch(mk_url("lc-deploy")).json(&body), 403),
        (
            "readonly",
            readonly.patch(mk_url("lc-readonly")).json(&body),
            403,
        ),
        ("anon", anon.patch(mk_url("lc-anon")).json(&body), 401),
    ])
    .await;
}

#[tokio::test]
async fn matrix_delete_release_admin_only() {
    let cp = harness::spawn_cp().await;

    // Pre-create 4 releases (one per role) so the admin success deletes
    // a different row than the 403/401 attempts touch.
    let r_admin = harness::create_release(&cp, &[("web-01", "/nix/store/del-admin")]).await;
    let r_deploy = harness::create_release(&cp, &[("web-01", "/nix/store/del-deploy")]).await;
    let r_readonly = harness::create_release(&cp, &[("web-01", "/nix/store/del-readonly")]).await;
    let r_anon = harness::create_release(&cp, &[("web-01", "/nix/store/del-anon")]).await;

    let (admin, deploy, readonly, anon) = four_clients();
    let mk_url = |id: &str| format!("{}/api/v1/releases/{id}", cp.base);
    assert_role_responses(vec![
        ("admin", admin.delete(mk_url(&r_admin)), 204),
        ("deploy", deploy.delete(mk_url(&r_deploy)), 403),
        ("readonly", readonly.delete(mk_url(&r_readonly)), 403),
        ("anon", anon.delete(mk_url(&r_anon)), 401),
    ])
    .await;
}

// =====================================================================
// Bearer token error paths
// =====================================================================
//
// Public routes (/health, /metrics) are covered by
// `route_coverage_misc.rs::health_returns_ok_without_auth` and
// `metrics_route_returns_200_without_auth`. No need to re-test them
// here through a 4-role fan-out — the auth layer is not on those
// routes at all.

#[tokio::test]
async fn invalid_bearer_token_returns_401() {
    let cp = harness::spawn_cp().await;
    let bad = client_with_key("nfk-not-a-real-key-zzz");
    let resp = bad
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "invalid bearer must surface as 401, not 403"
    );
}

#[tokio::test]
async fn missing_bearer_prefix_returns_401() {
    let cp = harness::spawn_cp().await;
    // Custom request without the "Bearer " prefix.
    let resp = reqwest::Client::new()
        .get(format!("{}/api/v1/machines", cp.base))
        .header("authorization", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}
