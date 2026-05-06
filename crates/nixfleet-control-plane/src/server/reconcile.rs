//! 30s reconcile loop; freshness gate prevents stale build-time bytes clobbering upstream-fresh snapshot.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use nixfleet_proto::FleetResolved;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{render_plan, tick, TickInputs};

use super::state::{AppState, HostCheckinRecord, RECONCILE_INTERVAL};

pub(super) fn spawn_reconcile_loop(
    cancel: CancellationToken,
    state: Arc<AppState>,
    inputs: TickInputs,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Build-time artifact is the fallback prime; never overwrite an already-primed upstream-fresh snapshot.
        {
            let already_primed = state.verified_fleet.read().await.is_some();
            if !already_primed {
                let prime_inputs = TickInputs {
                    now: Utc::now(),
                    ..inputs.clone()
                };
                if let Some((fleet, artifact_bytes)) = verify_fleet_only(&prime_inputs) {
                    let fleet_hash =
                        nixfleet_reconciler::canonical_hash_from_bytes(&artifact_bytes).ok();
                    if let Some(h) = fleet_hash {
                        *state.verified_fleet.write().await =
                            Some(crate::server::VerifiedFleetSnapshot {
                                fleet: Arc::new(fleet),
                                fleet_resolved_hash: h,
                            });
                    }
                    tracing::info!(
                        target: "reconcile",
                        "primed verified-fleet snapshot from build-time artifact (Forgejo prime unavailable)",
                    );
                } else {
                    tracing::warn!(
                        target: "reconcile",
                        "could not prime verified-fleet snapshot (verify failed); dispatch will block until first tick succeeds",
                    );
                }
            } else {
                tracing::debug!(
                    target: "reconcile",
                    "verified-fleet snapshot already populated; skipping build-time prime",
                );
            }
        }

        let mut ticker =
            tokio::time::interval_at(Instant::now() + RECONCILE_INTERVAL, RECONCILE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!(target: "shutdown", task = "reconcile_loop", "task shut down");
                    return;
                }
                _ = ticker.tick() => {}
            }
            let now = Utc::now();

            let channel_refs = {
                let cache = state.channel_refs_cache.read().await;
                cache.refs.clone()
            };
            let checkins = state.host_checkins.read().await.clone();

            // Snapshot the live verified-fleet cache once. Reconciler prefers
            // it over the static artifact so fleet.nix changes (rolloutPolicies,
            // selector tweaks, channel metadata) apply on the next polling
            // tick instead of waiting for lab to rebuild and re-link the
            // baked-in artifact path.
            let live_fleet = state.verified_fleet.read().await.clone();

            // Reconciler-side observed: same `observed_view::list_active_rollouts`
            // substrate as the dispatch endpoint, but unfiltered by
            // `current_rollout_ids`. The reconciler must see non-current
            // in-flight rollouts so `sweep_terminal_orphans` and
            // ConvergeRollout fire on stragglers; dispatch applies the
            // current-rollout filter inside `build_for_gates`.
            let rollouts: Vec<crate::db::RolloutDbSnapshot> = state
                .db
                .as_deref()
                .map(crate::observed_view::list_active_rollouts)
                .unwrap_or_default();

            let outstanding_compliance_events_by_rollout = match state
                .db
                .as_deref()
                .map(|db| db.reports().outstanding_compliance_events_by_rollout())
            {
                Some(Ok(m)) => m,
                Some(Err(err)) => {
                    tracing::warn!(
                        error = %err,
                        "reconcile: outstanding_compliance_events_by_rollout failed; treating as empty",
                    );
                    HashMap::new()
                }
                None => HashMap::new(),
            };

            // Empty projection falls back to file-backed observed.json (deploy-without-agents path).
            let inputs_now = TickInputs {
                now,
                ..inputs.clone()
            };

            let last_deferrals = state.last_deferrals.read().await.clone();
            // Load each active rollout's budget snapshot from its signed
            // manifest. Disk-backed lookup is fine at reconcile cadence
            // (~5 manifests, ~30s tick); cache later if it shows up in
            // a profile.
            let rollout_budgets =
                load_rollout_budgets(state.as_ref(), &rollouts).await;
            let (result, verified_fleet) = if checkins.is_empty() && channel_refs.is_empty() {
                (tick(&inputs_now), verify_fleet_only(&inputs_now))
            } else {
                run_tick_with_projection(
                    &inputs_now,
                    live_fleet.as_ref(),
                    &checkins,
                    &channel_refs,
                    &rollouts,
                    outstanding_compliance_events_by_rollout,
                    last_deferrals,
                    &rollout_budgets,
                )
            };

            // LOADBEARING: single write-lock atomic swap — dispatch readers
            // can never see a half-built snapshot. Compare signed_at (not
            // wall clock) so an out-of-order tick doesn't downgrade fresh state.
            if let Some((fleet, artifact_bytes)) = verified_fleet {
                let mut guard = state.verified_fleet.write().await;
                let should_overwrite = match guard.as_ref() {
                    None => true,
                    Some(existing) => {
                        match (existing.fleet.meta.signed_at, fleet.meta.signed_at) {
                            (Some(prev), Some(new)) => new >= prev,
                            _ => true,
                        }
                    }
                };
                if should_overwrite {
                    if let Ok(h) = nixfleet_reconciler::canonical_hash_from_bytes(&artifact_bytes)
                    {
                        *guard = Some(crate::server::VerifiedFleetSnapshot {
                            fleet: Arc::new(fleet),
                            fleet_resolved_hash: h,
                        });
                    }
                }
            }

            match result {
                Ok(mut out) => {
                    // The verify result above came from re-reading the static
                    // boot artifact at `inputs.artifact_path`. Dispatch decisions
                    // already operate on the live `verified_fleet` cache (kept
                    // fresh by the channel-refs poll), so the log line should
                    // reflect the same freshness — otherwise `ci_commit` and
                    // `signed_at` lag behind reality until the CP itself is
                    // restarted onto a closure containing the new artifact.
                    if let crate::VerifyOutcome::Ok(ok) = &mut out.verify {
                        if let Some(snapshot) = state.verified_fleet.read().await.as_ref() {
                            if let Some(snap_signed_at) = snapshot.fleet.meta.signed_at {
                                if snap_signed_at >= ok.signed_at {
                                    ok.signed_at = snap_signed_at;
                                    ok.ci_commit = snapshot.fleet.meta.ci_commit.clone();
                                }
                            }
                        }
                    }
                    apply_actions(&state, &out).await;
                    // Per-tick orphan sweep: rollouts that exist in the
                    // rollouts table (in-flight per the column filter)
                    // but whose channel has zero expected hosts in the
                    // live fleet snapshot — operator removed the
                    // channel or stripped closure_hash from every host
                    // on it. Without this, those rollouts sit in
                    // list_active() forever and the reconciler emits
                    // no-op ConvergeRollout actions on every tick.
                    sweep_terminal_orphans(&state, live_fleet.as_ref()).await;
                    let plan = render_plan(&out);
                    tracing::info!(target: "reconcile", "{}", plan.trim_end());
                }
                Err(err) => {
                    tracing::warn!(error = %err, "reconcile tick failed");
                }
            }
            *state.last_tick_at.write().await = Some(now);
        }
    })
}

