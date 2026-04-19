//! Wire-type re-exports from the shared `nixfleet-types` crate so the
//! agent and control plane share identical types at the source level.
//!
//! `MachineStatus` is available as `nixfleet_types::MachineStatus`.

pub use nixfleet_types::{DesiredGeneration, Report};
