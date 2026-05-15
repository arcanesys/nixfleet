//! CP-side anti-thrash quarantine. Trusted-input only: rows are written
//! by `server::reconcile::sweep_soaked_health_failures` based on CP-
//! observed probe state. Agent `ClosureQuarantined` reports are NOT
//! inserted here (they are unsigned and would let a compromised host
//! DoS the fleet by quarantining arbitrary SHAs).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub struct QuarantinedClosures<'a> {
    pub(super) conn: &'a Mutex<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineRow {
    pub channel: String,
    pub closure_hash: String,
    pub reason: String,
    pub quarantined_at: DateTime<Utc>,
    pub cleared_at: Option<DateTime<Utc>>,
}

impl<'a> QuarantinedClosures<'a> {
    /// Idempotent under (channel, closure_hash) primary key. Re-quarantining
    /// the same closure refreshes `reason` and reopens the entry (clears
    /// `cleared_at`) so a stale operator-driven clear can't mask a recurring
    /// failure.
    pub fn insert(&self, channel: &str, closure_hash: &str, reason: &str) -> Result<()> {
        let conn = self.conn.lock().expect("poisoned");
        conn.execute(
            "INSERT INTO quarantined_closures(channel, closure_hash, reason)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(channel, closure_hash) DO UPDATE SET
                 reason = excluded.reason,
                 cleared_at = NULL,
                 quarantined_at = datetime('now')",
            params![channel, closure_hash, reason],
        )
        .context("insert quarantine")?;
        Ok(())
    }

    /// Operator override (`nixfleet quarantine clear`) and auto-clear path.
    /// Returns rows affected; 0 means no active entry to clear.
    pub fn clear(&self, channel: &str, closure_hash: &str) -> Result<usize> {
        let conn = self.conn.lock().expect("poisoned");
        let n = conn
            .execute(
                "UPDATE quarantined_closures
                 SET cleared_at = datetime('now')
                 WHERE channel = ?1 AND closure_hash = ?2 AND cleared_at IS NULL",
                params![channel, closure_hash],
            )
            .context("clear quarantine")?;
        Ok(n)
    }

    /// Clears every active entry on a channel whose `closure_hash` does
    /// NOT match `current_declared`. Used by the projection on every tick
    /// to keep stale entries from outliving the operator pushing past the
    /// bad SHA. Returns count of rows cleared.
    pub fn clear_stale_for_channel(&self, channel: &str, current_declared: &str) -> Result<usize> {
        let conn = self.conn.lock().expect("poisoned");
        let n = conn
            .execute(
                "UPDATE quarantined_closures
                 SET cleared_at = datetime('now')
                 WHERE channel = ?1
                   AND closure_hash != ?2
                   AND cleared_at IS NULL",
                params![channel, current_declared],
            )
            .context("clear stale quarantines")?;
        Ok(n)
    }

    /// Active set keyed by channel -> {closure_hash}. The reconciler
    /// reads this on every tick via `Observed.quarantined_closures` to
    /// gate `DispatchHost`.
    pub fn active_by_channel(&self) -> Result<HashMap<String, HashSet<String>>> {
        let conn = self.conn.lock().expect("poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT channel, closure_hash
                 FROM quarantined_closures
                 WHERE cleared_at IS NULL",
            )
            .context("prepare active_by_channel")?;
        let mut rows = stmt.query([]).context("query active_by_channel")?;
        let mut out: HashMap<String, HashSet<String>> = HashMap::new();
        while let Some(row) = rows.next().context("step active_by_channel")? {
            let channel: String = row.get(0)?;
            let closure_hash: String = row.get(1)?;
            out.entry(channel).or_default().insert(closure_hash);
        }
        Ok(out)
    }

