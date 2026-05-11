//! Operational dispatch row, one per host (soft state). Orphan-confirm
//! recovers from loss. LOADBEARING: paired with append-only `dispatch_history`;
//! UPSERT replaces the operational row per new dispatch, audit trail lives in
//! history.
//!
//! `confirm_deadline` filter conventions across readers:
//!
//! | caller | filter | intent |
//! |--------|--------|--------|
//! | `confirm()` | `(state='pending' AND deadline > now) OR state='deferred-pending-reboot'` | timely + post-reboot confirms |
//! | `pending_deadlines()` | `state='pending' AND deadline < now` | timer sweeps expired |
//! | `pending_dispatch_exists()` | `state='pending'` | in-flight check; deferred excluded so CP can re-target |
//! | `active_rollouts_snapshot()` | `state IN ('pending','confirmed','deferred-pending-reboot')` | UI/gate view |
//! | `mark_deferred()` | `state='pending'` | only Pending rows transition to DeferredPendingReboot |
//!
//! FOOTGUN: never compare timestamps without `datetime(...)` - naked string
//! compare ranks `'T'` after `' '` and breaks the timer.
//!
//! `DeferredPendingReboot` is the human-paced state for switch-inhibitor
//! carve-out: agent ran `nix-env --set` but skipped live activation. The 360s
//! rollback timer must NOT sweep these; confirm accepts post-reboot confirms
//! without the deadline check.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::state::{HostRolloutState, PendingConfirmState, TerminalState};

#[derive(Debug, Clone)]
pub struct DispatchInsert<'a> {
    pub hostname: &'a str,
    pub rollout_id: &'a str,
    /// Persisted explicitly: rolloutIds are content hashes that don't encode the channel.
    pub channel: &'a str,
    pub wave: u32,
    pub target_closure_hash: &'a str,
    pub target_channel_ref: &'a str,
    pub confirm_deadline: DateTime<Utc>,
}

/// Joined snapshot for observed-state projection; terminal rows filtered out.
#[derive(Debug, Clone)]
pub struct RolloutDbSnapshot {
    pub rollout_id: String,
    pub channel: String,
    pub target_closure_hash: String,
    pub target_channel_ref: String,
    /// `host_rollout_state` wins when present; otherwise derived from operational state.
    pub host_states: HashMap<String, String>,
    /// Wave the host was dispatched into. Constant once dispatched;
    /// distinct from rollout-level `current_wave` which advances on PromoteWave.
    pub host_waves: HashMap<String, u32>,
    /// Excludes hosts not currently Healthy.
    pub last_healthy_since: HashMap<String, DateTime<Utc>>,
    /// Persisted wave index advanced by `apply_actions` on `PromoteWave`.
    /// Defaults to 0 for rollouts not yet in the rollouts table.
    pub current_wave: u32,
    /// `Some(t)` once terminal (ConvergeRollout or orphan-sweep). Plumbed to
    /// `Rollout::terminal_at` so `advance_rollout` short-circuits and gates
    /// can distinguish "predecessor converged" from "predecessor unknown".
    #[doc(hidden)]
    pub terminal_at: Option<DateTime<Utc>>,
}

/// `(hostname, rollout_id, wave, target_closure_hash)` for rows with a passed deadline.
pub type ExpiredDispatch = (String, String, u32, String);

pub struct HostDispatchState<'a> {
    pub(super) conn: &'a Mutex<Connection>,
}

