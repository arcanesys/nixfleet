//! P1, P2 — CP-side: desired-generation response includes poll_hint iff active rollout.
//!
//! Spec Section 10. The agent-side half (does the agent shorten its poll when
//! it sees `poll_hint`?) is covered by the VM/wiremock tests.

use super::harness;

use nixfleet_types::DesiredGeneration;

/// P1 — machine inside an active rollout → response carries `poll_hint = Some(5)`.
#[tokio::test]
async fn p1_poll_hint_present_when_rollout_active() {
    let (cp, _release_id, _rollout_id) =
        harness::spawn_cp_with_rollout("/nix/store/p1-web-01").await;

    // One executor tick so the batch transitions to `deploying` and the
    // machine's desired_generation is populated in FleetState.
    harness::tick_once(&cp).await;

    let resp = cp
        .admin
        .get(format!(
            "{}/api/v1/machines/web-01/desired-generation",
            cp.base
        ))
        .send()
        .await
        .expect("GET desired-generation");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let gen: DesiredGeneration =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("decode {body:?}: {e}"));

    assert_eq!(gen.hash, "/nix/store/p1-web-01");
    assert_eq!(
        gen.poll_hint,
        Some(5),
        "poll_hint must be 5 seconds when machine is in an active rollout"
    );
}

/// P2 — machine has a desired generation but no active rollout → `poll_hint` absent.
#[tokio::test]
async fn p2_poll_hint_absent_when_idle() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "api-01", &["api"]).await;

    // Seed desired generation directly (no rollout, no batch).
    cp.db
        .set_desired_generation("api-01", "/nix/store/p2-api-01")
        .unwrap();
    {
        let mut fleet = cp.fleet.write().await;
        let m = fleet.get_or_create("api-01");
        m.desired_generation = Some(DesiredGeneration {
            hash: "/nix/store/p2-api-01".to_string(),
            cache_url: None,
            poll_hint: None,
        });
    }

    let resp = cp
        .admin
        .get(format!(
            "{}/api/v1/machines/api-01/desired-generation",
            cp.base
        ))
        .send()
        .await
        .expect("GET desired-generation");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let gen: DesiredGeneration =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("decode {body:?}: {e}"));

    assert_eq!(gen.hash, "/nix/store/p2-api-01");
    assert!(
        gen.poll_hint.is_none(),
        "poll_hint must be absent when no rollout is active (got {:?})",
        gen.poll_hint
    );
}
