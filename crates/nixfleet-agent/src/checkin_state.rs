//! Persistence + shared closure-hash helpers for the checkin body.

use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nixfleet_proto::agent_wire::{EvaluatedTarget, FetchOutcome};

/// `<closure_hash>\n<rfc3339-timestamp>\n`; closure_hash binds the timestamp to
/// its generation so rollback suppresses it on the next checkin.
pub const LAST_CONFIRM_FILENAME: &str = "last_confirmed_at";

/// Written after dispatch, BEFORE activate; read at startup to retroactively
/// confirm a self-killed mid-switch.
pub const LAST_DISPATCH_FILENAME: &str = "last_dispatched";

/// Confirm breadcrumb replayed on every checkin so the wave-promotion gate
/// and outstandingComplianceFailures filter have something to compare against.
/// Persists across confirms (unlike `last_dispatched`).
pub const LAST_TARGET_FILENAME: &str = "last_target";

/// Drives CP's circuit breaker (`Decision::HoldAfterFailure`): a host stuck
/// on bad bytes stops being re-dispatched until a clean fetch shows up.
pub const LAST_FETCH_OUTCOME_FILENAME: &str = "last_fetch_outcome";

/// Suppresses redundant `ActivationDeferred` re-posts when the next target's
/// closure_hash matches the recorded value. Cleared on confirm success;
/// naturally bypassed when a fresher closure arrives.
pub const LAST_DEFERRED_FILENAME: &str = "last_deferred";

/// Suppresses retry of a closure that already failed within the quarantine
/// window. Single-record overwrite-on-write: a different closure_hash resets
/// the count, so a CI fix that advances the channel-ref clears suppression.
pub const LAST_FAILED_CLOSURE_FILENAME: &str = "last_failed_closure";

/// 24h: long enough to absorb operator-paced fixes, short enough that a stale
/// record from a recovered host doesn't suppress a legitimate retry.
pub const QUARANTINE_WINDOW_SECS: i64 = 24 * 60 * 60;

/// Re-post throttle: `ClosureQuarantined` at most once per hour while
/// suppressing the same closure_hash. Bounds journal volume during steady-state.
pub const QUARANTINE_REPOST_THROTTLE_SECS: i64 = 60 * 60;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LastDeferredRecord {
    pub closure_hash: String,
    pub channel_ref: String,
    pub component: String,
    pub deferred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LastFailedClosureRecord {
    pub closure_hash: String,
    pub channel_ref: String,
    pub last_failure_at: DateTime<Utc>,
    pub failure_count: u32,
    /// Fed back into `ClosureQuarantined`'s `reason` field on suppression posts.
    pub reason: String,
    /// Throttle anchor: at most one `ClosureQuarantined` per
    /// `QUARANTINE_REPOST_THROTTLE_SECS` window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_quarantine_post_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LastDispatchRecord {
    pub closure_hash: String,
    pub channel_ref: String,
    pub rollout_id: String,
    /// Channel's compliance mode at dispatch time; boot-recovery runs the
    /// runtime gate against this before retroactive confirm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compliance_mode: Option<String>,
    /// Wire-carried from `target.activate.confirm_endpoint`. Required - we
    /// only persist `last_dispatched` for confirmable targets.
    pub confirm_endpoint: String,
    pub dispatched_at: DateTime<Utc>,
}

/// Atomic tempfile + rename so crash mid-write can't leave half-written state.
fn write_atomic(state_dir: &Path, filename: &str, body: &[u8]) -> Result<()> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let final_path = state_dir.join(filename);
    let tmp_path = state_dir.join(format!("{filename}.tmp"));
    std::fs::write(&tmp_path, body).with_context(|| format!("write {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), final_path.display()))?;
    Ok(())
}

fn write_atomic_json<T: serde::Serialize>(
    state_dir: &Path,
    filename: &str,
    value: &T,
) -> Result<()> {
    let body = serde_json::to_string(value).with_context(|| format!("serialize {filename}"))?;
    write_atomic(state_dir, filename, body.as_bytes())
}

/// `Ok(None)` for absent OR malformed JSON; `Err` only on FS I/O. Malformed
/// reads log at `warn` so corrupt-state-dir failures aren't silent.
fn read_atomic_json<T: for<'de> serde::Deserialize<'de>>(
    state_dir: &Path,
    filename: &str,
) -> Result<Option<T>> {
    let path = state_dir.join(filename);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    match serde_json::from_str::<T>(&raw) {
        Ok(v) => Ok(Some(v)),
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "read_atomic_json: parse failed; treating as absent",
            );
            Ok(None)
        }
    }
}

