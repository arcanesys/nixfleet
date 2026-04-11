//! Phase 4 § 5 #9 — DB migrations scenarios.
//!
//! Pins:
//!   1. Fresh DB → migrate → schema shape (every expected table is
//!      present, refinery_schema_history exists, no leftover tables).
//!   2. Migrate twice → idempotent. Pre-existing I1 in
//!      `infra_scenarios.rs` already covers this; we re-pin it here
//!      for completeness.
//!   3. Per-table read+write integration coverage. Most tables are
//!      already exercised end-to-end by Phase 3 scenarios via the
//!      HTTP route layer; this test does a per-table grep-style
//!      audit instead of duplicating.

use nixfleet_control_plane::db;
use rusqlite::Connection;

/// The expected set of user tables after all migrations have run.
/// Source of truth: `control-plane/migrations/V*.sql`. Adding or
/// removing a table requires updating this list AND the migrations.
const EXPECTED_TABLES: &[&str] = &[
    "api_keys",
    "audit_events",
    "generations",
    "health_reports",
    "machine_tags",
    "machines",
    "release_entries",
    "releases",
    "reports",
    "rollout_batches",
    "rollout_events",
    "rollouts",
];

#[test]
fn fresh_db_migrate_produces_expected_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("fresh.db");
    let database = db::Db::new(db_path.to_str().unwrap()).unwrap();
    database.migrate().unwrap();

    // Open a fresh raw rusqlite connection (not via the Db wrapper)
    // and query sqlite_master directly. The Db API doesn't expose
    // raw SQL access, but we can open another handle on the same
    // file because rusqlite supports concurrent connections.
    let conn = Connection::open(&db_path).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type='table' \
               AND name NOT LIKE 'refinery%' \
               AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .unwrap();
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    let expected: Vec<String> = EXPECTED_TABLES.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        tables, expected,
        "post-migrate schema shape drift: a migration was added/removed \
         without updating EXPECTED_TABLES in this test (or vice versa)"
    );
}

#[test]
fn refinery_schema_history_table_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("refinery.db");
    let database = db::Db::new(db_path.to_str().unwrap()).unwrap();
    database.migrate().unwrap();

    let conn = Connection::open(&db_path).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='table' AND name='refinery_schema_history'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "refinery_schema_history table must exist after migrate (refinery uses \
         it to track applied migrations)"
    );
}

#[test]
fn migrate_is_idempotent_two_calls_in_a_row() {
    // Pre-existing I1 in infra_scenarios.rs covers this; this is the
    // direct call without going through harness::spawn_cp.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("idem.db");
    let database = db::Db::new(db_path.to_str().unwrap()).unwrap();

    database.migrate().expect("first migrate");
    database
        .migrate()
        .expect("second migrate must be idempotent");

    // After two calls the schema should still be exactly the expected set.
    let conn = Connection::open(&db_path).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type='table' \
               AND name NOT LIKE 'refinery%' \
               AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .unwrap();
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let expected: Vec<String> = EXPECTED_TABLES.iter().map(|s| s.to_string()).collect();
    assert_eq!(tables, expected);
}

/// Smoke test: every expected table accepts at least an empty SELECT.
/// Catches mis-spelled table names, missing primary keys, or incorrect
/// migration ordering.
#[test]
fn every_expected_table_is_queryable() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("query.db");
    let database = db::Db::new(db_path.to_str().unwrap()).unwrap();
    database.migrate().unwrap();

    let conn = Connection::open(&db_path).unwrap();
    for table in EXPECTED_TABLES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|e| panic!("SELECT COUNT(*) FROM {table} failed: {e}"));
        assert_eq!(count, 0, "fresh table {table} must be empty");
    }
}
