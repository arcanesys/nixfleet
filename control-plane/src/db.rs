use anyhow::{Context, Result};
use nixfleet_types::AuditEvent;
use rusqlite::Connection;
use serde_json;
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

    /// Check if any API keys exist in the database.
    pub fn has_api_keys(&self) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM api_keys",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
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

    /// Replace all tags for a machine.
    pub fn set_machine_tags(&self, machine_id: &str, tags: &[String]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().context("failed to start transaction")?;
        tx.execute(
            "DELETE FROM machine_tags WHERE machine_id = ?1",
            rusqlite::params![machine_id],
        )
        .context("failed to delete existing machine tags")?;
        for tag in tags {
            tx.execute(
                "INSERT INTO machine_tags (machine_id, tag) VALUES (?1, ?2)",
                rusqlite::params![machine_id, tag],
            )
            .context("failed to insert machine tag")?;
        }
        tx.commit().context("failed to commit tags")?;
        Ok(())
    }

    /// Get all tags for a machine, ordered by tag name.
    pub fn get_machine_tags(&self, machine_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT tag FROM machine_tags WHERE machine_id = ?1 ORDER BY tag")?;
        let rows = stmt
            .query_map(rusqlite::params![machine_id], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(rows)
    }

    /// Get machine IDs that have ALL given tags (AND logic).
    pub fn get_machines_by_tags(&self, tags: &[String]) -> Result<Vec<String>> {
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = (1..=tags.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "SELECT machine_id FROM machine_tags WHERE tag IN ({}) \
             GROUP BY machine_id HAVING COUNT(DISTINCT tag) = ?{}",
            placeholders.join(", "),
            tags.len() + 1
        );
        let conn = self.conn.lock().unwrap();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        for tag in tags {
            params.push(Box::new(tag.clone()));
        }
        params.push(Box::new(tags.len() as i64));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(rows)
    }

    /// Remove a single tag from a machine.
    pub fn remove_machine_tag(&self, machine_id: &str, tag: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM machine_tags WHERE machine_id = ?1 AND tag = ?2",
            rusqlite::params![machine_id, tag],
        )
        .context("failed to remove machine tag")?;
        Ok(())
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

    /// Create a new rollout, returns the generated id.
    #[allow(clippy::too_many_arguments)]
    pub fn create_rollout(
        &self,
        id: &str,
        generation_hash: &str,
        cache_url: Option<&str>,
        strategy: &str,
        batch_sizes: &str,
        failure_threshold: &str,
        on_failure: &str,
        health_timeout: i64,
        target_tags: Option<&str>,
        target_hosts: Option<&str>,
        previous_generation: Option<&str>,
        created_by: &str,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO rollouts (id, generation_hash, cache_url, strategy, batch_sizes,
             failure_threshold, on_failure, health_timeout, status, target_tags, target_hosts,
             previous_generation, created_at, updated_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'created', ?9, ?10, ?11,
             datetime('now'), datetime('now'), ?12)",
            rusqlite::params![
                id,
                generation_hash,
                cache_url,
                strategy,
                batch_sizes,
                failure_threshold,
                on_failure,
                health_timeout,
                target_tags,
                target_hosts,
                previous_generation,
                created_by,
            ],
        )
        .context("failed to create rollout")?;
        Ok(id.to_string())
    }

    /// Create a rollout batch.
    pub fn create_rollout_batch(
        &self,
        id: &str,
        rollout_id: &str,
        batch_index: i64,
        machine_ids: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO rollout_batches (id, rollout_id, batch_index, machine_ids)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, rollout_id, batch_index, machine_ids],
        )
        .context("failed to create rollout batch")?;
        Ok(())
    }

    /// Get a rollout by id.
    pub fn get_rollout(&self, id: &str) -> Result<Option<RolloutRow>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT id, generation_hash, cache_url, strategy, batch_sizes, failure_threshold,
             on_failure, health_timeout, status, target_tags, target_hosts, previous_generation,
             created_at, updated_at, created_by
             FROM rollouts WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(RolloutRow {
                    id: row.get(0)?,
                    generation_hash: row.get(1)?,
                    cache_url: row.get(2)?,
                    strategy: row.get(3)?,
                    batch_sizes: row.get(4)?,
                    failure_threshold: row.get(5)?,
                    on_failure: row.get(6)?,
                    health_timeout: row.get(7)?,
                    status: row.get(8)?,
                    target_tags: row.get(9)?,
                    target_hosts: row.get(10)?,
                    previous_generation: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    created_by: row.get(14)?,
                })
            },
        );
        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List rollouts with optional status filter, ordered by created_at DESC.
    pub fn list_rollouts_by_status(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RolloutRow>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, generation_hash, cache_url, strategy, batch_sizes, failure_threshold,
             on_failure, health_timeout, status, target_tags, target_hosts, previous_generation,
             created_at, updated_at, created_by FROM rollouts WHERE 1=1",
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(s) = status {
            sql.push_str(&format!(" AND status = ?{}", params.len() + 1));
            params.push(Box::new(s.to_string()));
        }

        sql.push_str(&format!(
            " ORDER BY created_at DESC LIMIT ?{}",
            params.len() + 1
        ));
        params.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(RolloutRow {
                    id: row.get(0)?,
                    generation_hash: row.get(1)?,
                    cache_url: row.get(2)?,
                    strategy: row.get(3)?,
                    batch_sizes: row.get(4)?,
                    failure_threshold: row.get(5)?,
                    on_failure: row.get(6)?,
                    health_timeout: row.get(7)?,
                    status: row.get(8)?,
                    target_tags: row.get(9)?,
                    target_hosts: row.get(10)?,
                    previous_generation: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    created_by: row.get(14)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Update a rollout's status.
    pub fn update_rollout_status(&self, id: &str, status: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "UPDATE rollouts SET status = ?2, updated_at = datetime('now') WHERE id = ?1",
                rusqlite::params![id, status],
            )
            .context("failed to update rollout status")?;
        Ok(rows > 0)
    }

    /// Get all batches for a rollout, ordered by batch_index.
    pub fn get_rollout_batches(&self, rollout_id: &str) -> Result<Vec<RolloutBatchRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, rollout_id, batch_index, machine_ids, status, started_at, completed_at
             FROM rollout_batches WHERE rollout_id = ?1 ORDER BY batch_index",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![rollout_id], |row| {
                Ok(RolloutBatchRow {
                    id: row.get(0)?,
                    rollout_id: row.get(1)?,
                    batch_index: row.get(2)?,
                    machine_ids: row.get(3)?,
                    status: row.get(4)?,
                    started_at: row.get(5)?,
                    completed_at: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Update a batch's status with conditional timestamps.
    /// "deploying" sets started_at, "succeeded"/"failed" sets completed_at.
    pub fn update_batch_status(&self, batch_id: &str, status: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let sql = match status {
            "deploying" => {
                "UPDATE rollout_batches SET status = ?2, started_at = datetime('now') WHERE id = ?1"
            }
            "succeeded" | "failed" => {
                "UPDATE rollout_batches SET status = ?2, completed_at = datetime('now') WHERE id = ?1"
            }
            _ => "UPDATE rollout_batches SET status = ?2 WHERE id = ?1",
        };
        let rows = conn
            .execute(sql, rusqlite::params![batch_id, status])
            .context("failed to update batch status")?;
        Ok(rows > 0)
    }

    /// Insert a health report.
    pub fn insert_health_report(
        &self,
        machine_id: &str,
        results: &str,
        all_passed: bool,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO health_reports (machine_id, results, all_passed)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![machine_id, results, all_passed as i32],
        )
        .context("failed to insert health report")?;
        Ok(())
    }

    /// Get health reports for a machine since a given timestamp, most recent first.
    pub fn get_health_reports_since(
        &self,
        machine_id: &str,
        since: &str,
    ) -> Result<Vec<HealthReportRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, machine_id, results, all_passed, received_at
             FROM health_reports
             WHERE machine_id = ?1 AND received_at >= ?2
             ORDER BY received_at DESC",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![machine_id, since], |row| {
                Ok(HealthReportRow {
                    id: row.get(0)?,
                    machine_id: row.get(1)?,
                    results: row.get(2)?,
                    all_passed: row.get::<_, i32>(3)? != 0,
                    received_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete health reports older than retention_hours. Returns number of deleted rows.
    pub fn cleanup_old_health_reports(&self, retention_hours: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "DELETE FROM health_reports WHERE received_at < datetime('now', ?1)",
                rusqlite::params![format!("-{retention_hours} hours")],
            )
            .context("failed to cleanup old health reports")?;
        Ok(rows)
    }

    /// Check if a machine is part of any active rollout (status: created/running/paused).
    /// Returns the rollout id if found.
    pub fn machine_in_active_rollout(&self, machine_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT r.id, rb.machine_ids FROM rollouts r
             JOIN rollout_batches rb ON rb.rollout_id = r.id
             WHERE r.status IN ('created', 'running', 'paused')",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        for (rollout_id, machine_ids_json) in rows {
            let machine_ids: Vec<String> =
                serde_json::from_str(&machine_ids_json).unwrap_or_default();
            if machine_ids.contains(&machine_id.to_string()) {
                return Ok(Some(rollout_id));
            }
        }
        Ok(None)
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

/// A rollout row as stored in SQLite.
#[derive(Debug, Clone)]
pub struct RolloutRow {
    pub id: String,
    pub generation_hash: String,
    pub cache_url: Option<String>,
    pub strategy: String,
    pub batch_sizes: String,
    pub failure_threshold: String,
    pub on_failure: String,
    pub health_timeout: i64,
    pub status: String,
    pub target_tags: Option<String>,
    pub target_hosts: Option<String>,
    pub previous_generation: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub created_by: String,
}

/// A rollout batch row as stored in SQLite.
#[derive(Debug, Clone)]
pub struct RolloutBatchRow {
    pub id: String,
    pub rollout_id: String,
    pub batch_index: i64,
    pub machine_ids: String,
    pub status: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// A health report row as stored in SQLite.
#[derive(Debug, Clone)]
pub struct HealthReportRow {
    pub id: i64,
    pub machine_id: String,
    pub results: String,
    pub all_passed: bool,
    pub received_at: String,
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
        db.set_desired_generation("dev-01", "/nix/store/def")
            .unwrap();
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
    fn test_has_api_keys_empty() {
        let (db, _dir) = make_db();
        assert!(!db.has_api_keys().unwrap());
    }

    #[test]
    fn test_has_api_keys_after_insert() {
        let (db, _dir) = make_db();
        db.insert_api_key("hash123", "admin", "admin").unwrap();
        assert!(db.has_api_keys().unwrap());
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

    #[test]
    fn test_set_and_get_machine_tags() {
        let (db, _dir) = make_db();
        db.register_machine("web-01", "active").unwrap();
        db.set_machine_tags("web-01", &["production".to_string(), "eu-west".to_string()])
            .unwrap();
        let tags = db.get_machine_tags("web-01").unwrap();
        assert_eq!(tags.len(), 2);
        assert!(tags.contains(&"production".to_string()));
        assert!(tags.contains(&"eu-west".to_string()));
    }

    #[test]
    fn test_set_machine_tags_replaces() {
        let (db, _dir) = make_db();
        db.register_machine("web-01", "active").unwrap();
        db.set_machine_tags("web-01", &["old-tag".to_string()])
            .unwrap();
        db.set_machine_tags("web-01", &["new-tag".to_string()])
            .unwrap();
        let tags = db.get_machine_tags("web-01").unwrap();
        assert_eq!(tags, vec!["new-tag".to_string()]);
    }

    #[test]
    fn test_get_machines_by_tags_and_logic() {
        let (db, _dir) = make_db();
        db.register_machine("web-01", "active").unwrap();
        db.register_machine("web-02", "active").unwrap();
        db.register_machine("db-01", "active").unwrap();
        db.set_machine_tags("web-01", &["web".to_string(), "production".to_string()])
            .unwrap();
        db.set_machine_tags("web-02", &["web".to_string(), "staging".to_string()])
            .unwrap();
        db.set_machine_tags("db-01", &["db".to_string(), "production".to_string()])
            .unwrap();
        let machines = db
            .get_machines_by_tags(&["web".to_string(), "production".to_string()])
            .unwrap();
        assert_eq!(machines, vec!["web-01".to_string()]);
    }

    #[test]
    fn test_remove_machine_tag() {
        let (db, _dir) = make_db();
        db.register_machine("web-01", "active").unwrap();
        db.set_machine_tags("web-01", &["web".to_string(), "production".to_string()])
            .unwrap();
        db.remove_machine_tag("web-01", "web").unwrap();
        let tags = db.get_machine_tags("web-01").unwrap();
        assert_eq!(tags, vec!["production".to_string()]);
    }

    #[test]
    fn test_get_machines_by_tags_empty() {
        let (db, _dir) = make_db();
        let machines = db.get_machines_by_tags(&[]).unwrap();
        assert!(machines.is_empty());
    }

    #[test]
    fn test_create_and_get_rollout() {
        let (db, _dir) = make_db();
        let id = db
            .create_rollout(
                "roll-1",
                "/nix/store/abc123",
                Some("http://cache.example.com"),
                "rolling",
                "[1, 2]",
                "1",
                "pause",
                300,
                Some("web,production"),
                None,
                Some("/nix/store/old"),
                "apikey:deploy",
            )
            .unwrap();
        assert_eq!(id, "roll-1");

        let rollout = db.get_rollout("roll-1").unwrap().unwrap();
        assert_eq!(rollout.generation_hash, "/nix/store/abc123");
        assert_eq!(
            rollout.cache_url,
            Some("http://cache.example.com".to_string())
        );
        assert_eq!(rollout.strategy, "rolling");
        assert_eq!(rollout.batch_sizes, "[1, 2]");
        assert_eq!(rollout.failure_threshold, "1");
        assert_eq!(rollout.on_failure, "pause");
        assert_eq!(rollout.health_timeout, 300);
        assert_eq!(rollout.status, "created");
        assert_eq!(rollout.target_tags, Some("web,production".to_string()));
        assert!(rollout.target_hosts.is_none());
        assert_eq!(
            rollout.previous_generation,
            Some("/nix/store/old".to_string())
        );
        assert_eq!(rollout.created_by, "apikey:deploy");
    }

    #[test]
    fn test_get_rollout_missing() {
        let (db, _dir) = make_db();
        let rollout = db.get_rollout("nonexistent").unwrap();
        assert!(rollout.is_none());
    }

    #[test]
    fn test_create_and_get_rollout_batches() {
        let (db, _dir) = make_db();
        db.create_rollout(
            "roll-1",
            "/nix/store/abc",
            None,
            "rolling",
            "[1]",
            "1",
            "pause",
            300,
            None,
            None,
            None,
            "admin",
        )
        .unwrap();

        db.create_rollout_batch("batch-1", "roll-1", 0, r#"["web-01"]"#)
            .unwrap();
        db.create_rollout_batch("batch-2", "roll-1", 1, r#"["web-02","web-03"]"#)
            .unwrap();

        let batches = db.get_rollout_batches("roll-1").unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].batch_index, 0);
        assert_eq!(batches[0].machine_ids, r#"["web-01"]"#);
        assert_eq!(batches[1].batch_index, 1);
        assert_eq!(batches[1].status, "pending");
    }

    #[test]
    fn test_update_rollout_status() {
        let (db, _dir) = make_db();
        db.create_rollout(
            "roll-1",
            "/nix/store/abc",
            None,
            "rolling",
            "[1]",
            "1",
            "pause",
            300,
            None,
            None,
            None,
            "admin",
        )
        .unwrap();

        let updated = db.update_rollout_status("roll-1", "running").unwrap();
        assert!(updated);

        let rollout = db.get_rollout("roll-1").unwrap().unwrap();
        assert_eq!(rollout.status, "running");

        let not_updated = db.update_rollout_status("nonexistent", "running").unwrap();
        assert!(!not_updated);
    }

    #[test]
    fn test_list_rollouts_by_status() {
        let (db, _dir) = make_db();
        db.create_rollout(
            "roll-1",
            "/nix/store/abc",
            None,
            "rolling",
            "[1]",
            "1",
            "pause",
            300,
            None,
            None,
            None,
            "admin",
        )
        .unwrap();
        db.create_rollout(
            "roll-2",
            "/nix/store/def",
            None,
            "rolling",
            "[1]",
            "1",
            "pause",
            300,
            None,
            None,
            None,
            "admin",
        )
        .unwrap();
        db.update_rollout_status("roll-2", "running").unwrap();

        let all = db.list_rollouts_by_status(None, 100).unwrap();
        assert_eq!(all.len(), 2);

        let created = db.list_rollouts_by_status(Some("created"), 100).unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].id, "roll-1");

        let running = db.list_rollouts_by_status(Some("running"), 100).unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, "roll-2");
    }

    #[test]
    fn test_update_batch_status() {
        let (db, _dir) = make_db();
        db.create_rollout(
            "roll-1",
            "/nix/store/abc",
            None,
            "rolling",
            "[1]",
            "1",
            "pause",
            300,
            None,
            None,
            None,
            "admin",
        )
        .unwrap();
        db.create_rollout_batch("batch-1", "roll-1", 0, r#"["web-01"]"#)
            .unwrap();

        db.update_batch_status("batch-1", "deploying").unwrap();
        let batches = db.get_rollout_batches("roll-1").unwrap();
        assert_eq!(batches[0].status, "deploying");
        assert!(batches[0].started_at.is_some());
        assert!(batches[0].completed_at.is_none());

        db.update_batch_status("batch-1", "succeeded").unwrap();
        let batches = db.get_rollout_batches("roll-1").unwrap();
        assert_eq!(batches[0].status, "succeeded");
        assert!(batches[0].completed_at.is_some());
    }

    #[test]
    fn test_insert_and_get_health_reports() {
        let (db, _dir) = make_db();
        db.insert_health_report("web-01", r#"{"disk": true}"#, true)
            .unwrap();
        db.insert_health_report("web-01", r#"{"disk": false}"#, false)
            .unwrap();

        let reports = db
            .get_health_reports_since("web-01", "2000-01-01 00:00:00")
            .unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].machine_id, "web-01");
        // Most recent first
        assert!(!reports[0].all_passed);
        assert!(reports[1].all_passed);
    }

    #[test]
    fn test_cleanup_old_health_reports() {
        let (db, _dir) = make_db();
        // Insert reports with an explicitly old timestamp
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO health_reports (machine_id, results, all_passed, received_at)
                 VALUES (?1, ?2, ?3, datetime('now', '-2 hours'))",
                rusqlite::params!["web-01", r#"{"disk": true}"#, 1],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO health_reports (machine_id, results, all_passed, received_at)
                 VALUES (?1, ?2, ?3, datetime('now', '-2 hours'))",
                rusqlite::params!["web-01", r#"{"disk": true}"#, 1],
            )
            .unwrap();
        }

        // Retain only last 1 hour — should delete both 2-hour-old reports
        let deleted = db.cleanup_old_health_reports(1).unwrap();
        assert_eq!(deleted, 2);

        let reports = db
            .get_health_reports_since("web-01", "2000-01-01 00:00:00")
            .unwrap();
        assert!(reports.is_empty());
    }

    #[test]
    fn test_machine_in_active_rollout_positive() {
        let (db, _dir) = make_db();
        db.create_rollout(
            "roll-1",
            "/nix/store/abc",
            None,
            "rolling",
            "[1]",
            "1",
            "pause",
            300,
            None,
            None,
            None,
            "admin",
        )
        .unwrap();
        db.create_rollout_batch("batch-1", "roll-1", 0, r#"["web-01","web-02"]"#)
            .unwrap();

        // Status is "created" (active)
        let result = db.machine_in_active_rollout("web-01").unwrap();
        assert_eq!(result, Some("roll-1".to_string()));
    }

    #[test]
    fn test_machine_in_active_rollout_negative() {
        let (db, _dir) = make_db();
        db.create_rollout(
            "roll-1",
            "/nix/store/abc",
            None,
            "rolling",
            "[1]",
            "1",
            "pause",
            300,
            None,
            None,
            None,
            "admin",
        )
        .unwrap();
        db.create_rollout_batch("batch-1", "roll-1", 0, r#"["web-01"]"#)
            .unwrap();
        db.update_rollout_status("roll-1", "succeeded").unwrap();

        // Rollout is succeeded (not active)
        let result = db.machine_in_active_rollout("web-01").unwrap();
        assert!(result.is_none());

        // Machine not in any rollout
        let result = db.machine_in_active_rollout("dev-99").unwrap();
        assert!(result.is_none());
    }
}
