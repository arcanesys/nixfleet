//! Channel-refs poll: every 60s fetch + verify; failure retains previous state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::RwLock;

use crate::polling::poller::SignedArtifactPoller;
use crate::polling::signed_fetch;

/// CI sign+push latency dominates; faster polling doesn't help.
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ChannelRefsSource {
    pub artifact_url: String,
    pub signature_url: String,
    /// Re-read each poll so token rotation propagates without restart.
    pub token_file: Option<PathBuf>,
    /// Re-read each poll so trust.json rotation propagates.
    pub trust_path: PathBuf,
    pub freshness_window: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelRefsCache {
    pub refs: HashMap<String, String>,
    pub last_refreshed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Cadence + optional `kick` (reconciler wakes us on `ConvergeRollout` / `SoakHost`).
/// Failure retains previous state; cadence is the safety net for missed kicks.
pub fn spawn(
    cancel: tokio_util::sync::CancellationToken,
    cache: Arc<RwLock<ChannelRefsCache>>,
    verified_fleet: Arc<RwLock<Option<crate::server::VerifiedFleetSnapshot>>>,
    db: Option<Arc<crate::db::Db>>,
    last_deferrals: Arc<
        RwLock<HashMap<String, nixfleet_reconciler::observed::DeferralRecord>>,
    >,
    config: ChannelRefsSource,
    kick: Option<tokio::sync::watch::Receiver<()>>,
) -> tokio::task::JoinHandle<()> {
    SignedArtifactPoller {
        interval: POLL_INTERVAL,
        label: "channel-refs",
    }
    .spawn_with_kick(cancel, kick, move |client| {
        let cache = Arc::clone(&cache);
        let verified_fleet = Arc::clone(&verified_fleet);
        let db = db.clone();
        let last_deferrals = Arc::clone(&last_deferrals);
        let config = config.clone();
        async move {
            let (refs, fleet, fleet_hash) = poll_once(&client, &config).await?;
            apply_verified_refs(
                &cache,
                &verified_fleet,
                db.as_deref(),
                &last_deferrals,
                refs,
                fleet,
                fleet_hash,
            )
            .await;
            Ok(())
        }
    })
}

/// (fleet, fleet_resolved_hash) under one RwLock so readers never see a torn
/// snapshot. After committing the snapshot, record each channel's current
/// rollout_id in the rollouts table (LOADBEARING for rebuild recovery —
/// this is the path that repopulates rollouts soft state without needing
/// any host to check in first).
async fn apply_verified_refs(
    cache: &RwLock<ChannelRefsCache>,
    verified_fleet: &RwLock<Option<crate::server::VerifiedFleetSnapshot>>,
    db: Option<&crate::db::Db>,
    last_deferrals: &RwLock<HashMap<String, nixfleet_reconciler::observed::DeferralRecord>>,
    refs: HashMap<String, String>,
    fleet: nixfleet_proto::FleetResolved,
    fleet_hash: String,
) {
    let new_signed_at = fleet.meta.signed_at;
    let new_ci_commit = fleet.meta.ci_commit.clone();

    // Compute current rollout_id per channel against the same snapshot bytes
    // we're about to publish, BEFORE moving fleet into the Arc. Channels
    // with no host having a closure declaration return Ok(None) — skip.
    let channel_rollouts: Vec<(String, String)> = if db.is_some() {
        fleet
            .channels
            .keys()
            .filter_map(|channel| {
                match nixfleet_reconciler::compute_rollout_id_for_channel(
                    &fleet,
                    &fleet_hash,
                    channel,
                ) {
                    Ok(Some(rid)) => Some((channel.clone(), rid)),
                    Ok(None) => None,
                    Err(err) => {
                        tracing::warn!(
                            channel = %channel,
                            error = %err,
                            "channel-refs poll: compute_rollout_id_for_channel failed; skipping",
                        );
                        None
                    }
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    {
        let mut guard = verified_fleet.write().await;
        *guard = Some(crate::server::VerifiedFleetSnapshot {
            fleet: Arc::new(fleet),
            fleet_resolved_hash: fleet_hash,
        });
    }

    let mut guard = cache.write().await;
    let changed = guard.refs != refs;
    guard.refs = refs.clone();
    guard.last_refreshed_at = Some(chrono::Utc::now());
    drop(guard);

    if let Some(db) = db {
        record_rollouts_gated_by_channel_edges(
            db,
            verified_fleet,
            last_deferrals,
            &channel_rollouts,
        )
        .await;
    }

    // INFO only when the artifact actually changed (new CI release or a
    // signed_at advance). DEBUG for the no-op cycles — every minute would
    // otherwise drown the dashboard's "Channel-refs poll" panel in
    // identical "no change" lines. Operators wanting raw cadence still get
    // it via DEBUG.
    if changed {
        tracing::info!(
            target: "channel_refs_poll",
            count = refs.len(),
            changed = changed,
            signed_at = ?new_signed_at,
            ci_commit = ?new_ci_commit,
            active_rollouts_recorded = channel_rollouts.len(),
            "channel-refs poll: verified-fleet snapshot refreshed",
        );
    } else {
        tracing::debug!(
            target: "channel_refs_poll",
            count = refs.len(),
            "channel-refs poll: no change",
        );
    }
}

/// One-shot fetch+verify at boot; without it dispatch uses the stale build-time artifact.
pub async fn prime_once(
    config: &ChannelRefsSource,
) -> Result<(nixfleet_proto::FleetResolved, String)> {
    let client = signed_fetch::build_client();
    let (_refs, fleet, hash) = poll_once(&client, config).await?;
    Ok((fleet, hash))
}

async fn poll_once(
    client: &reqwest::Client,
    config: &ChannelRefsSource,
) -> Result<(
    HashMap<String, String>,
    nixfleet_proto::FleetResolved,
    String,
)> {
    let token = signed_fetch::read_token(config.token_file.as_deref())?;
    let (artifact_bytes, signature_bytes) = signed_fetch::fetch_signed_pair(
        client,
        &config.artifact_url,
        &config.signature_url,
        token.as_deref(),
    )
    .await?;

    let now = chrono::Utc::now();
    let (trusted_keys, reject_before) = signed_fetch::read_trust_roots(&config.trust_path, now)?;

    let fleet_resolved = nixfleet_reconciler::verify_artifact(
        &artifact_bytes,
        &signature_bytes,
        &trusted_keys,
        now,
        config.freshness_window,
        reject_before,
    )
    .map_err(|e| anyhow::anyhow!("verify_artifact (channel-refs poll): {e:?}"))?;

    // Anchors every downstream rolloutId derivation. Hash the received
    // artifact bytes (NOT a re-serialised parsed struct) so additive
    // schema changes the CP's proto doesn't yet know about don't shift
    // the anchor relative to what CI signed. Same load-bearing reason
    // as the rollouts route's verify_content_address.
    let fleet_resolved_hash = nixfleet_reconciler::canonical_hash_from_bytes(&artifact_bytes)
        .map_err(|e| anyhow::anyhow!("canonical_hash_from_bytes (fleet.resolved): {e:?}"))?;

    // Single signing rev: every channel maps to the same CI commit.
    let ci_commit = fleet_resolved
        .meta
        .ci_commit
        .clone()
        .unwrap_or_else(|| "<unknown>".to_string());
    let mut refs = HashMap::new();
    for name in fleet_resolved.channels.keys() {
        refs.insert(name.clone(), ci_commit.clone());
    }
    Ok((refs, fleet_resolved, fleet_resolved_hash))
}

/// Gate `record_active_rollout` writes by `channelEdges`. The DB rollouts
/// table is the source of truth for /v1/rollouts and the reconciler's
/// `Observed.active_rollouts`; recording an entry for a channel whose
/// predecessor hasn't converged would defeat the channelEdges contract
/// at the storage layer (the reconciler's `RolloutDeferred` is journal-
/// only and doesn't touch the DB).
///
/// Iteration order is the same topological sort the reconciler uses, with
/// an in-poll `emitted_opens` set so a `before` channel recorded earlier
/// in this poll is seen as active by `after`'s predecessor check. This
/// mirrors the reconcile loop's invariants — the two layers stay
/// architecturally aligned.
///
/// Stale-rollout filter (LOADBEARING for fleet-rev transitions): the
/// `active_rollouts_snapshot` returns rows for any rollout that has ever
/// had host_dispatch_state writes, including the OLD rollout from the
/// previous fleet rev. That old rollout's hosts are typically `Converged`
/// at the moment a new rev is published, so the predecessor check would
/// see "edge=Converged → not active → not blocked" and let the new
/// successor through INSTANTLY — channelEdges silently bypassed for the
/// first poll of every release. We filter by current rolloutIds (the
/// derivation from the new fleet snapshot) so only rollouts belonging to
/// this rev contribute to predecessor state. Old rollouts get marked
/// superseded as soon as `record_active_rollout` runs for the new ones,
/// so the filter is a one-tick guard.
async fn record_rollouts_gated_by_channel_edges(
    db: &crate::db::Db,
    verified_fleet: &RwLock<Option<crate::server::VerifiedFleetSnapshot>>,
    last_deferrals: &RwLock<HashMap<String, nixfleet_reconciler::observed::DeferralRecord>>,
    channel_rollouts: &[(String, String)],
) {
    let fleet_snap = match verified_fleet.read().await.clone() {
        Some(s) => s,
        None => return,
    };
    let fleet = &fleet_snap.fleet;

    // Build the same `Observed.active_rollouts` view the reconciler sees,
    // so `predecessor_channel_blocking` resolves identically.
    let raw = match db.host_dispatch_state().active_rollouts_snapshot() {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "channel-refs poll: active_rollouts_snapshot failed; recording rollouts without channelEdges gate (non-fatal)");
            Vec::new()
        }
    };
    let superseded: std::collections::HashSet<String> = db
        .rollouts()
        .superseded_rollout_ids()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let current_rollout_ids: std::collections::HashSet<String> = channel_rollouts
        .iter()
        .map(|(_, rid)| rid.clone())
        .collect();
    let active_rollouts: Vec<nixfleet_reconciler::observed::Rollout> = raw
        .into_iter()
        .filter(|r| !superseded.contains(&r.rollout_id))
        .filter(|r| current_rollout_ids.contains(&r.rollout_id))
        .map(|snap| nixfleet_reconciler::observed::Rollout {
            id: snap.rollout_id,
            channel: snap.channel,
            target_ref: snap.target_channel_ref,
            state: nixfleet_reconciler::RolloutState::Executing,
            current_wave: snap.current_wave as usize,
            host_states: snap
                .host_states
                .iter()
                .filter_map(|(h, s)| {
                    nixfleet_reconciler::HostRolloutState::from_db_str(s)
                        .ok()
                        .map(|st| (h.clone(), st))
                })
                .collect(),
            last_healthy_since: snap.last_healthy_since,
            budgets: vec![],
            terminal_at: snap.terminal_at,
        })
        .collect();

    let pseudo_observed = nixfleet_reconciler::observed::Observed {
        channel_refs: HashMap::new(),
        last_rolled_refs: HashMap::new(),
        host_state: HashMap::new(),
        active_rollouts,
        outstanding_compliance_events_by_rollout: HashMap::new(),
        last_deferrals: HashMap::new(),
            host_probes_passing: HashMap::new(),
    };

    let channel_names: Vec<String> = channel_rollouts.iter().map(|(c, _)| c.clone()).collect();
    let order = nixfleet_reconciler::topological_channel_order(fleet, &channel_names);

    let mut emitted_opens: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut held: HashMap<String, nixfleet_reconciler::observed::DeferralRecord> = HashMap::new();
    let mut released: Vec<String> = Vec::new();
    for channel in order {
        let Some((_, rid)) = channel_rollouts.iter().find(|(c, _)| c == &channel) else {
            continue;
        };
        if let Some(blocker) = nixfleet_reconciler::gates::channel_edges::check_for_channel(
            fleet,
            &pseudo_observed,
            &emitted_opens,
            &channel,
            // Polling iterates predecessor → successor in topo order
            // and updates `emitted_opens` as it goes. The in-tick set
            // is the authoritative signal — Reconcile mode is correct.
            nixfleet_reconciler::gates::GateMode::Reconcile,
        ) {
            tracing::info!(
                channel = %channel,
                rollout = %rid,
                blocked_by = %blocker,
                "channel-refs poll: skip record_active_rollout — channelEdges holds until predecessor converges",
            );
            held.insert(
                channel.clone(),
                nixfleet_reconciler::observed::DeferralRecord {
                    target_ref: rid.clone(),
                    blocked_by: blocker,
                },
            );
            continue;
        }
        if let Err(err) = db.rollouts().record_active_rollout(rid, &channel) {
            tracing::warn!(
                channel = %channel,
                rollout = %rid,
                error = %err,
                "channel-refs poll: record_active_rollout failed (non-fatal)",
            );
        } else {
            emitted_opens.insert(channel.clone());
            released.push(channel);
        }
    }

    // Mirror the polling-layer hold/release decision into last_deferrals so
    // /v1/deferrals shows the held channel. The reconciler's RolloutDeferred
    // path can't reach this — it short-circuits on `has_active=true` when
    // the previous-rev rollout for the held channel is still in the table.
    if !held.is_empty() || !released.is_empty() {
        let mut guard = last_deferrals.write().await;
        for ch in &released {
            guard.remove(ch);
        }
        for (ch, rec) in held {
            guard.insert(ch, rec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_db;
    use crate::db::DispatchInsert;
    use crate::server::VerifiedFleetSnapshot;
    use crate::state::{HealthyMarker, HostRolloutState};
    use chrono::Utc;
    use nixfleet_proto::testing::FleetBuilder;
    use nixfleet_proto::{ChannelEdge, FleetResolved};

    fn fleet_edge_then_stable() -> FleetResolved {
        let mut f = FleetBuilder::new()
            .host("lab", "edge")
            .host_tag("lab", "server")
            .host_closure("lab", "lab-closure-new")
            .host("krach", "stable")
            .host_tag("krach", "dev")
            .host_closure("krach", "krach-closure-new")
            .policy_waves("p", vec![])
            .build();
        f.channel_edges = vec![ChannelEdge {
            gates: "edge".into(),
            gated: "stable".into(),
            reason: Some("canary".into()),
        }];
        f
    }

    fn seed_old_rollout(db: &crate::db::Db, rollout_id: &str, channel: &str, hostname: &str) {
        db.rollouts()
            .record_active_rollout(rollout_id, channel)
            .unwrap();
        db.host_dispatch_state()
            .record_dispatch(&DispatchInsert {
                hostname,
                rollout_id,
                channel,
                wave: 0,
                target_closure_hash: "old-closure",
                target_channel_ref: rollout_id,
                confirm_deadline: Utc::now() + chrono::Duration::seconds(60),
            })
            .unwrap();
        db.rollout_state()
            .transition_host_state(
                hostname,
                rollout_id,
                HostRolloutState::Converged,
                HealthyMarker::Untouched,
                None,
            )
            .unwrap();
    }

    /// Regression: without the `current_rollout_ids` filter, the previous
    /// rev's edge rollout (lab=Converged, terminal-for-ordering) would
    /// satisfy the predecessor check for the new stable rollout and
    /// channelEdges would silently bypass on the first poll of every
    /// release.
    #[tokio::test]
    async fn stale_predecessor_rollout_does_not_unblock_new_successor() {
        let db = fresh_db();
        let fleet = fleet_edge_then_stable();
        let verified_fleet = Arc::new(RwLock::new(Some(VerifiedFleetSnapshot {
            fleet: Arc::new(fleet),
            fleet_resolved_hash: "test-hash".into(),
        })));
        let last_deferrals = Arc::new(RwLock::new(HashMap::new()));

        // Previous rev: edge fully converged, stable also fully converged.
        // These rows survive in host_dispatch_state across a fleet rev
        // change until polling supersedes them.
        seed_old_rollout(&db, "old-edge-rid", "edge", "lab");
        seed_old_rollout(&db, "old-stable-rid", "stable", "krach");

        // New rev's rolloutIds — what compute_rollout_id_for_channel would
        // return for the just-published fleet.resolved.
        let channel_rollouts = vec![
            ("edge".into(), "new-edge-rid".into()),
            ("stable".into(), "new-stable-rid".into()),
        ];

        record_rollouts_gated_by_channel_edges(
            &db,
            &verified_fleet,
            &last_deferrals,
            &channel_rollouts,
        )
        .await;

        let active = db.rollouts().list_active().unwrap();
        let active_ids: std::collections::HashSet<String> =
            active.iter().map(|r| r.rollout_id.clone()).collect();
        assert!(
            active_ids.contains("new-edge-rid"),
            "edge has no predecessor; must be recorded. active={active_ids:?}",
        );
        assert!(
            !active_ids.contains("new-stable-rid"),
            "stable must be HELD by channelEdges — old converged predecessor must NOT count. active={active_ids:?}",
        );

        let supersede = db
            .rollouts()
            .supersede_status("old-edge-rid")
            .unwrap()
            .expect("old edge row must exist");
        assert!(
            supersede.is_superseded(),
            "old edge must be superseded by new edge in the same record_active_rollout txn",
        );

        let deferrals = last_deferrals.read().await;
        let stable_record = deferrals
            .get("stable")
            .expect("/v1/deferrals must show stable held — last_deferrals write missing");
        assert_eq!(stable_record.blocked_by, "edge");
        assert_eq!(stable_record.target_ref, "new-stable-rid");
    }

    /// Positive flow: once the predecessor's CURRENT rollout converges,
    /// the next poll opens the successor and clears the deferral.
    #[tokio::test]
    async fn successor_opens_once_current_predecessor_converges() {
        let db = fresh_db();
        let fleet = fleet_edge_then_stable();
        let verified_fleet = Arc::new(RwLock::new(Some(VerifiedFleetSnapshot {
            fleet: Arc::new(fleet),
            fleet_resolved_hash: "test-hash".into(),
        })));
        let last_deferrals = Arc::new(RwLock::new(HashMap::new()));

        let channel_rollouts = vec![
            ("edge".into(), "new-edge-rid".into()),
            ("stable".into(), "new-stable-rid".into()),
        ];

        // First poll: stable held (edge has no host_states yet → active).
        record_rollouts_gated_by_channel_edges(
            &db,
            &verified_fleet,
            &last_deferrals,
            &channel_rollouts,
        )
        .await;
        assert!(
            last_deferrals.read().await.contains_key("stable"),
            "first poll: stable must be held",
        );

        // Simulate lab activating + converging on the new edge rollout.
        db.host_dispatch_state()
            .record_dispatch(&DispatchInsert {
                hostname: "lab",
                rollout_id: "new-edge-rid",
                channel: "edge",
                wave: 0,
                target_closure_hash: "lab-closure-new",
                target_channel_ref: "new-edge-rid",
                confirm_deadline: Utc::now() + chrono::Duration::seconds(60),
            })
            .unwrap();
        db.rollout_state()
            .transition_host_state(
                "lab",
                "new-edge-rid",
                HostRolloutState::Converged,
                HealthyMarker::Untouched,
                None,
            )
            .unwrap();

        // Second poll: stable should now open.
        record_rollouts_gated_by_channel_edges(
            &db,
            &verified_fleet,
            &last_deferrals,
            &channel_rollouts,
        )
        .await;

        let active = db.rollouts().list_active().unwrap();
        let active_ids: std::collections::HashSet<String> =
            active.iter().map(|r| r.rollout_id.clone()).collect();
        assert!(
            active_ids.contains("new-stable-rid"),
            "stable must open once edge fully converged. active={active_ids:?}",
        );
        assert!(
            !last_deferrals.read().await.contains_key("stable"),
            "stable's last_deferrals entry must be cleared on release",
        );
    }
}
