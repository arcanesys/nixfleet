//! Linux/NixOS impl: `/proc` + `/run/booted-system`.

use std::fs;

use anyhow::{Context, Result};
use nixfleet_proto::agent_wire::PendingGeneration;

use crate::checkin_state::{closure_hash_from_path, current_closure_hash};

const BOOTED_SYSTEM: &str = "/run/booted-system";
const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";

pub fn boot_id() -> Result<String> {
    let raw = fs::read_to_string(BOOT_ID_PATH).with_context(|| format!("read {BOOT_ID_PATH}"))?;
    Ok(raw.trim().to_string())
}

pub fn pending_generation() -> Result<Option<PendingGeneration>> {
    let current = current_closure_hash()?;
    let booted = booted_closure_hash()?;
    if current == booted {
        return Ok(None);
    }
    Ok(Some(PendingGeneration {
        closure_hash: current,
    }))
}

fn booted_closure_hash() -> Result<String> {
    let target =
        fs::read_link(BOOTED_SYSTEM).with_context(|| format!("readlink {BOOTED_SYSTEM}"))?;
    Ok(closure_hash_from_path(&target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_id_returns_a_non_empty_string() {
        let id = boot_id().expect("boot_id() must succeed on linux");
        assert!(!id.is_empty(), "boot_id() returned an empty string");
    }

    #[test]
    fn boot_id_is_stable_within_a_process() {
        let a = boot_id().unwrap();
        let b = boot_id().unwrap();
        assert_eq!(a, b, "boot_id must be stable within the running process");
    }
}
