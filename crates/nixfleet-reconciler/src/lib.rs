#![allow(clippy::doc_lazy_continuation)]
//! Pure-function rollout reconciler + sidecar verification. Stateless.

pub mod action;
pub mod evidence;
pub mod gates;
pub mod host_state;
pub mod manifest;
pub mod observed;
pub mod reconcile;
pub mod rollout_state;
pub mod trust_rotation;
pub mod verify;

pub use action::Action;
pub use host_state::HostRolloutState;
pub use manifest::{compute_rollout_id_for_channel, current_rollout_ids, project_manifest};
pub use nixfleet_proto::FleetResolved;
pub use observed::{HostState, Observed, Rollout};
pub use reconcile::{reconcile, topological_channel_order};
pub use rollout_state::RolloutState;
pub use trust_rotation::check_trust_rotations;
pub use verify::{
    canonical_hash_from_bytes, compute_canonical_hash, compute_rollout_id, rollout_id_from_bytes,
    verify_artifact, verify_bootstrap_nonces, verify_revocations, verify_rollout_manifest,
    verify_signed_sidecar,
    SignedSidecar, VerifyError,
};
