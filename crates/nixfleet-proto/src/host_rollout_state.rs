//! Per-host rollout state machine.
//!
//! LOADBEARING: single source of truth for both CP (SQL CHECK round-trip)
//! and reconciler decision-procedure. Don't fork the variant set — adding
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
    /// Canonical literal — matches the SQL CHECK and `observed.json`.
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

    /// Terminal-for-ordering: host has cleared its observable activation
    /// (soak window passed or rollout reached Converged). Used by:
    ///   - `gates::channel_edges` (predecessor channel done?)
    ///   - `gates::host_edges` (gating host done?)
    ///
    /// Why both `Soaked` and `Converged`: treating only `Converged` as
    /// terminal would leave the gap between SoakHost transitions and the
    /// next reconcile tick's `ConvergeRollout` action holding the
    /// successor — small in practice but semantically wrong (a Soaked
    /// host has finished its observable activation).
    ///
    /// `Failed` / `Reverted` are NOT terminal-for-ordering: predecessor
    /// is in trouble, operator action is needed, successor must wait.
    pub fn is_terminal_for_ordering(&self) -> bool {
        matches!(self, Self::Soaked | Self::Converged)
    }

    /// In-flight: host is consuming a disruption-budget slot. Used by
    /// `gates::disruption_budget::in_flight_count` for cross-rollout
    /// budget enforcement.
    pub fn is_in_flight(&self) -> bool {
        matches!(
            self,
            Self::Dispatched | Self::Activating | Self::ConfirmWindow | Self::Healthy
        )
    }

    /// Stuck-and-staying-stuck: needs operator action. Distinct from
    /// `is_terminal_for_ordering` because Failed/Reverted hosts must
    /// hold their successor (the rollout is in trouble).
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed | Self::Reverted)
    }

    /// Stable numeric encoding for tabular dashboards (Grafana value
    /// mappings render the integer as the state name).
    ///
    /// Ordering is lifecycle-progress ascending: a host with a higher
    /// code has either advanced further OR fallen into a failure
    /// terminal. Failed/Reverted sit at 7/8 so threshold colouring
    /// (`>=7 → red`) flags them without per-state mappings.
    ///
    /// LOADBEARING: changing a code is a wire-format break for the
    /// dashboard's value mappings — bump the dashboard JSON in lock-step.
    pub fn state_code(&self) -> u32 {
        match self {
            Self::Queued => 0,
            Self::Dispatched => 1,
            Self::Activating => 2,
            Self::ConfirmWindow => 3,
            Self::Healthy => 4,
            Self::Soaked => 5,
            Self::Converged => 6,
            Self::Failed => 7,
            Self::Reverted => 8,
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

    #[test]
    fn state_codes_are_distinct_and_lifecycle_ordered() {
        let codes: Vec<u32> = [
            HostRolloutState::Queued,
            HostRolloutState::Dispatched,
            HostRolloutState::Activating,
            HostRolloutState::ConfirmWindow,
            HostRolloutState::Healthy,
            HostRolloutState::Soaked,
            HostRolloutState::Converged,
            HostRolloutState::Failed,
            HostRolloutState::Reverted,
        ]
        .iter()
        .map(|s| s.state_code())
        .collect();
        // Distinct.
        let mut sorted = codes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "state_code values must be unique");
        // Lifecycle-ascending up through Converged, then failure terminals.
        assert!(
            codes[0] < codes[1] && codes[1] < codes[2] && codes[2] < codes[3]
                && codes[3] < codes[4] && codes[4] < codes[5] && codes[5] < codes[6],
            "Queued..Converged must be strictly ascending: {codes:?}",
        );
        assert!(
            HostRolloutState::Failed.state_code() > HostRolloutState::Converged.state_code(),
            "Failed must rank above Converged so threshold colouring fires",
        );
        assert!(
            HostRolloutState::Reverted.state_code() > HostRolloutState::Converged.state_code(),
        );
    }
}
