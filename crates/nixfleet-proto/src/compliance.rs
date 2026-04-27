//! Compliance gate policy mode. Shared by mk-fleet, agent, and CP.
//! Unknown strings fall back to `Permissive` (failsafe: an operator who
//! set the field clearly wanted the gate active).

use serde::{Deserialize, Serialize};

/// Resolved gate mode. `Auto` is agent-side input only, never on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateMode {
    Disabled,
    /// Posts events on failure but does not block dispatch / confirm.
    Permissive,
    /// Failures block dispatch / confirm and trigger recovery.
    Enforce,
}

impl GateMode {
    /// `disabled`/`enforce` map directly; everything else (incl. `auto`,
    /// unknown) -> `Permissive`.
    pub fn from_wire_str(s: &str) -> Self {
        match s {
            "disabled" => GateMode::Disabled,
            "enforce" => GateMode::Enforce,
            _ => GateMode::Permissive,
        }
    }

    pub fn is_enforcing(self) -> bool {
        matches!(self, GateMode::Enforce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_wire_str_known_values() {
        assert_eq!(GateMode::from_wire_str("disabled"), GateMode::Disabled);
        assert_eq!(GateMode::from_wire_str("permissive"), GateMode::Permissive);
        assert_eq!(GateMode::from_wire_str("enforce"), GateMode::Enforce);
    }

    #[test]
    fn from_wire_str_unknown_falls_back_permissive() {
        assert_eq!(GateMode::from_wire_str("auto"), GateMode::Permissive);
        assert_eq!(GateMode::from_wire_str(""), GateMode::Permissive);
        assert_eq!(GateMode::from_wire_str("garbage"), GateMode::Permissive);
    }

    #[test]
    fn is_enforcing_only_for_enforce() {
        assert!(GateMode::Enforce.is_enforcing());
        assert!(!GateMode::Permissive.is_enforcing());
        assert!(!GateMode::Disabled.is_enforcing());
    }
}
