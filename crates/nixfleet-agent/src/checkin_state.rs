//! System introspection for checkin body assembly.
//!
//! Reads what the agent reports about itself: closure hash, pending
//! generation, boot ID. All file I/O is `std::fs::*` — these are
//! tiny reads of /run + /proc, no async needed.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use nixfleet_proto::agent_wire::{GenerationRef, PendingGeneration};

/// Path to the symlink pointing at the currently active system
/// closure. Reading it as a symlink target gives us the closure
/// store path; basename trimmed of the `-system-<rev>` suffix is
/// the closure hash.
const CURRENT_SYSTEM: &str = "/run/current-system";

/// Path to the symlink pointing at the system that booted. When
/// this differs from `/run/current-system`, the host has a pending
/// generation queued for next reboot.
const BOOTED_SYSTEM: &str = "/run/booted-system";

/// Linux's per-boot UUID. Stable for a single boot; rotates on
/// reboot. Used by the CP to detect that a host actually rebooted
/// (e.g. correlated with `pendingGeneration` clearing on next
/// checkin).
const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";

/// Read `/run/current-system`'s symlink target and extract the
/// store-path closure hash (the 32-char nix-store hash before the
/// `-` separator). Returns the full store path on platforms where
/// the symlink target shape doesn't match the expected pattern, so
/// the agent still reports something rather than failing the
/// checkin.
pub fn current_closure_hash() -> Result<String> {
    let target = std::fs::read_link(CURRENT_SYSTEM)
        .with_context(|| format!("readlink {CURRENT_SYSTEM}"))?;
    Ok(closure_hash_from_path(&target))
}

/// Same as [`current_closure_hash`] for `/run/booted-system`. The
/// caller compares the two to decide whether to populate
/// `pendingGeneration`.
fn booted_closure_hash() -> Result<String> {
    let target = std::fs::read_link(BOOTED_SYSTEM)
        .with_context(|| format!("readlink {BOOTED_SYSTEM}"))?;
    Ok(closure_hash_from_path(&target))
}

/// Extract the closure hash from a `/nix/store/<hash>-<name>` path.
/// Falls back to the full path string if the shape doesn't match,
/// so the field is always populated.
fn closure_hash_from_path(p: &PathBuf) -> String {
    let s = p.to_string_lossy();
    s.rsplit('/')
        .next()
        .and_then(|leaf| leaf.split('-').next())
        .map(str::to_string)
        .unwrap_or_else(|| s.to_string())
}

/// Read `/proc/sys/kernel/random/boot_id`. The file is a single
/// UUID + newline; we trim and return.
pub fn boot_id() -> Result<String> {
    let raw = std::fs::read_to_string(BOOT_ID_PATH)
        .with_context(|| format!("read {BOOT_ID_PATH}"))?;
    Ok(raw.trim().to_string())
}

/// Build the `currentGeneration` GenerationRef. `channel_ref` is
/// always `None` in PR-3 — the agent doesn't know its channel until
/// PR-4 wires the projection.
pub fn current_generation_ref() -> Result<GenerationRef> {
    Ok(GenerationRef {
        closure_hash: current_closure_hash()?,
        channel_ref: None,
        boot_id: boot_id()?,
    })
}

/// Build the `pendingGeneration` PendingGeneration when
/// `/run/booted-system` differs from `/run/current-system`. Returns
/// `Ok(None)` when they match (no pending), `Err` only on read
/// failures of either symlink.
pub fn pending_generation() -> Result<Option<PendingGeneration>> {
    let current = current_closure_hash()?;
    let booted = booted_closure_hash()?;
    if current == booted {
        return Ok(None);
    }
    Ok(Some(PendingGeneration {
        closure_hash: current,
        scheduled_for: None,
    }))
}

/// Wall-clock seconds since the agent process started. The caller
/// passes the start `Instant` (captured in `main` before the poll
/// loop starts).
pub fn uptime_secs(started_at: Instant) -> u64 {
    started_at.elapsed().as_secs()
}
