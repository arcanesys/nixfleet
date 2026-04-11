//! I1 — DB migrations are idempotent.

#[path = "harness.rs"]
mod harness;

#[tokio::test]
async fn i1_migrations_run_twice_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("idempotent.db")
        .to_string_lossy()
        .into_owned();
    let db = nixfleet_control_plane::db::Db::new(&path).unwrap();

    db.migrate().expect("first migrate");
    db.migrate()
        .expect("second migrate — must be a no-op, not an error");

    // Positive: the schema is valid enough to insert an api_key.
    db.insert_api_key("hash-i1", "idempotent", "admin").unwrap();

    // Negative: a known table from Phase 2's rewritten base migration
    // must exist; a removed table must not.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let count_release: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='releases'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count_release, 1, "releases table must exist");

    let count_policy: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='rollout_policies'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count_policy, 0,
        "rollout_policies table must NOT exist (archived Phase 2)"
    );

    // Harness supports this pattern via `spawn_cp` too — do the same
    // through the HTTP router for a second check.
    let _cp = harness::spawn_cp().await;
}
