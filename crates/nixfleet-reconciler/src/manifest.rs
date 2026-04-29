//! Pure projection: fleet.resolved + channel context -> RolloutManifest.
//! Producer (nixfleet-release) and CP (re-derivation) share this fn.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use nixfleet_proto::{FleetResolved, HostWave, Meta, RolloutBudget, RolloutManifest};

/// RolloutIds the current fleet snapshot expects across all channels. Filters
/// `host_dispatch_state` snapshots to "this rev's rollouts only" so stale
/// Converged rollouts from a previous rev don't poison gate evaluation.
/// Consumed by polling, the deferrals route, and the per-checkin dispatch
/// pipeline; centralising it keeps the filter consistent. Errored channels
/// are silently dropped (callers log at their site, this fn stays pure).
pub fn current_rollout_ids(
    fleet: &FleetResolved,
    fleet_resolved_hash: &str,
) -> std::collections::HashSet<String> {
    fleet
        .channels
        .keys()
        .filter_map(|ch| {
            compute_rollout_id_for_channel(fleet, fleet_resolved_hash, ch)
                .ok()
                .flatten()
        })
        .collect()
}

/// CP-side rolloutId for a host on `channel`. `Ok(None)` when the channel
/// has no host with a declared closure.
pub fn compute_rollout_id_for_channel(
    fleet: &FleetResolved,
    fleet_resolved_hash: &str,
    channel: &str,
) -> Result<Option<String>> {
    let signed_at = fleet
        .meta
        .signed_at
        .ok_or_else(|| anyhow!("fleet.meta.signedAt is None - cannot project manifest"))?;
    let ci_commit = fleet.meta.ci_commit.as_deref();
    let manifest = match project_manifest(
        fleet,
        channel,
        fleet_resolved_hash,
        signed_at,
        ci_commit,
        fleet.meta.signature_algorithm_or_default(),
    )? {
        Some(m) => m,
        None => return Ok(None),
    };
    let id = crate::verify::compute_rollout_id(&manifest)
        .map_err(|e| anyhow!("compute_rollout_id: {e:?}"))?;
    Ok(Some(id))
}

/// Project one channel out of fleet.resolved. `Ok(None)` when no host on
/// the channel has a `closureHash`. `host_set` sorted for canonical-byte
/// stability.
pub fn project_manifest(
    fleet: &FleetResolved,
    channel: &str,
    fleet_resolved_hash: &str,
    signed_at: DateTime<Utc>,
    ci_commit: Option<&str>,
    signature_algorithm: &str,
) -> Result<Option<RolloutManifest>> {
    let channel_def = fleet
        .channels
        .get(channel)
        .ok_or_else(|| anyhow!("channel {channel} missing from fleet.channels"))?;

    let policy = fleet
        .rollout_policies
        .get(&channel_def.rollout_policy)
        .ok_or_else(|| {
            anyhow!(
                "rollout policy {} for channel {channel} not found in fleet.rolloutPolicies",
                channel_def.rollout_policy
            )
        })?;

    let waves = fleet.waves.get(channel);

    let mut host_set: Vec<HostWave> = Vec::new();
    for (hostname, host) in fleet.hosts.iter() {
        if host.channel != channel {
            continue;
        }
        let target_closure = match host.closure_hash.as_ref() {
            Some(c) => c.clone(),
            None => continue,
        };
        let wave_index: u32 = match waves {
            Some(ws) => ws
                .iter()
                .position(|w| w.hosts.iter().any(|h| h == hostname))
                .map(|i| i as u32)
                .unwrap_or(0),
            None => 0,
        };
        host_set.push(HostWave {
            hostname: hostname.clone(),
            wave_index,
            target_closure,
        });
    }

    if host_set.is_empty() {
        return Ok(None);
    }
    host_set.sort_by(|a, b| a.hostname.cmp(&b.hostname));

    let display_name = format!(
        "{}@{}",
        channel,
        ci_commit
            .map(|c| c.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "unknown".to_string())
    );

    let channel_ref = ci_commit.unwrap_or_default().to_string();

    // Snapshot disruption budgets here; selectors resolve once and freeze.
    // Mid-rollout retags affect future rollouts, never this one. Hosts sorted
    // for JCS canonical-byte stability.
    let disruption_budgets: Vec<RolloutBudget> = fleet
        .disruption_budgets
        .iter()
        .map(|b| {
            let mut hosts = b.selector.resolve(fleet.hosts.iter());
            hosts.sort();
            RolloutBudget {
                selector: b.selector.clone(),
                hosts,
                max_in_flight: b.max_in_flight,
                max_in_flight_pct: b.max_in_flight_pct,
            }
        })
        .collect();

    Ok(Some(RolloutManifest {
        schema_version: 1,
        display_name,
        channel: channel.to_string(),
        channel_ref,
        fleet_resolved_hash: fleet_resolved_hash.to_string(),
        host_set,
        health_gate: policy.health_gate.clone(),
        compliance_frameworks: channel_def.compliance.frameworks.clone(),
        disruption_budgets,
        meta: Meta {
            schema_version: 1,
            signed_at: Some(signed_at),
            ci_commit: ci_commit.map(|c| c.to_string()),
            signature_algorithm: Some(signature_algorithm.to_string()),
        },
    }))
}
