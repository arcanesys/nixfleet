//! Typed enums for CP persistence rows. ToSql/FromSql route through
//! `as_db_str`/`from_db_str` so SQL boundaries stay strongly typed.

use anyhow::{anyhow, Result};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

/// Per-host activation lifecycle persisted in `host_dispatch_state.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PendingConfirmState {
    Pending,
    Confirmed,
    RolledBack,
    Cancelled,
}

impl PendingConfirmState {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            PendingConfirmState::Pending => "pending",
            PendingConfirmState::Confirmed => "confirmed",
            PendingConfirmState::RolledBack => "rolled-back",
            PendingConfirmState::Cancelled => "cancelled",
        }
    }

    /// Errors loudly on unknown strings to surface schema drift.
    pub fn from_db_str(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(PendingConfirmState::Pending),
            "confirmed" => Ok(PendingConfirmState::Confirmed),
            "rolled-back" => Ok(PendingConfirmState::RolledBack),
            "cancelled" => Ok(PendingConfirmState::Cancelled),
            other => Err(anyhow!("unknown host_dispatch_state.state: {other:?}")),
        }
    }
}

impl ToSql for PendingConfirmState {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(self.as_db_str().into()))
    }
}

impl FromSql for PendingConfirmState {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::from_db_str(s).map_err(|e| FromSqlError::Other(e.into()))
    }
}

/// Terminal class on `dispatch_history.terminal_state`; distinct from operational state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalState {
    Converged,
    RolledBack,
    Cancelled,
}

impl TerminalState {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            TerminalState::Converged => "converged",
            TerminalState::RolledBack => "rolled-back",
            TerminalState::Cancelled => "cancelled",
        }
    }
}

impl ToSql for TerminalState {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(self.as_db_str().into()))
    }
}

pub use nixfleet_proto::HostRolloutState;

/// Side-channel mutation of `last_healthy_since`; orthogonal to state transitions.
#[derive(Debug, Clone, Copy)]
pub enum HealthyMarker {
    Set(chrono::DateTime<chrono::Utc>),
    Untouched,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_known_values() {
        for v in [
            PendingConfirmState::Pending,
            PendingConfirmState::Confirmed,
            PendingConfirmState::RolledBack,
            PendingConfirmState::Cancelled,
        ] {
            assert_eq!(PendingConfirmState::from_db_str(v.as_db_str()).unwrap(), v);
        }
    }

    #[test]
    fn unknown_strings_error() {
        assert!(PendingConfirmState::from_db_str("").is_err());
        assert!(PendingConfirmState::from_db_str("Pending").is_err());
        assert!(PendingConfirmState::from_db_str("rolledback").is_err());
    }

    #[test]
    fn terminal_state_literals_match_check_constraint() {
        assert_eq!(TerminalState::Converged.as_db_str(), "converged");
        assert_eq!(TerminalState::RolledBack.as_db_str(), "rolled-back");
        assert_eq!(TerminalState::Cancelled.as_db_str(), "cancelled");
    }
}
