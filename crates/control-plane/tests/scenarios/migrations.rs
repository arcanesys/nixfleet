//! DB migrations scenarios.
//!
//! Pins:
//!   1. Fresh DB → migrate → schema shape (every expected table is
//!      present, `refinery_schema_history` exists, no leftover tables).
//!   2. `refinery_schema_history` table exists after migrate.
//!   3. Migrate twice → idempotent.
//!
//! Per-table read+write coverage is handled end-to-end by the scenario
//! suite via the HTTP route layer - no need to duplicate it here.

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
fn fresh_db_migrate_produces_expected_schema_and_every_table_is_queryable() {
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

    // Smoke-check that every expected table actually accepts SELECT.
    // Catches mis-spelled names, missing primary keys, and incorrect
    // migration ordering that would let sqlite_master list a table
    // that can't be queried.
    for table in EXPECTED_TABLES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|e| panic!("SELECT COUNT(*) FROM {table} failed: {e}"));
        assert_eq!(count, 0, "fresh table {table} must be empty");
    }
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

// (SELECT-queryability is folded into
// fresh_db_migrate_produces_expected_schema_and_every_table_is_queryable
// above to avoid spinning up a second tempdir/db for the same assertion.)
