use anyhow::{Context, Result};
use nixfleet_types::AuditEvent;
use rusqlite::Connection;
use std::sync::Mutex;

mod embedded {
    use refinery::embed_migrations;
    embed_migrations!("migrations");
}

/// SQLite-backed persistence for the control plane.
///
/// Stores generation assignments and agent reports as an audit trail.
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Open (or create) the SQLite database at the given path.
    pub fn new(path: &str) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).context("failed to create database directory")?;
            }
        }

        let conn = Connection::open(path).context("failed to open SQLite database")?;

        // Enable WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Run all pending database migrations.
    pub fn migrate(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        embedded::migrations::runner()
            .run(&mut *conn)
            .context("failed to run database migrations")?;
        Ok(())
    }

    /// Set (upsert) the desired generation for a machine.
    pub fn set_desired_generation(&self, machine_id: &str, hash: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO generations (machine_id, hash)
             VALUES (?1, ?2)
             ON CONFLICT(machine_id) DO UPDATE SET hash = ?2, set_at = datetime('now')",
            rusqlite::params![machine_id, hash],
        )
        .context("failed to set desired generation")?;
        Ok(())
    }

    /// Get the desired generation for a machine, if set.
    pub fn get_desired_generation(&self, machine_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT hash FROM generations WHERE machine_id = ?1",
            rusqlite::params![machine_id],
            |row| row.get(0),
        );
        match result {
            Ok(hash) => Ok(Some(hash)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List all desired generations (machine_id, hash).
    pub fn list_desired_generations(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT machine_id, hash FROM generations")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Store an agent report.
    pub fn insert_report(
        &self,
        machine_id: &str,
        generation: &str,
        success: bool,
        message: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO reports (machine_id, generation, success, message)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![machine_id, generation, success as i32, message],
        )
        .context("failed to insert report")?;
        Ok(())
    }

    /// Register a machine (upsert) with a given lifecycle state.
    pub fn register_machine(&self, machine_id: &str, lifecycle: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO machines (machine_id, lifecycle)
             VALUES (?1, ?2)
             ON CONFLICT(machine_id) DO UPDATE SET lifecycle = ?2",
            rusqlite::params![machine_id, lifecycle],
        )
        .context("failed to register machine")?;
        Ok(())
    }

    /// Get the lifecycle state for a machine.
    pub fn get_machine_lifecycle(&self, machine_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT lifecycle FROM machines WHERE machine_id = ?1",
            rusqlite::params![machine_id],
            |row| row.get(0),
        );
        match result {
            Ok(lifecycle) => Ok(Some(lifecycle)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Update a machine's lifecycle state.
    pub fn set_machine_lifecycle(&self, machine_id: &str, lifecycle: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "UPDATE machines SET lifecycle = ?2 WHERE machine_id = ?1",
                rusqlite::params![machine_id, lifecycle],
            )
            .context("failed to update machine lifecycle")?;
        Ok(rows > 0)
    }

    /// Insert an API key record.
    pub fn insert_api_key(&self, key_hash: &str, name: &str, role: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO api_keys (key_hash, name, role) VALUES (?1, ?2, ?3)",
            rusqlite::params![key_hash, name, role],
        )
        .context("failed to insert API key")?;
        Ok(())
    }

    /// Verify an API key and return its role if found.
    pub fn verify_api_key(&self, key_hash: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT role FROM api_keys WHERE key_hash = ?1",
            rusqlite::params![key_hash],
            |row| row.get(0),
        );
        match result {
            Ok(role) => Ok(Some(role)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get the name associated with an API key.
    pub fn get_api_key_name(&self, key_hash: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT name FROM api_keys WHERE key_hash = ?1",
            rusqlite::params![key_hash],
            |row| row.get(0),
        );
        match result {
            Ok(name) => Ok(Some(name)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List all registered machines.
    pub fn list_machines(&self) -> Result<Vec<MachineRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT machine_id, lifecycle, registered_at FROM machines")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(MachineRow {
                    machine_id: row.get(0)?,
                    lifecycle: row.get(1)?,
                    registered_at: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Insert an audit event.
    pub fn insert_audit_event(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        detail: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO audit_events (actor, action, target, detail) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![actor, action, target, detail],
        )
        .context("failed to insert audit event")?;
        Ok(())
    }

    /// Query audit events with optional filters, ordered by timestamp DESC.
    pub fn query_audit_events(
        &self,
        actor: Option<&str>,
        action: Option<&str>,
        target: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AuditEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, timestamp, actor, action, target, detail FROM audit_events WHERE 1=1",
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(a) = actor {
            sql.push_str(&format!(" AND actor = ?{}", params.len() + 1));
            params.push(Box::new(a.to_string()));
        }
        if let Some(a) = action {
            sql.push_str(&format!(" AND action = ?{}", params.len() + 1));
            params.push(Box::new(a.to_string()));
        }
        if let Some(t) = target {
            sql.push_str(&format!(" AND target = ?{}", params.len() + 1));
            params.push(Box::new(t.to_string()));
        }

        sql.push_str(&format!(
            " ORDER BY timestamp DESC LIMIT ?{}",
            params.len() + 1
        ));
        params.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(AuditEvent {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    actor: row.get(2)?,
                    action: row.get(3)?,
                    target: row.get(4)?,
                    detail: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Get recent reports for a machine (most recent first).
    pub fn get_recent_reports(&self, machine_id: &str, limit: usize) -> Result<Vec<ReportRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT machine_id, generation, success, message, received_at
             FROM reports
             WHERE machine_id = ?1
             ORDER BY received_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![machine_id, limit], |row| {
                Ok(ReportRow {
                    machine_id: row.get(0)?,
                    generation: row.get(1)?,
                    success: row.get::<_, i32>(2)? != 0,
                    message: row.get(3)?,
                    received_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// A report row as stored in SQLite.
#[derive(Debug, Clone)]
pub struct ReportRow {
    pub machine_id: String,
    pub generation: String,
    pub success: bool,
    pub message: Option<String>,
    pub received_at: String,
}

/// A machine row as stored in SQLite.
#[derive(Debug, Clone)]
pub struct MachineRow {
    pub machine_id: String,
    pub lifecycle: String,
    pub registered_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Db::new(db_path.to_str().unwrap()).unwrap();
        db.migrate().unwrap();
        (db, dir)
    }

    #[test]
    fn test_migrate_is_idempotent() {
        let (db, _dir) = make_db();
        db.migrate().unwrap();
    }

    #[test]
    fn test_set_and_get_desired_generation() {
        let (db, _dir) = make_db();
        db.set_desired_generation("web-01", "/nix/store/abc123")
            .unwrap();
        let hash = db.get_desired_generation("web-01").unwrap();
        assert_eq!(hash, Some("/nix/store/abc123".to_string()));
    }

    #[test]
    fn test_get_desired_generation_missing() {
        let (db, _dir) = make_db();
        let hash = db.get_desired_generation("nonexistent").unwrap();
        assert!(hash.is_none());
    }

    #[test]
    fn test_set_desired_generation_upsert() {
        let (db, _dir) = make_db();
        db.set_desired_generation("web-01", "/nix/store/gen1")
            .unwrap();
        db.set_desired_generation("web-01", "/nix/store/gen2")
            .unwrap();
        let hash = db.get_desired_generation("web-01").unwrap();
        assert_eq!(hash, Some("/nix/store/gen2".to_string()));
    }

    #[test]
    fn test_list_desired_generations() {
        let (db, _dir) = make_db();
        db.set_desired_generation("web-01", "/nix/store/abc")
            .unwrap();
        db.set_desired_generation("dev-01", "/nix/store/def").unwrap();
        let gens = db.list_desired_generations().unwrap();
        assert_eq!(gens.len(), 2);
    }

    #[test]
    fn test_insert_and_get_reports() {
        let (db, _dir) = make_db();
        db.insert_report("web-01", "/nix/store/abc", true, "deployed")
            .unwrap();
        db.insert_report("web-01", "/nix/store/abc", false, "rolled back")
            .unwrap();
        let reports = db.get_recent_reports("web-01", 10).unwrap();
        assert_eq!(reports.len(), 2);
        // Both reports present — one success, one failure
        let successes = reports.iter().filter(|r| r.success).count();
        let failures = reports.iter().filter(|r| !r.success).count();
        assert_eq!(successes, 1);
        assert_eq!(failures, 1);
    }

    #[test]
    fn test_reports_limit() {
        let (db, _dir) = make_db();
        for i in 0..5 {
            db.insert_report("web-01", &format!("/nix/store/gen{i}"), true, "ok")
                .unwrap();
        }
        let reports = db.get_recent_reports("web-01", 2).unwrap();
        assert_eq!(reports.len(), 2);
    }

    #[test]
    fn test_reports_isolated_per_machine() {
        let (db, _dir) = make_db();
        db.insert_report("web-01", "/nix/store/abc", true, "ok")
            .unwrap();
        db.insert_report("dev-01", "/nix/store/def", true, "ok")
            .unwrap();
        let web_01_reports = db.get_recent_reports("web-01", 10).unwrap();
        let dev_01_reports = db.get_recent_reports("dev-01", 10).unwrap();
        assert_eq!(web_01_reports.len(), 1);
        assert_eq!(dev_01_reports.len(), 1);
    }

    #[test]
    fn test_register_machine() {
        let (db, _dir) = make_db();
        db.register_machine("web-01", "pending").unwrap();
        let lc = db.get_machine_lifecycle("web-01").unwrap();
        assert_eq!(lc, Some("pending".to_string()));
    }

    #[test]
    fn test_register_machine_upsert() {
        let (db, _dir) = make_db();
        db.register_machine("web-01", "pending").unwrap();
        db.register_machine("web-01", "active").unwrap();
        let lc = db.get_machine_lifecycle("web-01").unwrap();
        assert_eq!(lc, Some("active".to_string()));
    }

    #[test]
    fn test_get_machine_lifecycle_missing() {
        let (db, _dir) = make_db();
        let lc = db.get_machine_lifecycle("nonexistent").unwrap();
        assert!(lc.is_none());
    }

    #[test]
    fn test_set_machine_lifecycle() {
        let (db, _dir) = make_db();
        db.register_machine("web-01", "active").unwrap();
        let updated = db.set_machine_lifecycle("web-01", "maintenance").unwrap();
        assert!(updated);
        let lc = db.get_machine_lifecycle("web-01").unwrap();
        assert_eq!(lc, Some("maintenance".to_string()));
    }

    #[test]
    fn test_set_machine_lifecycle_missing() {
        let (db, _dir) = make_db();
        let updated = db.set_machine_lifecycle("nonexistent", "active").unwrap();
        assert!(!updated);
    }

    #[test]
    fn test_insert_and_verify_api_key() {
        let (db, _dir) = make_db();
        db.insert_api_key("abc123hash", "test-key", "admin")
            .unwrap();
        let role = db.verify_api_key("abc123hash").unwrap();
        assert_eq!(role, Some("admin".to_string()));
    }

    #[test]
    fn test_verify_api_key_missing() {
        let (db, _dir) = make_db();
        let role = db.verify_api_key("nonexistent").unwrap();
        assert!(role.is_none());
    }

    #[test]
    fn test_insert_and_query_audit_event() {
        let (db, _dir) = make_db();
        db.insert_audit_event(
            "apikey:deploy-key",
            "set_generation",
            "web-01",
            Some("hash=/nix/store/abc123"),
        )
        .unwrap();
        let events = db.query_audit_events(None, None, None, 100).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actor, "apikey:deploy-key");
        assert_eq!(events[0].action, "set_generation");
        assert_eq!(events[0].target, "web-01");
    }

    #[test]
    fn test_query_audit_events_filter_by_actor() {
        let (db, _dir) = make_db();
        db.insert_audit_event("apikey:admin", "set_generation", "web-01", None)
            .unwrap();
        db.insert_audit_event("machine:dev-01", "report", "dev-01", None)
            .unwrap();
        let events = db
            .query_audit_events(Some("apikey:admin"), None, None, 100)
            .unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_query_audit_events_filter_by_action() {
        let (db, _dir) = make_db();
        db.insert_audit_event("apikey:admin", "set_generation", "web-01", None)
            .unwrap();
        db.insert_audit_event("apikey:admin", "register", "dev-01", None)
            .unwrap();
        let events = db
            .query_audit_events(None, Some("register"), None, 100)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "register");
    }

    #[test]
    fn test_list_machines() {
        let (db, _dir) = make_db();
        db.register_machine("web-01", "active").unwrap();
        db.register_machine("dev-01", "pending").unwrap();
        let machines = db.list_machines().unwrap();
        assert_eq!(machines.len(), 2);
    }
}
