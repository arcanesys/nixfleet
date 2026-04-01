use anyhow::{Context, Result};
use rusqlite::Connection;
use std::sync::Mutex;

/// SQLite-backed local state persistence.
///
/// Stores generation checks, deployments, rollbacks, and errors
/// so the agent has a local audit trail even when the control plane
/// is unreachable.
///
/// TODO: Store methods are synchronous and block the tokio runtime.
/// Wrap calls in `tokio::task::spawn_blocking` for production use.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open (or create) the SQLite database at the given path.
    pub fn new(path: &str) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).context("failed to create database directory")?;
        }

        let conn = Connection::open(path).context("failed to open SQLite database")?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Initialize database tables.
    pub fn init(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT    NOT NULL DEFAULT (datetime('now')),
                kind      TEXT    NOT NULL,
                hash      TEXT,
                message   TEXT
            );

            CREATE TABLE IF NOT EXISTS state (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .context("failed to initialize database tables")?;

        Ok(())
    }

    /// Log a generation check event.
    pub fn log_check(&self, hash: &str, status: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?;
        conn.execute(
            "INSERT INTO events (kind, hash, message) VALUES ('check', ?1, ?2)",
            rusqlite::params![hash, status],
        )
        .context("failed to log check event")?;
        Ok(())
    }

    /// Log a successful (or failed) deployment.
    ///
    /// Wraps INSERT + UPSERT in a transaction for atomicity.
    pub fn log_deploy(&self, hash: &str, success: bool) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?;
        let tx = conn
            .unchecked_transaction()
            .context("failed to begin transaction")?;
        let msg = if success { "success" } else { "failed" };
        tx.execute(
            "INSERT INTO events (kind, hash, message) VALUES ('deploy', ?1, ?2)",
            rusqlite::params![hash, msg],
        )
        .context("failed to log deploy event")?;

        if success {
            tx.execute(
                "INSERT OR REPLACE INTO state (key, value) VALUES ('current_generation', ?1)",
                rusqlite::params![hash],
            )
            .context("failed to update current generation state")?;
        }
        tx.commit().context("failed to commit deploy transaction")?;
        Ok(())
    }

    /// Log a rollback event with the reason.
    pub fn log_rollback(&self, reason: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?;
        conn.execute(
            "INSERT INTO events (kind, message) VALUES ('rollback', ?1)",
            rusqlite::params![reason],
        )
        .context("failed to log rollback event")?;
        Ok(())
    }

    /// Log an error.
    pub fn log_error(&self, message: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?;
        conn.execute(
            "INSERT INTO events (kind, message) VALUES ('error', ?1)",
            rusqlite::params![message],
        )
        .context("failed to log error event")?;
        Ok(())
    }

    /// Count events of a given kind (used in tests).
    #[cfg(test)]
    fn count_events(&self, kind: &str) -> Result<i64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind = ?1",
            rusqlite::params![kind],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Read a state value by key (used in tests).
    #[cfg(test)]
    fn get_state(&self, key: &str) -> Result<Option<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?;
        let result = conn.query_row(
            "SELECT value FROM state WHERE key = ?1",
            rusqlite::params![key],
            |row| row.get(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> (Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = Store::new(db_path.to_str().unwrap()).unwrap();
        store.init().unwrap();
        (store, dir)
    }

    #[test]
    fn test_store_init_and_log() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = Store::new(db_path.to_str().unwrap()).unwrap();
        store.init().unwrap();
        store.log_check("/nix/store/abc", "up-to-date").unwrap();
        store.log_deploy("/nix/store/abc", true).unwrap();
        store.log_rollback("test reason").unwrap();
        store.log_error("test error").unwrap();
    }

    #[test]
    fn test_init_is_idempotent() {
        let (store, _dir) = make_store();
        // Second init should not fail (IF NOT EXISTS)
        store.init().unwrap();
    }

    #[test]
    fn test_log_check_creates_event() {
        let (store, _dir) = make_store();
        store.log_check("/nix/store/abc123", "up-to-date").unwrap();
        let count = store.count_events("check").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_log_check_mismatch() {
        let (store, _dir) = make_store();
        store.log_check("/nix/store/abc123", "mismatch").unwrap();
        let count = store.count_events("check").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_log_deploy_success_updates_state() {
        let (store, _dir) = make_store();
        store.log_deploy("/nix/store/abc123", true).unwrap();
        let count = store.count_events("deploy").unwrap();
        assert_eq!(count, 1);
        let current = store.get_state("current_generation").unwrap();
        assert_eq!(current, Some("/nix/store/abc123".to_string()));
    }

    #[test]
    fn test_log_deploy_failure_does_not_update_state() {
        let (store, _dir) = make_store();
        store.log_deploy("/nix/store/abc123", false).unwrap();
        let count = store.count_events("deploy").unwrap();
        assert_eq!(count, 1);
        let current = store.get_state("current_generation").unwrap();
        assert!(current.is_none());
    }

    #[test]
    fn test_log_deploy_updates_state_on_sequential_deploys() {
        let (store, _dir) = make_store();
        store.log_deploy("/nix/store/gen1", true).unwrap();
        store.log_deploy("/nix/store/gen2", true).unwrap();
        let current = store.get_state("current_generation").unwrap();
        assert_eq!(current, Some("/nix/store/gen2".to_string()));
    }

    #[test]
    fn test_log_rollback_creates_event() {
        let (store, _dir) = make_store();
        store.log_rollback("health check failed").unwrap();
        let count = store.count_events("rollback").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_log_error_creates_event() {
        let (store, _dir) = make_store();
        store.log_error("something went wrong").unwrap();
        let count = store.count_events("error").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_multiple_event_kinds_tracked_independently() {
        let (store, _dir) = make_store();
        store.log_check("/nix/store/abc", "up-to-date").unwrap();
        store.log_check("/nix/store/abc", "up-to-date").unwrap();
        store.log_deploy("/nix/store/abc", true).unwrap();
        store.log_rollback("test").unwrap();
        store.log_error("test error").unwrap();
        assert_eq!(store.count_events("check").unwrap(), 2);
        assert_eq!(store.count_events("deploy").unwrap(), 1);
        assert_eq!(store.count_events("rollback").unwrap(), 1);
        assert_eq!(store.count_events("error").unwrap(), 1);
    }

    #[test]
    fn test_new_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c").join("state.db");
        let store = Store::new(nested.to_str().unwrap()).unwrap();
        store.init().unwrap();
        assert!(nested.exists());
    }
}
