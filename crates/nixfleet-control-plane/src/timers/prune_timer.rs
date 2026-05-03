//! Hourly SQLite + backup-file hygiene sweep; idempotent steps, kill-safe at any tick.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio_util::sync::CancellationToken;

use crate::db::Db;

const TICK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const TOKEN_REPLAY_RETENTION_HOURS: i64 = 24;
const DISPATCH_HISTORY_RETENTION_HOURS: i64 = 24 * 90;
const HOST_REPORTS_RETENTION_HOURS: i64 = 24 * 7;
/// Match `dispatch_history` (90d) - the rollouts table is the
/// other side of the same audit story. Operators investigating a
/// 60-day-old release on host-05 still want to see the per-host
/// states it produced, not just the dispatch records.
const FINISHED_ROLLOUTS_RETENTION_HOURS: i64 = 24 * 90;
const BACKUP_RETENTION_DAYS: u64 = 14;
const BACKUP_FILENAME_PREFIX: &str = "state.db.pre-";

/// `db_path = None` skips the filesystem backup sweep (in-memory deployments).
pub fn spawn(
    cancel: CancellationToken,
    db: Arc<Db>,
    db_path: Option<PathBuf>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!(target: "shutdown", task = "prune_timer", "task shut down");
                    return;
                }
                _ = ticker.tick() => {}
            }
            let token_pruned = try_prune("token_replay", || {
                db.tokens().prune_token_replay(TOKEN_REPLAY_RETENTION_HOURS)
            });
            let history_pruned = try_prune("dispatch_history", || {
                db.dispatch_history()
                    .prune_history(DISPATCH_HISTORY_RETENTION_HOURS)
            });
            let reports_pruned = try_prune("host_reports", || {
                db.reports()
                    .prune_host_reports(HOST_REPORTS_RETENTION_HOURS)
            });
            let (hrs_pruned, rollouts_pruned) = match db
                .rollouts()
                .prune_finished_rollouts(FINISHED_ROLLOUTS_RETENTION_HOURS)
            {
                Ok(pair) => pair,
                Err(err) => {
                    tracing::warn!(error = %err, "prune timer: finished_rollouts failed");
                    (0, 0)
                }
            };
            let backups_pruned = db_path
                .as_deref()
                .and_then(Path::parent)
                .map(|parent| {
                    try_prune("state.db backup sweep", || {
                        prune_backup_files(parent, BACKUP_FILENAME_PREFIX, BACKUP_RETENTION_DAYS)
                    })
                })
                .unwrap_or(0);
            tracing::info!(
                target: "prune",
                token_replay = token_pruned,
                dispatch_history = history_pruned,
                host_reports = reports_pruned,
                host_rollout_state = hrs_pruned,
                rollouts = rollouts_pruned,
                state_db_backups = backups_pruned,
                "prune timer: hourly sweep complete",
            );
        }
    })
}

/// On `Err` logs a warn and returns 0 so the sweep continues.
fn try_prune<E>(name: &str, f: impl FnOnce() -> std::result::Result<usize, E>) -> usize
where
    E: std::fmt::Display,
{
    match f() {
        Ok(n) => n,
        Err(err) => {
            tracing::warn!(error = %err, "prune timer: {name} failed");
            0
        }
    }
}

/// Per-file delete errors are logged + skipped; enumeration errors propagate.
pub(crate) fn prune_backup_files(
    parent: &Path,
    prefix: &str,
    retention_days: u64,
) -> std::io::Result<usize> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(retention_days * 24 * 60 * 60))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let mut deleted = 0usize;
    let entries = match std::fs::read_dir(parent) {
        Ok(it) => it,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(error = %err, "prune timer: read_dir entry failed");
                continue;
            }
        };
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.starts_with(prefix) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!(
                    file = %name_str,
                    error = %err,
                    "prune timer: backup metadata failed",
                );
                continue;
            }
        };
        if !metadata.is_file() {
            continue;
        }
        let mtime = match metadata.modified() {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(
                    file = %name_str,
                    error = %err,
                    "prune timer: backup mtime unavailable",
                );
                continue;
            }
        };
        if mtime >= cutoff {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(
                    target: "prune",
                    file = %path.display(),
                    "pruned stale state.db backup",
                );
                deleted += 1;
            }
            Err(err) => {
                tracing::warn!(
                    file = %path.display(),
                    error = %err,
                    "prune timer: backup delete failed",
                );
            }
        }
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(path: &Path, age: Duration) {
        let f = std::fs::File::create(path).unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
    }

    #[test]
    fn prune_backup_files_drops_old_keeps_young() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("state.db.pre-phase2-20240101-000000");
        let young = dir.path().join("state.db.pre-phase2-20260430-235959");
        let unrelated = dir.path().join("state.db");
        touch(&old, Duration::from_secs(30 * 24 * 60 * 60));
        touch(&young, Duration::from_secs(60));
        touch(&unrelated, Duration::from_secs(30 * 24 * 60 * 60));

        let pruned = prune_backup_files(dir.path(), "state.db.pre-", 14).unwrap();
        assert_eq!(pruned, 1);
        assert!(!old.exists(), "old backup should be deleted");
        assert!(young.exists(), "young backup should be kept");
        assert!(unrelated.exists(), "non-backup file should be untouched");
    }

    #[test]
    fn prune_backup_files_returns_zero_when_dir_missing() {
        let n = prune_backup_files(
            Path::new("/nonexistent/path/that/should/not/exist"),
            "state.db.pre-",
            14,
        )
        .unwrap();
        assert_eq!(n, 0);
    }
}
