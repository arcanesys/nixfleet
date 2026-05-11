//! Per-host OS primitives (`boot_id`, `pending_generation`); cfg-gated re-export.

use anyhow::Result;
use nixfleet_proto::agent_wire::GenerationRef;

use crate::checkin_state::current_closure_hash;

#[cfg(target_os = "macos")]
mod darwin;
#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
pub use darwin::{boot_id, pending_generation};
#[cfg(target_os = "linux")]
pub use linux::{boot_id, pending_generation};

/// `channel_ref` is `None` until the projection correlates it.
pub fn current_generation_ref() -> Result<GenerationRef> {
    Ok(GenerationRef {
        closure_hash: current_closure_hash()?,
        channel_ref: None,
        boot_id: boot_id()?,
    })
}
