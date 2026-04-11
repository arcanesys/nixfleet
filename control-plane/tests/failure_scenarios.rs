//! F4, F5, F-stale-resume, F-paused-cancel — failure-path scenarios.
//!
//! All of the scenarios in this file verify what the executor does
//! when a deploy is unhealthy: generation gate filtering, threshold
//! breaches, stale-report regression guards, and operator cancel
//! from Paused. Happy-path transitions (Created → Running → Completed,
//! Running → Cancelled) live in deploy_scenarios.rs and
//! route_coverage.rs.

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
    let (cp, _release_id, rollout_id) =
        harness::spawn_cp_with_rollout("/nix/store/f4-correct").await;

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
    cp.db.insert_health_report("web-01", "{}", true).unwrap();

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

    // Stage 2: agent re-applies and reports the correct store path.
    harness::agent_reports_health(&cp, "web-01", "/nix/store/f4-correct", true).await;

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

    // 6 healthy, 4 unhealthy. Threshold = ceil(10 * 0.30) = 3. 4 > 3 → fail.
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

/// Resume must not re-flip a batch to `failed` on a stale pre-resume report.
///
/// Sequence:
///   1. Rollout reaches `paused` because the agent reported unhealthy on
///      the desired generation.
///   2. Operator clears the underlying problem and POSTs `/resume`.
///   3. The executor's next tick reads `recent_reports[0]` and must
///      NOT flip the batch back to `failed` from the stale
///      pre-resume unhealthy report — that would defeat the resume
///      before the agent could send a fresh healthy report.
///
/// The `received_at < started_at` filter in
/// `executor.rs::evaluate_batch::on_desired_gen` treats a stale report
/// as pending instead of unhealthy. This test pins that filter.
///
/// Test design notes:
///   * `on_desired_gen=true` is what triggers the fallback branch we
///     care about, so the agent must report on the matching gen path.
///   * `update_batch_status(deploying)` resets `started_at`, so any
///     report inserted BEFORE resume has `received_at < started_at`
///     post-resume. The harness's `fake_agent_report` writes
///     `received_at = datetime('now')` so the inserted report's
///     timestamp is genuinely earlier than the resume tick's
///     `started_at`.
#[tokio::test]
async fn f_stale_resume_does_not_reflip_on_pre_resume_report() {
    let (cp, _release_id, rollout_id) =
        harness::spawn_cp_with_rollout("/nix/store/fstale-web-01").await;

    // Tick once: pending → deploying.
    harness::tick_once(&cp).await;

    // Stage 1: agent reports failure on the desired gen → batch fails →
    // rollout pauses (on_failure=Pause).
    harness::agent_reports_health(&cp, "web-01", "/nix/store/fstale-web-01", false).await;
    harness::tick_once(&cp).await;

    let _ = harness::wait_rollout_status(
        &cp,
        &rollout_id,
        RolloutStatus::Paused,
        std::time::Duration::from_secs(2),
    )
    .await;

    // Stage 2: backdate the existing reports so they are guaranteed to
    // be older than the post-resume started_at. SQLite's
    // datetime('now') is second-precision, so without a backdate the
    // failure report and the resume tick may share the same wall-clock
    // second and the stale-filter check `received_at < started_at`
    // would not exercise the filter branch at all.
    //
    // We open a fresh rusqlite connection on the harness's db_path
    // (no public accessor on Db; this is the integration-test escape
    // hatch). Backdating by 10 seconds is far more than needed and
    // gives a comfortable margin for slow CI.
    {
        let conn = rusqlite::Connection::open(&cp.db_path).unwrap();
        conn.execute(
            "UPDATE reports SET received_at = datetime('now', '-10 seconds') \
             WHERE machine_id = 'web-01'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE health_reports SET received_at = datetime('now', '-10 seconds') \
             WHERE machine_id = 'web-01'",
            [],
        )
        .unwrap();
    }

    // Stage 3: operator resumes via the HTTP API. This calls
    // update_batch_status(deploying) which resets the batch's started_at
    // to NOW, making the (now-backdated) failure report stale relative
    // to the new batch evaluation window.
    let resume = cp
        .admin
        .post(format!(
            "{}/api/v1/rollouts/{}/resume",
            cp.base, rollout_id
        ))
        .send()
        .await
        .unwrap();
    assert!(
        resume.status().is_success(),
        "resume must succeed; got {}",
        resume.status()
    );

    // Two ticks WITHOUT inserting a fresh report:
    //   tick A: process_rollout finds the batch in "pending" state
    //           (resume_rollout reset failed → pending) and calls
    //           deploy_batch, which sets started_at=NOW and the batch
    //           to "deploying". No evaluation happens this tick.
    //   tick B: process_rollout finds the batch in "deploying" and
    //           calls evaluate_batch. The (backdated) unhealthy report
    //           must be recognised as stale (received_at < started_at)
    //           and treated as pending, so the batch transitions to
    //           "waiting_health" and the rollout stays Running. Without
    //           the filter, the backdated report would flip the batch
    //           straight back to failed.
    harness::tick_once(&cp).await;
    harness::tick_once(&cp).await;

    let detail: nixfleet_types::rollout::RolloutDetail = serde_json::from_str(
        &cp.admin
            .get(format!("{}/api/v1/rollouts/{}", cp.base, rollout_id))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        !matches!(detail.status, RolloutStatus::Paused),
        "rollout must NOT re-pause on stale pre-resume report; got {:?}",
        detail.status
    );

    // Stage 3: now insert a fresh healthy report and tick again. The
    // batch should reach `succeeded` and the rollout should complete.
    harness::agent_reports_health(&cp, "web-01", "/nix/store/fstale-web-01", true).await;
    // Two ticks: first to flip the batch to `succeeded`, second to
    // promote the rollout to `completed`.
    harness::tick_once(&cp).await;
    harness::tick_once(&cp).await;

    let _ = harness::wait_rollout_status(
        &cp,
        &rollout_id,
        RolloutStatus::Completed,
        std::time::Duration::from_secs(2),
    )
    .await;
}

