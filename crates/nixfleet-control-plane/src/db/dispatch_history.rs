//! Append-only dispatch audit (soft state); 90d retention.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::sync::Mutex;

use crate::state::TerminalState;

use super::host_dispatch_state::DispatchInsert;

#[derive(Debug, Clone)]
pub struct DispatchHistoryRow {
    pub id: i64,
    pub hostname: String,
    pub rollout_id: String,
    pub channel: String,
    pub wave: u32,
    pub target_closure_hash: String,
    pub target_channel_ref: String,
    pub dispatched_at: String,
    pub terminal_state: Option<String>,
    pub terminal_at: Option<String>,
}

pub struct DispatchHistory<'a> {
    pub(super) conn: &'a Mutex<Connection>,
}

impl DispatchHistory<'_> {
    /// Idempotent: returns 0 when no open row exists for (rollout, host).
    pub fn mark_terminal_for_rollout_host(
        &self,
        rollout_id: &str,
        hostname: &str,
        terminal: TerminalState,
        at: DateTime<Utc>,
    ) -> Result<usize> {
        super::read(self.conn, |c| {
            c.execute(
                "UPDATE dispatch_history
                 SET terminal_state = ?1, terminal_at = ?2
                 WHERE id = (
                     SELECT id FROM dispatch_history
                     WHERE rollout_id = ?3 AND hostname = ?4
                       AND terminal_state IS NULL
                     ORDER BY dispatched_at DESC, id DESC
                     LIMIT 1
                 )",
                params![terminal, at.to_rfc3339(), rollout_id, hostname],
            )
            .context("mark_terminal_for_rollout_host")
        })
    }

    /// Stamp every open row of a converged rollout with `terminal_state = 'converged'`.
    pub fn mark_rollout_converged(
        &self,
        rollout_id: &str,
        at: DateTime<Utc>,
    ) -> Result<usize> {
        super::read(self.conn, |c| {
            c.execute(
                "UPDATE dispatch_history
                 SET terminal_state = ?1, terminal_at = ?2
                 WHERE rollout_id = ?3 AND terminal_state IS NULL",
                params![TerminalState::Converged, at.to_rfc3339(), rollout_id],
            )
            .context("mark_rollout_converged")
        })
    }

    /// Drop terminal rows older than `max_age_hours`; open rows never pruned.
    pub fn prune_history(&self, max_age_hours: i64) -> Result<usize> {
        super::read(self.conn, |c| {
            c.execute(
                "DELETE FROM dispatch_history
                 WHERE terminal_state IS NOT NULL
                   AND datetime(terminal_at) < datetime('now', ?1)",
                params![format!("-{max_age_hours} hours")],
            )
            .context("prune dispatch_history")
        })
    }

    /// Wave-major, dispatched_at-ascending — operator reads top-to-bottom
    /// as the rollout progressed. Used by `/v1/rollouts/{id}/trace`.
    pub fn for_rollout(&self, rollout_id: &str) -> Result<Vec<DispatchHistoryRow>> {
        super::read(self.conn, |c| {
            let mut stmt = c.prepare(
                "SELECT id, hostname, rollout_id, channel, wave,
                        target_closure_hash, target_channel_ref,
                        dispatched_at, terminal_state, terminal_at
                 FROM dispatch_history
                 WHERE rollout_id = ?1
                 ORDER BY wave ASC, dispatched_at ASC, id ASC",
            )?;
            let rows = stmt
                .query_map(params![rollout_id], row_to_history_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Newest-first; ordering is part of the contract.
    pub fn recent_for_host(
        &self,
        hostname: &str,
        limit: usize,
    ) -> Result<Vec<DispatchHistoryRow>> {
        super::read(self.conn, |c| {
            let mut stmt = c.prepare(
                "SELECT id, hostname, rollout_id, channel, wave,
                        target_closure_hash, target_channel_ref,
                        dispatched_at, terminal_state, terminal_at
                 FROM dispatch_history
                 WHERE hostname = ?1
                 ORDER BY dispatched_at DESC, id DESC
                 LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![hostname, limit as i64], row_to_history_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }
}

pub(super) fn insert_history(conn: &Connection, row: &DispatchInsert<'_>) -> Result<i64> {
    conn.execute(
        "INSERT INTO dispatch_history(
             hostname, rollout_id, channel, wave,
             target_closure_hash, target_channel_ref
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            row.hostname,
            row.rollout_id,
            row.channel,
            row.wave,
            row.target_closure_hash,
            row.target_channel_ref,
        ],
    )
    .context("insert dispatch_history")?;
    Ok(conn.last_insert_rowid())
}

fn row_to_history_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatchHistoryRow> {
    Ok(DispatchHistoryRow {
        id: row.get(0)?,
        hostname: row.get(1)?,
        rollout_id: row.get(2)?,
        channel: row.get(3)?,
        wave: row.get(4)?,
        target_closure_hash: row.get(5)?,
        target_channel_ref: row.get(6)?,
        dispatched_at: row.get(7)?,
        terminal_state: row.get(8)?,
        terminal_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::{dispatch_insert, fresh_db};
    use crate::state::TerminalState;
    use chrono::Utc;

    #[test]
    fn for_rollout_returns_wave_major_ascending_order() {
        use crate::db::host_dispatch_state::DispatchInsert;
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        // Out-of-order wave + dispatch_at insertion; query must reorder.
        let inserts = [
            ("krach", 2u32),
            ("lab", 0u32),
            ("ohm", 1u32),
            ("aether", 0u32),
        ];
        for (host, wave) in inserts {
            db.host_dispatch_state()
                .record_dispatch(&DispatchInsert {
                    hostname: host,
                    rollout_id: "stable@trace1",
                    channel: "stable",
                    wave,
                    target_closure_hash: "system-r1",
                    target_channel_ref: "stable@trace1",
                    confirm_deadline: deadline,
                })
                .unwrap();
        }
        let trace = db
            .dispatch_history()
            .for_rollout("stable@trace1")
            .unwrap();
        let waves: Vec<u32> = trace.iter().map(|r| r.wave).collect();
        assert_eq!(
            waves,
            vec![0, 0, 1, 2],
            "wave-major ascending order required: {waves:?}",
        );
    }

    #[test]
    fn for_rollout_returns_empty_when_unknown() {
        let db = fresh_db();
        let trace = db.dispatch_history().for_rollout("absent").unwrap();
        assert!(trace.is_empty());
    }

    #[test]
    fn append_only_grows_on_each_dispatch() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        for rollout in ["stable@r1", "stable@r2", "stable@r3"] {
            db.host_dispatch_state()
                .record_dispatch(&dispatch_insert("ohm", rollout, "system", deadline))
                .unwrap();
        }
        let history = db.dispatch_history().recent_for_host("ohm", 10).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].rollout_id, "stable@r3");
        assert_eq!(history[2].rollout_id, "stable@r1");
    }

    #[test]
    fn mark_terminal_for_rollout_host_idempotent() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system", deadline))
            .unwrap();
        let now = Utc::now();
        let n = db
            .dispatch_history()
            .mark_terminal_for_rollout_host("stable@r1", "ohm", TerminalState::RolledBack, now)
            .unwrap();
        assert_eq!(n, 1);
        let n = db
            .dispatch_history()
            .mark_terminal_for_rollout_host("stable@r1", "ohm", TerminalState::RolledBack, now)
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn mark_rollout_converged_stamps_all_open_rows() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        for host in ["ohm", "krach"] {
            db.host_dispatch_state()
                .record_dispatch(&dispatch_insert(host, "stable@r1", "system", deadline))
                .unwrap();
        }
        let n = db
            .dispatch_history()
            .mark_rollout_converged("stable@r1", Utc::now())
            .unwrap();
        assert_eq!(n, 2);
        for host in ["ohm", "krach"] {
            let rows = db.dispatch_history().recent_for_host(host, 10).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].terminal_state.as_deref(), Some("converged"));
            assert!(rows[0].terminal_at.is_some());
        }
    }

    #[test]
    fn mark_rollout_converged_skips_terminal_rows() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("krach", "stable@r1", "system", deadline))
            .unwrap();
        db.dispatch_history()
            .mark_terminal_for_rollout_host(
                "stable@r1",
                "krach",
                TerminalState::RolledBack,
                Utc::now(),
            )
            .unwrap();
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system", deadline))
            .unwrap();
        let n = db
            .dispatch_history()
            .mark_rollout_converged("stable@r1", Utc::now())
            .unwrap();
        assert_eq!(n, 1, "only ohm's open row flips; krach already terminal");
        let krach = db.dispatch_history().recent_for_host("krach", 1).unwrap();
        assert_eq!(krach[0].terminal_state.as_deref(), Some("rolled-back"));
    }

    #[test]
    fn prune_history_drops_old_terminal_rows_only() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@old", "system", deadline))
            .unwrap();
        let old_terminal_at = Utc::now() - chrono::Duration::days(200);
        db.dispatch_history()
            .mark_terminal_for_rollout_host(
                "stable@old",
                "ohm",
                TerminalState::RolledBack,
                old_terminal_at,
            )
            .unwrap();
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("krach", "stable@live", "system", deadline))
            .unwrap();
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("pixel", "stable@recent", "system", deadline))
            .unwrap();
        db.dispatch_history()
            .mark_terminal_for_rollout_host(
                "stable@recent",
                "pixel",
                TerminalState::Converged,
                Utc::now(),
            )
            .unwrap();

        let pruned = db.dispatch_history().prune_history(24 * 90).unwrap();
        assert_eq!(pruned, 1);
        assert!(db
            .dispatch_history()
            .recent_for_host("ohm", 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            db.dispatch_history().recent_for_host("krach", 10).unwrap().len(),
            1,
            "open row must not be pruned",
        );
        assert_eq!(
            db.dispatch_history().recent_for_host("pixel", 10).unwrap().len(),
            1,
            "fresh terminal row must not be pruned",
        );
    }
}
