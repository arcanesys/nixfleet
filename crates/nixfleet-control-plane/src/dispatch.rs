//! Dispatch decision: pure 3-way compare of current/declared/in-flight; caller handles DB.

use chrono::{DateTime, Utc};

use nixfleet_proto::{
    agent_wire::{ActivateBlock, CheckinRequest, EvaluatedTarget, FetchResult},
    FleetResolved,
};

const CONFIRM_ENDPOINT: &str = "/v1/agent/confirm";

// FOOTGUN: PartialEq is intentionally NOT derived (EvaluatedTarget doesn't
// implement it and `evaluated_at` equality wouldn't be meaningful). Tests
// pattern-match on variants.
#[derive(Debug, Clone)]
pub enum Decision {
    Converged,
    /// Not in `fleet.resolved.hosts`.
    Unmanaged,
    /// Listed but no `closureHash`.
    NoDeclaration,
    /// Operational dispatch already in flight.
    InFlight,
    /// Last fetch failed; hold rather than dispatch.
    HoldAfterFailure,
    /// Host's `wave_index` exceeds the rollout's `current_wave`. Holds wave-N
    /// hosts at the dispatch endpoint until PromoteWave advances current_wave.
    WaveNotReached,
    Dispatch {
        target: EvaluatedTarget,
        rollout_id: String,
        wave_index: Option<u32>,
    },
}

