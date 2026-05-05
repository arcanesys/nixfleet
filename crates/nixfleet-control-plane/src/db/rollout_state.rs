//! Per-host soak markers + state machine (soft state); agent-attested timestamps recover loss.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::state::{HealthyMarker, HostRolloutState, PendingConfirmState};

pub struct RolloutState<'a> {
    pub(super) conn: &'a Mutex<Connection>,
}

/// Single-source-of-truth host_rollout_state transition. Takes a
/// `&Connection` so it composes inside a `Transaction` (rusqlite
/// `Transaction` derefs to `Connection`) — used both by
/// `RolloutState::transition_host_state` (lock-and-delegate) and by
/// `host_dispatch_state::record_confirmed_dispatch_with_healthy_marker`'s
/// atomic txn. One SQL definition for both paths.
///
/// `expected_from = Some(prev)` is the state-machine guard — concurrent
/// reconcilers can't both flip `Failed → Reverted`; the second UPDATE
/// is a no-op (returns 0). `None` upserts unconditionally.
pub(super) fn transition_host_state_inner(
    conn: &Connection,
    hostname: &str,
    rollout_id: &str,
    new_state: HostRolloutState,
    marker: HealthyMarker,
    expected_from: Option<HostRolloutState>,
) -> Result<usize> {
    let new_state_str = new_state.as_db_str();
    // GOTCHA: NULL marker_bind + COALESCE preserves the existing column on Untouched (writing NULL would clobber).
    let marker_bind: Option<String> = match marker {
        HealthyMarker::Set(ts) => Some(ts.to_rfc3339()),
        HealthyMarker::Untouched => None,
    };

    let n = match expected_from {
        None => conn
            .execute(
                "INSERT INTO host_rollout_state(rollout_id, hostname,
                                                host_state,
                                                last_healthy_since,
                                                updated_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))
                 ON CONFLICT(rollout_id, hostname) DO UPDATE SET
                   host_state = excluded.host_state,
                   last_healthy_since = COALESCE(
                       excluded.last_healthy_since,
                       host_rollout_state.last_healthy_since),
                   updated_at = datetime('now')",
                params![rollout_id, hostname, new_state_str, marker_bind],
            )
            .context("upsert host_rollout_state")?,
        Some(prev) => conn
            .execute(
                "UPDATE host_rollout_state
                 SET host_state = ?3,
                     last_healthy_since = COALESCE(?4, last_healthy_since),
                     updated_at = datetime('now')
                 WHERE rollout_id = ?1 AND hostname = ?2
                   AND host_state = ?5",
                params![
                    rollout_id,
                    hostname,
                    new_state_str,
                    marker_bind,
                    prev.as_db_str()
                ],
            )
            .context("guarded transition host_rollout_state")?,
    };

    Ok(n)
}