/// F-paused-cancel — operator can cancel a paused rollout instead of resuming.
///
/// The Running → Cancelled path is already covered by route_coverage.rs
/// (`rollouts_cancel_running_succeeds_and_state_changes`). This test
/// pins the Paused → Cancelled branch, which only route_coverage's
/// `rollouts_cancel_already_cancelled_returns_409` touches tangentially
/// (and only for the second call). Every other executor state
/// transition (Created → Running, Running → Completed, Cancelled
/// terminal) is covered by deploy_scenarios.rs + route_coverage.rs,
/// so no dedicated executor_transition file is needed.
#[tokio::test]
async fn f_paused_cancel_transitions_to_cancelled() {
    let (cp, _, rollout_id) =
        harness::spawn_cp_with_rollout("/nix/store/p2c-web-01").await;

    // Pause via failure: insert an unhealthy report on the desired gen.
    harness::tick_once(&cp).await;
    harness::agent_reports_health(&cp, "web-01", "/nix/store/p2c-web-01", false).await;
    harness::tick_once(&cp).await;
    let _ = harness::wait_rollout_status(
        &cp,
        &rollout_id,
        RolloutStatus::Paused,
        std::time::Duration::from_secs(2),
    )
    .await;

    // Now cancel the paused rollout.
    harness::assert_status(
        cp.admin
            .post(format!("{}/api/v1/rollouts/{}/cancel", cp.base, rollout_id)),
        200,
    )
    .await;

    let detail = harness::wait_rollout_status(
        &cp,
        &rollout_id,
        RolloutStatus::Cancelled,
        std::time::Duration::from_secs(2),
    )
    .await;
    assert_eq!(detail.status, RolloutStatus::Cancelled);
}
