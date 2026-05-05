//! Per-rollout supersession state (soft state; reconstructible after rebuild
//! from channel-refs polling + on-dispatch inserts). Source of truth for
//! "is this rollout still in flight, or has a newer rollout for the same
//! channel replaced it?"

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

pub struct Rollouts<'a> {
    pub(super) conn: &'a Mutex<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersedeStatus {
    pub superseded_at: Option<DateTime<Utc>>,
    pub superseded_by: Option<String>,
    pub terminal_at: Option<DateTime<Utc>>,
}

impl SupersedeStatus {
    pub fn is_superseded(&self) -> bool {
        self.superseded_at.is_some()
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal_at.is_some()
    }

    /// Single predicate for "this rollout is no longer in flight" — the
    /// reconciler and dispatch path treat both as equivalent (don't
    /// advance, don't include in gate observed). Terminal vs superseded
    /// is only useful for diagnostic/audit surfaces.
    pub fn is_finished(&self) -> bool {
        self.is_superseded() || self.is_terminal()
    }
}

impl Rollouts<'_> {
    /// Idempotent insert + same-channel supersede in one txn.
    ///
    /// LOADBEARING:
    /// 1. `INSERT OR IGNORE` ensures concurrent dispatches with the same
    ///    `(rollout_id, channel)` don't fight — first writer wins, the rest
    ///    no-op.
    /// 2. The supersede UPDATE has `WHERE rollout_id != ?` so we never mark
    ///    ourselves as superseded.
    /// 3. Channels are namespaces — supersession is strictly intra-channel.
    /// 4. Timestamps are RFC3339 strings to match the convention used by
    ///    the rest of the schema (read paths use `parse::<DateTime<Utc>>()`).
    pub fn record_active_rollout(&self, rollout_id: &str, channel: &str) -> Result<()> {
        let now_rfc = Utc::now().to_rfc3339();
        let mut guard = super::lock_conn(self.conn)?;
        let txn = guard.transaction().context("begin record_active_rollout")?;
        txn.execute(
            "INSERT OR IGNORE INTO rollouts(rollout_id, channel, created_at)
             VALUES (?1, ?2, ?3)",
            params![rollout_id, channel, now_rfc],
        )
        .context("INSERT OR IGNORE rollouts")?;
        txn.execute(
            "UPDATE rollouts
             SET superseded_at = ?3,
                 superseded_by = ?2
             WHERE channel = ?1
               AND rollout_id != ?2
               AND superseded_at IS NULL",
            params![channel, rollout_id, now_rfc],
        )
        .context("UPDATE rollouts supersede prior")?;
        txn.commit().context("commit record_active_rollout")?;
        Ok(())
    }

    /// `Ok(None)` when the rollout isn't tracked. Lifecycle endpoint
    /// returns 404 in that case — callers don't fabricate supersession
    /// state for unknown rids (no historical reconstruction).
    pub fn supersede_status(&self, rollout_id: &str) -> Result<Option<SupersedeStatus>> {
        let guard = super::lock_conn(self.conn)?;
        let row = guard
            .query_row(
                "SELECT superseded_at, superseded_by, terminal_at
                 FROM rollouts
                 WHERE rollout_id = ?1",
                params![rollout_id],
                |row| {
                    let at: Option<String> = row.get(0)?;
                    let by: Option<String> = row.get(1)?;
                    let term: Option<String> = row.get(2)?;
                    Ok((at, by, term))
                },
            )
            .optional()
            .context("query rollouts.supersede_status")?;
        let parsed = row
            .map(|(at_raw, by, term_raw)| -> Result<SupersedeStatus> {
                let parse_ts = |raw: Option<String>, field: &str| -> Result<Option<DateTime<Utc>>> {
                    match raw {
                        Some(s) => Ok(Some(
                            s.parse::<DateTime<Utc>>()
                                .with_context(|| format!("parse rollouts.{field}: {s}"))?,
                        )),
                        None => Ok(None),
                    }
                };
                Ok(SupersedeStatus {
                    superseded_at: parse_ts(at_raw, "superseded_at")?,
                    superseded_by: by,
                    terminal_at: parse_ts(term_raw, "terminal_at")?,
                })
            })
            .transpose()?;
        Ok(parsed)
    }

    /// Mark a rollout as terminal — no longer in flight, won't appear in
    /// `list_active` or in the gate observed. Idempotent: re-marking is
    /// a no-op (returns 0) so the reconciler can call this every time
    /// `Action::ConvergeRollout` fires without bookkeeping a "did we
    /// already?" flag.
    ///
    /// Two trigger sites:
    ///   1. `Action::ConvergeRollout` — every host on this rollout has
    ///      reached terminal-for-ordering (Soaked/Converged/Reverted),
    ///      and the wave is the last wave.
    ///   2. Per-tick orphan sweep — the rollout's channel has zero
    ///      expected hosts in the current fleet snapshot (the operator
    ///      removed them from fleet.nix, or the closure_hash was
    ///      stripped). Without this sweep the rollout sits "in flight"
    ///      forever even with no hosts to converge.
    pub fn mark_terminal(&self, rollout_id: &str, now: DateTime<Utc>) -> Result<usize> {
        let guard = super::lock_conn(self.conn)?;
        let n = guard
            .execute(
                "UPDATE rollouts
                 SET terminal_at = ?2
                 WHERE rollout_id = ?1 AND terminal_at IS NULL",
                params![rollout_id, now.to_rfc3339()],
            )
            .context("UPDATE rollouts terminal_at")?;
        Ok(n)
    }

    /// Monotonic wave-index advance. The `WHERE current_wave < ?2` guard
    /// ensures concurrent reconciler ticks can't race a rollout backwards;
    /// the second update is a no-op (returns 0).
    pub fn set_current_wave(&self, rollout_id: &str, wave: u32) -> Result<usize> {
        let guard = super::lock_conn(self.conn)?;
        let n = guard
            .execute(
                "UPDATE rollouts
                 SET current_wave = ?2
                 WHERE rollout_id = ?1 AND current_wave < ?2",
                params![rollout_id, wave as i64],
            )
            .context("set_current_wave")?;
        Ok(n)
    }

    pub fn current_wave(&self, rollout_id: &str) -> Result<Option<u32>> {
        let guard = super::lock_conn(self.conn)?;
        let n = guard
            .query_row(
                "SELECT current_wave FROM rollouts WHERE rollout_id = ?1",
                params![rollout_id],
                |row| row.get::<_, i64>(0).map(|w| w as u32),
            )
            .optional()
            .context("query rollouts.current_wave")?;
        Ok(n)
    }

    /// Used by `active_rollouts_snapshot` to filter out superseded rollouts
    /// without joining (snapshot is grouped by rollout_id; this returns the
    /// set of superseded ids to exclude).
    pub fn superseded_rollout_ids(&self) -> Result<Vec<String>> {
        let guard = super::lock_conn(self.conn)?;
        let mut stmt =
            guard.prepare("SELECT rollout_id FROM rollouts WHERE superseded_at IS NOT NULL")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Returns rollout-ids no longer in flight — superseded OR terminal.
    /// Single set so callers don't have to track two filters; the
    /// reconciler and dispatch path treat both states equivalently
    /// (don't advance, exclude from gate observed).
    pub fn finished_rollout_ids(&self) -> Result<Vec<String>> {
        let guard = super::lock_conn(self.conn)?;
        let mut stmt = guard.prepare(
            "SELECT rollout_id FROM rollouts
             WHERE superseded_at IS NOT NULL OR terminal_at IS NOT NULL",
        )?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Canonical source for the dispatch + reconciler gate observed
    /// builders. Filters superseded only — terminal rollouts MUST stay
    /// visible to gates so channelEdges can find a converged predecessor
    /// and release the successor (the rollout's `host_states` are all
    /// terminal-for-ordering, so `Rollout::is_active_for_ordering()`
    /// returns false → gate releases).
    ///
    /// Hiding terminal rollouts from gates was the regression that
    /// surfaced after the lifecycle fix: dispatch endpoint with
    /// `GateMode::Dispatch` falls into the
    /// "fleet has hosts on predecessor → block" arm because the
    /// predecessor disappeared from observed, while the reconciler
    /// (non-conservative) allowed dispatch — asymmetric verdicts on
    /// the same input, leaving krach stuck in dispatch_host loops
    /// the dispatch endpoint then refused.
    ///
    /// UI consumers (/v1/rollouts list, deferrals view, metrics)
    /// want "what's still in flight from the operator's perspective"
    /// — those use `list_in_flight()` instead.
    pub fn list_active(&self) -> Result<Vec<ActiveRollout>> {
        self.list_filtered(false)
    }

    /// "What's in flight" for UI: filters BOTH superseded AND terminal.
    /// A converged rollout with no successor still gets removed once
    /// `Action::ConvergeRollout` stamps terminal_at, matching the
    /// operator mental model "this rollout is done, don't show it as
    /// pending work."
    ///
    /// NOT used by the dispatch / reconciler gate observed builders
    /// — those need terminal rollouts visible so channelEdges can
    /// detect "predecessor converged" via host_states inspection.
    /// See `list_active()` for that path.
    pub fn list_in_flight(&self) -> Result<Vec<ActiveRollout>> {
        self.list_filtered(true)
    }

    fn list_filtered(&self, exclude_terminal: bool) -> Result<Vec<ActiveRollout>> {
        let guard = super::lock_conn(self.conn)?;
        // SQL composed at compile time via two static strings — no
        // user input interpolated, no injection risk. The terminal_at
        // filter toggles the WHERE clause cleanly.
        let sql = if exclude_terminal {
            "SELECT rollout_id, channel, current_wave, created_at, terminal_at
             FROM rollouts
             WHERE superseded_at IS NULL AND terminal_at IS NULL
             ORDER BY created_at DESC, rollout_id"
        } else {
            "SELECT rollout_id, channel, current_wave, created_at, terminal_at
             FROM rollouts
             WHERE superseded_at IS NULL
             ORDER BY created_at DESC, rollout_id"
        };
        let mut stmt = guard.prepare(sql)?;
        let rows = stmt
            .query_map([], |row| {
                let terminal_at_raw: Option<String> = row.get(4)?;
                Ok((
                    ActiveRollout {
                        rollout_id: row.get(0)?,
                        channel: row.get(1)?,
                        current_wave: row.get::<_, i64>(2)? as u32,
                        created_at: row.get::<_, String>(3)?,
                        terminal_at: None,
                    },
                    terminal_at_raw,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // Parse terminal_at outside the closure so error context is precise.
        rows.into_iter()
            .map(|(mut row, raw)| -> Result<ActiveRollout> {
                row.terminal_at = match raw {
                    Some(s) => Some(
                        s.parse::<DateTime<Utc>>()
                            .with_context(|| format!("parse rollouts.terminal_at: {s}"))?,
                    ),
                    None => None,
                };
                Ok(row)
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRollout {
    pub rollout_id: String,
    pub channel: String,
    pub current_wave: u32,
    pub created_at: String,
    /// Set when `Action::ConvergeRollout` fires or the orphan sweep
    /// stamps a rollout whose channel has no expected hosts in the
    /// current fleet. `None` while the rollout is still progressing
    /// through waves. Plumbed through to the in-memory `Rollout`
    /// (in nixfleet-reconciler) so `advance_rollout` short-circuits
    /// instead of re-emitting `ConvergeRollout` every tick — and so
    /// `channel_edges` can distinguish "predecessor converged" from
    /// "predecessor unknown" without inferring it from absence.
    pub terminal_at: Option<DateTime<Utc>>,
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
    fn record_active_rollout_inserts_first_one_as_active() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        let status = db.rollouts().supersede_status("r1").unwrap();
        let s = status.expect("rollout present");
        assert!(!s.is_superseded(), "first rollout on a channel must be active");
    }

    #[test]
    fn record_active_rollout_supersedes_prior_on_same_channel() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r2", "stable")
            .unwrap();

        let r1 = db.rollouts().supersede_status("r1").unwrap().unwrap();
        assert!(r1.is_superseded());
        assert_eq!(r1.superseded_by.as_deref(), Some("r2"));

        let r2 = db.rollouts().supersede_status("r2").unwrap().unwrap();
        assert!(!r2.is_superseded());
    }

    #[test]
    fn record_active_rollout_does_not_supersede_across_channels() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r2", "edge-slow")
            .unwrap();

        // Both should be active in their own channel.
        assert!(!db
            .rollouts()
            .supersede_status("r1")
            .unwrap()
            .unwrap()
            .is_superseded());
        assert!(!db
            .rollouts()
            .supersede_status("r2")
            .unwrap()
            .unwrap()
            .is_superseded());
    }

    #[test]
    fn record_active_rollout_is_idempotent_for_same_id_same_channel() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        // r1 must still be active — re-recording itself never marks it superseded.
        assert!(!db
            .rollouts()
            .supersede_status("r1")
            .unwrap()
            .unwrap()
            .is_superseded());
    }

    #[test]
    fn supersede_status_returns_none_for_unknown_rollout() {
        let db = fresh_db();
        assert!(db.rollouts().supersede_status("ghost").unwrap().is_none());
    }

    #[test]
    fn superseded_rollout_ids_lists_only_superseded() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r2", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r3", "edge-slow")
            .unwrap();
        let mut ids = db.rollouts().superseded_rollout_ids().unwrap();
        ids.sort();
        assert_eq!(ids, vec!["r1".to_string()]);
    }

    #[test]
    fn list_active_returns_only_non_superseded_with_channel_and_wave() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r2", "edge-slow")
            .unwrap();
        // Supersede r1 with a new stable rollout r3.
        db.rollouts()
            .record_active_rollout("r3", "stable")
            .unwrap();
        // Advance r3 to wave 1 (stable's promotion).
        db.rollouts().set_current_wave("r3", 1).unwrap();

        let mut rows = db.rollouts().list_active().unwrap();
        rows.sort_by(|a, b| a.rollout_id.cmp(&b.rollout_id));
        assert_eq!(rows.len(), 2, "list_active excludes superseded r1");
        let r2 = rows.iter().find(|r| r.rollout_id == "r2").unwrap();
        assert_eq!(r2.channel, "edge-slow");
        assert_eq!(r2.current_wave, 0);
        let r3 = rows.iter().find(|r| r.rollout_id == "r3").unwrap();
        assert_eq!(r3.channel, "stable");
        assert_eq!(r3.current_wave, 1);
    }

    #[test]
    fn set_current_wave_is_monotonic_no_op_on_backwards() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        assert_eq!(db.rollouts().current_wave("r1").unwrap(), Some(0));
        let n = db.rollouts().set_current_wave("r1", 1).unwrap();
        assert_eq!(n, 1);
        assert_eq!(db.rollouts().current_wave("r1").unwrap(), Some(1));
        // Backwards is no-op.
        let n = db.rollouts().set_current_wave("r1", 0).unwrap();
        assert_eq!(n, 0);
        assert_eq!(db.rollouts().current_wave("r1").unwrap(), Some(1));
    }

    /// LOADBEARING regression: rebuild scenario. After a rebuild the table
    /// starts empty; the polling tick must populate it idempotently for
    /// each channel's current rid. Stale rids that NEVER re-enter the table
    /// stay absent — the lifecycle endpoint returns 404 for them and
    /// render.sh skips, no fabricated supersession state.
    #[test]
    fn rebuild_recovery_repopulates_via_repeated_record_calls() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r-current", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r-current", "stable")
            .unwrap();
        let s = db
            .rollouts()
            .supersede_status("r-current")
            .unwrap()
            .expect("current rid present after polling tick");
        assert!(!s.is_superseded());
        assert!(db.rollouts().supersede_status("r-old").unwrap().is_none());
    }

    /// **Regression guard** for the asymmetry that surfaced after the first
    /// lifecycle attempt: filtering terminal rollouts at `list_active`
    /// caused channelEdges to lose sight of converged predecessors, which
    /// then disagreed with itself between dispatch (conservative) and
    /// reconciler (non-conservative) modes. This test pins the load-
    /// bearing semantic: terminal rollouts STAY visible in `list_active`
    /// (the gate observed source) but are HIDDEN from `list_in_flight`
    /// (the UI source). Same row, different views.
    #[test]
    fn mark_terminal_keeps_rollout_in_list_active_but_drops_from_list_in_flight() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r2", "edge")
            .unwrap();

        // Both visible in both views before any terminal stamp.
        assert_eq!(db.rollouts().list_active().unwrap().len(), 2);
        assert_eq!(db.rollouts().list_in_flight().unwrap().len(), 2);

        // Mark r1 terminal; idempotent on re-call.
        let now = chrono::Utc::now();
        let n = db.rollouts().mark_terminal("r1", now).unwrap();
        assert_eq!(n, 1);
        let n2 = db.rollouts().mark_terminal("r1", now).unwrap();
        assert_eq!(n2, 0, "re-marking is idempotent");

        // list_active KEEPS r1 — gates need to see converged predecessors
        // so channel_edges can return is_active_for_ordering=false.
        let active = db.rollouts().list_active().unwrap();
        assert_eq!(active.len(), 2, "list_active must include terminal rollouts so gates can see converged predecessors");
        let r1_active = active.iter().find(|r| r.rollout_id == "r1").unwrap();
        assert!(r1_active.terminal_at.is_some(), "terminal_at must populate through to ActiveRollout");

        // list_in_flight DROPS r1 — UI shows only ongoing work.
        let in_flight = db.rollouts().list_in_flight().unwrap();
        assert_eq!(in_flight.len(), 1);
        assert_eq!(in_flight[0].rollout_id, "r2");

        // supersede_status reflects terminal.
        let s = db.rollouts().supersede_status("r1").unwrap().unwrap();
        assert!(s.is_terminal());
        assert!(!s.is_superseded(), "terminal is independent of superseded");
        assert!(s.is_finished());
    }

    /// Superseded rollouts are dropped from BOTH views regardless of
    /// terminal_at — supersession is the stronger signal (newer
    /// rollout for the same channel exists, gates evaluate against it).
    #[test]
    fn superseded_dropped_from_both_list_active_and_list_in_flight() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r2", "stable")
            .unwrap(); // supersedes r1

        for rid in db.rollouts().list_active().unwrap().iter() {
            assert_ne!(rid.rollout_id, "r1", "superseded must not appear in list_active");
        }
        for rid in db.rollouts().list_in_flight().unwrap().iter() {
            assert_ne!(rid.rollout_id, "r1", "superseded must not appear in list_in_flight");
        }

        // Even after marking r1 terminal, it stays out of both —
        // superseded was already excluding it.
        db.rollouts().mark_terminal("r1", chrono::Utc::now()).unwrap();
        for rid in db.rollouts().list_active().unwrap().iter() {
            assert_ne!(rid.rollout_id, "r1");
        }
    }

    #[test]
    fn finished_rollout_ids_unions_superseded_and_terminal() {
        let db = fresh_db();
        // r1 → r2 same channel: r1 superseded.
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r2", "stable")
            .unwrap();
        // r3 standalone, then marked terminal.
        db.rollouts()
            .record_active_rollout("r3", "edge")
            .unwrap();
        db.rollouts()
            .mark_terminal("r3", chrono::Utc::now())
            .unwrap();

        let mut ids = db.rollouts().finished_rollout_ids().unwrap();
        ids.sort();
        assert_eq!(ids, vec!["r1".to_string(), "r3".to_string()]);

        // r2 (active, neither superseded nor terminal) absent from finished set.
        assert!(!ids.contains(&"r2".to_string()));
    }

    /// Supersession overrides terminal: superseded rollouts can't be
    /// "un-marked" by a later terminal stamp, and terminal can be
    /// stamped on a superseded rollout (idempotent — finished is the
    /// union). Either field alone is sufficient to drop from in-flight.
    #[test]
    fn terminal_and_superseded_compose_independently() {
        let db = fresh_db();
        db.rollouts()
            .record_active_rollout("r1", "stable")
            .unwrap();
        db.rollouts()
            .record_active_rollout("r2", "stable")
            .unwrap();
        // r1 is now superseded by r2.
        let s1_before = db.rollouts().supersede_status("r1").unwrap().unwrap();
        assert!(s1_before.is_superseded());
        assert!(!s1_before.is_terminal());

        // Stamping r1 terminal too is allowed (UPDATE only fires on terminal_at IS NULL).
        let n = db
            .rollouts()
            .mark_terminal("r1", chrono::Utc::now())
            .unwrap();
        assert_eq!(n, 1);

        let s1_after = db.rollouts().supersede_status("r1").unwrap().unwrap();
        assert!(s1_after.is_superseded());
        assert!(s1_after.is_terminal());
        assert!(s1_after.is_finished());
    }
}
