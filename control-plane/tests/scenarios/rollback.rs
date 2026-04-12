//! Rollback via redeploying an old release as a forward rollout.

use super::harness;

use nixfleet_types::rollout::{OnFailure, RolloutStatus, RolloutStrategy};

#[tokio::test]
async fn redeploy_old_release_as_forward_rollback() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;

    let old = harness::create_release(&cp, &[("web-01", "/nix/store/rb4-old")]).await;
    let new = harness::create_release(&cp, &[("web-01", "/nix/store/rb4-new")]).await;

    // Stage 1: deploy new.
    let fwd = harness::create_rollout_for_tag(
        &cp,
        &new,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;
    harness::tick_once(&cp).await;
    harness::agent_reports_health(&cp, "web-01", "/nix/store/rb4-new", true).await;
    harness::tick_once(&cp).await;
    harness::tick_once(&cp).await;
    let _ = harness::wait_rollout_status(
        &cp,
        &fwd,
        RolloutStatus::Completed,
        std::time::Duration::from_secs(2),
    )
    .await;

    // Stage 2: forward rollback — redeploy the old release.
    let back = harness::create_rollout_for_tag(
        &cp,
        &old,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;
    harness::tick_once(&cp).await;
    harness::agent_reports_health(&cp, "web-01", "/nix/store/rb4-old", true).await;
    harness::tick_once(&cp).await;
    harness::tick_once(&cp).await;

    let detail = harness::wait_rollout_status(
        &cp,
        &back,
        RolloutStatus::Completed,
        std::time::Duration::from_secs(2),
    )
    .await;

    assert!(matches!(detail.status, RolloutStatus::Completed));
    assert_eq!(
        detail.release_id, old,
        "rolled-back rollout must reference the old release id, got {}",
        detail.release_id
    );

    // Negative: the original forward rollout is still Completed (history preserved).
    let resp = cp
        .admin
        .get(format!("{}/api/v1/rollouts/{}", cp.base, fwd))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    let fwd_detail: nixfleet_types::rollout::RolloutDetail = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("decode RolloutDetail from {body:?}: {e}"));
    assert!(
        matches!(fwd_detail.status, RolloutStatus::Completed),
        "original forward rollout must stay Completed after a subsequent rollout; got {:?}",
        fwd_detail.status
    );
}
