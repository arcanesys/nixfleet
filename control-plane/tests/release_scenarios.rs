//! Release lifecycle scenarios (R4, R5, R6).
//!
//! Spec: docs/superpowers/specs/2026-04-10-core-hardening-cycle-design.md Section 4
//! Audit: docs/adr/009-core-hardening-audit.md Category 2 (release routes)

#[path = "harness.rs"]
mod harness;

use nixfleet_types::release::ReleaseDiff;

/// R4 — release diff A→B → correct added/removed/changed entries.
#[tokio::test]
async fn r4_release_diff_classifies_entries() {
    let cp = harness::spawn_cp().await;

    let a = harness::create_release(
        &cp,
        &[
            ("web-01", "/nix/store/hash-a-web-01"),
            ("web-02", "/nix/store/hash-a-web-02"),
            ("db-01", "/nix/store/hash-a-db-01"),
        ],
    )
    .await;

    // B: web-01 changed, web-02 unchanged, db-01 removed, api-01 added.
    let b = harness::create_release(
        &cp,
        &[
            ("web-01", "/nix/store/hash-b-web-01"),
            ("web-02", "/nix/store/hash-a-web-02"),
            ("api-01", "/nix/store/hash-b-api-01"),
        ],
    )
    .await;

    let resp = cp
        .admin
        .get(format!("{}/api/v1/releases/{}/diff/{}", cp.base, a, b))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let diff: ReleaseDiff = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("decode ReleaseDiff from {body:?}: {e}"));

    // `added` and `removed` are Vec<String> of hostnames in the schema;
    // `changed` is Vec<ReleaseDiffEntry> with old/new store paths.
    let changed_hosts: Vec<&String> = diff.changed.iter().map(|e| &e.hostname).collect();

    // Positive assertions
    assert!(
        diff.added.iter().any(|h| h == "api-01"),
        "api-01 should be in added, got added={:?}",
        diff.added
    );
    assert!(
        diff.removed.iter().any(|h| h == "db-01"),
        "db-01 should be in removed, got removed={:?}",
        diff.removed
    );
    assert!(
        changed_hosts.iter().any(|h| *h == "web-01"),
        "web-01 hash changed, should be in changed, got changed={:?}",
        changed_hosts
    );

    // Negative assertions
    assert!(
        !changed_hosts.iter().any(|h| *h == "web-02"),
        "web-02 unchanged — must NOT appear in 'changed'; changed={:?}",
        changed_hosts
    );
    assert!(
        !diff.added.iter().any(|h| h == "web-01"),
        "web-01 exists in both releases — must NOT appear in 'added'; added={:?}",
        diff.added
    );
    assert!(
        !diff.removed.iter().any(|h| h == "web-02"),
        "web-02 exists in both releases — must NOT appear in 'removed'; removed={:?}",
        diff.removed
    );

    // web-02 should be in unchanged
    assert!(
        diff.unchanged.iter().any(|h| h == "web-02"),
        "web-02 unchanged — should be in unchanged; unchanged={:?}",
        diff.unchanged
    );
}

/// R5 — delete a release that a rollout references → 409.
#[tokio::test]
async fn r5_delete_referenced_release_returns_409() {
    let (cp, release_id, _rollout_id) =
        harness::spawn_cp_with_rollout("/nix/store/hash-referenced-web-01").await;

    let resp = cp
        .admin
        .delete(format!("{}/api/v1/releases/{}", cp.base, release_id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409, "deleting a referenced release must 409");

    // Negative: the release and its entries are still present.
    let still = cp
        .admin
        .get(format!("{}/api/v1/releases/{}", cp.base, release_id))
        .send()
        .await
        .unwrap();
    assert_eq!(
        still.status(),
        200,
        "release must still exist after rejected delete"
    );
}

/// R6 — delete an orphan release → 204 + cascade to release_entries.
#[tokio::test]
async fn r6_delete_orphan_release_cascades_entries() {
    let cp = harness::spawn_cp().await;

    let release_id =
        harness::create_release(&cp, &[("web-01", "/nix/store/hash-orphan-web-01")]).await;

    // Sanity: entries exist before delete.
    let before = cp.db.get_release_entries(&release_id).unwrap();
    assert_eq!(before.len(), 1, "release must have 1 entry before delete");

    let resp = cp
        .admin
        .delete(format!("{}/api/v1/releases/{}", cp.base, release_id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "orphan delete must succeed");

    // Release is gone.
    let after_get = cp
        .admin
        .get(format!("{}/api/v1/releases/{}", cp.base, release_id))
        .send()
        .await
        .unwrap();
    assert_eq!(after_get.status(), 404);

    // Negative: entries cascade.
    let after_entries = cp.db.get_release_entries(&release_id).unwrap();
    assert!(
        after_entries.is_empty(),
        "release_entries must cascade-delete (FK ON DELETE CASCADE + PRAGMA foreign_keys=ON)"
    );
}