/// Atomic; failures fall back to next-checkin re-dispatch.
pub fn write_last_dispatched(state_dir: &Path, record: &LastDispatchRecord) -> Result<()> {
    write_atomic_json(state_dir, LAST_DISPATCH_FILENAME, record)
}

pub fn read_last_dispatched(state_dir: &Path) -> Result<Option<LastDispatchRecord>> {
    read_atomic_json(state_dir, LAST_DISPATCH_FILENAME)
}

/// Idempotent: absent file returns `Ok`.
pub fn clear_last_dispatched(state_dir: &Path) -> Result<()> {
    let path = state_dir.join(LAST_DISPATCH_FILENAME);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("remove {}", path.display())),
    }
}

/// Persists the most recently confirmed `EvaluatedTarget` so the agent can
/// carry its `last_evaluated_target` breadcrumb on every subsequent checkin.
pub fn write_last_target(state_dir: &Path, target: &EvaluatedTarget) -> Result<()> {
    write_atomic_json(state_dir, LAST_TARGET_FILENAME, target)
}

pub fn read_last_target(state_dir: &Path) -> Result<Option<EvaluatedTarget>> {
    read_atomic_json(state_dir, LAST_TARGET_FILENAME)
}

/// Failure is non-fatal - next-fetch will retry.
pub fn write_last_fetch_outcome(state_dir: &Path, outcome: &FetchOutcome) -> Result<()> {
    write_atomic_json(state_dir, LAST_FETCH_OUTCOME_FILENAME, outcome)
}

pub fn read_last_fetch_outcome(state_dir: &Path) -> Result<Option<FetchOutcome>> {
    read_atomic_json(state_dir, LAST_FETCH_OUTCOME_FILENAME)
}

/// Best-effort delete on settled-state so the dashboard's "verify-failed"
/// badge doesn't stick on hosts that have recovered.
pub fn clear_last_fetch_outcome(state_dir: &Path) -> Result<()> {
    let path = state_dir.join(LAST_FETCH_OUTCOME_FILENAME);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(anyhow::anyhow!("remove {}: {err}", path.display())),
    }
}

/// `Ok(None)` for absent or malformed records.
pub fn read_last_deferred(state_dir: &Path) -> Result<Option<LastDeferredRecord>> {
    read_atomic_json(state_dir, LAST_DEFERRED_FILENAME)
}

/// First-defer write path; suppression short-circuits subsequent calls.
pub fn write_last_deferred(state_dir: &Path, record: &LastDeferredRecord) -> Result<()> {
    write_atomic_json(state_dir, LAST_DEFERRED_FILENAME, record)
}

/// Cleared on confirm success; stale records for non-current closure_hash
/// values are inert (suppression only matches on equal hashes).
pub fn clear_last_deferred(state_dir: &Path) -> Result<()> {
    let path = state_dir.join(LAST_DEFERRED_FILENAME);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(anyhow::anyhow!("remove {}: {err}", path.display())),
    }
}

/// `Ok(None)` for absent or malformed records.
pub fn read_last_failed_closure(state_dir: &Path) -> Result<Option<LastFailedClosureRecord>> {
    read_atomic_json(state_dir, LAST_FAILED_CLOSURE_FILENAME)
}

/// Raw write. Use `record_switch_failure` for increment-or-reset semantics;
/// the suppression handler uses this to update `last_quarantine_post_at`
/// without touching `failure_count`.
pub fn write_last_failed_closure(state_dir: &Path, record: &LastFailedClosureRecord) -> Result<()> {
    write_atomic_json(state_dir, LAST_FAILED_CLOSURE_FILENAME, record)
}

