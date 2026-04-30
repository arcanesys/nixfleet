//! `verify_rollout_manifest` + `compute_rollout_id` integration.

mod common;

use chrono::{Duration as ChronoDuration, Utc};
use common::signing::{fresh_signing_key, sign_artifact, trust_root_for};
use ed25519_dalek::Signer;
use nixfleet_canonicalize::canonicalize;
use nixfleet_reconciler::{compute_rollout_id, verify_rollout_manifest, VerifyError};
use std::time::Duration;

const FIXTURE_MANIFEST: &str = r#"{
  "schemaVersion": 1,
  "displayName": "stable@def4567",
  "channel": "stable",
  "channelRef": "def4567abc123def4567abc123def4567abc123d",
  "fleetResolvedHash": "1111111111111111111111111111111111111111111111111111111111111111",
  "hostSet": [
    {"hostname": "agent-01", "waveIndex": 0, "targetClosure": "0000000000000000000000000000000000000000-host-a"},
    {"hostname": "agent-02", "waveIndex": 1, "targetClosure": "1111111111111111111111111111111111111111-host-b"}
  ],
  "healthGate": {},
  "complianceFrameworks": ["anssi-bp028"],
  "meta": {
    "schemaVersion": 1,
    "signedAt": "2026-04-30T12:00:00Z",
    "ciCommit": "def45678",
    "signatureAlgorithm": "ed25519"
  }
}"#;

#[test]
fn verify_rollout_manifest_ok_returns_manifest() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_MANIFEST);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_rollout_manifest(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    );
    let m = result.expect("verify_rollout_manifest_ok");
    assert_eq!(m.schema_version, 1);
    assert_eq!(m.channel, "stable");
    assert_eq!(m.host_set.len(), 2);
    assert_eq!(m.host_set[0].hostname, "agent-01");
    assert_eq!(m.host_set[1].wave_index, 1);
    assert!(m.host_set[0].target_closure.starts_with("0000"));
    assert!(m.host_set[1].target_closure.starts_with("1111"));
}

#[test]
fn verify_rollout_manifest_rejects_tampered_signature() {
    let (bytes, mut sig, trust, signed_at) = sign_artifact(FIXTURE_MANIFEST);
    sig[0] ^= 0xFF;
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_rollout_manifest(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::BadSignature));
}

#[test]
fn verify_rollout_manifest_rejects_stale() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_MANIFEST);
    let now = signed_at + ChronoDuration::hours(4);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_rollout_manifest(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::Stale { .. }));
}

#[test]
fn compute_rollout_id_is_64_hex_chars() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_MANIFEST);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let m = verify_rollout_manifest(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    )
    .expect("verify ok");

    let id = compute_rollout_id(&m).expect("compute_rollout_id");
    assert_eq!(id.len(), 64, "sha256 hex must be 64 chars: {id}");
    assert!(
        id.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "id must be hex lowercase only: {id}"
    );
}

#[test]
fn compute_rollout_id_stable_across_round_trip() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_MANIFEST);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let m = verify_rollout_manifest(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    )
    .expect("verify ok");

    let id1 = compute_rollout_id(&m).unwrap();
    let raw = serde_json::to_string(&m).unwrap();
    let m2: nixfleet_proto::RolloutManifest = serde_json::from_str(&raw).unwrap();
    let id2 = compute_rollout_id(&m2).unwrap();

    assert_eq!(id1, id2, "id must survive serialize/parse round-trip");
}

#[test]
fn compute_rollout_id_changes_with_field_change() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_MANIFEST);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let m = verify_rollout_manifest(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    )
    .expect("verify ok");
    let id1 = compute_rollout_id(&m).unwrap();

    let mut m2 = m.clone();
    m2.host_set[0].target_closure =
        "9999999999999999999999999999999999999999-perturbed".to_string();
    let id2 = compute_rollout_id(&m2).unwrap();

    assert_ne!(id1, id2);
}

#[test]
fn verify_rollout_manifest_rejects_unsigned() {
    let signing_key = fresh_signing_key();
    let trust = trust_root_for(&signing_key);
    let json = r#"{
      "schemaVersion": 1,
      "displayName": "stable@def4567",
      "channel": "stable",
      "channelRef": "def4567abc123def4567abc123def4567abc123d",
      "fleetResolvedHash": "1111111111111111111111111111111111111111111111111111111111111111",
      "hostSet": [],
      "healthGate": {},
      "complianceFrameworks": [],
      "meta": {
        "schemaVersion": 1,
        "signedAt": null,
        "ciCommit": "def45678",
        "signatureAlgorithm": "ed25519"
      }
    }"#;
    let reserialized =
        serde_json::to_string(&serde_json::from_str::<serde_json::Value>(json).unwrap()).unwrap();
    let canonical = canonicalize(&reserialized).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();
    let err = verify_rollout_manifest(
        canonical.as_bytes(),
        &sig,
        std::slice::from_ref(&trust),
        Utc::now(),
        Duration::from_secs(3600),
        None,
    )
    .unwrap_err();
    assert!(
        matches!(err, VerifyError::NotSigned),
        "unsigned manifest must be rejected; got {err:?}"
    );
}

