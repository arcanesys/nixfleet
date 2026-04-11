//! Phase 4 § 5 #8 — auth role × endpoint matrix.
//!
//! Builds a comprehensive matrix asserting that every admin route
//! returns the right HTTP status for every role tier (admin, deploy,
//! readonly, anonymous). Role gates are scattered across handlers; this
//! file is the single place that exercises every cell.
//!
//! Pre-existing coverage:
//!   - `Actor::has_role` unit matrix in `control-plane/src/auth.rs`
//!   - A1 / A2 / A4 in `auth_scenarios.rs` (anonymous → 401, role
//!     enforcement on POST /rollouts)
//!   - Task 13/14 + cn_validation_scenarios.rs for the agent-route
//!     mTLS CN validation layer
//!
//! What this file adds: explicit per-route matrix entries for every
//! ROUTE × ROLE pair the route_coverage_*.rs files don't already
//! exhaustively cover.

#[path = "harness.rs"]
mod harness;

use harness::{client_anonymous, client_with_key, TEST_API_KEY, TEST_DEPLOY_KEY, TEST_READONLY_KEY};
use reqwest::Method;
use serde_json::json;

/// Status sets — represent the expected response per role for a route.
#[derive(Debug, Clone, Copy)]
struct ExpectedAuth {
    admin: u16,
    deploy: u16,
    readonly: u16,
    anonymous: u16,
}

/// Read-only routes accessible to admin/deploy/readonly, 401 for anon.
/// This is the only shape driven through `run_matrix_row` below. The
/// deploy-or-admin and admin-only shapes are exercised by dedicated
/// tests further down in this file because those routes require
/// state setup (release_id / rollout_id) that doesn't fit the
/// generic helper.
const READ_ONLY: ExpectedAuth = ExpectedAuth {
    admin: 200,
    deploy: 200,
    readonly: 200,
    anonymous: 401,
};