/// Increment `failure_count` on closure_hash match, else reset to 1. Single-
/// record design: two distinct closures failing in quick succession only
/// retain the most-recent count. Intentional - tracking every closure that
/// ever failed bloats state-dir indefinitely.
pub fn record_switch_failure(
    state_dir: &Path,
    closure_hash: &str,
    channel_ref: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<u32> {
    let existing = read_last_failed_closure(state_dir).unwrap_or(None);
    let new_record = match existing {
        Some(r) if r.closure_hash == closure_hash => LastFailedClosureRecord {
            closure_hash: r.closure_hash,
            channel_ref: r.channel_ref,
            last_failure_at: now,
            failure_count: r.failure_count.saturating_add(1),
            reason: reason.to_string(),
            // Preserve last_quarantine_post_at so a flapping closure doesn't
            // reset the throttle window every attempt.
            last_quarantine_post_at: r.last_quarantine_post_at,
        },
        _ => LastFailedClosureRecord {
            closure_hash: closure_hash.to_string(),
            channel_ref: channel_ref.to_string(),
            last_failure_at: now,
            failure_count: 1,
            reason: reason.to_string(),
            last_quarantine_post_at: None,
        },
    };
    let count = new_record.failure_count;
    write_last_failed_closure(state_dir, &new_record)?;
    Ok(count)
}

/// Best-effort delete. Currently unused; passive supersession handles the
/// common cases. Exposed for symmetry and as a hook for an explicit
/// "release this host from quarantine" admin action.
pub fn clear_last_failed_closure(state_dir: &Path) -> Result<()> {
    let path = state_dir.join(LAST_FAILED_CLOSURE_FILENAME);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(anyhow::anyhow!("remove {}: {err}", path.display())),
    }
}

// FOOTGUN: closure_hash is the FULL store basename, not the 32-char hash - wire-equality trap.
const CURRENT_SYSTEM: &str = "/run/current-system";

pub fn current_closure_hash() -> Result<String> {
    let target =
        std::fs::read_link(CURRENT_SYSTEM).with_context(|| format!("readlink {CURRENT_SYSTEM}"))?;
    Ok(closure_hash_from_path(&target))
}

/// FOOTGUN: returns full basename, NOT 32-char prefix - byte-equality required across CP/CI/agent.
pub(crate) fn closure_hash_from_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.rsplit('/')
        .next()
        .map(str::to_string)
        .unwrap_or_else(|| s.to_string())
}

pub fn uptime_secs(started_at: Instant) -> u64 {
    started_at.elapsed().as_secs()
}

/// Plain text `<closure_hash>\n<rfc3339>\n`. Manual line parsing lets the
/// reader do its own closure + skew checks.
pub fn write_last_confirmed(state_dir: &Path, closure_hash: &str, at: DateTime<Utc>) -> Result<()> {
    let body = format!("{closure_hash}\n{}\n", at.to_rfc3339());
    write_atomic(state_dir, LAST_CONFIRM_FILENAME, body.as_bytes())
}

/// LOADBEARING: same steps in the same order from BOTH dispatch and boot-
/// recovery. Without `write_last_target` the CP's outstanding-failure filter
/// sees every event forever. Best-effort; clears run LAST so a partial crash
/// leaves dispatch + defer records around for retry.
pub fn record_confirm_success(state_dir: &Path, target: &EvaluatedTarget, at: DateTime<Utc>) {
    if let Err(err) = write_last_confirmed(state_dir, &target.closure_hash, at) {
        tracing::warn!(
            error = %err,
            state_dir = %state_dir.display(),
            "write_last_confirmed failed; soak attestation will be missing on next checkin",
        );
    }
    if let Err(err) = write_last_target(state_dir, target) {
        tracing::warn!(
            error = %err,
            "write_last_target failed; checkin will report no last_evaluated_target until next confirm",
        );
    }
    if let Err(err) = clear_last_dispatched(state_dir) {
        tracing::warn!(error = %err, "clear_last_dispatched failed (non-fatal)");
    }
    // Confirm-after-reboot means the deferred sentinel is now stale; failure
    // is harmless (stale records only suppress on matching hashes).
    if let Err(err) = clear_last_deferred(state_dir) {
        tracing::warn!(error = %err, "clear_last_deferred failed (non-fatal)");
    }
}

