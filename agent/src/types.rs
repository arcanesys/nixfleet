/// Re-export shared types from nixfleet-types.
///
/// This module previously defined DesiredGeneration, Report, and MachineStatus
/// locally. They now live in the shared `nixfleet-types` crate so the control
/// plane and agent share identical wire types.
pub use nixfleet_types::{DesiredGeneration, Report};

// MachineStatus is available via nixfleet_types::MachineStatus if needed.

