//! D2, D3 — deploy happy paths (staged + all-at-once).
//!
//! Spec Section 4. Strategy behaviour under real executor ticks.

#[path = "harness.rs"]
mod harness;

use nixfleet_types::rollout::{OnFailure, RolloutStatus, RolloutStrategy};

/// D2 — staged strategy with 3 batches → sequential completion.
#[tokio::test]
async fn d2_staged_strategy_completes_sequentially() {
    let cp = harness::spawn_cp().await;

    // Register 6 machines on the "web" tag, 2 per batch.
    let hosts: Vec<String> = (1..=6).map(|i| format!("web-{i:02}")).collect();
    for h in &hosts {
        harness::register_machine(&cp, h, &["web"]).await;
    }

    let release_entries: Vec<(String, String)> = hosts
        .iter()
        .map(|h| (h.clone(), format!("/nix/store/d2-{h}")))
        .collect();
    let ref_entries: Vec<(&str, &str)> = release_entries
        .iter()
        .map(|(h, p)| (h.as_str(), p.as_str()))
        .collect();
    let release_id = harness::create_release(&cp, &ref_entries).await;

    let rollout_id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "web",
        RolloutStrategy::Staged,
        Some(vec!["2", "2", "2"]),
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    // Drive tick by tick, simulating healthy reports after each deploy.
    for batch_idx in 0..3 {
        harness::tick_once(&cp).await; // pending → deploying

        // Which machines are in this batch?
        let batches = cp.db.get_rollout_batches(&rollout_id).unwrap();
        let current = batches
            .iter()
            .find(|b| b.status == "deploying" || b.status == "waiting_health")
            .unwrap_or_else(|| panic!("no active batch at step {batch_idx}"));
        let machine_ids: Vec<String> = serde_json::from_str(&current.machine_ids).unwrap();
        assert_eq!(
            machine_ids.len(),
            2,
            "batch {batch_idx} must contain exactly 2 machines (staged sizing 2,2,2)"
        );

        // Report each machine healthy on the desired generation.
        for m in &machine_ids {
            let path = format!("/nix/store/d2-{m}");
            harness::agent_reports_health(&cp, m, &path, true).await;
        }

        harness::tick_once(&cp).await; // deploying → succeeded
    }

    // After all 3 batches, one more tick transitions the rollout to completed.
    harness::tick_once(&cp).await;

    let detail = harness::wait_rollout_status(
        &cp,
        &rollout_id,
        RolloutStatus::Completed,
        std::time::Duration::from_secs(2),
    )
    .await;

    assert!(
        matches!(detail.status, RolloutStatus::Completed),
        "rollout must reach Completed"
    );

    // Negative: no batch should be in "failed" status.
    let batches = cp.db.get_rollout_batches(&rollout_id).unwrap();
    for b in &batches {
        assert_ne!(b.status, "failed", "no batch should have failed: {:?}", b);
    }
}

/// D3 — all-at-once strategy → one batch → all succeed.
#[tokio::test]
async fn d3_all_at_once_single_batch_completes() {
    let cp = harness::spawn_cp().await;

    for i in 1..=3 {
        harness::register_machine(&cp, &format!("api-{i:02}"), &["api"]).await;
    }
    let release_id = harness::create_release(
        &cp,
        &[
            ("api-01", "/nix/store/d3-api-01"),
            ("api-02", "/nix/store/d3-api-02"),
            ("api-03", "/nix/store/d3-api-03"),
        ],
    )
    .await;

    let rollout_id = harness::create_rollout_for_tag(
        &cp,
        &release_id,
        "api",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    harness::tick_once(&cp).await; // pending → deploying

    let batches = cp.db.get_rollout_batches(&rollout_id).unwrap();
    assert_eq!(batches.len(), 1, "all-at-once must produce exactly 1 batch");

    for m in ["api-01", "api-02", "api-03"] {
        let path = format!("/nix/store/d3-{m}");
        harness::fake_agent_report(&cp, m, &path, true, "applied", &["api"]).await;
        cp.db.insert_health_report(m, "{}", true).unwrap();
    }

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
    assert_eq!(detail.batches.len(), 1);
}
