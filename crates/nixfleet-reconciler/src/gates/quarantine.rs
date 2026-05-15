//! Anti-thrash quarantine gate: refuse to dispatch a `(channel, closure_hash)`
//! that `server::reconcile::sweep_soaked_health_failures` has quarantined after
//! sustained probe failures. Runs in `evaluate_for_host` so the reconciler
//! action emission AND the CP dispatch endpoint share the same decision; a
//! split-brain (reconciler emits Skip but checkin endpoint serves the SHA)
//! would otherwise let agents keep activating the bad closure on a loop.

use super::{GateBlock, GateInput};

pub fn check(input: &GateInput) -> Option<GateBlock> {
    let host = input.fleet.hosts.get(input.host)?;
    let target_closure = host.closure_hash.as_deref()?;
    let quarantined = input.observed.quarantined_closures.get(&host.channel)?;
    if quarantined.contains(target_closure) {
        Some(GateBlock::Quarantined {
            channel: host.channel.clone(),
            closure_hash: target_closure.to_string(),
        })
    } else {
        None
    }
}
