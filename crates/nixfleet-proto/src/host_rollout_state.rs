//! Per-host rollout state machine. LOADBEARING: single source of truth for
//! both CP (SQL CHECK round-trip) and reconciler decision-procedure - adding
//! a state requires updating the SQL CHECK constraint in the CP migration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRolloutStateParseError {
    pub got: String,
}

impl std::fmt::Display for HostRolloutStateParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown host_rollout_state: {:?}", self.got)
    }
}

impl std::error::Error for HostRolloutStateParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostRolloutState {
    Queued,
    Dispatched,
    Activating,
    ConfirmWindow,
    Healthy,
    Soaked,
    Converged,
    Reverted,
    Failed,
}

impl HostRolloutState {
    /// Canonical literal - matches the SQL CHECK and `observed.json`.
    pub fn as_db_str(&self) -> &'static str {
        match self {
            HostRolloutState::Queued => "Queued",
            HostRolloutState::Dispatched => "Dispatched",
            HostRolloutState::Activating => "Activating",
            HostRolloutState::ConfirmWindow => "ConfirmWindow",
            HostRolloutState::Healthy => "Healthy",
            HostRolloutState::Soaked => "Soaked",
            HostRolloutState::Converged => "Converged",
            HostRolloutState::Reverted => "Reverted",
            HostRolloutState::Failed => "Failed",
        }
    }

    pub fn from_db_str(s: &str) -> Result<Self, HostRolloutStateParseError> {
        match s {
            "Queued" => Ok(HostRolloutState::Queued),
            "Dispatched" => Ok(HostRolloutState::Dispatched),
            "Activating" => Ok(HostRolloutState::Activating),
            "ConfirmWindow" => Ok(HostRolloutState::ConfirmWindow),
            "Healthy" => Ok(HostRolloutState::Healthy),
            "Soaked" => Ok(HostRolloutState::Soaked),
            "Converged" => Ok(HostRolloutState::Converged),
            "Reverted" => Ok(HostRolloutState::Reverted),
            "Failed" => Ok(HostRolloutState::Failed),
            other => Err(HostRolloutStateParseError {
                got: other.to_string(),
            }),
        }
    }

    /// Host has cleared its observable activation (soak window passed or
    /// rollout reached Converged). Both Soaked and Converged count: treating
    /// only Converged as terminal would hold the successor across the gap
    /// between SoakHost and the next reconcile tick. `Failed`/`Reverted` are
    /// NOT terminal-for-ordering - successor must wait for operator action.
    pub fn is_terminal_for_ordering(&self) -> bool {
        matches!(self, Self::Soaked | Self::Converged)
    }

    /// Host is consuming a disruption-budget slot.
    pub fn is_in_flight(&self) -> bool {
        matches!(
            self,
            Self::Dispatched | Self::Activating | Self::ConfirmWindow | Self::Healthy
        )
    }

    /// Stuck and staying stuck; needs operator action. Distinct from
    /// `is_terminal_for_ordering` because Failed/Reverted hosts must hold
    /// their successor.
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed | Self::Reverted)
    }
}

#[cfg(feature = "rusqlite")]
mod rusqlite_impls {
    use super::*;
    use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

    impl ToSql for HostRolloutState {
        fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
            Ok(ToSqlOutput::Borrowed(self.as_db_str().into()))
        }
    }

    impl FromSql for HostRolloutState {
        fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
            let s = value.as_str()?;
            Self::from_db_str(s).map_err(|e| FromSqlError::Other(Box::new(e)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_known_values() {
        for v in [
            HostRolloutState::Queued,
            HostRolloutState::Dispatched,
            HostRolloutState::Activating,
            HostRolloutState::ConfirmWindow,
            HostRolloutState::Healthy,
            HostRolloutState::Soaked,
            HostRolloutState::Converged,
            HostRolloutState::Reverted,
            HostRolloutState::Failed,
        ] {
            assert_eq!(HostRolloutState::from_db_str(v.as_db_str()).unwrap(), v);
        }
    }

    #[test]
    fn unknown_strings_error() {
        assert!(HostRolloutState::from_db_str("").is_err());
        assert!(HostRolloutState::from_db_str("healthy").is_err());
        assert!(HostRolloutState::from_db_str("soaked").is_err());
        assert!(HostRolloutState::from_db_str("Healhty").is_err());
    }
}
