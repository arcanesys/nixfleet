//! M1, M2 — machine state and sync.
//!
//! M1 exercises the lifecycle transition (`active` → `decommissioned`) via
//! `PATCH /api/v1/machines/{id}/lifecycle` and asserts that a decommissioned
//! machine is removed from the rollout target set returned by
//! `get_machines_by_tags`.
//!
//! M2 exercises the auto-registration path: an unknown agent posts a health
//! report with tags, and the CP must persist the machine + its tags so that
//! `get_machines_by_tags` surfaces it immediately.

#[path = "harness.rs"]
mod harness;

#[tokio::test]
async fn m1_lifecycle_register_active_decommissioned() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;

    // Sanity: freshly registered machine is in the rollout target set for "web".
    let initial = cp.db.get_machines_by_tags(&["web".to_string()]).unwrap();
    assert!(
        initial.contains(&"web-01".to_string()),
        "active web-01 must appear in get_machines_by_tags('web') before decommission"
    );

    // Move to decommissioned via PATCH.
    let resp = cp
        .admin
        .patch(format!("{}/api/v1/machines/web-01/lifecycle", cp.base))
        .json(&serde_json::json!({"lifecycle": "decommissioned"}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "PATCH lifecycle failed: {} {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    let machines = cp.db.list_machines().unwrap();
    let web = machines
        .iter()
        .find(|m| m.machine_id == "web-01")
        .expect("web-01 must still exist after decommission");
    assert_eq!(web.lifecycle, "decommissioned");

    // Negative: `get_machines_by_tags` must NOT return a decommissioned machine
    // when used as a rollout target. If the DB query returns it, any rollout
    // built for `tag=web` would accidentally target a decommissioned host.
    let targets = cp.db.get_machines_by_tags(&["web".to_string()]).unwrap();
    assert!(
        !targets.contains(&"web-01".to_string()),
        "decommissioned machine must NOT appear in get_machines_by_tags('web'); got {targets:?}"
    );
}

#[tokio::test]
async fn m2_tag_sync_from_report_enables_tag_filter() {
    let cp = harness::spawn_cp().await;

    // Machine not pre-registered — post_report should auto-register it and
    // persist its tags.
    harness::fake_agent_report(
        &cp,
        "auto-01",
        "/nix/store/m2-auto-01",
        true,
        "ok",
        &["auto", "canary"],
    )
    .await;

    let tags = cp.db.get_machine_tags("auto-01").unwrap();
    assert!(
        tags.contains(&"auto".to_string()),
        "reported tag 'auto' must be persisted; got {tags:?}"
    );
    assert!(
        tags.contains(&"canary".to_string()),
        "reported tag 'canary' must be persisted; got {tags:?}"
    );

    // `get_machines_by_tags(["auto"])` must surface this machine.
    let found = cp.db.get_machines_by_tags(&["auto".to_string()]).unwrap();
    assert!(
        found.contains(&"auto-01".to_string()),
        "auto-01 must be discoverable via tag 'auto'; got {found:?}"
    );

    // Negative: filtering by a tag the agent did NOT report must return empty.
    let empty = cp
        .db
        .get_machines_by_tags(&["nonexistent".to_string()])
        .unwrap();
    assert!(
        empty.is_empty(),
        "unknown tag must return empty set; got {empty:?}"
    );
}