/// `None` when absent, mismatched closure, malformed, or future-dated.
pub fn read_last_confirmed(
    state_dir: &Path,
    current_closure: &str,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    let path = state_dir.join(LAST_CONFIRM_FILENAME);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    let mut lines = raw.lines();
    let recorded_closure = match lines.next() {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(None),
    };
    let recorded_ts = match lines.next() {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(None),
    };
    if recorded_closure != current_closure {
        return Ok(None);
    }
    let parsed: DateTime<Utc> = match recorded_ts.parse() {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    if parsed > now {
        return Ok(None);
    }
    Ok(Some(parsed))
}

#[cfg(test)]
mod write_read_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_then_read_round_trips_when_closure_matches() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        let stamp = now - chrono::Duration::seconds(30);
        write_last_confirmed(dir.path(), "abc-system", stamp).unwrap();
        let got = read_last_confirmed(dir.path(), "abc-system", now)
            .unwrap()
            .expect("present");
        assert_eq!(got.timestamp(), stamp.timestamp());
    }

    #[test]
    fn read_returns_none_when_closure_mismatch() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        write_last_confirmed(dir.path(), "old-system", now).unwrap();
        let got = read_last_confirmed(dir.path(), "new-system", now).unwrap();
        assert!(
            got.is_none(),
            "rolled-back closure must not surface stale timestamp",
        );
    }

    #[test]
    fn read_returns_none_when_no_file() {
        let dir = TempDir::new().unwrap();
        let got = read_last_confirmed(dir.path(), "any", Utc::now()).unwrap();
        assert!(got.is_none(), "absent state file is the first-boot case");
    }

    #[test]
    fn read_returns_none_when_timestamp_future() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        let future = now + chrono::Duration::hours(1);
        write_last_confirmed(dir.path(), "abc-system", future).unwrap();
        let got = read_last_confirmed(dir.path(), "abc-system", now).unwrap();
        assert!(
            got.is_none(),
            "future-dated stamp suppressed (clock-skew / tamper guard)",
        );
    }

    #[test]
    fn read_returns_none_on_malformed_body() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(LAST_CONFIRM_FILENAME), "only-one-line").unwrap();
        let got = read_last_confirmed(dir.path(), "anything", Utc::now()).unwrap();
        assert!(got.is_none());
    }

    fn sample_dispatch() -> LastDispatchRecord {
        LastDispatchRecord {
            closure_hash: "abc-nixos-system".into(),
            channel_ref: "stable@deadbeef".into(),
            rollout_id: "stable@deadbeef".into(),
            compliance_mode: Some("enforce".into()),
            confirm_endpoint: "/v1/agent/confirm".into(),
            dispatched_at: Utc::now(),
        }
    }

    fn sample_target() -> EvaluatedTarget {
        EvaluatedTarget {
            closure_hash: "abc-nixos-system-test".into(),
            channel_ref: "stable@deadbeef".into(),
            evaluated_at: Utc::now(),
            rollout_id: "stable@deadbeef".into(),
            wave_index: Some(0),
            activate: None,
            signed_at: Utc::now(),
            freshness_window_secs: 3600,
            compliance_mode: Some("enforce".into()),
        }
    }

    #[test]
    fn last_target_round_trips() {
        let dir = TempDir::new().unwrap();
        let t = sample_target();
        write_last_target(dir.path(), &t).unwrap();
        let got = read_last_target(dir.path()).unwrap().expect("present");
        assert_eq!(got.closure_hash, t.closure_hash);
        assert_eq!(got.channel_ref, t.channel_ref);
        assert_eq!(got.rollout_id, t.rollout_id);
        assert_eq!(got.compliance_mode, t.compliance_mode);
    }

    #[test]
    fn last_target_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_last_target(dir.path()).unwrap().is_none());
    }

    #[test]
    fn last_target_malformed_returns_none() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(LAST_TARGET_FILENAME), "{not-json").unwrap();
        assert!(read_last_target(dir.path()).unwrap().is_none());
    }

    #[test]
    fn last_fetch_outcome_round_trips() {
        use nixfleet_proto::agent_wire::FetchResult;
        let dir = TempDir::new().unwrap();
        let outcome = FetchOutcome {
            result: FetchResult::VerifyFailed,
            error: Some("synthetic test".into()),
            rollout_id: Some("synthetic-rollout".into()),
        };
        write_last_fetch_outcome(dir.path(), &outcome).unwrap();
        let got = read_last_fetch_outcome(dir.path())
            .unwrap()
            .expect("present");
        assert_eq!(got.result, FetchResult::VerifyFailed);
        assert_eq!(got.error.as_deref(), Some("synthetic test"));
    }

    #[test]
    fn last_fetch_outcome_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_last_fetch_outcome(dir.path()).unwrap().is_none());
    }

    #[test]
    fn last_dispatched_round_trips() {
        let dir = TempDir::new().unwrap();
        let r = sample_dispatch();
        write_last_dispatched(dir.path(), &r).unwrap();
        let got = read_last_dispatched(dir.path()).unwrap().expect("present");
        assert_eq!(got.closure_hash, r.closure_hash);
        assert_eq!(got.channel_ref, r.channel_ref);
        assert_eq!(got.rollout_id, r.rollout_id);
    }

    #[test]
    fn last_dispatched_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_last_dispatched(dir.path()).unwrap().is_none());
    }

    #[test]
    fn last_dispatched_malformed_returns_none() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(LAST_DISPATCH_FILENAME), "{not-json").unwrap();
        assert!(read_last_dispatched(dir.path()).unwrap().is_none());
    }

    fn sample_deferred() -> LastDeferredRecord {
        LastDeferredRecord {
            closure_hash: "abc-nixos-system-test".into(),
            channel_ref: "stable@deadbeef".into(),
            component: "dbus".into(),
            deferred_at: Utc::now(),
        }
    }

    fn sample_failed() -> LastFailedClosureRecord {
        LastFailedClosureRecord {
            closure_hash: "broken-nixos-system-test".into(),
            channel_ref: "stable@deadbeef".into(),
            last_failure_at: Utc::now(),
            failure_count: 1,
            reason: "switch-poll-timeout exit=2".into(),
            last_quarantine_post_at: None,
        }
    }

    #[test]
    fn last_deferred_round_trips() {
        let dir = TempDir::new().unwrap();
        let r = sample_deferred();
        write_last_deferred(dir.path(), &r).unwrap();
        let got = read_last_deferred(dir.path()).unwrap().expect("present");
        assert_eq!(got.closure_hash, r.closure_hash);
        assert_eq!(got.channel_ref, r.channel_ref);
        assert_eq!(got.component, r.component);
    }

    #[test]
    fn last_deferred_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_last_deferred(dir.path()).unwrap().is_none());
    }

    #[test]
    fn clear_last_deferred_is_idempotent() {
        let dir = TempDir::new().unwrap();
        clear_last_deferred(dir.path()).unwrap();
        write_last_deferred(dir.path(), &sample_deferred()).unwrap();
        clear_last_deferred(dir.path()).unwrap();
        assert!(read_last_deferred(dir.path()).unwrap().is_none());
    }

    #[test]
    fn last_failed_closure_round_trips() {
        let dir = TempDir::new().unwrap();
        let r = sample_failed();
        write_last_failed_closure(dir.path(), &r).unwrap();
        let got = read_last_failed_closure(dir.path())
            .unwrap()
            .expect("present");
        assert_eq!(got.closure_hash, r.closure_hash);
        assert_eq!(got.failure_count, 1);
        assert_eq!(got.reason, r.reason);
        assert!(got.last_quarantine_post_at.is_none());
    }

    #[test]
    fn record_switch_failure_increments_count_for_same_hash() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        let n1 = record_switch_failure(
            dir.path(),
            "broken-h1",
            "stable@dead",
            "phase=switch-poll-timeout",
            now,
        )
        .unwrap();
        assert_eq!(n1, 1, "first failure should be count=1");
        let n2 = record_switch_failure(
            dir.path(),
            "broken-h1",
            "stable@dead",
            "phase=switch-poll-timeout",
            now + chrono::Duration::seconds(60),
        )
        .unwrap();
        assert_eq!(n2, 2, "second failure for same closure should be count=2");
    }

    #[test]
    fn record_switch_failure_resets_count_for_different_hash() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        record_switch_failure(dir.path(), "h-old", "stable@a", "phase=switch", now).unwrap();
        record_switch_failure(dir.path(), "h-old", "stable@a", "phase=switch", now).unwrap();
        let n = record_switch_failure(
            dir.path(),
            "h-new",
            "stable@b",
            "phase=verify",
            now + chrono::Duration::seconds(120),
        )
        .unwrap();
        assert_eq!(
            n, 1,
            "new closure_hash should reset failure_count to 1 (single-record overwrite)",
        );
        let stored = read_last_failed_closure(dir.path())
            .unwrap()
            .expect("present");
        assert_eq!(stored.closure_hash, "h-new");
        assert_eq!(stored.channel_ref, "stable@b");
    }

    #[test]
    fn record_switch_failure_preserves_quarantine_post_timestamp_across_same_hash() {
        // Throttle invariant: a same-hash flap shouldn't bypass the 1h
        // post-throttle window by clearing last_quarantine_post_at on each
        // new failure. The repost timer is governed by wall-clock since
        // last post, not since last failure.
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        record_switch_failure(dir.path(), "h", "ch", "r1", now).unwrap();
        // Simulate a quarantine post landing in the record.
        let mut r = read_last_failed_closure(dir.path()).unwrap().unwrap();
        r.last_quarantine_post_at = Some(now);
        write_last_failed_closure(dir.path(), &r).unwrap();
        // Another same-hash failure 5 minutes later.
        record_switch_failure(
            dir.path(),
            "h",
            "ch",
            "r2",
            now + chrono::Duration::seconds(300),
        )
        .unwrap();
        let after = read_last_failed_closure(dir.path()).unwrap().unwrap();
        assert_eq!(after.failure_count, 2);
        assert_eq!(
            after.last_quarantine_post_at,
            Some(now),
            "post-throttle ts preserved"
        );
    }

    #[test]
    fn last_failed_closure_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_last_failed_closure(dir.path()).unwrap().is_none());
    }

    #[test]
    fn clear_last_failed_closure_is_idempotent() {
        let dir = TempDir::new().unwrap();
        clear_last_failed_closure(dir.path()).unwrap();
        write_last_failed_closure(dir.path(), &sample_failed()).unwrap();
        clear_last_failed_closure(dir.path()).unwrap();
        assert!(read_last_failed_closure(dir.path()).unwrap().is_none());
    }

    #[test]
    fn record_confirm_success_clears_last_deferred() {
        // Issue #56: post-reboot retroactive confirm must wipe the
        // suppression sentinel so a subsequent activation isn't silently
        // suppressed by a stale entry.
        let dir = TempDir::new().unwrap();
        write_last_deferred(dir.path(), &sample_deferred()).unwrap();
        record_confirm_success(dir.path(), &sample_target(), Utc::now());
        assert!(
            read_last_deferred(dir.path()).unwrap().is_none(),
            "record_confirm_success must clear last_deferred",
        );
    }

    #[test]
    fn clear_last_dispatched_is_idempotent() {
        let dir = TempDir::new().unwrap();
        clear_last_dispatched(dir.path()).unwrap();
        write_last_dispatched(dir.path(), &sample_dispatch()).unwrap();
        clear_last_dispatched(dir.path()).unwrap();
        assert!(read_last_dispatched(dir.path()).unwrap().is_none());
        clear_last_dispatched(dir.path()).unwrap();
    }

    /// Regression: boot-recovery's confirm-success path used to skip
    /// `write_last_target`, which silently broke the CP's outstanding-failure
    /// filter and active-rollouts panel. Both call sites (dispatch + recovery)
    /// now go through `record_confirm_success`; this test pins all three side
    /// effects so adding a fourth state file forces a test update.
    #[test]
    fn record_confirm_success_writes_all_three_files_and_clears_dispatch() {
        let dir = TempDir::new().unwrap();
        write_last_dispatched(dir.path(), &sample_dispatch()).unwrap();
        let target = sample_target();
        let now = Utc::now();

        record_confirm_success(dir.path(), &target, now);

        let confirmed = read_last_confirmed(dir.path(), &target.closure_hash, now)
            .unwrap()
            .expect("last_confirmed must exist after record_confirm_success");
        assert_eq!(confirmed.timestamp(), now.timestamp());

        let recorded_target = read_last_target(dir.path())
            .unwrap()
            .expect("last_target must exist after record_confirm_success");
        assert_eq!(recorded_target.closure_hash, target.closure_hash);
        assert_eq!(recorded_target.rollout_id, target.rollout_id);

        assert!(
            read_last_dispatched(dir.path()).unwrap().is_none(),
            "last_dispatched must be cleared after successful confirm",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closure_hash_is_full_basename_not_hash_prefix() {
        let p: PathBuf =
            "/nix/store/2zlnf66xlf35xwm7150kx05q93cwp8jk-nixos-system-host-05-20260427-0810_5176864f_turbo-otter"
                .into();
        let got = closure_hash_from_path(&p);
        assert_eq!(
            got,
            "2zlnf66xlf35xwm7150kx05q93cwp8jk-nixos-system-host-05-20260427-0810_5176864f_turbo-otter",
            "closure_hash must be the full /nix/store basename - same shape the CP declares",
        );
        assert_ne!(got, "2zlnf66xlf35xwm7150kx05q93cwp8jk");
    }

    #[test]
    fn closure_hash_falls_back_to_full_path_for_non_store_shape() {
        let p: PathBuf = "/some/odd/path".into();
        let got = closure_hash_from_path(&p);
        assert_eq!(got, "path", "rsplit/next still returns the leaf");
    }
}