/// Wake the channel-refs poll on relevant state transitions so a
/// freshly-released channelEdges successor gets its rollout recorded
/// without waiting up to 60 s. Fire-and-forget — `watch::Sender::send`
/// only fails when all receivers are dropped, which means the polling
/// task has exited; we log + continue so the reconciler doesn't seize.
fn kick_channel_refs_poll(state: &AppState, reason: &'static str) {
    if let Err(err) = state.channel_refs_kick.send(()) {
        tracing::debug!(
            target: "polling",
            reason,
            error = %err,
            "channel-refs kick: no receivers (poll task exited?); falling back to cadence",
        );
    } else {
        tracing::debug!(
            target: "polling",
            reason,
            "channel-refs kick sent (event-driven poll wake)",
        );
    }
}

/// At-least-once action handler; SoakHost + ConvergeRollout mutate DB, others are journal-only.
async fn apply_actions(state: &AppState, out: &crate::TickOutput) {
    use nixfleet_reconciler::observed::DeferralRecord;
    use nixfleet_reconciler::Action;

    let actions = match &out.verify {
        crate::VerifyOutcome::Ok(ok) => &ok.actions,
        crate::VerifyOutcome::Failed { .. } => return,
    };
    // Stamp / clear deferral state BEFORE the DB gate below — deferrals are
    // pure-journal and the debounce must work even on a CP started without
    // --db. OpenRollout for a previously-deferred channel clears the entry
    // so a same-ref re-block (rare: predecessor converges → fresh rollout
    // opens on it before this channel starts) re-emits as a transition
    // rather than being silenced by stale state.
    {
        let mut deferrals = state.last_deferrals.write().await;
        for action in actions {
            match action {
                Action::RolloutDeferred {
                    channel,
                    target_ref,
                    blocked_by,
                    ..
                } => {
                    deferrals.insert(
                        channel.clone(),
                        DeferralRecord {
                            target_ref: target_ref.clone(),
                            blocked_by: blocked_by.clone(),
                        },
                    );
                }
                Action::OpenRollout { channel, .. } => {
                    deferrals.remove(channel);
                }
                _ => {}
            }
        }
    }
    let Some(db) = state.db.as_ref() else {
        return;
    };
    for action in actions {
        match action {
            Action::SoakHost { rollout, host } => {
                match db.rollout_state().transition_host_state(
                    host,
                    rollout,
                    crate::state::HostRolloutState::Soaked,
                    crate::state::HealthyMarker::Untouched,
                    Some(crate::state::HostRolloutState::Healthy),
                ) {
                    Ok(0) => {
                        tracing::debug!(
                            target: "soak",
                            host = %host,
                            rollout = %rollout,
                            "soak: transition Healthy → Soaked no-op (host not in Healthy)",
                        );
                    }
                    Ok(_) => {
                        // metric fires from inside transition_host_state.
                        tracing::info!(
                            target: "soak",
                            host = %host,
                            rollout = %rollout,
                            "soak: host transitioned Healthy → Soaked",
                        );
                        // A newly-Soaked host can flip the predecessor's
                        // `is_active_for_ordering()` to false; channelEdges
                        // for any successor needs to know now, not at the
                        // next 60 s polling tick.
                        kick_channel_refs_poll(state, "SoakHost transition");
                    }
                    Err(err) => {
                        tracing::warn!(
                            host = %host,
                            rollout = %rollout,
                            error = %err,
                            "soak: transition Healthy → Soaked failed",
                        );
                    }
                }
            }
            Action::ConvergeRollout { rollout } => {
                match db
                    .dispatch_history()
                    .mark_rollout_converged(rollout, chrono::Utc::now())
                {
                    Ok(0) => {}
                    Ok(n) => {
                        tracing::info!(
                            target: "converge",
                            rollout = %rollout,
                            history_rows_marked = n,
                            "converge: stamped dispatch_history terminal_state=converged",
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            rollout = %rollout,
                            error = %err,
                            "converge: dispatch_history terminal stamp failed",
                        );
                    }
                }
                // Settle host_rollout_state: Soaked → Converged for every
                // host in this rollout. The reconciler's wave-staging only
                // takes hosts as far as Soaked (via SoakHost actions);
                // ConvergeRollout is the final transition that stamps the
                // per-host terminal state. Without this, the dashboard
                // shows hosts as Soaked indefinitely after the rollout
                // completes, and predecessor_channel_blocking would have
                // to special-case Soaked-as-terminal everywhere.
                match db.rollout_state().mark_rollout_hosts_converged(rollout) {
                    Ok(0) => {}
                    Ok(n) => {
                        // metric fires from inside mark_rollout_hosts_converged.
                        tracing::info!(
                            target: "converge",
                            rollout = %rollout,
                            host_rollout_state_rows_marked = n,
                            "converge: transitioned host_rollout_state Soaked → Converged",
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            rollout = %rollout,
                            error = %err,
                            "converge: host_rollout_state Soaked → Converged sweep failed",
                        );
                    }
                }
                // Stamp terminal_at on the rollouts table — closes the
                // lifecycle and stops list_active() from returning this
                // rollout to subsequent ticks. Without this the
                // reconciler emits ConvergeRollout every tick for a
                // already-converged rollout (history rows already
                // stamped, host_rollout_state already Converged → both
                // are no-ops at n=0), which is wasted work and clutters
                // the action stream.
                match db.rollouts().mark_terminal(rollout, chrono::Utc::now()) {
                    Ok(0) => {
                        tracing::debug!(
                            target: "converge",
                            rollout = %rollout,
                            "converge: mark_terminal no-op (rollout absent or already terminal)",
                        );
                    }
                    Ok(_) => {
                        tracing::info!(
                            target: "converge",
                            rollout = %rollout,
                            "converge: stamped rollouts.terminal_at — rollout removed from in-flight",
                        );
                        // Predecessor just went terminal — channelEdges
                        // for any successor channel can now release.
                        // Wake the poll so the successor's rollout gets
                        // recorded immediately rather than waiting up to
                        // 60 s for the next cadence tick.
                        kick_channel_refs_poll(state, "ConvergeRollout terminal_at");
                    }
                    Err(err) => {
                        tracing::warn!(
                            rollout = %rollout,
                            error = %err,
                            "converge: mark_terminal failed (rollout will keep re-emitting ConvergeRollout)",
                        );
                    }
                }
            }
            Action::PromoteWave { rollout, new_wave } => {
                // LOADBEARING: persists the advance so subsequent ticks see
                // the new wave through `RolloutDbSnapshot.current_wave`.
                // Without this the projection layer always reports
                // current_wave=0 → multi-wave channels can never reach the
                // ConvergeRollout terminal branch.
                let wave: u32 = (*new_wave).try_into().unwrap_or(u32::MAX);
                match db.rollouts().set_current_wave(rollout, wave) {
                    Ok(0) => {
                        tracing::debug!(
                            target: "promote",
                            rollout = %rollout,
                            new_wave = new_wave,
                            "promote: wave advance no-op (already at or beyond)",
                        );
                    }
                    Ok(_) => {
                        tracing::info!(
                            target: "promote",
                            rollout = %rollout,
                            new_wave = new_wave,
                            "promote: rollout advanced to next wave",
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            rollout = %rollout,
                            new_wave = new_wave,
                            error = %err,
                            "promote: set_current_wave failed",
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Per-tick safety net for the rollouts.terminal_at lifecycle.
///
/// `Action::ConvergeRollout` already stamps terminal_at when a rollout
/// converges naturally. This sweep catches the residual case: a rollout
/// is in-flight per the rollouts table but has no expected hosts in the
/// current fleet snapshot, so the reconciler will never emit
/// ConvergeRollout for it (no host_states → wave_all_soaked is vacuously
/// satisfied but advance_rollout's terminal predicate doesn't fire on
/// empty rollouts). Without this sweep the rollout sits in list_active
/// forever, surfacing as a ghost in the deferrals view and triggering
/// no-op DB writes on every tick.
///
/// Conservative: only stamps terminal when BOTH the channel has zero
/// hosts with a closure_hash AND the live fleet snapshot is loaded
/// (skip when verified-fleet is None — better a one-tick delay than a
/// premature stamp during a cold-boot prime).
async fn sweep_terminal_orphans(
    state: &AppState,
    live_fleet: Option<&crate::server::VerifiedFleetSnapshot>,
) {
    let Some(snapshot) = live_fleet else {
        return;
    };
    let Some(db) = state.db.as_deref() else {
        return;
    };

    // Channels that still have at least one host expecting a closure.
    // A channel with no such hosts is a candidate for terminal-stamping
    // any rollouts on it.
    let mut channels_with_expected_hosts: std::collections::HashSet<&str> =
        std::collections::HashSet::new();
    for host in snapshot.fleet.hosts.values() {
        if host.closure_hash.is_some() {
            channels_with_expected_hosts.insert(host.channel.as_str());
        }
    }

    let in_flight = match db.rollouts().list_active() {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "orphan-sweep: list_active failed");
            return;
        }
    };

    let now = chrono::Utc::now();
    for rollout in in_flight {
        if channels_with_expected_hosts.contains(rollout.channel.as_str()) {
            continue;
        }
        match db.rollouts().mark_terminal(&rollout.rollout_id, now) {
            Ok(0) => {}
            Ok(_) => {
                tracing::info!(
                    target: "converge",
                    rollout = %rollout.rollout_id,
                    channel = %rollout.channel,
                    "orphan-sweep: stamped terminal_at — channel has no expected hosts in live fleet",
                );
            }
            Err(err) => {
                tracing::warn!(
                    rollout = %rollout.rollout_id,
                    error = %err,
                    "orphan-sweep: mark_terminal failed",
                );
            }
        }
    }
}

/// Returns `(tick_output, fleet)`; fleet `None` on verify failure so caller preserves prior snapshot.
#[allow(clippy::too_many_arguments)]
fn run_tick_with_projection(
    inputs: &TickInputs,
    live_fleet: Option<&crate::server::VerifiedFleetSnapshot>,
    checkins: &HashMap<String, HostCheckinRecord>,
    channel_refs: &HashMap<String, String>,
    rollouts: &[crate::db::RolloutDbSnapshot],
    outstanding_compliance_events_by_rollout: HashMap<String, HashMap<String, usize>>,
    last_deferrals: HashMap<String, nixfleet_reconciler::observed::DeferralRecord>,
    rollout_budgets: &HashMap<String, Vec<nixfleet_proto::RolloutBudget>>,
) -> (
    anyhow::Result<crate::TickOutput>,
    Option<(FleetResolved, Vec<u8>)>,
) {
    // LOADBEARING: prefer the live verified-fleet snapshot (kept fresh by
    // the channel-refs polling loop) over re-reading the static artifact
    // baked into the CP closure at build time. The polling layer already
    // verified the signature when populating the cache; re-verifying here
    // would double-pay verification cost without changing the outcome,
    // and using the static artifact pins reconciler decisions to whatever
    // fleet.resolved was bundled in the running closure — meaning fleet.nix
    // metadata changes (rolloutPolicies, selectors, channel intervals)
    // wouldn't apply until lab rebuilds onto a fresher closure.
    if let Some(snapshot) = live_fleet {
        let fleet = (*snapshot.fleet).clone();
        let signed_at = match fleet.meta.signed_at {
            Some(ts) => ts,
            None => {
                return (
                    Err(anyhow::anyhow!(
                        "verified artifact lacks meta.signedAt despite §4 contract — verify layer bug",
                    )),
                    None,
                );
            }
        };
        let ci_commit = fleet.meta.ci_commit.clone();
        let observed = crate::observed_projection::project(
            checkins,
            channel_refs,
            rollouts,
            outstanding_compliance_events_by_rollout,
            last_deferrals,
            rollout_budgets,
        );
        let actions = nixfleet_reconciler::reconcile(&fleet, &observed, inputs.now);
        // Snapshot already verified — no fresh bytes; caller's None case
        // preserves the live snapshot's existing fleet_resolved_hash.
        return (
            Ok(crate::TickOutput {
                now: inputs.now,
                verify: crate::VerifyOutcome::Ok(Box::new(crate::VerifyOk {
                    signed_at,
                    ci_commit,
                    observed,
                    actions,
                })),
            }),
            None,
        );
    }

    // Fallback: no live snapshot yet (first boot, polling hasn't primed).
    // Read + verify the static artifact baked into the closure.
    use anyhow::Context;
    let artifact = match std::fs::read(&inputs.artifact_path)
        .with_context(|| format!("read artifact {}", inputs.artifact_path.display()))
    {
        Ok(b) => b,
        Err(e) => return (Err(e), None),
    };
    let signature = match std::fs::read(&inputs.signature_path)
        .with_context(|| format!("read signature {}", inputs.signature_path.display()))
    {
        Ok(b) => b,
        Err(e) => return (Err(e), None),
    };
    let (trusted_keys, reject_before) =
        match crate::polling::signed_fetch::read_trust_roots(&inputs.trust_path, inputs.now) {
            Ok(t) => t,
            Err(e) => return (Err(e), None),
        };

    let (verify, fleet) = match nixfleet_reconciler::verify_artifact(
        &artifact,
        &signature,
        &trusted_keys,
        inputs.now,
        inputs.freshness_window,
        reject_before,
    ) {
        Ok(fleet) => {
            let signed_at = match fleet.meta.signed_at {
                Some(ts) => ts,
                None => {
                    return (
                        Err(anyhow::anyhow!(
                            "verified artifact lacks meta.signedAt despite §4 contract — verify layer bug",
                        )),
                        None,
                    );
                }
            };
            let ci_commit = fleet.meta.ci_commit.clone();
            let observed = crate::observed_projection::project(
                checkins,
                channel_refs,
                rollouts,
                outstanding_compliance_events_by_rollout,
                last_deferrals.clone(),
            rollout_budgets,
        );
            let actions = nixfleet_reconciler::reconcile(&fleet, &observed, inputs.now);
            (
                crate::VerifyOutcome::Ok(Box::new(crate::VerifyOk {
                    signed_at,
                    ci_commit,
                    observed,
                    actions,
                })),
                Some((fleet, artifact.clone())),
            )
        }
        Err(err) => (
            crate::VerifyOutcome::Failed {
                reason: format!("{:?}", err),
            },
            None,
        ),
    };

    (
        Ok(crate::TickOutput {
            now: inputs.now,
            verify,
        }),
        fleet,
    )
}

/// Load each active rollout's `disruption_budgets` snapshot from its signed
/// manifest. Returns an empty map entry on read or parse failure — the
/// reconciler then dispatches without a budget gate for that rollout, which
/// is the same as "no budget declared". Deliberately permissive: a missing
/// manifest blocks dispatch in a more correct way (the host's last-rolled-
/// ref check will hold the rollout open), so failing the budget gate hard
/// here would double-block dispatches without informational value.
async fn load_rollout_budgets(
    state: &AppState,
    rollouts: &[crate::db::RolloutDbSnapshot],
) -> HashMap<String, Vec<nixfleet_proto::RolloutBudget>> {
    let mut out: HashMap<String, Vec<nixfleet_proto::RolloutBudget>> = HashMap::new();
    let dir = match state.rollouts_dir.as_ref() {
        Some(d) => d.clone(),
        None => return out,
    };
    for r in rollouts {
        let manifest_path = dir.join(format!("{}.json", r.rollout_id));
        let bytes = match tokio::fs::read(&manifest_path).await {
            Ok(b) => b,
            Err(err) => {
                tracing::debug!(
                    rollout = %r.rollout_id,
                    path = %manifest_path.display(),
                    error = %err,
                    "load_rollout_budgets: manifest unavailable; budget gate no-ops for this rollout",
                );
                continue;
            }
        };
        match serde_json::from_slice::<nixfleet_proto::RolloutManifest>(&bytes) {
            Ok(m) => {
                out.insert(r.rollout_id.clone(), m.disruption_budgets);
            }
            Err(err) => {
                tracing::warn!(
                    rollout = %r.rollout_id,
                    error = %err,
                    "load_rollout_budgets: manifest parse failed; budget gate no-ops for this rollout",
                );
            }
        }
    }
    out
}

/// `None` on verify failure → caller preserves prior snapshot.
/// Returns (parsed_fleet, raw_artifact_bytes) so callers can compute
/// `fleet_resolved_hash` from the received bytes (cross-version-stable),
/// not from a re-serialised parsed struct.
pub(super) fn verify_fleet_only(inputs: &TickInputs) -> Option<(FleetResolved, Vec<u8>)> {
    let artifact = std::fs::read(&inputs.artifact_path).ok()?;
    let signature = std::fs::read(&inputs.signature_path).ok()?;
    let (trusted_keys, reject_before) =
        crate::polling::signed_fetch::read_trust_roots(&inputs.trust_path, inputs.now).ok()?;
    let fleet = nixfleet_reconciler::verify_artifact(
        &artifact,
        &signature,
        &trusted_keys,
        inputs.now,
        inputs.freshness_window,
        reject_before,
    )
    .ok()?;
    Some((fleet, artifact))
}
