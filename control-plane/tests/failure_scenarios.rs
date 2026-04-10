//! F4, F5 — failure-path scenarios.

#[path = "harness.rs"]
mod harness;

use nixfleet_types::rollout::{OnFailure, RolloutStatus, RolloutStrategy};

/// F4 — Generation gate: report with wrong generation does not count as healthy.
///
/// The `evaluate_batch` function in executor.rs reads the latest report and
/// compares `report.generation` to the release entry's `store_path`. If they
/// differ, the machine is treated as pending — not healthy, regardless of
/// the report's `success` flag. This test drives that branch directly.
#[tokio::test]
async fn f4_generation_mismatch_counted_as_pending_then_accepted() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;

    let release_id =
        harness::create_release(&cp, &[("web-01", "/nix/store/f4-correct")]).await;
    let rollout_id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "1",
        OnFailure::Pause,
        60,
    )
    .await;

    harness::tick_once(&cp).await; // pending → deploying

    // Stage 1: agent reports success=true BUT on the WRONG generation.
    harness::fake_agent_report(
        &cp,
        "web-01",
        "/nix/store/f4-stale", // wrong store path
        true,
        "applied wrong gen",
        &["web"],
    )
    .await;
    cp.db
        .insert_health_report("web-01", "{}", true)
        .unwrap();

    harness::tick_once(&cp).await;

    // Rollout must NOT have completed — the report was filtered out by
    // the generation gate (on_desired_gen = false → pending branch).
    let batches = cp.db.get_rollout_batches(&rollout_id).unwrap();
    let b = &batches[0];
    assert_ne!(
        b.status, "succeeded",
        "stale-generation report must NOT mark batch succeeded"
    );
    assert_ne!(
        b.status, "failed",
        "success=true on stale gen must NOT mark batch failed either"
    );

    // Wait for the next SQLite `datetime('now')` tick so Stage 2 reports
    // have a strictly larger `received_at` than Stage 1 — avoids tie-break
    // ambiguity in `ORDER BY received_at DESC LIMIT 1`.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    // Stage 2: agent re-applies and reports the correct store path.
    harness::fake_agent_report(
        &cp,
        "web-01",
        "/nix/store/f4-correct",
        true,
        "applied correct gen",
        &["web"],
    )
    .await;
    cp.db
        .insert_health_report("web-01", "{}", true)
        .unwrap();

    harness::tick_once(&cp).await; // → succeeded
    harness::tick_once(&cp).await; // → completed

    let detail = harness::wait_rollout_status(
        &cp,
        &rollout_id,
        RolloutStatus::Completed,
        std::time::Duration::from_secs(2),
    )
    .await;
    assert!(matches!(detail.status, RolloutStatus::Completed));
}

/// F5 — 10 machines, 30% threshold → 4 fail → rollout paused.
#[tokio::test]
async fn f5_failure_threshold_30_percent_pauses_on_4_of_10() {
    let cp = harness::spawn_cp().await;

    let mut entries = Vec::new();
    for i in 1..=10 {
        let id = format!("node-{i:02}");
        harness::register_machine(&cp, &id, &["bulk"]).await;
        entries.push((id, format!("/nix/store/f5-node-{i:02}")));
    }
    let ref_entries: Vec<(&str, &str)> = entries
        .iter()
        .map(|(h, p)| (h.as_str(), p.as_str()))
        .collect();
    let release_id = harness::create_release(&cp, &ref_entries).await;

    let rollout_id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "bulk",
        RolloutStrategy::AllAtOnce,
        None,
        "30%",
        OnFailure::Pause,
        60,
    )
    .await;

    harness::tick_once(&cp).await; // → deploying

    // 6 healthy, 4 unhealthy. Threshold = ceil(10 * 0.30) = 3. 4 >= 3 → fail.
    for i in 1..=6 {
        let id = format!("node-{i:02}");
        let path = format!("/nix/store/f5-node-{i:02}");
        harness::fake_agent_report(&cp, &id, &path, true, "ok", &["bulk"]).await;
        cp.db.insert_health_report(&id, "{}", true).unwrap();
    }
    for i in 7..=10 {
        let id = format!("node-{i:02}");
        let path = format!("/nix/store/f5-node-{i:02}");
        harness::fake_agent_report(&cp, &id, &path, false, "boom", &["bulk"]).await;
        cp.db
            .insert_health_report(&id, "{\"fail\":true}", false)
            .unwrap();
    }

    harness::tick_once(&cp).await;

    let detail = harness::wait_rollout_status(
        &cp,
        &rollout_id,
        RolloutStatus::Paused,
        std::time::Duration::from_secs(2),
    )
    .await;
    assert!(matches!(detail.status, RolloutStatus::Paused));

    // Negative: the rollout must not have reached Completed despite 6 healthy.
    assert!(
        !matches!(detail.status, RolloutStatus::Completed),
        "rollout must not complete when failure threshold was breached"
    );

    // An event must record the pause reason.
    let events = cp.db.get_rollout_events(&rollout_id).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "status_change" && e.detail.contains("paused")),
        "rollout must have a status_change event recording the pause"
    );
}