#[test]
fn verify_rollout_manifest_rejects_malformed_json() {
    let signing_key = fresh_signing_key();
    let trust = trust_root_for(&signing_key);
    let canonical = canonicalize(r#"{"not":"a-manifest"}"#).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();
    let err = verify_rollout_manifest(
        canonical.as_bytes(),
        &sig,
        std::slice::from_ref(&trust),
        Utc::now(),
        Duration::from_secs(3600),
        None,
    )
    .unwrap_err();
    assert!(
        matches!(err, VerifyError::Parse(_)),
        "expected ParseError, got {err:?}"
    );
}

#[test]
fn verify_rollout_manifest_rejects_when_trust_roots_empty() {
    let (bytes, sig, _trust, signed_at) = sign_artifact(FIXTURE_MANIFEST);
    let now = signed_at + ChronoDuration::minutes(30);
    let err = verify_rollout_manifest(&bytes, &sig, &[], now, Duration::from_secs(3600), None)
        .unwrap_err();
    assert!(
        matches!(err, VerifyError::NoTrustRoots),
        "empty trust roots -> NoTrustRoots; got {err:?}"
    );
}

#[test]
fn verify_rollout_manifest_reject_before_rejects_pre_compromise() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_MANIFEST);
    let now = signed_at + ChronoDuration::minutes(30);
    let reject_before = signed_at + ChronoDuration::seconds(1);
    let err = verify_rollout_manifest(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        Duration::from_secs(3600),
        Some(reject_before),
    )
    .unwrap_err();
    assert!(
        matches!(err, VerifyError::RejectedBeforeTimestamp { .. }),
        "reject_before must apply to rollout manifest; got {err:?}"
    );
}

#[test]
fn rollout_id_from_bytes_is_cross_version_stable_across_additive_changes() {
    // Regression for the agent-bricking failure where an older agent's
    // RolloutManifest proto missed a field the producer added (the
    // disruption_budgets snapshot) and therefore re-serialised to a
    // smaller canonical payload, producing a different sha256 from
    // the producer's advertised rolloutId.
    //
    // The fix: verifiers MUST hash the bytes they received, never a
    // re-serialised parsed struct. We simulate the cross-version case
    // by handing both functions a JSON object with a forward-compatible
    // "futureField" the proto's RolloutManifest doesn't know about.
    use nixfleet_reconciler::{
        canonical_hash_from_bytes, compute_rollout_id, rollout_id_from_bytes,
    };

    // Producer-canonical bytes including a hypothetical future additive field.
    let producer_bytes = canonicalize(
        r#"{
            "schemaVersion": 1,
            "displayName": "stable@deadbeef",
            "channel": "stable",
            "channelRef": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            "fleetResolvedHash": "1111111111111111111111111111111111111111111111111111111111111111",
            "hostSet": [],
            "healthGate": {},
            "complianceFrameworks": [],
            "disruptionBudgets": [],
            "futureField": {"v0_3_property": "value"},
            "meta": {
                "schemaVersion": 1,
                "signedAt": "2026-04-30T12:00:00Z",
                "ciCommit": "deadbeef",
                "signatureAlgorithm": "ed25519"
            }
        }"#,
    )
    .expect("canonicalize")
    .into_bytes();

    let from_bytes = rollout_id_from_bytes(&producer_bytes).expect("from_bytes");

    // Round-trip via parsed struct (the LOSSY path the agent used to take).
    let parsed: nixfleet_proto::RolloutManifest =
        serde_json::from_slice(&producer_bytes).expect("parse");
    let from_struct = compute_rollout_id(&parsed).expect("from_struct");

    assert_ne!(
        from_bytes, from_struct,
        "Sanity check: round-tripping a manifest with unknown fields through \
         the typed proto LOSES those fields, producing a different hash. \
         If this assertion ever fails it means RolloutManifest gained a \
         catch-all map and the regression's premise no longer holds.",
    );

    // The producer would also have computed the bytes-hash. Verify the
    // hash that the verifier computes from raw bytes matches the canonical
    // bytes (it's a tautology - that's exactly the property we want).
    let recomputed = canonical_hash_from_bytes(&producer_bytes).expect("recompute");
    assert_eq!(
        from_bytes, recomputed,
        "rollout_id_from_bytes is a thin alias for canonical_hash_from_bytes",
    );
}
