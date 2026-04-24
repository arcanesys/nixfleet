//! Step 0 — signature verification + freshness window.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_canonicalize::canonicalize;
use nixfleet_proto::TrustedPubkey;
use nixfleet_reconciler::{verify_artifact, VerifyError};
use rand::rngs::OsRng;
use rand::TryRngCore;
use std::time::Duration;

/// Generate a fresh ed25519 signing key using the OS CSPRNG.
///
/// We go through `rand::rngs::OsRng` (rand 0.9) and feed raw bytes to
/// `SigningKey::from_bytes`, bypassing `SigningKey::generate` — the latter
/// wants a `rand_core` 0.6 `CryptoRngCore`, which rand 0.9's `OsRng` does
/// not implement.
fn fresh_signing_key() -> SigningKey {
    let mut seed = [0u8; 32];
    OsRng.try_fill_bytes(&mut seed).expect("OS CSPRNG");
    SigningKey::from_bytes(&seed)
}

fn trust_root_for(signing_key: &SigningKey) -> TrustedPubkey {
    TrustedPubkey {
        algorithm: "ed25519".to_string(),
        public: BASE64_STANDARD.encode(signing_key.verifying_key().as_bytes()),
    }
}

/// Build a signed fleet.resolved artifact from JSON source.
///
/// Returns (signed_bytes, signature, trust_root, signed_at).
fn sign_artifact(json: &str) -> (Vec<u8>, [u8; 64], TrustedPubkey, DateTime<Utc>) {
    let signing_key = fresh_signing_key();
    let trust = trust_root_for(&signing_key);

    let value: serde_json::Value = serde_json::from_str(json).expect("parse");
    let signed_at: DateTime<Utc> = value["meta"]["signedAt"]
        .as_str()
        .expect("fixture must have meta.signedAt set")
        .parse()
        .expect("parse RFC 3339");

    let reserialized = serde_json::to_string(&value).unwrap();
    let canonical = canonicalize(&reserialized).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();

    (canonical.into_bytes(), sig, trust, signed_at)
}

const FIXTURE_SIGNED: &str =
    include_str!("../../nixfleet-proto/tests/fixtures/signed-artifact.json");

#[test]
fn verify_ok_returns_fleet() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(&bytes, &sig, std::slice::from_ref(&trust), now, window);

    let fleet = result.expect("verify_ok");
    assert_eq!(fleet.schema_version, 1);
    assert!(fleet.hosts.contains_key("h1"));
}

#[test]
fn verify_bad_signature() {
    let (bytes, mut sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    sig[0] ^= 0xFF;
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(&bytes, &sig, std::slice::from_ref(&trust), now, window).unwrap_err();
    assert!(matches!(err, VerifyError::BadSignature));
}

#[test]
fn verify_stale() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at + ChronoDuration::hours(4);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(&bytes, &sig, std::slice::from_ref(&trust), now, window).unwrap_err();
    assert!(matches!(err, VerifyError::Stale { .. }));
}

#[test]
fn verify_at_exact_window_boundary_is_fresh() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let window_secs: u64 = 3 * 3600;
    let now = signed_at + ChronoDuration::seconds(window_secs as i64);
    let window = Duration::from_secs(window_secs);

    let result = verify_artifact(&bytes, &sig, std::slice::from_ref(&trust), now, window);
    assert!(
        result.is_ok(),
        "age == window must be treated as fresh: {result:?}"
    );
}

#[test]
fn verify_unsigned() {
    let json = include_str!("../../nixfleet-proto/tests/fixtures/every-nullable.json");

    let signing_key = fresh_signing_key();
    let trust = trust_root_for(&signing_key);
    let canonical = canonicalize(json).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();

    let now = Utc::now();
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(
        canonical.as_bytes(),
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::NotSigned));
}

#[test]
fn verify_rejects_malleable_signature() {
    // Canonical ed25519 signatures have s < L where L is the curve order.
    // verify_strict rejects any s >= L. We construct a malleable sig by
    // adding L to the scalar component — ed25519-dalek 2's verify_strict
    // catches this; the weaker verify would accept it.
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);

    // L (little-endian 32 bytes) = 2^252 + 27742317777372353535851937790883648493
    const L_LE: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde,
        0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x10,
    ];

    let mut malleable = sig;
    let mut carry: u16 = 0;
    for i in 0..32 {
        let v = malleable[32 + i] as u16 + L_LE[i] as u16 + carry;
        malleable[32 + i] = v as u8;
        carry = v >> 8;
    }

    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(
        &bytes,
        &malleable,
        std::slice::from_ref(&trust),
        now,
        window,
    );
    assert!(
        matches!(result, Err(VerifyError::BadSignature)),
        "verify_strict must reject malleable s >= L: got {result:?}"
    );
}

