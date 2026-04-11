//! AU1, AU2 — audit logging and CSV export.

#[path = "harness.rs"]
mod harness;

use nixfleet_types::rollout::{OnFailure, RolloutStrategy};

/// AU1 — every mutation writes an audit event.
#[tokio::test]
async fn au1_mutations_write_audit_events() {
    let cp = harness::spawn_cp().await;

    // Baseline count.
    let events_before = cp.db.query_audit_events(None, None, None, 1000).unwrap();
    let before_count = events_before.len();

    // Mutation 1: register_machine via HTTP route so the real audit write
    // happens in the handler.
    let resp = cp
        .admin
        .post(format!("{}/api/v1/machines/reg-01/register", cp.base))
        .json(&serde_json::json!({"tags": ["web"]}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Mutation 2: post_report (auto-registers + audit).
    harness::fake_agent_report(&cp, "rep-01", "/nix/store/au1-rep", true, "", &["web"]).await;

    // Mutation 3: update_lifecycle.
    let resp = cp
        .admin
        .patch(format!("{}/api/v1/machines/reg-01/lifecycle", cp.base))
        .json(&serde_json::json!({"lifecycle": "decommissioned"}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Mutation 4: create_release (this release is NOT referenced by any
    // rollout, so it remains deletable for mutation 7).
    let release_id = harness::create_release(&cp, &[("reg-01", "/nix/store/au1-reg-01")]).await;

    // Mutation 5: create_rollout — uses a second release so we can still
    // delete the first one in mutation 7.
    harness::register_machine(&cp, "web-01", &["web"]).await;
    let rel2 = harness::create_release(&cp, &[("web-01", "/nix/store/au1-web-01")]).await;
    let rollout_id = harness::create_rollout_for_tag(
        &cp,
        &rel2,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    // Mutation 6: cancel_rollout.
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, rollout_id))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Mutation 7: delete_release (orphan release, never referenced by a rollout).
    let resp = cp
        .admin
        .delete(format!("{}/api/v1/releases/{}", cp.base, release_id))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "delete_release failed: HTTP {} — body: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    // Collect all events written since the start.
    let events_after = cp.db.query_audit_events(None, None, None, 1000).unwrap();
    let after_count = events_after.len();
    assert!(
        after_count - before_count >= 7,
        "expected at least 7 new audit events, got {}",
        after_count - before_count
    );

    // Positive: every expected action appears at least once.
    let actions: std::collections::HashSet<String> =
        events_after.iter().map(|e| e.action.clone()).collect();
    for want in [
        "register",
        "report",
        "update_lifecycle",
        "create_release",
        "rollout.created",
        "rollout.cancelled",
        "delete_release",
    ] {
        assert!(actions.contains(want), "audit missing action '{want}'");
    }

    // Negative: no 'set_tags' action (handler does not exist —
    // tags are synced implicitly via the report handler).
    assert!(
        !actions.contains("set_tags"),
        "set_tags action must not appear — tags sync via the report handler, not a dedicated route"
    );
}

/// AU2 — CSV export returns rows, escaping is in effect.
#[tokio::test]
async fn au2_csv_export_happy_path_and_injection_safe() {
    let cp = harness::spawn_cp().await;

    // Write an audit event whose detail contains a CSV-injection payload.
    cp.db
        .insert_audit_event(
            "tester",
            "test.injection",
            "target-1",
            Some("=HYPERLINK(\"http://evil\")"),
        )
        .unwrap();

    let resp = cp
        .admin
        .get(format!("{}/api/v1/audit/export", cp.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let csv = resp.text().await.unwrap();

    assert!(csv.contains("test.injection"));

    // Negative: the raw `=HYPERLINK(...)` MUST NOT appear as a bare leading
    // `=` field — it must be escaped (prefixed with `'` or wrapped in quotes
    // per `escape_csv_field`). The specific escape strategy depends on the
    // implementation; this assertion catches the unescaped-leading-equals
    // case.
    for line in csv.lines() {
        for field in line.split(',') {
            assert!(
                !field.trim().starts_with("=HYPERLINK"),
                "CSV must not contain a bare '=HYPERLINK' field (injection risk)"
            );
        }
    }
}
