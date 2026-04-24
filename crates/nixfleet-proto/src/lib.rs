//! NixFleet v0.2 boundary-contract types.
//!
//! Every type in this crate mirrors an artifact declared in
//! `docs/CONTRACTS.md §I`. Changes here are contract changes and
//! follow the amendment procedure in §VII.
//!
//! # Unknown-field posture
//!
//! Per `docs/CONTRACTS.md §V` every contract evolves additively
//! within its major version, and consumers MUST ignore unknown
//! fields. Serde defaults to ignoring unknown fields; no type in
//! this crate uses `#[serde(deny_unknown_fields)]`.
//!
//! # Optional-field posture
//!
//! Optional fields use `Option<T>` with `#[serde(default)]` and
//! WITHOUT `skip_serializing_if`. This matches Stream B's emitted
//! shape, where `null` is present on unset optional fields rather
//! than the field being omitted entirely. JCS canonical bytes are
//! thereby byte-identical across Nix emission and Rust round-trip.
//!
//! Fields that are only present in some artifacts (e.g. `meta` on
//! a signed vs unsigned fixture) are handled at the domain level,
//! not the serde level.

pub mod fleet_resolved;
pub mod trust;

pub use fleet_resolved::{
    Channel, Compliance, ComplianceProbes, DisruptionBudget, Edge, FleetResolved, HealthGate, Host,
    Meta, PolicyWave, RolloutPolicy, Selector, SystemdFailedUnits, Wave,
};
pub use trust::{AtticKeySlot, AtticPubkey, KeySlot, TrustConfig, TrustedPubkey};
