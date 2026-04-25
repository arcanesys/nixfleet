//! Signed per-channel rollout manifest (`releases/rollouts/<rolloutId>.json`).
//! LOADBEARING: rolloutId is the SHA-256 of canonical received bytes (not a
//! label) - verifiers MUST canonicalize the received bytes and assert the
//! hash before consuming any other field. They MUST NOT hash a re-serialised
//! parsed `RolloutManifest`: re-serialisation drops fields the verifier's
//! proto doesn't know about, breaking content-addressing across additive
//! schema changes.

use serde::{Deserialize, Serialize};

use crate::fleet_resolved::{HealthGate, Meta, Selector};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RolloutManifest {
    pub schema_version: u32,

    /// `<channel>@<short-ci-commit>` - display only, not the identifier.
    pub display_name: String,

    pub channel: String,

    pub channel_ref: String,

    /// LOADBEARING: anchors the manifest to one signed `fleet.resolved`
    /// snapshot - different snapshot produces a different rolloutId, blocking
    /// cross-snapshot mix-and-match.
    pub fleet_resolved_hash: String,

    /// FOOTGUN: MUST be sorted by `hostname` ascending - JCS sorts object
    /// keys but not array elements; producer's order IS the canonical order.
    pub host_set: Vec<HostWave>,

    pub health_gate: HealthGate,

    /// Mirrored from `channels[channel].compliance.frameworks`.
    pub compliance_frameworks: Vec<String>,

    /// Disruption-budget snapshot resolved against `fleet.hosts.tags` at
    /// projection time and frozen for the rollout's life - mid-rollout
    /// retag does NOT reshape these. Cross-rollout in-flight counting
    /// matches by `selector` equality so fleet-wide enforcement survives
    /// the snapshot model. FOOTGUN: per-budget `hosts` MUST be sorted
    /// alphabetically (JCS canonicalizes keys, not array elements).
    #[serde(default)]
    pub disruption_budgets: Vec<RolloutBudget>,

    pub meta: Meta,
}

/// Per-rollout snapshot of a fleet-wide disruption budget. Selector is
/// preserved so cross-rollout sums match by intent even when host
/// membership has shifted between rollout opens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RolloutBudget {
    pub selector: Selector,
    pub hosts: Vec<String>,
    #[serde(default)]
    pub max_in_flight: Option<u32>,
    #[serde(default)]
    pub max_in_flight_pct: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct HostWave {
    pub hostname: String,
    /// Frozen at projection time; reshaping waves produces a new rolloutId.
    pub wave_index: u32,
    /// Per-host closure. Agent re-asserts this against the CP-advertised
    /// closure to detect retargeting.
    pub target_closure: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet_resolved::Meta;
    use nixfleet_canonicalize::canonicalize;

    fn meta_v1() -> Meta {
        Meta {
            schema_version: 1,
            signed_at: Some("2026-04-30T12:00:00Z".parse().unwrap()),
            ci_commit: Some("def45678".into()),
            signature_algorithm: Some("ed25519".into()),
        }
    }

    fn sample_manifest() -> RolloutManifest {
        RolloutManifest {
            schema_version: 1,
            display_name: "stable@def4567".into(),
            channel: "stable".into(),
            channel_ref: "def4567abc123def4567abc123def4567abc123d".into(),
            fleet_resolved_hash: "1111111111111111111111111111111111111111111111111111111111111111"
                .into(),
            host_set: vec![
                HostWave {
                    hostname: "agent-01".into(),
                    wave_index: 0,
                    target_closure: "0000000000000000000000000000000000000000-host-a".into(),
                },
                HostWave {
                    hostname: "agent-02".into(),
                    wave_index: 1,
                    target_closure: "1111111111111111111111111111111111111111-host-b".into(),
                },
            ],
            health_gate: HealthGate::default(),
            compliance_frameworks: vec!["anssi-bp028".into()],
            disruption_budgets: vec![],
            meta: meta_v1(),
        }
    }

    /// LOADBEARING: rolloutId = sha256(canonical(m)) depends on canonical-byte
    /// stability across round-trips.
    #[test]
    fn manifest_canonical_bytes_stable_across_round_trip() {
        let m = sample_manifest();
        let raw1 = serde_json::to_string(&m).unwrap();
        let canon1 = canonicalize(&raw1).unwrap();

        let parsed: RolloutManifest = serde_json::from_str(&canon1).unwrap();
        let raw2 = serde_json::to_string(&parsed).unwrap();
        let canon2 = canonicalize(&raw2).unwrap();

        assert_eq!(canon1, canon2);
    }

    #[test]
    fn manifest_host_set_order_changes_canonical_bytes() {
        let mut m1 = sample_manifest();
        let mut m2 = sample_manifest();
        m2.host_set.reverse();

        let canon1 = canonicalize(&serde_json::to_string(&m1).unwrap()).unwrap();
        let canon2 = canonicalize(&serde_json::to_string(&m2).unwrap()).unwrap();

        assert_ne!(
            canon1, canon2,
            "host_set order must affect canonical bytes (CI must emit sorted)"
        );

        m2.host_set.sort_by(|a, b| a.hostname.cmp(&b.hostname));
        let canon2_resorted = canonicalize(&serde_json::to_string(&m2).unwrap()).unwrap();
        assert_eq!(canon1, canon2_resorted);

        let _ = &mut m1;
    }

    #[test]
    fn fleet_resolved_hash_change_changes_canonical_bytes() {
        let m1 = sample_manifest();
        let mut m2 = sample_manifest();
        m2.fleet_resolved_hash =
            "2222222222222222222222222222222222222222222222222222222222222222".into();

        let canon1 = canonicalize(&serde_json::to_string(&m1).unwrap()).unwrap();
        let canon2 = canonicalize(&serde_json::to_string(&m2).unwrap()).unwrap();

        assert_ne!(canon1, canon2);
    }

    #[test]
    fn host_target_closure_change_changes_canonical_bytes() {
        let m1 = sample_manifest();
        let mut m2 = sample_manifest();
        m2.host_set[0].target_closure = "9999999999999999999999999999999999999999-perturbed".into();

        let canon1 = canonicalize(&serde_json::to_string(&m1).unwrap()).unwrap();
        let canon2 = canonicalize(&serde_json::to_string(&m2).unwrap()).unwrap();

        assert_ne!(canon1, canon2);
    }

    #[test]
    fn host_wave_round_trip() {
        let h = HostWave {
            hostname: "agent-03".into(),
            wave_index: 2,
            target_closure: "abcdef1234567890abcdef1234567890abcdef12-test".into(),
        };
        let s = serde_json::to_string(&h).unwrap();
        let parsed: HostWave = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, h);
        assert!(s.contains("\"waveIndex\":2"));
        assert!(s.contains("\"targetClosure\""));
    }
}