impl HostDispatchState<'_> {
    /// LOADBEARING: operational UPSERT + history append in one txn - partial
    /// failure leaves audit trail aligned with operational state.
    pub fn record_dispatch(&self, row: &DispatchInsert<'_>) -> Result<()> {
        super::txn(self.conn, "dispatch", |t| {
            upsert_operational(t, row, PendingConfirmState::Pending, None)?;
            super::dispatch_history::insert_history(t, row)?;
            Ok(())
        })
    }

    /// Atomic confirm: operational + history + host_rollout_state in one txn.
    /// Partial commit would leave hrs NULL → soak timer never fires → host
    /// stuck Healthy → wave promotion blocks. `target_state` = `Healthy` for
    /// freshly-confirmed (enters soak), `Converged` for converged-at-dispatch
    /// (host was already on target). `healthy_since` may differ from
    /// `confirmed_at` for attestation recovery (anchor on earlier moment).
    #[allow(clippy::too_many_arguments)]
    pub fn record_confirmed_dispatch_with_state(
        &self,
        hostname: &str,
        rollout_id: &str,
        channel: &str,
        wave: u32,
        target_closure_hash: &str,
        target_channel_ref: &str,
        confirmed_at: DateTime<Utc>,
        target_state: HostRolloutState,
        healthy_since: DateTime<Utc>,
    ) -> Result<()> {
        let row = DispatchInsert {
            hostname,
            rollout_id,
            channel,
            wave,
            target_closure_hash,
            target_channel_ref,
            confirm_deadline: confirmed_at,
        };
        super::txn(self.conn, "atomic-confirm", |t| {
            upsert_operational(t, &row, PendingConfirmState::Confirmed, Some(confirmed_at))?;
            super::dispatch_history::insert_history(t, &row)?;
            // Shared with RolloutState::transition_host_state via the free fn so
            // the host_rollout_state row lands in the same txn - partial commit
            // would leave hrs NULL and the soak timer never fires.
            super::rollout_state::transition_host_state_inner(
                t,
                hostname,
                rollout_id,
                target_state,
                crate::state::HealthyMarker::Set(healthy_since),
                None,
            )?;
            Ok(())
        })
    }

    /// True if the host has a `'pending'` row.
    pub fn pending_dispatch_exists(&self, hostname: &str) -> Result<bool> {
        super::read(self.conn, |c| {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM host_dispatch_state
                     WHERE hostname = ?1 AND state = ?2",
                    params![hostname, PendingConfirmState::Pending],
                    |row| row.get(0),
                )
                .context("count host_dispatch_state pending")?;
            Ok(n > 0)
        })
    }

    /// Flips → confirmed. Two acceptable source states:
    ///   - `Pending` with deadline still in the future (normal live-switch confirm)
    ///   - `DeferredPendingReboot` (post-reboot confirm; deadline is irrelevant
    ///     because the lifecycle was paused waiting for the operator's reboot)
    ///
    /// The deadline gate continues to reject late `Pending` confirms - that
    /// safety property is what ensures the rollback timer can't be raced by a
    /// stale confirm. The deferred branch explicitly opts out of the gate.
    pub fn confirm(&self, hostname: &str, rollout_id: &str) -> Result<usize> {
        super::read(self.conn, |c| {
            c.execute(
                "UPDATE host_dispatch_state
                 SET confirmed_at = datetime('now'),
                     state = ?3
                 WHERE hostname = ?1
                   AND rollout_id = ?2
                   AND (
                         (state = ?4 AND datetime(confirm_deadline) > datetime('now'))
                      OR state = ?5
                   )",
                params![
                    hostname,
                    rollout_id,
                    PendingConfirmState::Confirmed,
                    PendingConfirmState::Pending,
                    PendingConfirmState::DeferredPendingReboot,
                ],
            )
            .context("update host_dispatch_state confirmed")
        })
    }

    /// Flips pending → deferred-pending-reboot. Idempotent: only Pending rows
    /// transition. A row already in DeferredPendingReboot stays put (the agent
    /// re-posting `ActivationDeferred` for the same closure_hash should be a
    /// no-op, not a state-machine cycle). Mismatched rollout / non-Pending
    /// state returns 0.
    pub fn mark_deferred(&self, hostname: &str, rollout_id: &str) -> Result<usize> {
        super::read(self.conn, |c| {
            c.execute(
                "UPDATE host_dispatch_state
                 SET state = ?3
                 WHERE hostname = ?1
                   AND rollout_id = ?2
                   AND state = ?4",
                params![
                    hostname,
                    rollout_id,
                    PendingConfirmState::DeferredPendingReboot,
                    PendingConfirmState::Pending,
                ],
            )
            .context("update host_dispatch_state deferred-pending-reboot")
        })
    }

    /// `datetime(...)` wrapper is required: naked string compare ranks 'T' > ' ' and breaks the timer.
    pub fn pending_deadlines(&self) -> Result<Vec<ExpiredDispatch>> {
        super::read(self.conn, |c| {
            let mut stmt = c.prepare(
                "SELECT hostname, rollout_id, wave, target_closure_hash
                 FROM host_dispatch_state
                 WHERE state = ?1
                   AND datetime(confirm_deadline) < datetime('now')",
            )?;
            let rows = stmt
                .query_map(params![PendingConfirmState::Pending], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok::<_, anyhow::Error>(rows)
        })
    }

    /// Idempotent: only flips rows still in 'pending'.
    pub fn mark_rolled_back(&self, pairs: &[(String, String)]) -> Result<usize> {
        if pairs.is_empty() {
            return Ok(0);
        }
        super::txn(self.conn, "mark_rolled_back", |t| {
            let mut updated = 0usize;
            let mut stmt = t.prepare(
                "UPDATE host_dispatch_state
                 SET state = ?3
                 WHERE hostname = ?1
                   AND rollout_id = ?2
                   AND state = ?4",
            )?;
            for (hostname, rollout_id) in pairs {
                updated += stmt.execute(params![
                    hostname,
                    rollout_id,
                    PendingConfirmState::RolledBack,
                    PendingConfirmState::Pending,
                ])?;
            }
            Ok(updated)
        })
    }

    /// Race-resistant: WHERE rollout_id guard makes a stale id a no-op when overwritten.
    pub fn record_terminal(
        &self,
        hostname: &str,
        rollout_id: &str,
        terminal: TerminalState,
    ) -> Result<usize> {
        // LOADBEARING: Converged stays Confirmed at the operational level; only RolledBack/Cancelled flip the column.
        let new_state = match terminal {
            TerminalState::Converged => return Ok(0),
            TerminalState::RolledBack => PendingConfirmState::RolledBack,
            TerminalState::Cancelled => PendingConfirmState::Cancelled,
        };
        super::read(self.conn, |c| {
            c.execute(
                "UPDATE host_dispatch_state
                 SET state = ?3
                 WHERE hostname = ?1
                   AND rollout_id = ?2",
                params![hostname, rollout_id, new_state],
            )
            .context("record_terminal host_dispatch_state")
        })
    }

    pub fn host_state(&self, hostname: &str) -> Result<Option<HostDispatchStateRow>> {
        super::read(self.conn, |c| {
            Ok(c.query_row(
                "SELECT hostname, rollout_id, channel, wave,
                        target_closure_hash, target_channel_ref,
                        state, dispatched_at, confirm_deadline,
                        confirmed_at
                 FROM host_dispatch_state
                 WHERE hostname = ?1",
                params![hostname],
                row_to_host_dispatch_state,
            )
            .ok())
        })
    }

    /// Filtering terminal rows prevents the reconciler defaulting absent host-states to Queued and re-dispatching.
    pub fn active_rollouts_snapshot(&self) -> Result<Vec<RolloutDbSnapshot>> {
        use std::collections::BTreeMap;

        super::read(self.conn, |c| {
            let mut stmt = c.prepare(
                "SELECT hds.rollout_id, hds.channel, hds.hostname,
                        hds.target_closure_hash, hds.target_channel_ref,
                        hds.state, hds.wave,
                        hrs.host_state, hrs.last_healthy_since,
                        COALESCE(r.current_wave, 0) AS current_wave
                 FROM host_dispatch_state hds
                 LEFT JOIN host_rollout_state hrs
                        ON hrs.rollout_id = hds.rollout_id
                       AND hrs.hostname = hds.hostname
                 LEFT JOIN rollouts r
                        ON r.rollout_id = hds.rollout_id
                 WHERE hds.state IN (?1, ?2, ?3)
                 ORDER BY hds.rollout_id, hds.hostname",
            )?;
            let rows = stmt
                .query_map(
                    params![
                        PendingConfirmState::Pending,
                        PendingConfirmState::Confirmed,
                        PendingConfirmState::DeferredPendingReboot,
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, i64>(9)?,
                        ))
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            let mut by_rollout: BTreeMap<String, RolloutDbSnapshot> = BTreeMap::new();
            for (
                rollout_id,
                row_channel,
                hostname,
                target_closure,
                target_ref,
                op_state,
                host_wave,
                hrs_state,
                hrs_ts,
                current_wave,
            ) in rows
            {
                let host_state = match hrs_state {
                    Some(s) => HostRolloutState::from_db_str(&s)?.as_db_str().to_string(),
                    None => match PendingConfirmState::from_db_str(&op_state)? {
                        PendingConfirmState::Pending => HostRolloutState::ConfirmWindow,
                        PendingConfirmState::Confirmed => HostRolloutState::Healthy,
                        // Deferred maps to ConfirmWindow for reconciler purposes:
                        // the host is still "in the confirm window" - just paused
                        // waiting for the operator's reboot. Wave promotion treats
                        // it as in-flight, which is correct (we don't want to
                        // promote past a host whose new gen hasn't actually
                        // activated yet).
                        PendingConfirmState::DeferredPendingReboot => HostRolloutState::ConfirmWindow,
                        PendingConfirmState::RolledBack | PendingConfirmState::Cancelled => {
                            unreachable!(
                                "filtered by WHERE hds.state IN ('pending','confirmed','deferred-pending-reboot') in the SELECT",
                            )
                        }
                    }
                    .as_db_str()
                    .to_string(),
                };

                let entry =
                    by_rollout
                        .entry(rollout_id.clone())
                        .or_insert_with(|| RolloutDbSnapshot {
                            rollout_id: rollout_id.clone(),
                            channel: row_channel.clone(),
                            target_closure_hash: target_closure.clone(),
                            target_channel_ref: target_ref.clone(),
                            host_states: HashMap::new(),
                            host_waves: HashMap::new(),
                            last_healthy_since: HashMap::new(),
                            current_wave: current_wave as u32,
                            // terminal_at lives on the rollouts table; gate observed
                            // builders merge from db.rollouts().list_active().
                            terminal_at: None,
                        });
                entry.host_states.insert(hostname.clone(), host_state);
                entry.host_waves.insert(hostname.clone(), host_wave as u32);
                if let Some(ts) = hrs_ts {
                    let parsed = ts
                        .parse::<DateTime<Utc>>()
                        .with_context(|| format!("parse last_healthy_since for {hostname}"))?;
                    entry.last_healthy_since.insert(hostname, parsed);
                }
            }
            Ok(by_rollout.into_values().collect())
        })
    }
}