/// LOADBEARING: `fleet_resolved_hash` anchors rolloutId to the verified
/// snapshot's canonical bytes. Different snapshot ⇒ different rolloutId,
/// preserving the wire promise that every advertised rolloutId resolves to a
/// CI-signed manifest. `current_wave = None` ⇒ wave 0 (rollouts open at 0).
#[allow(clippy::too_many_arguments)]
pub fn decide_target(
    hostname: &str,
    request: &CheckinRequest,
    fleet: &FleetResolved,
    fleet_resolved_hash: &str,
    pending_for_host: bool,
    now: DateTime<Utc>,
    confirm_window_secs: u32,
    current_wave: Option<u32>,
) -> Decision {
    let host = match fleet.hosts.get(hostname) {
        Some(h) => h,
        None => return Decision::Unmanaged,
    };

    let target_closure = match host.closure_hash.as_ref() {
        Some(h) => h,
        None => return Decision::NoDeclaration,
    };

    if request.current_generation.closure_hash == *target_closure {
        return Decision::Converged;
    }

    if pending_for_host {
        return Decision::InFlight;
    }

    if let Some(outcome) = &request.last_fetch_outcome {
        if matches!(
            outcome.result,
            FetchResult::VerifyFailed | FetchResult::FetchFailed
        ) {
            return Decision::HoldAfterFailure;
        }
    }

    let rollout_id = match nixfleet_reconciler::compute_rollout_id_for_channel(
        fleet,
        fleet_resolved_hash,
        &host.channel,
    ) {
        Ok(Some(id)) => id,
        Ok(None) => return Decision::NoDeclaration,
        Err(err) => {
            tracing::error!(
                hostname = %hostname,
                error = ?err,
                "dispatch: compute_rollout_id_for_channel failed; holding",
            );
            return Decision::HoldAfterFailure;
        }
    };

    let wave_index: Option<u32> = fleet.waves.get(&host.channel).and_then(|waves| {
        waves
            .iter()
            .position(|w| w.hosts.iter().any(|h| h == hostname))
            .map(|i| i as u32)
    });

    // Wave-promotion gate. The reconciler advances `current_wave` via
    // PromoteWave actions when the previous wave reaches Soaked; until
    // then, hosts in later waves are held at the dispatch endpoint.
    // Without this gate, the agent-facing checkin path would serve
    // wave-N hosts their target the instant a rollout opens, even
    // though the reconciler's handle_wave only emits DispatchHost
    // actions for the current wave. Two layers, same invariant.
    //
    // `current_wave = None` means the rollout hasn't been recorded in
    // the DB yet; treat as wave 0 (the start of every staged rollout).
    // Hosts not in any declared wave (`wave_index = None`) are
    // ungated - that path is exercised by single-wave channels with
    // `selector.all = true` where the wave doesn't filter by host.
    if let Some(host_wave) = wave_index {
        let effective_current_wave = current_wave.unwrap_or(0);
        if host_wave > effective_current_wave {
            return Decision::WaveNotReached;
        }
    }

    // Both invariants per the verified-artifact contract: §4 requires
    // `meta.signedAt`, and the host's channel exists by construction
    // (we already resolved `host.channel` above).
    let signed_at = fleet
        .meta
        .signed_at
        .expect("verified artifact carries meta.signedAt per §4 contract");
    let freshness_window_secs = fleet
        .channels
        .get(&host.channel)
        .map(|ch| ch.freshness_window.saturating_mul(60))
        .expect("host's declared channel resolves in verified fleet");

    Decision::Dispatch {
        target: EvaluatedTarget {
            closure_hash: target_closure.clone(),
            channel_ref: rollout_id.clone(),
            evaluated_at: now,
            rollout_id: rollout_id.clone(),
            wave_index,
            activate: Some(ActivateBlock {
                confirm_window_secs,
                confirm_endpoint: CONFIRM_ENDPOINT.to_string(),
            }),
            signed_at,
            freshness_window_secs,
            compliance_mode: fleet
                .channels
                .get(&host.channel)
                .map(|ch| ch.compliance.mode.clone()),
        },
        rollout_id,
        wave_index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nixfleet_proto::{
        agent_wire::{FetchOutcome, GenerationRef},
        fleet_resolved::Meta,
        Host,
    };

    const TEST_FLEET_HASH: &str =
        "0000000000000000000000000000000000000000000000000000000000000000";

    fn fleet_with(hostname: &str, host: Host) -> FleetResolved {
        use nixfleet_proto::testing::FleetBuilder;
        let mut f = FleetBuilder::new()
            .channel("stable", "p")
            .policy_strategy("p", "waves")
            .policy_waves("p", vec![])
            .meta(Meta {
                schema_version: 1,
                signed_at: Some(
                    DateTime::parse_from_rfc3339("2026-04-26T00:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
                ci_commit: Some("abc12345deadbeef".to_string()),
                signature_algorithm: Some("ed25519".into()),
            })
            .build();
        // Caller passes in a fully-formed Host (varies per test), so insert
        // raw rather than going through host() defaults. Channel "stable"
        // declares rollout_policy "p" - match what host.channel references.
        f.hosts.insert(hostname.to_string(), host);
        f
    }

    fn host(closure_hash: Option<&str>) -> Host {
        Host {
            system: "x86_64-linux".to_string(),
            tags: vec![],
            channel: "stable".to_string(),
            closure_hash: closure_hash.map(String::from),
            pubkey: None,
            pin: None,
        }
    }

    fn checkin(closure_hash: &str, fetch: Option<FetchResult>) -> CheckinRequest {
        CheckinRequest {
            hostname: "test-host".to_string(),
            agent_version: "test".to_string(),
            current_generation: GenerationRef {
                closure_hash: closure_hash.to_string(),
                channel_ref: None,
                boot_id: "boot".to_string(),
            },
            pending_generation: None,
            last_evaluated_target: None,
            last_fetch_outcome: fetch.map(|r| FetchOutcome {
                result: r,
                error: None,
            }),
            uptime_secs: None,
            last_confirmed_at: None,
            attestation_signature: None,
            health_probes: vec![],
            health_check_mode: None,
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-04-26T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn unmanaged_when_host_not_in_fleet() {
        let fleet = fleet_with("ohm", host(Some("declared-system")));
        let req = checkin("running-system", Some(FetchResult::Ok));
        assert!(matches!(
            decide_target(
                "test-host",
                &req,
                &fleet,
                TEST_FLEET_HASH,
                false,
                now(),
                120,
                None
            ),
            Decision::Unmanaged
        ));
    }

    #[test]
    fn no_declaration_when_fleet_omits_closure() {
        let fleet = fleet_with("test-host", host(None));
        let req = checkin("running-system", Some(FetchResult::Ok));
        assert!(matches!(
            decide_target(
                "test-host",
                &req,
                &fleet,
                TEST_FLEET_HASH,
                false,
                now(),
                120,
                None
            ),
            Decision::NoDeclaration
        ));
    }

    #[test]
    fn converged_when_current_matches_target() {
        let fleet = fleet_with("test-host", host(Some("matched-system")));
        let req = checkin("matched-system", Some(FetchResult::Ok));
        assert!(matches!(
            decide_target(
                "test-host",
                &req,
                &fleet,
                TEST_FLEET_HASH,
                false,
                now(),
                120,
                None
            ),
            Decision::Converged
        ));
    }

    #[test]
    fn in_flight_when_pending_row_exists() {
        let fleet = fleet_with("test-host", host(Some("declared-system")));
        let req = checkin("running-system", Some(FetchResult::Ok));
        assert!(matches!(
            decide_target(
                "test-host",
                &req,
                &fleet,
                TEST_FLEET_HASH,
                /* pending */ true,
                now(),
                120,
                None
            ),
            Decision::InFlight
        ));
    }

    #[test]
    fn hold_after_verify_failed() {
        let fleet = fleet_with("test-host", host(Some("declared-system")));
        let req = checkin("running-system", Some(FetchResult::VerifyFailed));
        assert!(matches!(
            decide_target(
                "test-host",
                &req,
                &fleet,
                TEST_FLEET_HASH,
                false,
                now(),
                120,
                None
            ),
            Decision::HoldAfterFailure
        ));
    }

    #[test]
    fn hold_after_fetch_failed() {
        let fleet = fleet_with("test-host", host(Some("declared-system")));
        let req = checkin("running-system", Some(FetchResult::FetchFailed));
        assert!(matches!(
            decide_target(
                "test-host",
                &req,
                &fleet,
                TEST_FLEET_HASH,
                false,
                now(),
                120,
                None
            ),
            Decision::HoldAfterFailure
        ));
    }

    #[test]
    fn dispatch_when_diverged_and_no_pending() {
        let fleet = fleet_with("test-host", host(Some("declared-system")));
        let req = checkin("running-system", Some(FetchResult::Ok));
        let d = decide_target(
            "test-host",
            &req,
            &fleet,
            TEST_FLEET_HASH,
            false,
            now(),
            120,
            None,
        );
        let Decision::Dispatch {
            target,
            rollout_id,
            wave_index,
        } = d
        else {
            panic!("expected Dispatch, got {:?}", d);
        };
        assert_eq!(target.closure_hash, "declared-system");
        assert_eq!(rollout_id.len(), 64);
        assert!(rollout_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(target.channel_ref, rollout_id);
        assert_eq!(target.evaluated_at, now());
        assert_eq!(target.rollout_id, rollout_id);
        assert_eq!(wave_index, None);
        assert_eq!(target.wave_index, None);
        let activate = target.activate.expect("activate block populated");
        assert_eq!(activate.confirm_window_secs, 120);
        assert_eq!(activate.confirm_endpoint, "/v1/agent/confirm");
    }

    #[test]
    fn dispatch_surfaces_wave_index_when_waves_declared() {
        // test-host is in wave 1; current_wave must be ≥ 1 for dispatch.
        // (The wave-promotion gate is locked separately in
        // wave_promotion_gate_blocks_dispatch_for_later_waves.)
        let mut fleet = fleet_with("test-host", host(Some("declared-system")));
        fleet.waves.insert(
            "stable".to_string(),
            vec![
                nixfleet_proto::Wave {
                    hosts: vec!["other-host".to_string()],
                    soak_minutes: 5,
                },
                nixfleet_proto::Wave {
                    hosts: vec!["test-host".to_string()],
                    soak_minutes: 5,
                },
            ],
        );
        let req = checkin("running-system", Some(FetchResult::Ok));
        let d = decide_target(
            "test-host",
            &req,
            &fleet,
            TEST_FLEET_HASH,
            false,
            now(),
            120,
            Some(1),
        );
        let Decision::Dispatch {
            target, wave_index, ..
        } = d
        else {
            panic!("expected Dispatch");
        };
        assert_eq!(wave_index, Some(1));
        assert_eq!(target.wave_index, Some(1));
    }

    #[test]
    fn dispatch_yields_distinct_rollout_ids_for_distinct_snapshots() {
        let fleet = fleet_with("test-host", host(Some("declared-system")));
        let req = checkin("running-system", Some(FetchResult::Ok));
        let d1 = decide_target(
            "test-host",
            &req,
            &fleet,
            "1111111111111111111111111111111111111111111111111111111111111111",
            false,
            now(),
            120,
            None,
        );
        let d2 = decide_target(
            "test-host",
            &req,
            &fleet,
            "2222222222222222222222222222222222222222222222222222222222222222",
            false,
            now(),
            120,
            None,
        );
        let (id1, id2) = match (d1, d2) {
            (
                Decision::Dispatch { rollout_id: a, .. },
                Decision::Dispatch { rollout_id: b, .. },
            ) => (a, b),
            other => panic!("expected two Dispatch decisions, got {other:?}"),
        };
        assert_ne!(id1, id2);
    }

    #[test]
    fn dispatch_threads_confirm_window_into_activate_block() {
        let fleet = fleet_with("test-host", host(Some("declared-system")));
        let req = checkin("running-system", Some(FetchResult::Ok));
        let d = decide_target(
            "test-host",
            &req,
            &fleet,
            TEST_FLEET_HASH,
            false,
            now(),
            240,
            None,
        );
        let Decision::Dispatch { target, .. } = d else {
            panic!("expected Dispatch");
        };
        let activate = target.activate.expect("activate block populated");
        assert_eq!(activate.confirm_window_secs, 240);
    }

    #[test]
    fn wave_promotion_gate_blocks_dispatch_for_later_waves() {
        // test-host is in wave 1; rollout's current_wave is 0 → block.
        let mut fleet = fleet_with("test-host", host(Some("declared-system")));
        fleet.waves.insert(
            "stable".to_string(),
            vec![
                nixfleet_proto::Wave {
                    hosts: vec!["other-host".to_string()],
                    soak_minutes: 5,
                },
                nixfleet_proto::Wave {
                    hosts: vec!["test-host".to_string()],
                    soak_minutes: 5,
                },
            ],
        );
        let req = checkin("running-system", Some(FetchResult::Ok));

        let blocked = decide_target(
            "test-host",
            &req,
            &fleet,
            TEST_FLEET_HASH,
            false,
            now(),
            120,
            Some(0),
        );
        assert!(
            matches!(blocked, Decision::WaveNotReached),
            "wave-1 host must be held when current_wave=0; got {blocked:?}",
        );

        // Same fleet, current_wave advanced to 1 → dispatch.
        let allowed = decide_target(
            "test-host",
            &req,
            &fleet,
            TEST_FLEET_HASH,
            false,
            now(),
            120,
            Some(1),
        );
        assert!(
            matches!(allowed, Decision::Dispatch { .. }),
            "wave-1 host must dispatch once current_wave=1; got {allowed:?}",
        );
    }

    #[test]
    fn wave_promotion_gate_treats_missing_current_wave_as_zero() {
        // Pre-DB-record case: rollouts table has no entry yet, so the
        // checkin path passes None. Wave-0 hosts dispatch (the rollout
        // semantically opens at wave 0 by default); wave-1 hosts hold.
        let mut fleet = fleet_with("test-host", host(Some("declared-system")));
        fleet.waves.insert(
            "stable".to_string(),
            vec![
                nixfleet_proto::Wave {
                    hosts: vec!["test-host".to_string()],
                    soak_minutes: 5,
                },
                nixfleet_proto::Wave {
                    hosts: vec!["other-host".to_string()],
                    soak_minutes: 5,
                },
            ],
        );
        let req = checkin("running-system", Some(FetchResult::Ok));
        let d = decide_target(
            "test-host",
            &req,
            &fleet,
            TEST_FLEET_HASH,
            false,
            now(),
            120,
            None,
        );
        assert!(
            matches!(d, Decision::Dispatch { .. }),
            "wave-0 host with no rollout-record yet should still dispatch (None ≡ wave 0); got {d:?}",
        );
    }

    #[test]
    fn dispatch_when_no_fetch_outcome_yet() {
        let fleet = fleet_with("test-host", host(Some("declared-system")));
        let req = checkin("running-system", None);
        let d = decide_target(
            "test-host",
            &req,
            &fleet,
            TEST_FLEET_HASH,
            false,
            now(),
            120,
            None,
        );
        assert!(matches!(d, Decision::Dispatch { .. }));
    }
}
