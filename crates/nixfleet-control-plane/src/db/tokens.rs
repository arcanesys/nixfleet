//! Bootstrap-token nonces (soft state); loss bounded by one TTL.

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::sync::Mutex;

pub struct Tokens<'a> {
    pub(super) conn: &'a Mutex<Connection>,
}

/// Distinguishes concurrent-replay race (409) from transient DB failure (500).
#[derive(Debug, PartialEq, Eq)]
pub enum RecordTokenOutcome {
    Recorded,
    AlreadyRecorded,
}

impl Tokens<'_> {
    pub fn token_seen(&self, nonce: &str) -> Result<bool> {
        super::read(self.conn, |c| {
            c.query_row(
                "SELECT 1 FROM token_replay WHERE nonce = ?1",
                params![nonce],
                |_| Ok(true),
            )
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(false),
                e => Err(e),
            })
            .context("query token_replay")
        })
    }

    /// Plain INSERT (not OR IGNORE): PK conflict surfaces as `AlreadyRecorded` for atomic check-and-set.
    pub fn record_token_nonce(&self, nonce: &str, hostname: &str) -> Result<RecordTokenOutcome> {
        super::read(self.conn, |c| {
            match c.execute(
                "INSERT INTO token_replay(nonce, hostname) VALUES (?1, ?2)",
                params![nonce, hostname],
            ) {
                Ok(_) => Ok(RecordTokenOutcome::Recorded),
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Ok(RecordTokenOutcome::AlreadyRecorded)
                }
                Err(e) => Err(anyhow::Error::from(e).context("insert token_replay")),
            }
        })
    }

    pub fn prune_token_replay(&self, max_age_hours: i64) -> Result<usize> {
        super::read(self.conn, |c| {
            c.execute(
                "DELETE FROM token_replay
                 WHERE first_seen < datetime('now', ?1)",
                params![format!("-{max_age_hours} hours")],
            )
            .context("prune token_replay")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::fresh_db;

    #[test]
    fn token_replay_round_trip() {
        let db = fresh_db();
        assert!(!db.tokens().token_seen("nonce-1").unwrap());
        let outcome = db
            .tokens()
            .record_token_nonce("nonce-1", "test-host")
            .unwrap();
        assert_eq!(outcome, super::RecordTokenOutcome::Recorded);
        assert!(db.tokens().token_seen("nonce-1").unwrap());
    }

    #[test]
    fn record_token_nonce_returns_already_recorded_on_repeat() {
        let db = fresh_db();
        let first = db
            .tokens()
            .record_token_nonce("nonce-1", "test-host")
            .unwrap();
        assert_eq!(first, super::RecordTokenOutcome::Recorded);

        let second = db
            .tokens()
            .record_token_nonce("nonce-1", "test-host")
            .unwrap();
        assert_eq!(second, super::RecordTokenOutcome::AlreadyRecorded);
    }
}
