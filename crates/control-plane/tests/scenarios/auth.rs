//! A1, A2, A4 - authentication and RBAC scenarios.
//!
//! A1 exercises the first-admin bootstrap endpoint: a freshly-wiped DB lets
//! one anonymous `POST /api/v1/keys/bootstrap` succeed with 200, a second
//! call returns 409, and no second key is ever inserted.
//!
//! A2 asserts that an admin endpoint (`/api/v1/machines`) rejects anonymous
//! callers with 401, while the public `/health` endpoint stays reachable
//! without credentials.
//!
//! A4 exercises role enforcement on `POST /api/v1/rollouts`: the seeded
//! `readonly` key is forbidden (403), the `deploy` key succeeds (201), and
//! readonly can still `GET /api/v1/releases` (200) and `GET /api/v1/rollouts`
//! (200).
//!
//! A5 pins the bearer-token shape errors: an unrecognised token surfaces as
//! 401 (not 403), and an Authorization header without the `Bearer ` prefix
//! also surfaces as 401. These are the gaps route_coverage and A1–A4 leave.

use super::harness;

use nixfleet_types::rollout::{CreateRolloutRequest, OnFailure, RolloutStrategy, RolloutTarget};
use rusqlite::Connection;
use serde_json::Value;

#[tokio::test]
async fn a1_bootstrap_first_admin_then_409() {
    let cp = harness::spawn_cp().await;

    // The harness seeds three API keys. Wipe them so the bootstrap path
    // actually sees an empty table.
    {
        let conn = Connection::open(&cp.db_path).expect("open db for wipe");
        conn.execute("DELETE FROM api_keys", [])
            .expect("wipe api_keys");
    }

    let anon = harness::client_anonymous();

    // First bootstrap: must succeed anonymously.
    let resp = anon
        .post(format!("{}/api/v1/keys/bootstrap", cp.base))
        .json(&serde_json::json!({ "name": "first-admin" }))
        .send()
        .await
        .expect("bootstrap 1 request");
    assert_eq!(resp.status(), 200, "first bootstrap must return 200");
    let body: Value = resp.json().await.expect("bootstrap 1 body");
    let key = body
        .get("key")
        .and_then(Value::as_str)
        .expect("response has key field");
    assert!(
        key.starts_with("nfk-"),
        "bootstrap key must start with nfk-, got {key}"
    );
    assert_eq!(
        body.get("name").and_then(Value::as_str),
        Some("first-admin"),
        "bootstrap response must echo requested name"
    );
    assert_eq!(
        body.get("role").and_then(Value::as_str),
        Some("admin"),
        "bootstrap response must grant admin role"
    );

    // Second bootstrap: must 409.
    let resp = anon
        .post(format!("{}/api/v1/keys/bootstrap", cp.base))
        .json(&serde_json::json!({ "name": "second-admin" }))
        .send()
        .await
        .expect("bootstrap 2 request");
    assert_eq!(
        resp.status(),
        409,
        "second bootstrap must return 409 Conflict"
    );

    // Negative: the second bootstrap must not have created a key.
    let conn = Connection::open(&cp.db_path).expect("open db for count");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM api_keys", [], |row| row.get(0))
        .expect("count api_keys");
    assert_eq!(
        count, 1,
        "bootstrap must be idempotent: exactly 1 key must exist after two calls"
    );
}

#[tokio::test]
async fn a2_admin_endpoint_requires_auth_health_stays_public() {
    let cp = harness::spawn_cp().await;
    let anon = harness::client_anonymous();

    // Anonymous admin endpoint: must 401.
    let resp = anon
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .expect("machines request");
    assert_eq!(
        resp.status(),
        401,
        "anonymous GET /api/v1/machines must return 401 Unauthorized"
    );

    // Negative: /health must remain public (no auth required).
    let resp = anon
        .get(format!("{}/health", cp.base))
        .send()
        .await
        .expect("health request");
    assert_eq!(
        resp.status(),
        200,
        "anonymous GET /health must stay reachable (200)"
    );
}

#[tokio::test]
async fn a4_role_enforcement_on_rollout_creation() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let release_id = harness::create_release(&cp, &[("web-01", "/nix/store/aaaa-web01")]).await;

    let readonly = harness::client_with_key(harness::TEST_READONLY_KEY);
    let deploy = harness::client_with_key(harness::TEST_DEPLOY_KEY);

    let body = CreateRolloutRequest {
        release_id: release_id.clone(),
        cache_url: None,
        strategy: RolloutStrategy::AllAtOnce,
        batch_sizes: None,
        failure_threshold: "0".to_string(),
        on_failure: OnFailure::Pause,
        health_timeout: Some(60),
        target: RolloutTarget::Tags(vec!["web".to_string()]),
    };

    // readonly must be forbidden.
    let resp = readonly
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&body)
        .send()
        .await
        .expect("readonly POST /rollouts request");
    assert_eq!(
        resp.status(),
        403,
        "readonly key must be forbidden from POST /api/v1/rollouts"
    );

    // deploy must succeed.
    let resp = deploy
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&body)
        .send()
        .await
        .expect("deploy POST /rollouts request");
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(
        status, 201,
        "deploy key must create rollout: HTTP {status}, body={text}"
    );

    // Negative: readonly can still read releases and rollouts. These
    // read routes are gated on READ_ONLY (any authenticated role passes);
    // the distinct assertion here vs. route_coverage's per-route tests
    // is that the same seeded readonly key traverses both read and
    // write paths in one scenario.
    for path in ["/api/v1/releases", "/api/v1/rollouts"] {
        let resp = readonly
            .get(format!("{}{path}", cp.base))
            .send()
            .await
            .expect("readonly GET request");
        assert_eq!(
            resp.status(),
            200,
            "readonly key must still be able to GET {path}"
        );
    }
}

/// A5 - bearer token shape errors.
///
/// Unrecognised tokens and missing `Bearer ` prefix must surface as 401,
/// not 403. 401 means "no identity was established"; 403 means "identity
/// is known but lacks the role". Returning 403 here would leak the
/// existence of the route to unauthenticated callers.
#[tokio::test]
async fn a5_invalid_bearer_token_returns_401() {
    let cp = harness::spawn_cp().await;
    let bad = harness::client_with_key("nfk-not-a-real-key-zzz");
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
async fn a5_missing_bearer_prefix_returns_401() {
    let cp = harness::spawn_cp().await;
    // Custom request without the "Bearer " prefix - the auth layer
    // must treat this as unauthenticated rather than trying to parse
    // the raw token.
    let resp = reqwest::Client::new()
        .get(format!("{}/api/v1/machines", cp.base))
        .header("authorization", harness::TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}
