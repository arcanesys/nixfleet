//! Shared read-model for fleet state. Consumed by `/v1/hosts`,
//! `/v1/deferrals`, the metrics exporter, and the CLI status renderer - so
//! row shapes and label sets agree by construction.

use std::collections::HashMap;

use nixfleet_proto::agent_wire::ReportEvent;
use nixfleet_proto::{HostRolloutState, HostStatusEntry};
use nixfleet_reconciler::compute_rollout_id_for_channel;
use nixfleet_reconciler::evidence::SignatureStatus;
use tracing::warn;

use crate::server::AppState;

#[derive(Debug)]
pub enum StateViewError {
    /// Verified fleet snapshot not yet primed.
    FleetNotPrimed,
}

/// Joins fleet declarations × per-host checkins × report buffers into a
/// one-row-per-host view sorted by hostname. Resolution-by-replacement:
/// events from older rollouts than `last_rollout_id` are treated as resolved.
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

    // Memoise per-channel rollout ID; project once per channel, not per host.
    // Projection failure warns so a broken manifest surfaces in logs rather
    // than silently empty rollout_state.
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
            // GOTCHA: cur_rollout uses agent-reported `last_rollout_id`, so
            // event-buffer counters can lag by one tick after fresh deploys.
            // DB-backed signals (pending_reboot) are immune; event-ring
            // signals (quarantined_closure) can drift, but agent re-posts
            // hourly so worst case is one tick.
            let cur_rollout = last_rollout_id.as_deref();
            let mut compliance_failures = 0usize;
            let mut runtime_gate_errors = 0usize;
            let mut verified_count = 0usize;
            // Most-recent ClosureQuarantined for current rollout. Buf iter
            // is oldest-first; overwrite as we find newer.
            let mut quarantined_closure: Option<String> = None;
            if let Some(buf) = host_buf {
                for record in buf.iter() {
                    let is_compliance =
                        matches!(record.report.event, ReportEvent::ComplianceFailure { .. });
                    let is_runtime_gate =
                        matches!(record.report.event, ReportEvent::RuntimeGateError { .. });
                    let is_quarantined =
                        matches!(record.report.event, ReportEvent::ClosureQuarantined { .. });
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
                    if let ReportEvent::ClosureQuarantined { closure_hash, .. } =
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
            // Falls back to false on absent DB or read failure - same shape
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
            //     transitioned for this rollout yet) - silent.
            //   - DB error: lock poisoned, schema drift, I/O - warn (was
            //     previously swallowed as Ok(None)).
            //   - parse error: unrecognised state string in DB row - warn
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
                // Issue #88: pass through the pin from the fleet snapshot.
                // mkFleet has already resolved the most-specific level
                // (host > tag > channel) and `nixfleet-release` filtered
                // expired pins at signing time, so what we render is the
                // active pin for this host.
                pin: host_decl.pin.clone(),
                // Issue #86: count probes in non-Pass state from the
                // host's latest checkin. Snapshot-driven (not event-ring
                // like compliance/runtime-gate above), because probe
                // state IS the latest snapshot - the wire carries the
                // current value, not a stream of failure events.
                outstanding_health_failures: checkin
                    .map(|c| {
                        c.checkin
                            .health_probes
                            .iter()
                            .filter(|p| !matches!(
                                p.status,
                                nixfleet_proto::agent_wire::ProbeStatus::Pass
                            ))
                            .count()
                    })
                    .unwrap_or(0),
            }
        })
        .collect();
    entries.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    Ok(entries)
}

// ----------------------------------------------------------------------
// Cross-channel deferral state - moved from deferrals_view.rs.
//
// Shares the `observed_view::build_for_gates_from_state` projection
// with the dispatch endpoint and the disruption-budget metric. Three
// operator-facing surfaces, one source of truth. Distinct from the
// CP's in-memory `last_deferrals` debounce snapshot - debounce state
// and observability state answer different questions and must not
// converge on the same source.
// ----------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ChannelDeferral {
    pub channel: String,
    pub target_ref: String,
    pub blocked_by: String,
    pub reason: String,
}

/// Compute the current deferred-channels view. Empty vec when no
/// verified-fleet snapshot is primed yet.
pub async fn compute_channel_deferrals(state: &AppState) -> Vec<ChannelDeferral> {
    let snapshot = match state.verified_fleet.read().await.clone() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let fleet = &snapshot.fleet;

    let channel_refs = state.channel_refs_cache.read().await.refs.clone();
    let observed = crate::observed_view::build_for_gates_from_state(
        state,
        fleet,
        &snapshot.fleet_resolved_hash,
    )
    .await;

    let mut deferrals: Vec<ChannelDeferral> = Vec::new();
    for (channel, current_ref) in &channel_refs {
        if !fleet.channels.contains_key(channel) {
            continue;
        }
        // Channels already running an active rollout aren't "deferred"  -
        // they're executing.
        let has_active = observed
            .active_rollouts
            .iter()
            .any(|r| &r.channel == channel);
        if has_active {
            continue;
        }
        // Empty in-tick set: live snapshot read, not a reconcile tick.
        let no_in_tick_opens = std::collections::HashSet::new();
        if let Some(blocker) = nixfleet_reconciler::gates::channel_edges::check_for_channel(
            fleet,
            &observed,
            &no_in_tick_opens,
            channel,
            // Live read for the dashboard, not the dispatch path.
            // Missing predecessor here just means "not yet recorded";
            // fresh-boot CP shouldn't surface every successor as
            // `deferred`. Reconcile mode is the right one.
            nixfleet_reconciler::gates::GateMode::Reconcile,
        ) {
            let reason = fleet
                .channel_edges
                .iter()
                .find(|e| e.gated == *channel && e.gates == blocker)
                .and_then(|e| e.reason.clone())
                .unwrap_or_else(|| {
                    format!("predecessor channel '{blocker}' has an unfinished rollout")
                });
            deferrals.push(ChannelDeferral {
                channel: channel.clone(),
                target_ref: current_ref.clone(),
                blocked_by: blocker,
                reason,
            });
        }
    }

    // Stable order: alphabetical by channel - deterministic across ticks
    // for both the JSON response and the Prometheus label set.
    deferrals.sort_by(|a, b| a.channel.cmp(&b.channel));
    deferrals
}
