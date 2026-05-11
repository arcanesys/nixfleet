//! `verify_revocations` - signature, freshness window, reject_before gate.

mod common;

use chrono::{Duration as ChronoDuration, Utc};
use common::signing::{fresh_signing_key, sign_artifact, trust_root_for};
use ed25519_dalek::Signer;
use nixfleet_canonicalize::canonicalize;
use nixfleet_reconciler::{verify_revocations, VerifyError};
use std::time::Duration;

const FIXTURE_REVOCATIONS: &str = r#"{
  "meta": {
    "schemaVersion": 1,
    "signedAt": "2026-04-28T10:00:00Z",
    "ciCommit": "abc12345",
    "signatureAlgorithm": "ed25519"
  },
  "revocations": [
    {
      "hostname": "old-laptop",
      "notBefore": "2026-04-26T00:00:00Z",
      "reason": "decommissioned",
      "revokedBy": "operator"
    }
  ],
  "schemaVersion": 1
}"#;

#[test]
fn verify_revocations_ok_returns_revocations() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_REVOCATIONS);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_revocations(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    );
    let revs = result.expect("verify_revocations_ok");
    assert_eq!(revs.schema_version, 1);
    assert_eq!(revs.revocations.len(), 1);
    assert_eq!(revs.revocations[0].hostname, "old-laptop");
}

#[test]
fn verify_revocations_rejects_tampered_signature() {
    let (bytes, mut sig, trust, signed_at) = sign_artifact(FIXTURE_REVOCATIONS);
    sig[0] ^= 0xFF;
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_revocations(
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
fn verify_revocations_rejects_stale() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_REVOCATIONS);
    let now = signed_at + ChronoDuration::hours(4);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_revocations(
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
fn verify_revocations_rejects_unsigned() {
    let signing_key = fresh_signing_key();
    let trust = trust_root_for(&signing_key);
    let json = r#"{
      "meta": { "schemaVersion": 1, "signedAt": null, "ciCommit": "abc12345", "signatureAlgorithm": "ed25519" },
      "revocations": [],
      "schemaVersion": 1
    }"#;
    let reserialized =
        serde_json::to_string(&serde_json::from_str::<serde_json::Value>(json).unwrap()).unwrap();
    let canonical = canonicalize(&reserialized).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();
    let err = verify_revocations(
        canonical.as_bytes(),
        &sig,
        std::slice::from_ref(&trust),
        Utc::now(),
        Duration::from_secs(3600),
        None,
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::NotSigned), "got {err:?}");
}

#[test]
fn verify_revocations_empty_list_is_valid() {
    let json = r#"{
      "meta": {
        "schemaVersion": 1,
        "signedAt": "2026-04-28T10:00:00Z",
        "ciCommit": "abc12345",
        "signatureAlgorithm": "ed25519"
      },
      "revocations": [],
      "schemaVersion": 1
    }"#;
    let (bytes, sig, trust, signed_at) = sign_artifact(json);
    let now = signed_at + ChronoDuration::minutes(5);
    let revs = verify_revocations(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        Duration::from_secs(3600),
        None,
    )
    .expect("empty list verifies");
    assert!(revs.revocations.is_empty());
}

// Per-sidecar coverage: guards against a future bypass-in-wrapper.

#[test]
fn verify_revocations_rejects_malformed_json() {
    // Sig-verifies but schema-parse fails.
    let signing_key = fresh_signing_key();
    let trust = trust_root_for(&signing_key);
    let canonical = canonicalize(r#"{"not":"a-revocations"}"#).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();
    let err = verify_revocations(
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
fn verify_revocations_rejects_when_trust_roots_empty() {
    let (bytes, sig, _trust, signed_at) = sign_artifact(FIXTURE_REVOCATIONS);
    let now = signed_at + ChronoDuration::minutes(30);
    let err =
        verify_revocations(&bytes, &sig, &[], now, Duration::from_secs(3600), None).unwrap_err();
    assert!(
        matches!(err, VerifyError::NoTrustRoots),
        "empty trust roots → NoTrustRoots; got {err:?}"
    );
}

#[test]
fn verify_revocations_reject_before_rejects_pre_compromise() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_REVOCATIONS);
    let now = signed_at + ChronoDuration::minutes(30);
    let reject_before = signed_at + ChronoDuration::seconds(1);
    let err = verify_revocations(
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
        "reject_before must apply to revocations; got {err:?}"
    );
}

#[test]
fn verify_revocations_reject_before_none_disables_gate() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_REVOCATIONS);
    let now = signed_at + ChronoDuration::minutes(30);
    verify_revocations(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        Duration::from_secs(3600),
        None,
    )
    .expect("None disables the gate, same as verify_artifact");
}