    /// Operator-surface listing (CLI `nixfleet quarantine list`).
    /// Includes cleared entries within the window so the operator can
    /// audit recent activity; gates use `active_by_channel` instead.
    pub fn list_active(&self) -> Result<Vec<QuarantineRow>> {
        let conn = self.conn.lock().expect("poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT channel, closure_hash, reason, quarantined_at, cleared_at
                 FROM quarantined_closures
                 WHERE cleared_at IS NULL
                 ORDER BY quarantined_at DESC",
            )
            .context("prepare list_active")?;
        let mut rows = stmt.query([]).context("query list_active")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().context("step list_active")? {
            let channel: String = row.get(0)?;
            let closure_hash: String = row.get(1)?;
            let reason: String = row.get(2)?;
            let qat: String = row.get(3)?;
            let cat: Option<String> = row.get(4)?;
            out.push(QuarantineRow {
                channel,
                closure_hash,
                reason,
                quarantined_at: parse_sqlite_ts(&qat)?,
                cleared_at: cat.as_deref().map(parse_sqlite_ts).transpose()?,
            });
        }
        Ok(out)
    }
}

fn parse_sqlite_ts(s: &str) -> Result<DateTime<Utc>> {
    // SQLite `datetime('now')` emits `YYYY-MM-DD HH:MM:SS` (no TZ); chrono
    // parses it via NaiveDateTime then we assert UTC.
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|n| n.and_utc())
        .with_context(|| format!("parse sqlite ts {s}"))
}

#[cfg(test)]
mod tests {
    use crate::db::Db;

    fn fresh_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.migrate().unwrap();
        db
    }

    #[test]
    fn insert_and_list_active() {
        let db = fresh_db();
        let q = db.quarantined_closures();
        q.insert("stable", "abc123", "probe failure").unwrap();
        let rows = q.list_active().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].channel, "stable");
        assert_eq!(rows[0].closure_hash, "abc123");
        assert_eq!(rows[0].reason, "probe failure");
        assert!(rows[0].cleared_at.is_none());
    }

    #[test]
    fn idempotent_insert_refreshes_reason() {
        let db = fresh_db();
        let q = db.quarantined_closures();
        q.insert("stable", "abc", "first reason").unwrap();
        q.insert("stable", "abc", "second reason").unwrap();
        let rows = q.list_active().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].reason, "second reason");
    }

    #[test]
    fn reinsert_after_clear_reopens() {
        let db = fresh_db();
        let q = db.quarantined_closures();
        q.insert("stable", "abc", "r1").unwrap();
        q.clear("stable", "abc").unwrap();
        assert!(q.list_active().unwrap().is_empty());
        q.insert("stable", "abc", "r2").unwrap();
        let rows = q.list_active().unwrap();
        assert_eq!(rows.len(), 1, "re-insert must reopen the entry");
    }

    #[test]
    fn clear_stale_for_channel_leaves_current_alone() {
        let db = fresh_db();
        let q = db.quarantined_closures();
        q.insert("stable", "abc", "r1").unwrap();
        q.insert("stable", "xyz", "r2").unwrap();
        q.insert("edge", "abc", "r3").unwrap();
        // Channel "stable" advances to xyz: abc on stable should clear,
        // xyz on stable stays, abc on edge stays.
        let cleared = q.clear_stale_for_channel("stable", "xyz").unwrap();
        assert_eq!(cleared, 1);
        let active = q.active_by_channel().unwrap();
        assert_eq!(active["stable"].len(), 1);
        assert!(active["stable"].contains("xyz"));
        assert!(active["edge"].contains("abc"));
    }

    #[test]
    fn active_by_channel_groups_correctly() {
        let db = fresh_db();
        let q = db.quarantined_closures();
        q.insert("stable", "abc", "r").unwrap();
        q.insert("stable", "def", "r").unwrap();
        q.insert("edge", "ghi", "r").unwrap();
        let by_chan = q.active_by_channel().unwrap();
        assert_eq!(by_chan["stable"].len(), 2);
        assert_eq!(by_chan["edge"].len(), 1);
    }
}