#[test]
fn verify_unsupported_schema() {
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE_SIGNED).unwrap();
    value["schemaVersion"] = serde_json::json!(2);
    let json = value.to_string();

    let signing_key = fresh_signing_key();
    let trust = trust_root_for(&signing_key);
    let canonical = canonicalize(&json).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();

    let signed_at: DateTime<Utc> = value["meta"]["signedAt"].as_str().unwrap().parse().unwrap();
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(
        canonical.as_bytes(),
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::SchemaVersionUnsupported(2)));
}

#[test]
fn verify_malformed_json() {
    let signing_key = fresh_signing_key();
    let trust = trust_root_for(&signing_key);
    let bytes = b"{not json";
    let sig = [0u8; 64];

    let err = verify_artifact(
        bytes,
        &sig,
        std::slice::from_ref(&trust),
        Utc::now(),
        Duration::from_secs(60),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::Parse(_)));
}

#[test]
fn verify_tampered_payload() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let mut tampered = bytes.clone();
    if let Some(byte) = tampered.iter_mut().find(|b| **b == b'"') {
        *byte = b'_';
    }
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err =
        verify_artifact(&tampered, &sig, std::slice::from_ref(&trust), now, window).unwrap_err();
    assert!(
        matches!(err, VerifyError::Parse(_) | VerifyError::BadSignature),
        "got {err:?}"
    );
}

// ---- New tests exercising the trust-root architecture -----------------

#[test]
fn verify_with_empty_trust_roots_errors() {
    let (bytes, sig, _trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(&bytes, &sig, &[], now, window).unwrap_err();
    assert!(matches!(err, VerifyError::NoTrustRoots));
}

#[test]
fn verify_rotation_with_two_keys_tries_each_in_order() {
    // Simulate a rotation grace window: old key is declared first, new
    // key is declared second. The signature was produced by the new key.
    // Verifier tries the old key (fails) then the new key (succeeds).
    let old_key = fresh_signing_key();
    let new_key = fresh_signing_key();
    let trust_roots = vec![trust_root_for(&old_key), trust_root_for(&new_key)];

    let value: serde_json::Value = serde_json::from_str(FIXTURE_SIGNED).unwrap();
    let signed_at: DateTime<Utc> = value["meta"]["signedAt"].as_str().unwrap().parse().unwrap();
    let canonical = canonicalize(&value.to_string()).unwrap();
    let sig = new_key.sign(canonical.as_bytes()).to_bytes();

    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(canonical.as_bytes(), &sig, &trust_roots, now, window);
    assert!(
        result.is_ok(),
        "rotation-order list must accept the second key: {result:?}"
    );
}

#[test]
fn verify_rejects_when_only_unknown_algorithm_declared() {
    // Operator declares a trust root with a future algorithm this binary
    // doesn't know about. Verifier rejects with UnsupportedAlgorithm —
    // NOT BadSignature — so ops logs are actionable.
    let (bytes, sig, _trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let future_only = vec![TrustedPubkey {
        algorithm: "p256".to_string(),
        public: "somebase64value==".to_string(),
    }];
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(&bytes, &sig, &future_only, now, window).unwrap_err();
    match err {
        VerifyError::UnsupportedAlgorithm { algorithm } => {
            assert_eq!(algorithm, "p256");
        }
        other => panic!("expected UnsupportedAlgorithm, got {other:?}"),
    }
}

#[test]
fn verify_skips_unknown_algorithm_when_known_also_present() {
    // Mixed declaration: an unknown-to-this-binary algorithm is listed
    // alongside the ed25519 key that actually signed. Verifier skips the
    // unknown entry, matches the known one, returns Ok. This is the
    // forward-compat path for a rolling upgrade where some operators
    // have a newer Nix declaration but an older verifier binary.
    let (bytes, sig, ed_trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let mixed = vec![
        TrustedPubkey {
            algorithm: "p256".to_string(),
            public: "somebase64value==".to_string(),
        },
        ed_trust,
    ];
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(&bytes, &sig, &mixed, now, window);
    assert!(
        result.is_ok(),
        "mixed-algorithm list with one known key must verify: {result:?}"
    );
}

#[test]
fn verify_rejects_malformed_pubkey_encoding() {
    let (bytes, sig, _trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let bad_key = vec![TrustedPubkey {
        algorithm: "ed25519".to_string(),
        public: "!!! not base64 !!!".to_string(),
    }];
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    // Malformed key doesn't verify → fall through to BadSignature. Operators
    // see "no key verified" rather than a per-key decode error. If the
    // opposite behavior is desired (surface decode errors loudly), a future
    // PR can change verify_ed25519 to propagate BadPubkeyEncoding; this test
    // pins the current "skip on decode failure" behavior so the change is
    // deliberate.
    let err = verify_artifact(&bytes, &sig, &bad_key, now, window).unwrap_err();
    assert!(matches!(err, VerifyError::BadSignature));
}
