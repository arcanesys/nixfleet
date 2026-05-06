#![allow(clippy::doc_lazy_continuation)]
//! Boundary-contract types. Optional fields serialize `null` (not omitted)
//! to match the Nix evaluator's shape so JCS bytes round-trip identically.

pub mod agent_wire;
pub mod compliance;
pub mod enroll_wire;
pub mod evidence_signing;
pub mod fleet_resolved;
pub mod fleet_view;
pub mod host_key;
pub mod host_rollout_state;
pub mod revocations;
pub mod rollout_manifest;
pub mod trust;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use fleet_resolved::{
    Channel, ChannelEdge, Compliance, ComplianceProbes, DisruptionBudget, Edge, FleetResolved,
    HealthGate, Host, Meta, OnHealthFailure, PolicyWave, RolloutPolicy, Selector,
    SystemdFailedUnits, Wave,
};
pub use fleet_view::{HostStatusEntry, HostsResponse, RolloutTrace, RolloutTraceEvent};
pub use host_rollout_state::HostRolloutState;
pub use revocations::{RevocationEntry, Revocations};
pub use rollout_manifest::{HostWave, RolloutBudget, RolloutManifest};
pub use trust::{KeySlot, TrustConfig, TrustedPubkey};