/// Run a single (method, path, body, expected) row against all four
/// role clients on a fresh CP. Each row is its own test so failures
/// are easy to attribute.
async fn run_matrix_row(
    method: Method,
    path_template: &str,
    body: Option<serde_json::Value>,
    expected: ExpectedAuth,
    seeded_machine: bool,
) {
    let cp = harness::spawn_cp().await;
    if seeded_machine {
        // Some routes need an existing machine to return 200 instead
        // of a 4xx that would mask the auth check. Pre-seed via the
        // HTTP register endpoint so fleet state has matching tags.
        cp.admin
            .post(format!("{}/api/v1/machines/web-01/register", cp.base))
            .json(&json!({"tags": ["web"]}))
            .send()
            .await
            .unwrap();
    }

    let path = path_template.replace("{base}", &cp.base);

    let admin = client_with_key(TEST_API_KEY);
    let deploy = client_with_key(TEST_DEPLOY_KEY);
    let readonly = client_with_key(TEST_READONLY_KEY);
    let anon = client_anonymous();

    for (label, client, expected_status) in [
        ("admin", &admin, expected.admin),
        ("deploy", &deploy, expected.deploy),
        ("readonly", &readonly, expected.readonly),
        ("anon", &anon, expected.anonymous),
    ] {
        let mut req = client.request(method.clone(), &path);
        if let Some(ref b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.unwrap();
        assert_eq!(
            resp.status().as_u16(),
            expected_status,
            "{label} {method} {path}: expected {expected_status}, got {}",
            resp.status().as_u16()
        );
    }
}

// =====================================================================
// READ_ONLY routes (admin/deploy/readonly = 200, anon = 401)
// =====================================================================

#[tokio::test]
async fn matrix_get_machines() {
    run_matrix_row(
        Method::GET,
        "{base}/api/v1/machines",
        None,
        READ_ONLY,
        false,
    )
    .await;
}

#[tokio::test]
async fn matrix_get_rollouts() {
    run_matrix_row(
        Method::GET,
        "{base}/api/v1/rollouts",
        None,
        READ_ONLY,
        false,
    )
    .await;
}

#[tokio::test]
async fn matrix_get_releases() {
    run_matrix_row(
        Method::GET,
        "{base}/api/v1/releases",
        None,
        READ_ONLY,
        false,
    )
    .await;
}

#[tokio::test]
async fn matrix_get_audit() {
    run_matrix_row(
        Method::GET,
        "{base}/api/v1/audit",
        None,
        READ_ONLY,
        false,
    )
    .await;
}

#[tokio::test]
async fn matrix_get_audit_export() {
    run_matrix_row(
        Method::GET,
        "{base}/api/v1/audit/export",
        None,
        READ_ONLY,
        false,
    )
    .await;
}

// =====================================================================
// DEPLOY_OR_ADMIN routes (readonly = 403, anon = 401)
// =====================================================================
//
// POST /rollouts and the resume/cancel routes are deploy-or-admin.
// They need a real release_id and rollout_id, so we don't go through
// the run_matrix_row helper. Direct tests below.

#[tokio::test]
async fn matrix_post_rollouts_role_check() {
    // Each role hits a different (machine, tag, release) so the
    // already-active-rollout 409 doesn't conflict with the auth check.
    let cp = harness::spawn_cp().await;

    let admin = client_with_key(TEST_API_KEY);
    let deploy = client_with_key(TEST_DEPLOY_KEY);
    let readonly = client_with_key(TEST_READONLY_KEY);
    let anon = client_anonymous();

    // Pre-register four machines under four distinct tags so each
    // role's rollout has its own scope.
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

    let resp = admin
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&mk_body(&r_admin, "tag-admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "admin must succeed");

    let resp = deploy
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&mk_body(&r_deploy, "tag-deploy"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "deploy must succeed");

    let resp = readonly
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&mk_body(&r_readonly, "tag-readonly"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    let resp = anon
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&mk_body(&r_anon, "tag-anon"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// =====================================================================
// ADMIN_ONLY routes (deploy and readonly = 403, anon = 401)
// =====================================================================
//
// Admin-only routes:
//   - POST   /api/v1/machines/{id}/register
//   - PATCH  /api/v1/machines/{id}/lifecycle
//   - DELETE /api/v1/machines/{id}/tags/{tag}
//   - DELETE /api/v1/releases/{id}

#[tokio::test]
async fn matrix_post_machines_register_admin_only() {
    // Each role hits a different machine ID so the 200 case doesn't
    // 409 on the second attempt.
    let cp = harness::spawn_cp().await;
    let admin = client_with_key(TEST_API_KEY);
    let deploy = client_with_key(TEST_DEPLOY_KEY);
    let readonly = client_with_key(TEST_READONLY_KEY);
    let anon = client_anonymous();

    let body = json!({"tags": []});

    let resp = admin
        .post(format!("{}/api/v1/machines/m-admin/register", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "admin must succeed");

    let resp = deploy
        .post(format!("{}/api/v1/machines/m-deploy/register", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    let resp = readonly
        .post(format!("{}/api/v1/machines/m-readonly/register", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    let resp = anon
        .post(format!("{}/api/v1/machines/m-anon/register", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn matrix_patch_machine_lifecycle_admin_only() {
    let cp = harness::spawn_cp().await;
    // Pre-register all four machines as admin so the role tests
    // exercise the auth check, not 404.
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

    // admin → 200
    let resp = admin
        .patch(format!("{}/api/v1/machines/lc-admin/lifecycle", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // deploy → 403
    let resp = client_with_key(TEST_DEPLOY_KEY)
        .patch(format!("{}/api/v1/machines/lc-deploy/lifecycle", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // readonly → 403
    let resp = client_with_key(TEST_READONLY_KEY)
        .patch(format!("{}/api/v1/machines/lc-readonly/lifecycle", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // anon → 401
    let resp = client_anonymous()
        .patch(format!("{}/api/v1/machines/lc-anon/lifecycle", cp.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn matrix_delete_release_admin_only() {
    let cp = harness::spawn_cp().await;

    // Pre-create four releases (one per role) so the role test
    // exercises the auth check, not 404.
    let r_admin = harness::create_release(&cp, &[("web-01", "/nix/store/del-admin")]).await;
    let r_deploy = harness::create_release(&cp, &[("web-01", "/nix/store/del-deploy")]).await;
    let r_readonly = harness::create_release(&cp, &[("web-01", "/nix/store/del-readonly")]).await;
    let r_anon = harness::create_release(&cp, &[("web-01", "/nix/store/del-anon")]).await;

    let admin = client_with_key(TEST_API_KEY);
    let resp = admin
        .delete(format!("{}/api/v1/releases/{}", cp.base, r_admin))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let resp = client_with_key(TEST_DEPLOY_KEY)
        .delete(format!("{}/api/v1/releases/{}", cp.base, r_deploy))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    let resp = client_with_key(TEST_READONLY_KEY)
        .delete(format!("{}/api/v1/releases/{}", cp.base, r_readonly))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    let resp = client_anonymous()
        .delete(format!("{}/api/v1/releases/{}", cp.base, r_anon))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// =====================================================================
// Public routes (no auth required, all four roles → 200)
// =====================================================================

#[tokio::test]
async fn matrix_get_health_no_auth() {
    let cp = harness::spawn_cp().await;
    for client in [
        client_with_key(TEST_API_KEY),
        client_with_key(TEST_DEPLOY_KEY),
        client_with_key(TEST_READONLY_KEY),
        client_anonymous(),
    ] {
        let resp = client
            .get(format!("{}/health", cp.base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}

#[tokio::test]
async fn matrix_get_metrics_no_auth() {
    let cp = harness::spawn_cp().await;
    for client in [
        client_with_key(TEST_API_KEY),
        client_with_key(TEST_DEPLOY_KEY),
        client_with_key(TEST_READONLY_KEY),
        client_anonymous(),
    ] {
        let resp = client
            .get(format!("{}/metrics", cp.base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}

// =====================================================================
// Invalid bearer token returns 401 (not 403)
// =====================================================================

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
