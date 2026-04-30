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
/// `Transaction` derefs to `Connection`) - used both by
/// `RolloutState::transition_host_state` (lock-and-delegate) and by
/// `host_dispatch_state::record_confirmed_dispatch_with_state`'s
/// atomic txn. One SQL definition for both paths.
///
/// `expected_from = Some(prev)` is the state-machine guard - concurrent
/// reconcilers can't both flip `Failed -> Reverted`; the second UPDATE
/// is a no-op (returns 0). `None` upserts unconditionally.
pub(super) fn transition_host_state_inner(
    conn: &Connection,
    hostname: &str,
    rollout_id: &str,
    new_state: HostRolloutState,
    marker: HealthyMarker,
    expected_from: Option<HostRolloutState>,
) -> Result<usize> {
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
                params![rollout_id, hostname, new_state, marker_bind],
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
                params![rollout_id, hostname, new_state, marker_bind, prev],
            )
            .context("guarded transition host_rollout_state")?,
    };

    Ok(n)
}

impl RolloutState<'_> {
    /// Lock-and-delegate wrapper around `transition_host_state_inner`.
    /// The inner function holds the canonical UPSERT SQL and is shared
    /// with the orphan-confirm atomic txn - both paths write through one
    /// definition.
    pub fn transition_host_state(
        &self,
        hostname: &str,
        rollout_id: &str,
        new_state: HostRolloutState,
        marker: HealthyMarker,
        expected_from: Option<HostRolloutState>,
    ) -> Result<usize> {
        super::read(self.conn, |c| {
            transition_host_state_inner(c, hostname, rollout_id, new_state, marker, expected_from)
        })
    }

    /// Bulk-transition every activation-complete host of `rollout_id` to
    /// `Converged`. Accepts both `Healthy` and `Soaked` as the source state.
    ///
    /// `Soaked` is the normal canary path: agent reports Healthy, the
    /// reconciler waits `wave.soak_minutes`, then emits `Action::SoakHost`
    /// (Healthy to Soaked), then `Action::ConvergeRollout` once every host
    /// in the wave is Soaked.
    ///
    /// `Healthy` is the path taken by `all-at-once` rollouts where
    /// `fleet.waves[channel]` is empty: the reconciler's `advance_rollout`
    /// finds no wave at `current_wave=0` and emits
    /// `Action::ConvergeRollout` directly, without ever queueing a
    /// `SoakHost`. Hosts that completed activation are still in
    /// `Healthy` when the rollout terminates. Including `Healthy` here
    /// closes that gap; otherwise those hosts stay in `Healthy`
    /// permanently and `is_terminal_for_ordering` (which counts only
    /// `Soaked`/`Converged`) reports the channel as still active,
    /// blocking any successor channel via `channelEdges` indefinitely.
    ///
    /// Mid-flight states (`Queued`, `Dispatched`, `Activating`,
    /// `ConfirmWindow`) and failure states (`Failed`, `Reverted`) are
    /// untouched on purpose: the first group has not reported successful
    /// activation, the second requires operator action.
    ///
    /// Idempotent.
    pub fn mark_rollout_hosts_converged(&self, rollout_id: &str) -> Result<usize> {
        super::read(self.conn, |c| {
            c.execute(
                "UPDATE host_rollout_state
                 SET host_state = 'Converged',
                     updated_at = datetime('now')
                 WHERE rollout_id = ?1
                   AND host_state IN ('Healthy', 'Soaked')",
                params![rollout_id],
            )
            .context("mark_rollout_hosts_converged: Healthy/Soaked -> Converged sweep")
        })
    }

    /// GOTCHA: nulls only `last_healthy_since` - `host_state` is left for the
    /// reconciler. The soak timer must restart on next Healthy attestation.
    pub fn clear_healthy_marker(&self, hostname: &str, rollout_id: &str) -> Result<usize> {
        super::read(self.conn, |c| {
            c.execute(
                "UPDATE host_rollout_state
                 SET last_healthy_since = NULL,
                     updated_at = datetime('now')
                 WHERE rollout_id = ?1 AND hostname = ?2
                   AND last_healthy_since IS NOT NULL",
                params![rollout_id, hostname],
            )
            .context("clear host_rollout_state.last_healthy_since")
        })
    }

    /// Row absent -> `Ok(None)`. A real DB error (lock poisoned, schema drift,
    /// I/O) propagates as `Err` so the caller can warn rather than silently
    /// rendering "no rollout state".
    pub fn host_state(&self, hostname: &str, rollout_id: &str) -> Result<Option<String>> {
        super::read(self.conn, |c| {
            match c.query_row(
                "SELECT host_state FROM host_rollout_state
                 WHERE rollout_id = ?1 AND hostname = ?2",
                params![rollout_id, hostname],
                |row| row.get::<_, String>(0),
            ) {
                Ok(s) => Ok(Some(s)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e).context("host_rollout_state lookup"),
            }
        })
    }

    /// Existing row authoritative over re-attestation; recovery skips when row exists.
    pub fn host_rollout_state_exists(&self, hostname: &str, rollout_id: &str) -> Result<bool> {
        super::read(self.conn, |c| {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM host_rollout_state
                     WHERE rollout_id = ?1 AND hostname = ?2",
                    params![rollout_id, hostname],
                    |row| row.get(0),
                )
                .context("count host_rollout_state")?;
            Ok(n > 0)
        })
    }

    /// Healthy hosts and entry timestamp; excludes NULL markers.
    pub fn host_soak_state_for_rollout(
        &self,
        rollout_id: &str,
    ) -> Result<HashMap<String, DateTime<Utc>>> {
        let rows: Vec<(String, String)> = super::read(self.conn, |c| {
            let mut stmt = c.prepare(
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
            Ok(rows)
        })?;
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
        super::read(self.conn, |c| {
            let mut stmt = c.prepare(
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
                .query_map(params![hostname, PendingConfirmState::Confirmed], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// (rollout_id, target_channel_ref) for Failed rollouts of this host.
    pub fn failed_rollouts_for_host(&self, hostname: &str) -> Result<Vec<(String, String)>> {
        super::read(self.conn, |c| {
            let mut stmt = c.prepare(
                "SELECT hrs.rollout_id, hds.target_channel_ref
                 FROM host_rollout_state hrs
                 JOIN host_dispatch_state hds
                   ON hds.hostname = hrs.hostname
                  AND hds.rollout_id = hrs.rollout_id
                 WHERE hrs.hostname = ?1
                   AND hrs.host_state = ?2",
            )?;
            let rows = stmt
                .query_map(params![hostname, HostRolloutState::Failed], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
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
        mark_healthy(&db, "host-02", "stable@r1", now);
        mark_healthy(&db, "host-01", "stable@r1", now);
        mark_healthy(&db, "host-04", "edge@r2", now);

        let r1 = db
            .rollout_state()
            .host_soak_state_for_rollout("stable@r1")
            .unwrap();
        assert_eq!(r1.len(), 2);
        assert!(r1.contains_key("host-02"));
        assert!(r1.contains_key("host-01"));

        let r2 = db
            .rollout_state()
            .host_soak_state_for_rollout("edge@r2")
            .unwrap();
        assert_eq!(r2.len(), 1);
        assert!(r2.contains_key("host-04"));
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

        let n = db
            .host_dispatch_state()
            .confirm("test-host", "stable@r1")
            .unwrap();
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
        db.host_dispatch_state()
            .confirm("test-host", "stable@r1")
            .unwrap();
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
        assert_eq!(to_soaked(&db, "host-02", "stable@r1"), 0);
        mark_healthy(&db, "host-02", "stable@r1", Utc::now());
        assert_eq!(to_soaked(&db, "host-02", "stable@r1"), 1);
        assert_eq!(to_soaked(&db, "host-02", "stable@r1"), 0);

        let future = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("host-02", "stable@r1", "target", future))
            .unwrap();
        db.host_dispatch_state()
            .confirm("host-02", "stable@r1")
            .unwrap();
        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(
            snap[0].host_states.get("host-02").map(String::as_str),
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
        assert!(got.is_none(), "no row -> None, got {got:?}");
    }

    #[test]
    fn host_state_returns_some_after_transition() {
        let db = fresh_db();
        mark_healthy(&db, "host-02", "stable@r1", Utc::now());
        let got = db
            .rollout_state()
            .host_state("host-02", "stable@r1")
            .expect("present row must be Ok(Some(...))");
        assert_eq!(got.as_deref(), Some("Healthy"));
    }

    /// Regression: `Action::ConvergeRollout` must transition `Healthy`
    /// hosts to `Converged`, not just `Soaked`. The `all-at-once` rollout
    /// strategy with `fleet.waves[channel] = []` emits ConvergeRollout
    /// directly (no SoakHost), so hosts that successfully activated stay
    /// in `Healthy` when the rollout terminates. Without this, the channel
    /// reads as "still active for ordering" and any successor channel
    /// gated by `channelEdges` stays blocked indefinitely.
    #[test]
    fn mark_rollout_hosts_converged_transitions_healthy_to_converged() {
        let db = fresh_db();
        mark_healthy(&db, "host-aa", "edge@r1", Utc::now());
        assert_eq!(
            db.rollout_state()
                .host_state("host-aa", "edge@r1")
                .unwrap()
                .as_deref(),
            Some("Healthy"),
        );
        let updated = db
            .rollout_state()
            .mark_rollout_hosts_converged("edge@r1")
            .unwrap();
        assert_eq!(updated, 1, "Healthy host must be swept to Converged");
        assert_eq!(
            db.rollout_state()
                .host_state("host-aa", "edge@r1")
                .unwrap()
                .as_deref(),
            Some("Converged"),
        );
    }

    /// The Soaked branch (canary path) must keep working. Same shape as
    /// the Healthy regression above, with one extra step to drive the host
    /// to Soaked via `transition_host_state`.
    #[test]
    fn mark_rollout_hosts_converged_transitions_soaked_to_converged() {
        let db = fresh_db();
        mark_healthy(&db, "host-soak", "stable@r1", Utc::now());
        db.rollout_state()
            .transition_host_state(
                "host-soak",
                "stable@r1",
                HostRolloutState::Soaked,
                HealthyMarker::Untouched,
                Some(HostRolloutState::Healthy),
            )
            .unwrap();
        let updated = db
            .rollout_state()
            .mark_rollout_hosts_converged("stable@r1")
            .unwrap();
        assert_eq!(updated, 1);
        assert_eq!(
            db.rollout_state()
                .host_state("host-soak", "stable@r1")
                .unwrap()
                .as_deref(),
            Some("Converged"),
        );
    }

    /// Mid-flight and failure states must NOT be swept. Walks
    /// `Dispatched` and `Failed` past the call to assert they are
    /// untouched (preventing future regressions if the SQL filter widens).
    #[test]
    fn mark_rollout_hosts_converged_leaves_non_terminal_alone() {
        let db = fresh_db();
        // Seed a Healthy row so the rollout exists, then transition it
        // to Dispatched (mid-flight) for the check.
        mark_healthy(&db, "host-mid", "stable@r2", Utc::now());
        db.rollout_state()
            .transition_host_state(
                "host-mid",
                "stable@r2",
                HostRolloutState::Dispatched,
                HealthyMarker::Untouched,
                Some(HostRolloutState::Healthy),
            )
            .unwrap();
        // Another row in Failed for the same rollout.
        mark_healthy(&db, "host-fail", "stable@r2", Utc::now());
        db.rollout_state()
            .transition_host_state(
                "host-fail",
                "stable@r2",
                HostRolloutState::Failed,
                HealthyMarker::Untouched,
                Some(HostRolloutState::Healthy),
            )
            .unwrap();
        let updated = db
            .rollout_state()
            .mark_rollout_hosts_converged("stable@r2")
            .unwrap();
        assert_eq!(
            updated, 0,
            "no Healthy/Soaked rows present; non-terminal/failure rows must not be touched",
        );
        assert_eq!(
            db.rollout_state()
                .host_state("host-mid", "stable@r2")
                .unwrap()
                .as_deref(),
            Some("Dispatched"),
        );
        assert_eq!(
            db.rollout_state()
                .host_state("host-fail", "stable@r2")
                .unwrap()
                .as_deref(),
            Some("Failed"),
        );
    }
}