#[derive(Debug, Clone)]
pub struct HostDispatchStateRow {
    pub hostname: String,
    pub rollout_id: String,
    pub channel: String,
    pub wave: u32,
    pub target_closure_hash: String,
    pub target_channel_ref: String,
    pub state: String,
    pub dispatched_at: String,
    pub confirm_deadline: String,
    pub confirmed_at: Option<String>,
}

fn row_to_host_dispatch_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostDispatchStateRow> {
    Ok(HostDispatchStateRow {
        hostname: row.get(0)?,
        rollout_id: row.get(1)?,
        channel: row.get(2)?,
        wave: row.get(3)?,
        target_closure_hash: row.get(4)?,
        target_channel_ref: row.get(5)?,
        state: row.get(6)?,
        dispatched_at: row.get(7)?,
        confirm_deadline: row.get(8)?,
        confirmed_at: row.get(9)?,
    })
}

fn upsert_operational(
    conn: &Connection,
    row: &DispatchInsert<'_>,
    state: PendingConfirmState,
    confirmed_at: Option<DateTime<Utc>>,
) -> Result<()> {
    let confirmed_at_str = confirmed_at.map(|t| t.to_rfc3339());
    conn.execute(
        "INSERT INTO host_dispatch_state(
             hostname, rollout_id, channel, wave,
             target_closure_hash, target_channel_ref,
             state, dispatched_at, confirm_deadline, confirmed_at
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'), ?8, ?9)
         ON CONFLICT(hostname) DO UPDATE SET
             rollout_id = excluded.rollout_id,
             channel = excluded.channel,
             wave = excluded.wave,
             target_closure_hash = excluded.target_closure_hash,
             target_channel_ref = excluded.target_channel_ref,
             state = excluded.state,
             dispatched_at = excluded.dispatched_at,
             confirm_deadline = excluded.confirm_deadline,
             confirmed_at = excluded.confirmed_at",
        params![
            row.hostname,
            row.rollout_id,
            row.channel,
            row.wave,
            row.target_closure_hash,
            row.target_channel_ref,
            state,
            row.confirm_deadline.to_rfc3339(),
            confirmed_at_str,
        ],
    )
    .context("upsert host_dispatch_state")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::{dispatch_insert, fresh_db, mark_healthy};
    use crate::state::{HostRolloutState, TerminalState};
    use chrono::Utc;

    #[test]
    fn record_dispatch_writes_operational_and_history() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@abc", "system-r1", deadline))
            .unwrap();
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.rollout_id, "stable@abc");
        assert_eq!(row.state, "pending");
        let history = db.dispatch_history().recent_for_host("ohm", 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].rollout_id, "stable@abc");
        assert!(history[0].terminal_state.is_none());
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "old", deadline))
            .unwrap();
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r2", "new", deadline))
            .unwrap();
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.rollout_id, "stable@r2");
        assert_eq!(row.target_closure_hash, "new");
        let history = db.dispatch_history().recent_for_host("ohm", 10).unwrap();
        assert_eq!(history.len(), 2, "history grows on each dispatch");
    }

    #[test]
    fn confirm_flips_state() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system-r1", deadline))
            .unwrap();
        let n = db
            .host_dispatch_state()
            .confirm("ohm", "stable@r1")
            .unwrap();
        assert_eq!(n, 1);
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.state, "confirmed");
        assert!(row.confirmed_at.is_some());
    }

    #[test]
    fn confirm_no_op_when_deadline_passed() {
        let db = fresh_db();
        let past_deadline = Utc::now() - chrono::Duration::seconds(30);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "ohm",
                "stable@expired",
                "system-r1",
                past_deadline,
            ))
            .unwrap();
        let n = db
            .host_dispatch_state()
            .confirm("ohm", "stable@expired")
            .unwrap();
        assert_eq!(
            n, 0,
            "confirm must not flip a pending row whose deadline has passed",
        );
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(
            row.state, "pending",
            "row stays pending until rollback_timer or 410-handler transitions it",
        );
    }

    #[test]
    fn confirm_no_op_on_wrong_rollout() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system-r1", deadline))
            .unwrap();
        let n = db
            .host_dispatch_state()
            .confirm("ohm", "stable@r2")
            .unwrap();
        assert_eq!(n, 0);
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.state, "pending");
    }

    #[test]
    fn pending_deadlines_picks_past_window() {
        let db = fresh_db();
        let past = Utc::now() - chrono::Duration::seconds(60);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@old", "system", past))
            .unwrap();
        let future = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("krach", "stable@new", "system", future))
            .unwrap();
        let expired = db.host_dispatch_state().pending_deadlines().unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, "ohm");
        assert_eq!(expired[0].1, "stable@old");
    }

    #[test]
    fn mark_rolled_back_flips_pending_only() {
        let db = fresh_db();
        let past = Utc::now() - chrono::Duration::seconds(60);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system", past))
            .unwrap();
        // First call: row is pending → flips to rolled-back.
        let n = db
            .host_dispatch_state()
            .mark_rolled_back(&[("ohm".to_string(), "stable@r1".to_string())])
            .unwrap();
        assert_eq!(n, 1);
        let n = db
            .host_dispatch_state()
            .mark_rolled_back(&[("ohm".to_string(), "stable@r1".to_string())])
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn record_terminal_no_op_when_rollout_id_mismatches() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "ohm",
                "stable@new",
                "system-new",
                deadline,
            ))
            .unwrap();
        let n = db
            .host_dispatch_state()
            .record_terminal("ohm", "stable@old", TerminalState::RolledBack)
            .unwrap();
        assert_eq!(n, 0);
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.state, "pending", "newer dispatch must not be flipped");
    }

    #[test]
    fn record_terminal_flips_matching_rollout() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system", deadline))
            .unwrap();
        let n = db
            .host_dispatch_state()
            .record_terminal("ohm", "stable@r1", TerminalState::RolledBack)
            .unwrap();
        assert_eq!(n, 1);
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.state, "rolled-back");
    }

    #[test]
    fn atomic_confirm_writes_confirmed_state() {
        let db = fresh_db();
        let now = Utc::now();
        db.host_dispatch_state()
            .record_confirmed_dispatch_with_state(
                "ohm",
                "stable@orphan",
                "stable",
                0,
                "target-system",
                "stable@orphan",
                now,
                HostRolloutState::Healthy,
                now,
            )
            .unwrap();
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.state, "confirmed");
        assert!(row.confirmed_at.is_some());
        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(
            snap[0].host_states.get("ohm").map(String::as_str),
            Some("Healthy"),
        );
    }

    #[test]
    fn active_rollouts_snapshot_excludes_terminal_states() {
        let db = fresh_db();
        let past = Utc::now() - chrono::Duration::seconds(60);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@dead", "system", past))
            .unwrap();
        let pairs = vec![("ohm".to_string(), "stable@dead".to_string())];
        db.host_dispatch_state().mark_rolled_back(&pairs).unwrap();
        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert!(snap.is_empty());
    }

    #[test]
    fn active_rollouts_snapshot_pending_surfaces_as_confirmwindow() {
        let db = fresh_db();
        let future = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "ohm",
                "stable@abc12345",
                "system-r1",
                future,
            ))
            .unwrap();
        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert_eq!(snap.len(), 1);
        let r = &snap[0];
        assert_eq!(r.rollout_id, "stable@abc12345");
        assert_eq!(r.channel, "stable");
        assert_eq!(r.target_closure_hash, "system-r1");
        assert_eq!(
            r.host_states.get("ohm").map(String::as_str),
            Some("ConfirmWindow"),
        );
        assert!(r.last_healthy_since.is_empty());
    }

    #[test]
    fn active_rollouts_snapshot_confirmed_uses_host_rollout_state() {
        let db = fresh_db();
        let future = Utc::now() + chrono::Duration::seconds(120);
        let now = Utc::now();
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "ohm",
                "stable@abc12345",
                "system-r1",
                future,
            ))
            .unwrap();
        db.host_dispatch_state()
            .confirm("ohm", "stable@abc12345")
            .unwrap();
        mark_healthy(&db, "ohm", "stable@abc12345", now);
        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert_eq!(snap.len(), 1);
        let r = &snap[0];
        assert_eq!(
            r.host_states.get("ohm").map(String::as_str),
            Some("Healthy"),
        );
        let stored = r
            .last_healthy_since
            .get("ohm")
            .expect("Healthy host has soak ts");
        assert_eq!(stored.timestamp(), now.timestamp());
    }

    #[test]
    fn active_rollouts_snapshot_uses_explicit_channel_for_sha_rollout_id() {
        let db = fresh_db();
        let future = Utc::now() + chrono::Duration::seconds(120);
        let sha_rollout = "1111111111111111111111111111111111111111111111111111111111111111";
        let mut row = dispatch_insert("ohm", sha_rollout, "system-r1", future);
        row.channel = "edge-slow";
        db.host_dispatch_state().record_dispatch(&row).unwrap();
        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].channel, "edge-slow");
    }

    #[test]
    fn mark_deferred_flips_pending_only() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system-r1", deadline))
            .unwrap();
        let n = db
            .host_dispatch_state()
            .mark_deferred("ohm", "stable@r1")
            .unwrap();
        assert_eq!(n, 1);
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.state, "deferred-pending-reboot");
        // Idempotent: a second mark_deferred is a no-op (the row is no longer Pending).
        let n = db
            .host_dispatch_state()
            .mark_deferred("ohm", "stable@r1")
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn mark_deferred_no_op_on_non_pending_states() {
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system-r1", deadline))
            .unwrap();
        db.host_dispatch_state()
            .confirm("ohm", "stable@r1")
            .unwrap();
        // Confirmed → not flipped to deferred.
        let n = db
            .host_dispatch_state()
            .mark_deferred("ohm", "stable@r1")
            .unwrap();
        assert_eq!(n, 0);
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.state, "confirmed");
    }

    #[test]
    fn confirm_accepts_deferred_state_without_deadline_check() {
        // The whole point of the deferred state: a stale confirm-deadline
        // (which would block a Pending confirm) does NOT block a confirm
        // against a DeferredPendingReboot row.
        let db = fresh_db();
        let past_deadline = Utc::now() - chrono::Duration::seconds(7200);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "ohm",
                "stable@r1",
                "system-r1",
                past_deadline,
            ))
            .unwrap();
        db.host_dispatch_state()
            .mark_deferred("ohm", "stable@r1")
            .unwrap();
        // Two hours after deadline, the host finally reboots and confirms.
        let n = db
            .host_dispatch_state()
            .confirm("ohm", "stable@r1")
            .unwrap();
        assert_eq!(n, 1, "deferred confirm must succeed despite stale deadline");
        let row = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(row.state, "confirmed");
        assert!(row.confirmed_at.is_some());
    }

    #[test]
    fn pending_deadlines_skips_deferred_rows() {
        // Rollback timer must NEVER sweep deferred rows - that's the whole
        // correctness guarantee for issue #56's human-paced lifecycle.
        let db = fresh_db();
        let past = Utc::now() - chrono::Duration::seconds(7200);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "ohm",
                "stable@deferred",
                "system-r1",
                past,
            ))
            .unwrap();
        db.host_dispatch_state()
            .mark_deferred("ohm", "stable@deferred")
            .unwrap();
        // Also seed a genuinely-expired Pending row to confirm the sweep
        // still picks up that one (sanity check).
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert(
                "krach",
                "stable@pending",
                "system-r1",
                past,
            ))
            .unwrap();
        let expired = db.host_dispatch_state().pending_deadlines().unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(
            expired[0].0, "krach",
            "deferred ohm must not appear in expired set"
        );
    }

    #[test]
    fn active_rollouts_snapshot_includes_deferred_as_confirmwindow() {
        // Reconciler must see deferred hosts as in-flight so wave promotion
        // doesn't skip past a host whose new gen hasn't actually activated.
        let db = fresh_db();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system-r1", deadline))
            .unwrap();
        db.host_dispatch_state()
            .mark_deferred("ohm", "stable@r1")
            .unwrap();
        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(
            snap[0].host_states.get("ohm").map(String::as_str),
            Some("ConfirmWindow"),
        );
    }

    #[test]
    fn pending_dispatch_exists_returns_only_for_pending() {
        let db = fresh_db();
        let future = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&dispatch_insert("ohm", "stable@r1", "system", future))
            .unwrap();
        assert!(db
            .host_dispatch_state()
            .pending_dispatch_exists("ohm")
            .unwrap());
        db.host_dispatch_state()
            .confirm("ohm", "stable@r1")
            .unwrap();
        assert!(
            !db.host_dispatch_state()
                .pending_dispatch_exists("ohm")
                .unwrap(),
            "confirmed row is not pending",
        );
    }

    /// **Regression guard**: the recovery path must land BOTH the
    /// host_dispatch_state operational row AND the host_rollout_state
    /// Healthy marker - never one without the other.
    ///
    /// If the two writes ever split into separate transactions, a
    /// second-write failure leaves the operational row at `confirmed`
    /// with NO host_rollout_state row. The snapshot LEFT JOIN then
    /// projects "Healthy with NULL last_healthy_since"; the soak
    /// timer never fires; the host sticks at Healthy and blocks
    /// wave promotion for the whole rollout.
    ///
    /// This test pins the happy path (both rows present after
    /// success). The atomic-rollback property is enforced by SQLite
    /// at the engine level; we can't inject partial failure without
    /// a fault injector. If a future refactor splits the txn the
    /// `last_healthy_since populated` assertion catches the
    /// regression - because the bug only triggers when the SECOND
    /// write fails under DB lock contention or process kill.
    #[test]
    fn orphan_confirm_lands_both_rows_atomically() {
        let db = fresh_db();
        let now = Utc::now();
        db.host_dispatch_state()
            .record_confirmed_dispatch_with_state(
                "ohm",
                "stable@r1",
                "stable",
                0,
                "system-r1",
                "stable@r1",
                now,
                HostRolloutState::Healthy,
                now,
            )
            .unwrap();

        // host_dispatch_state operational row landed.
        let op = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(op.state, "confirmed");
        assert_eq!(op.rollout_id, "stable@r1");

        // host_rollout_state Healthy marker landed in the SAME txn  -
        // the LEFT JOIN in active_rollouts_snapshot will now find it
        // and the soak timer will fire.
        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        let r = snap.iter().find(|r| r.rollout_id == "stable@r1").unwrap();
        assert_eq!(
            r.host_states.get("ohm").map(|s| s.as_str()),
            Some(HostRolloutState::Healthy.as_db_str()),
        );
        assert!(
            r.last_healthy_since.contains_key("ohm"),
            "last_healthy_since must populate or soak timer never fires; got {:?}",
            r.last_healthy_since,
        );
    }

    /// Pins the `healthy_since != confirmed_at` shape used by soak-state
    /// recovery from agent attestation. The agent reports it has been
    /// healthy since some moment BEFORE this checkin; the soak timer
    /// must anchor on that earlier moment, not on `now`. A regression
    /// that collapses the two parameters back into one would force
    /// recovered hosts to restart their soak from scratch on every CP
    /// rebuild.
    #[test]
    fn atomic_confirm_with_distinct_healthy_since_anchors_on_healthy_since() {
        let db = fresh_db();
        let confirmed_at = Utc::now();
        let healthy_since = confirmed_at - chrono::Duration::minutes(7);
        db.host_dispatch_state()
            .record_confirmed_dispatch_with_state(
                "ohm",
                "stable@r1",
                "stable",
                0,
                "system-r1",
                "stable@r1",
                confirmed_at,
                HostRolloutState::Healthy,
                healthy_since,
            )
            .unwrap();

        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        let r = snap.iter().find(|r| r.rollout_id == "stable@r1").unwrap();
        let stamped = r
            .last_healthy_since
            .get("ohm")
            .expect("soak anchor must populate");
        assert_eq!(
            stamped.timestamp(),
            healthy_since.timestamp(),
            "soak anchor must use healthy_since (the agent-reported earlier moment), \
             not confirmed_at (the recovery moment)",
        );
    }

    /// Pins the `target_state = Converged` shape used by
    /// converged-at-dispatch (case 3 in dispatch_target.rs: host
    /// already on target closure before any dispatch attempt).
    /// Both rows must land atomically with the host_rollout_state
    /// row directly in Converged. A regression that hardcodes
    /// Healthy back into the helper would land a Converged-named
    /// caller in Healthy state, leaking soak-window behaviour into
    /// what should be terminal.
    #[test]
    fn atomic_confirm_with_converged_state_writes_both_rows() {
        let db = fresh_db();
        let now = Utc::now();
        db.host_dispatch_state()
            .record_confirmed_dispatch_with_state(
                "ohm",
                "stable@r1",
                "stable",
                0,
                "system-r1",
                "stable@r1",
                now,
                HostRolloutState::Converged,
                now,
            )
            .unwrap();

        let op = db.host_dispatch_state().host_state("ohm").unwrap().unwrap();
        assert_eq!(op.state, "confirmed");
        assert_eq!(op.rollout_id, "stable@r1");

        let snap = db.host_dispatch_state().active_rollouts_snapshot().unwrap();
        let r = snap.iter().find(|r| r.rollout_id == "stable@r1").unwrap();
        assert_eq!(
            r.host_states.get("ohm").map(|s| s.as_str()),
            Some(HostRolloutState::Converged.as_db_str()),
            "target_state=Converged must produce a Converged hrs row",
        );
        assert!(
            r.last_healthy_since.contains_key("ohm"),
            "last_healthy_since stamps even on Converged (audit-trail nicety)",
        );
    }
}
