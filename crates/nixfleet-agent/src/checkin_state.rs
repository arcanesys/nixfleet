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
/// + outstandingComplianceFailures filter have something to compare against.
/// Distinct from `last_dispatched` (cleared on confirm) — this one persists.
pub const LAST_TARGET_FILENAME: &str = "last_target";

/// CP's circuit breaker (`Decision::HoldAfterFailure`): a host stuck on bad
/// bytes stops being re-dispatched until a clean fetch shows up.
pub const LAST_FETCH_OUTCOME_FILENAME: &str = "last_fetch_outcome";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LastDispatchRecord {
    pub closure_hash: String,
    pub channel_ref: String,
    pub rollout_id: String,
    /// Channel's compliance mode at dispatch time; consumed by boot-recovery
    /// to run the runtime gate on the activated closure before retroactive confirm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compliance_mode: Option<String>,
    /// Wire-carried confirm endpoint from `target.activate.confirm_endpoint`.
    /// Required: we only persist `last_dispatched` for confirmable targets,
    /// so a record without an endpoint is impossible by construction.
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
    std::fs::rename(&tmp_path, &final_path).with_context(|| {
        format!("rename {} -> {}", tmp_path.display(), final_path.display())
    })?;
    Ok(())
}

fn write_atomic_json<T: serde::Serialize>(
    state_dir: &Path,
    filename: &str,
    value: &T,
) -> Result<()> {
    let body = serde_json::to_string(value)
        .with_context(|| format!("serialize {filename}"))?;
    write_atomic(state_dir, filename, body.as_bytes())
}

/// `Ok(None)` for both absent and malformed JSON; `Err` only on FS I/O failures.
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
    Ok(serde_json::from_str::<T>(&raw).ok())
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

/// Failure is non-fatal — next-fetch will retry.
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

// FOOTGUN: closure_hash is the FULL store basename, not the 32-char hash — wire-equality trap.
const CURRENT_SYSTEM: &str = "/run/current-system";

pub fn current_closure_hash() -> Result<String> {
    let target = std::fs::read_link(CURRENT_SYSTEM)
        .with_context(|| format!("readlink {CURRENT_SYSTEM}"))?;
    Ok(closure_hash_from_path(&target))
}

/// FOOTGUN: returns full basename, NOT 32-char prefix — byte-equality required across CP/CI/agent.
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

/// `<closure_hash>\n<rfc3339>\n` plain-text — `read_last_confirmed` does its
/// own line parsing + closure/skew checks, so JSON would just add ceremony.
pub fn write_last_confirmed(state_dir: &Path, closure_hash: &str, at: DateTime<Utc>) -> Result<()> {
    let body = format!("{closure_hash}\n{}\n", at.to_rfc3339());
    write_atomic(state_dir, LAST_CONFIRM_FILENAME, body.as_bytes())
}

/// LOADBEARING: same three persistence steps in the same order from BOTH
/// dispatch and boot-recovery — without `write_last_target` the CP's
/// outstanding-failure filter sees every recorded event forever. Each step
/// is best-effort; `clear_last_dispatched` runs last so a partial crash
/// leaves the dispatch record around for retry.
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
            "/nix/store/2zlnf66xlf35xwm7150kx05q93cwp8jk-nixos-system-lab-20260427-0810_5176864f_turbo-otter"
                .into();
        let got = closure_hash_from_path(&p);
        assert_eq!(
            got,
            "2zlnf66xlf35xwm7150kx05q93cwp8jk-nixos-system-lab-20260427-0810_5176864f_turbo-otter",
            "closure_hash must be the full /nix/store basename — same shape the CP declares",
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