impl RolloutState<'_> {
    /// Lock-and-delegate wrapper around `transition_host_state_inner`.
    /// The inner function holds the canonical UPSERT SQL and is shared
    /// with the orphan-confirm atomic txn — both paths write through one
    /// definition.
    pub fn transition_host_state(
        &self,
        hostname: &str,
        rollout_id: &str,
        new_state: HostRolloutState,
        marker: HealthyMarker,
        expected_from: Option<HostRolloutState>,
    ) -> Result<usize> {
        let guard = super::lock_conn(self.conn)?;
        transition_host_state_inner(&guard, hostname, rollout_id, new_state, marker, expected_from)
    }

    /// Bulk-transition every `Soaked` host of `rollout_id` to `Converged`.
    /// Called by `apply_actions` when the reconciler emits `ConvergeRollout`
    /// — the rollout has terminally completed (all waves soaked, last wave
    /// reached) so the per-host state machine settles at Converged.
    ///
    /// Idempotent (only matches Soaked rows); subsequent calls return 0.
    /// Other states (Failed, Reverted, Healthy, ConfirmWindow, etc.) are
    /// untouched — those represent un-completed work the reconciler must
    /// still resolve, and stamping them Converged would lose information.
    pub fn mark_rollout_hosts_converged(&self, rollout_id: &str) -> Result<usize> {
        let guard = super::lock_conn(self.conn)?;
        let n = guard
            .execute(
                "UPDATE host_rollout_state
                 SET host_state = 'Converged',
                     updated_at = datetime('now')
                 WHERE rollout_id = ?1
                   AND host_state = 'Soaked'",
                params![rollout_id],
            )
            .context("mark_rollout_hosts_converged: Soaked → Converged sweep")?;
        Ok(n)
    }

    /// GOTCHA: nulls only `last_healthy_since` — `host_state` is left for the
    /// reconciler. The soak timer must restart on next Healthy attestation.
    pub fn clear_healthy_marker(&self, hostname: &str, rollout_id: &str) -> Result<usize> {
        let guard = super::lock_conn(self.conn)?;
        let n = guard
            .execute(
                "UPDATE host_rollout_state
                 SET last_healthy_since = NULL,
                     updated_at = datetime('now')
                 WHERE rollout_id = ?1 AND hostname = ?2
                   AND last_healthy_since IS NOT NULL",
                params![rollout_id, hostname],
            )
            .context("clear host_rollout_state.last_healthy_since")?;
        Ok(n)
    }

    /// Row absent → `Ok(None)`. A real DB error (lock poisoned, schema drift,
    /// I/O) propagates as `Err` so the caller can warn rather than silently
    /// rendering "no rollout state".
    pub fn host_state(&self, hostname: &str, rollout_id: &str) -> Result<Option<String>> {
        let guard = super::lock_conn(self.conn)?;
        match guard.query_row(
            "SELECT host_state FROM host_rollout_state
             WHERE rollout_id = ?1 AND hostname = ?2",
            params![rollout_id, hostname],
            |row| row.get::<_, String>(0),
        ) {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e).context("host_rollout_state lookup"),
        }
    }

    /// Existing row authoritative over re-attestation; recovery skips when row exists.
    pub fn host_rollout_state_exists(&self, hostname: &str, rollout_id: &str) -> Result<bool> {
        let guard = super::lock_conn(self.conn)?;
        let n: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM host_rollout_state
                 WHERE rollout_id = ?1 AND hostname = ?2",
                params![rollout_id, hostname],
                |row| row.get(0),
            )
            .context("count host_rollout_state")?;
        Ok(n > 0)
    }

    /// Healthy hosts and entry timestamp; excludes NULL markers.
    pub fn host_soak_state_for_rollout(
        &self,
        rollout_id: &str,
    ) -> Result<HashMap<String, DateTime<Utc>>> {
        let guard = super::lock_conn(self.conn)?;
        let mut stmt = guard.prepare(
            "SELECT hostname, last_healthy_since
             FROM host_rollout_state
             WHERE rollout_id = ?1
               AND last_healthy_since IS NOT NULL",
        )?;
        let rows = stmt
            .query_map(params![rollout_id], |row| {
                let hostname: String = row.get(0)?;
                let ts: String = row.get(1)?;
                Ok((hostname, ts))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut out = HashMap::with_capacity(rows.len());
        for (hostname, ts) in rows {
            let parsed = ts
                .parse::<DateTime<Utc>>()
                .with_context(|| format!("parse last_healthy_since for {hostname}"))?;
            out.insert(hostname, parsed);
        }
        Ok(out)
    }

    /// (rollout_id, target_closure_hash) for currently-Healthy rollouts of this host.
    pub fn healthy_rollouts_for_host(&self, hostname: &str) -> Result<Vec<(String, String)>> {
        let guard = super::lock_conn(self.conn)?;
        let mut stmt = guard.prepare(
            "SELECT hrs.rollout_id, hds.target_closure_hash
             FROM host_rollout_state hrs
             JOIN host_dispatch_state hds
               ON hds.hostname = hrs.hostname
              AND hds.rollout_id = hrs.rollout_id
             WHERE hrs.hostname = ?1
               AND hrs.last_healthy_since IS NOT NULL
               AND hds.state = ?2",
        )?;
        let rows = stmt
            .query_map(
                params![hostname, PendingConfirmState::Confirmed.as_db_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// (rollout_id, target_channel_ref) for Failed rollouts of this host.
    pub fn failed_rollouts_for_host(&self, hostname: &str) -> Result<Vec<(String, String)>> {
        let guard = super::lock_conn(self.conn)?;
        let mut stmt = guard.prepare(
            "SELECT hrs.rollout_id, hds.target_channel_ref
             FROM host_rollout_state hrs
             JOIN host_dispatch_state hds
               ON hds.hostname = hrs.hostname
              AND hds.rollout_id = hrs.rollout_id
             WHERE hrs.hostname = ?1
               AND hrs.host_state = ?2",
        )?;
        let rows = stmt
            .query_map(
                params![hostname, HostRolloutState::Failed.as_db_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::{dispatch_insert, fresh_db, mark_healthy};
    use crate::state::{HealthyMarker, HostRolloutState};
    use chrono::Utc;

    #[test]
    fn transition_to_healthy_round_trips() {
        let db = fresh_db();
        let now = Utc::now();
        mark_healthy(&db, "test-host", "stable@abc12345", now);
        let map = db
            .rollout_state()
            .host_soak_state_for_rollout("stable@abc12345")
            .unwrap();
        assert_eq!(map.len(), 1, "expected one Healthy host: {map:?}");
        let stored = map.get("test-host").expect("test-host present");
        assert_eq!(stored.timestamp(), now.timestamp());
    }

    #[test]
    fn transition_to_healthy_upserts_timestamp() {
        let db = fresh_db();
        let t1 = Utc::now() - chrono::Duration::seconds(120);
        let t2 = Utc::now();
        mark_healthy(&db, "test-host", "stable@r1", t1);
        mark_healthy(&db, "test-host", "stable@r1", t2);
        let map = db
            .rollout_state()
            .host_soak_state_for_rollout("stable@r1")
            .unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(
            map["test-host"].timestamp(),
            t2.timestamp(),
            "second Healthy upsert must overwrite first"
        );
    }

    #[test]
    fn clear_healthy_marker_nulls_timestamp() {
        let db = fresh_db();
        mark_healthy(&db, "test-host", "stable@r1", Utc::now());
        let n = db
            .rollout_state()
            .clear_healthy_marker("test-host", "stable@r1")
            .unwrap();
        assert_eq!(n, 1);
        let map = db
            .rollout_state()
            .host_soak_state_for_rollout("stable@r1")
            .unwrap();
        assert!(
            map.is_empty(),
            "cleared host must drop out of soak state: {map:?}"
        );
    }

    #[test]
    fn clear_healthy_marker_is_noop_when_already_clear() {
        let db = fresh_db();
        let n = db
            .rollout_state()
            .clear_healthy_marker("test-host", "stable@r1")
            .unwrap();
        assert_eq!(n, 0, "clear on missing row is no-op");
        mark_healthy(&db, "test-host", "stable@r1", Utc::now());
        assert_eq!(
            db.rollout_state()
                .clear_healthy_marker("test-host", "stable@r1")
                .unwrap(),
            1
        );
        assert_eq!(
            db.rollout_state()
                .clear_healthy_marker("test-host", "stable@r1")
                .unwrap(),
            0
        );
    }

    #[test]
    fn host_soak_state_scopes_to_rollout() {
        let db = fresh_db();
        let now = Utc::now();
        mark_healthy(&db, "ohm", "stable@r1", now);
        mark_healthy(&db, "krach", "stable@r1", now);
        mark_healthy(&db, "pixel", "edge@r2", now);

        let r1 = db
            .rollout_state()
            .host_soak_state_for_rollout("stable@r1")
            .unwrap();
        assert_eq!(r1.len(), 2);
        assert!(r1.contains_key("ohm"));
        assert!(r1.contains_key("krach"));

        let r2 = db
            .rollout_state()
            .host_soak_state_for_rollout("edge@r2")
            .unwrap();
        assert_eq!(r2.len(), 1);
        assert!(r2.contains_key("pixel"));
    }

    #[test]
    fn healthy_rollouts_for_host_joins_dispatch_state() {
        let db = fresh_db();
        let future = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "test-host",
                "stable@r1",
                "target-system-r1",
                future,
            ))
            .unwrap();
        mark_healthy(&db, "test-host", "stable@r1", Utc::now());
        let pre = db
            .rollout_state()
            .healthy_rollouts_for_host("test-host")
            .unwrap();
        assert!(
            pre.is_empty(),
            "must not surface rollouts whose operational row is still pending: {pre:?}"
        );

        let n = db.host_dispatch_state().confirm("test-host", "stable@r1").unwrap();
        assert_eq!(n, 1);
        let post = db
            .rollout_state()
            .healthy_rollouts_for_host("test-host")
            .unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(post[0].0, "stable@r1");
        assert_eq!(post[0].1, "target-system-r1");
    }

    #[test]
    fn healthy_rollouts_for_host_excludes_cleared_rows() {
        let db = fresh_db();
        let future = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "test-host",
                "stable@r1",
                "target-system-r1",
                future,
            ))
            .unwrap();
        db.host_dispatch_state().confirm("test-host", "stable@r1").unwrap();
        mark_healthy(&db, "test-host", "stable@r1", Utc::now());
        assert_eq!(
            db.rollout_state()
                .healthy_rollouts_for_host("test-host")
                .unwrap()
                .len(),
            1
        );

        db.rollout_state()
            .clear_healthy_marker("test-host", "stable@r1")
            .unwrap();
        assert!(db
            .rollout_state()
            .healthy_rollouts_for_host("test-host")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn transition_to_soaked_only_from_healthy() {
        let db = fresh_db();
        let to_soaked = |db: &super::super::Db, host: &str, rollout: &str| {
            db.rollout_state()
                .transition_host_state(
                    host,
                    rollout,
                    HostRolloutState::Soaked,
                    HealthyMarker::Untouched,
                    Some(HostRolloutState::Healthy),
                )
                .unwrap()
        };
        assert_eq!(to_soaked(&db, "ohm", "stable@r1"), 0);
        mark_healthy(&db, "ohm", "stable@r1", Utc::now());
        assert_eq!(to_soaked(&db, "ohm", "stable@r1"), 1);
        assert_eq!(to_soaked(&db, "ohm", "stable@r1"), 0);

        let future = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "target", future))
            .unwrap();
        db.host_dispatch_state().confirm("ohm", "stable@r1").unwrap();
        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(
            snap[0].host_states.get("ohm").map(String::as_str),
            Some("Soaked"),
        );
    }

    #[test]
    fn host_state_returns_none_when_row_absent() {
        let db = fresh_db();
        let got = db
            .rollout_state()
            .host_state("ghost-host", "stable@r-never")
            .expect("absent row must be Ok(None), not Err");
        assert!(got.is_none(), "no row → None, got {got:?}");
    }

    #[test]
    fn host_state_returns_some_after_transition() {
        let db = fresh_db();
        mark_healthy(&db, "ohm", "stable@r1", Utc::now());
        let got = db
            .rollout_state()
            .host_state("ohm", "stable@r1")
            .expect("present row must be Ok(Some(...))");
        assert_eq!(got.as_deref(), Some("Healthy"));
    }
}
