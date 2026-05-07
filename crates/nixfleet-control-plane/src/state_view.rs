//! Reusable read-model substrate for fleet state. Consumed by the
//! `/v1/hosts` HTTP route and (forthcoming) Prometheus metrics exporter
//! + CLI status renderer. Sharing this means the row shape and label
//! set agree by construction across all three surfaces.

use std::collections::HashMap;

use nixfleet_proto::agent_wire::ReportEvent;
use nixfleet_proto::{HostRolloutState, HostStatusEntry};
use nixfleet_reconciler::compute_rollout_id_for_channel;
use nixfleet_reconciler::evidence::SignatureStatus;
use tracing::warn;

use crate::server::AppState;

#[derive(Debug)]
pub enum StateViewError {
    /// Verified fleet snapshot not yet primed (CP just started; channel-refs
    /// poll hasn't completed a successful verify yet, or file-backed artifact
    /// failed verification).
    FleetNotPrimed,
}

/// Joins verified fleet declarations × per-host checkins × report buffers
/// into a one-row-per-declared-host view, sorted by hostname for stable
/// output. Outstanding-event counts apply resolution-by-replacement:
/// events from older rollouts than the host's `last_rollout_id` are
/// treated as resolved.
pub async fn fleet_state_view(state: &AppState) -> Result<Vec<HostStatusEntry>, StateViewError> {
    let snapshot = state
        .verified_fleet
        .read()
        .await
        .clone()
        .ok_or(StateViewError::FleetNotPrimed)?;
    let fleet = snapshot.fleet;
    let fleet_hash = snapshot.fleet_resolved_hash;
    let checkins = state.host_checkins.read().await;
    let reports = state.host_reports.read().await;

    // Memoise per-channel rollout ID so we project the manifest once per
    // channel, not per host. Projection failure is louder than the legitimate
    // "no host with closure on this channel" Ok(None) case — warn so a broken
    // fleet manifest surfaces in logs instead of as silently empty rollout_state.
    let mut current_rollout_for_channel: HashMap<String, Option<String>> = HashMap::new();
    for channel in fleet.channels.keys() {
        let id = match compute_rollout_id_for_channel(&fleet, &fleet_hash, channel) {
            Ok(id) => id,
            Err(e) => {
                warn!(
                    target: "state_view",
                    channel = %channel,
                    error = %e,
                    "compute_rollout_id_for_channel failed; rollout_state will be None for hosts on this channel",
                );
                None
            }
        };
        current_rollout_for_channel.insert(channel.clone(), id);
    }

    let mut entries: Vec<HostStatusEntry> = fleet
        .hosts
        .iter()
        .map(|(hostname, host_decl)| {
            let checkin = checkins.get(hostname);
            let last_checkin_at = checkin.map(|c| c.last_checkin);
            let current = checkin.map(|c| c.checkin.current_generation.closure_hash.clone());
            let pending = checkin.and_then(|c| {
                c.checkin
                    .pending_generation
                    .as_ref()
                    .map(|p| p.closure_hash.clone())
            });
            let last_rollout_id = checkin.and_then(|c| {
                c.checkin
                    .last_evaluated_target
                    .as_ref()
                    .map(|t| t.rollout_id.clone())
            });
            let converged = match (&host_decl.closure_hash, &current) {
                (Some(declared), Some(running)) => declared == running,
                _ => false,
            };

            let host_buf = reports.get(hostname);
            // GOTCHA: cur_rollout uses agent-reported `last_rollout_id`, not
            // the fleet's current rollout. After a fresh deploy this can lag
            // by one tick — pre-existing pattern, applies to all event-buffer
            // counters here. `pending_reboot` is DB-backed below, so it's not
            // affected by this drift; `quarantined_closure` is event-ring
            // based and CAN drift, but the agent re-posts hourly while
            // suppressing so the worst-case staleness is one tick.
            let cur_rollout = last_rollout_id.as_deref();
            let mut compliance_failures = 0usize;
            let mut runtime_gate_errors = 0usize;
            let mut verified_count = 0usize;
            // Most recent RolloutQuarantined for current rollout — None when
            // no quarantine event present, Some(closure_hash) otherwise.
            // Buf iter is oldest-first, so we overwrite as we find newer.
            let mut quarantined_closure: Option<String> = None;
            if let Some(buf) = host_buf {
                for record in buf.iter() {
                    let is_compliance =
                        matches!(record.report.event, ReportEvent::ComplianceFailure { .. });
                    let is_runtime_gate =
                        matches!(record.report.event, ReportEvent::RuntimeGateError { .. });
                    let is_quarantined =
                        matches!(record.report.event, ReportEvent::RolloutQuarantined { .. });
                    if !is_compliance && !is_runtime_gate && !is_quarantined {
                        continue;
                    }
                    let event_rollout = record.report.rollout.as_deref();
                    let outstanding = !matches!(
                        (cur_rollout, event_rollout),
                        (Some(cur), Some(ev_r)) if cur != ev_r
                    );
                    if !outstanding {
                        continue;
                    }
                    if is_compliance {
                        compliance_failures += 1;
                    }
                    if is_runtime_gate {
                        runtime_gate_errors += 1;
                    }
                    if let ReportEvent::RolloutQuarantined { closure_hash, .. } =
                        &record.report.event
                    {
                        quarantined_closure = Some(closure_hash.clone());
                    }
                    if matches!(record.signature_status, Some(SignatureStatus::Verified))
                        && !is_quarantined
                    {
                        verified_count += 1;
                    }
                }
            }
            // pending_reboot is DB-backed: the durable `host_dispatch_state`
            // row is the single source of truth. Survives CP restart, doesn't
            // depend on the in-memory ring's eviction policy. The agent-side
            // `apply_deferred_pending_reboot_transition` parks the row when
            // `ActivationDeferred` arrives; `confirm()` (post-reboot) flips
            // it to Confirmed which clears the signal here naturally.
            //
            // Falls back to false on absent DB or read failure — same shape
            // as `rollout_state` below; metrics rather than crash semantics.
            let pending_reboot = state
                .db
                .as_ref()
                .and_then(|db| match db.host_dispatch_state().host_state(hostname) {
                    Ok(Some(row)) => Some(row.state == "deferred-pending-reboot"),
                    Ok(None) => Some(false),
                    Err(e) => {
                        warn!(
                            target: "state_view",
                            hostname = %hostname,
                            error = %e,
                            "host_dispatch_state read for pending_reboot failed; rendering as false",
                        );
                        None
                    }
                })
                .unwrap_or(false);

            let last_uptime_secs = checkin.and_then(|c| c.checkin.uptime_secs);

            // GOTCHA: query state for the FLEET's current rolloutId for this
            // channel, not the agent-reported last_rollout_id (may be stale
            // after a fresh deploy supersedes).
            //
            // Three classes of "None" with different meanings:
            //   - benign: no DB / no current rollout / row absent (host hasn't
            //     transitioned for this rollout yet) — silent.
            //   - DB error: lock poisoned, schema drift, I/O — warn (was
            //     previously swallowed as Ok(None)).
            //   - parse error: unrecognised state string in DB row — warn
            //     (data-integrity issue, not normal).
            let rollout_state = state.db.as_ref().and_then(|db| {
                let rid = current_rollout_for_channel
                    .get(&host_decl.channel)
                    .and_then(|o| o.as_deref())?;
                let row = match db.rollout_state().host_state(hostname, rid) {
                    Ok(row) => row,
                    Err(e) => {
                        warn!(
                            target: "state_view",
                            hostname = %hostname,
                            rollout_id = %rid,
                            error = %e,
                            "rollout_state DB lookup failed; rendering host as None",
                        );
                        return None;
                    }
                };
                let s = row?;
                match HostRolloutState::from_db_str(&s) {
                    Ok(parsed) => Some(parsed),
                    Err(e) => {
                        warn!(
                            target: "state_view",
                            hostname = %hostname,
                            rollout_id = %rid,
                            raw = %s,
                            error = %e,
                            "host_rollout_state row has unrecognised state string; rendering as None",
                        );
                        None
                    }
                }
            });

            HostStatusEntry {
                hostname: hostname.clone(),
                channel: host_decl.channel.clone(),
                declared_closure_hash: host_decl.closure_hash.clone(),
                current_closure_hash: current,
                pending_closure_hash: pending,
                last_checkin_at,
                last_rollout_id,
                converged,
                outstanding_compliance_failures: compliance_failures,
                outstanding_runtime_gate_errors: runtime_gate_errors,
                verified_event_count: verified_count,
                last_uptime_secs,
                rollout_state,
                pending_reboot,
                quarantined_closure,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    Ok(entries)
}
