//! `verify_artifact` - signature, freshness window, schema, algorithm rotation.

mod common;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use common::signing::{fresh_signing_key, sign_artifact, trust_root_for, FIXTURE_SIGNED};
use ed25519_dalek::Signer;
use nixfleet_canonicalize::canonicalize;
use nixfleet_proto::TrustedPubkey;
use nixfleet_reconciler::{verify_artifact, VerifyError};
use rand::rngs::OsRng;
use rand::TryRngCore;
use std::time::Duration;

#[test]
fn verify_ok_returns_fleet() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    );

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

    let err = verify_artifact(
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
fn verify_stale() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at + ChronoDuration::hours(4);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(
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
fn verify_future_dated_beyond_slack_is_rejected() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at - ChronoDuration::days(2);
    let window = Duration::from_secs(86400);

    let err = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    )
    .unwrap_err();
    assert!(
        matches!(err, VerifyError::FutureDated { .. }),
        "future-dated signed_at must be rejected, got {err:?}",
    );
}

#[test]
fn verify_future_dated_within_slack_is_accepted() {
    // Benign clock skew within CLOCK_SKEW_SLACK_SECS verifies cleanly.
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at - ChronoDuration::seconds(30);
    let window = Duration::from_secs(86400);

    let result = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    );
    assert!(
        result.is_ok(),
        "30s-future signed_at within 60s slack must verify, got {:?}",
        result.err(),
    );
}

#[test]
fn verify_at_exact_window_boundary_is_fresh() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let window_secs: u64 = 3 * 3600;
    let now = signed_at + ChronoDuration::seconds(window_secs as i64);
    let window = Duration::from_secs(window_secs);

    let result = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    );
    assert!(
        result.is_ok(),
        "age == window must be treated as fresh: {result:?}"
    );
}

#[test]
fn verify_within_clock_skew_slack_is_fresh() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let window_secs: u64 = 3 * 3600;
    let now = signed_at + ChronoDuration::seconds(window_secs as i64 + 30);
    let window = Duration::from_secs(window_secs);

    let result = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    );
    assert!(
        result.is_ok(),
        "age within slack must be treated as fresh: {result:?}"
    );
}

#[test]
fn verify_just_past_slack_is_stale() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let window_secs: u64 = 3 * 3600;
    let now = signed_at + ChronoDuration::seconds(window_secs as i64 + 61);
    let window = Duration::from_secs(window_secs);

    let err = verify_artifact(
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
        None,
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::NotSigned));
}

#[test]
fn verify_rejects_malleable_signature() {
    // Construct a malleable sig by adding L to the scalar component;
    // verify_strict catches it where weaker verify would not.
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
        None,
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
        None,
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
        None,
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

    let err = verify_artifact(
        &tampered,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        None,
    )
    .unwrap_err();
    assert!(
        matches!(err, VerifyError::Parse(_) | VerifyError::BadSignature),
        "got {err:?}"
    );
}

#[test]
fn verify_with_empty_trust_roots_errors() {
    let (bytes, sig, _trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(&bytes, &sig, &[], now, window, None).unwrap_err();
    assert!(matches!(err, VerifyError::NoTrustRoots));
}

#[test]
fn verify_rotation_with_two_keys_tries_each_in_order() {
    let old_key = fresh_signing_key();
    let new_key = fresh_signing_key();
    let trust_roots = vec![trust_root_for(&old_key), trust_root_for(&new_key)];

    let value: serde_json::Value = serde_json::from_str(FIXTURE_SIGNED).unwrap();
    let signed_at: DateTime<Utc> = value["meta"]["signedAt"].as_str().unwrap().parse().unwrap();
    let canonical = canonicalize(&value.to_string()).unwrap();
    let sig = new_key.sign(canonical.as_bytes()).to_bytes();

    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(canonical.as_bytes(), &sig, &trust_roots, now, window, None);
    assert!(
        result.is_ok(),
        "rotation-order list must accept the second key: {result:?}"
    );
}

#[test]
fn verify_rejects_when_only_unknown_algorithm_declared() {
    // Distinguish UnsupportedAlgorithm from BadSignature for actionable logs.
    let (bytes, sig, _trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let future_only = vec![TrustedPubkey {
        algorithm: "dilithium3".to_string(),
        public: "somebase64value==".to_string(),
    }];
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(&bytes, &sig, &future_only, now, window, None).unwrap_err();
    match err {
        VerifyError::UnsupportedAlgorithm { algorithm } => {
            assert_eq!(algorithm, "dilithium3");
        }
        other => panic!("expected UnsupportedAlgorithm, got {other:?}"),
    }
}

#[test]
fn verify_skips_unknown_algorithm_when_known_also_present() {
    // Forward-compat: unknown algorithms skipped, known one matches.
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

    let result = verify_artifact(&bytes, &sig, &mixed, now, window, None);
    assert!(
        result.is_ok(),
        "mixed-algorithm list with one known key must verify: {result:?}"
    );
}

/// P-256 curve order `n` big-endian - used to build high-s twin sigs.
const P256_N_BE: [u8; 32] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xBC, 0xE6, 0xFA, 0xAD, 0xA7, 0x17, 0x9E, 0x84, 0xF3, 0xB9, 0xCA, 0xC2, 0xFC, 0x63, 0x25, 0x51,
];

/// 32-byte big-endian subtraction; no modular reduction.
fn be_sub_32(minuend: &[u8; 32], subtrahend: &[u8; 32]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut borrow: i32 = 0;
    for i in (0..32).rev() {
        let v = minuend[i] as i32 - subtrahend[i] as i32 - borrow;
        if v < 0 {
            result[i] = (v + 256) as u8;
            borrow = 1;
        } else {
            result[i] = v as u8;
            borrow = 0;
        }
    }
    result
}

/// Returns (sig 64-byte R||S, trust root with 64-byte X||Y pubkey).
fn sign_p256(canonical_bytes: &[u8]) -> ([u8; 64], TrustedPubkey) {
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::{Signature, SigningKey};

    let mut seed = [0u8; 32];
    OsRng.try_fill_bytes(&mut seed).expect("OS CSPRNG");
    let signing_key = SigningKey::from_slice(&seed).expect("derive p256 key from 32 bytes");
    let verifying_key = signing_key.verifying_key();

    let sig: Signature = signing_key.sign(canonical_bytes);
    // Normalize to low-s: production signers should emit canonical form.
    let sig = sig.normalize_s().unwrap_or(sig);
    let sig_bytes: [u8; 64] = sig.to_bytes().into();

    // 64-byte X||Y, no 0x04 tag.
    let tagged = verifying_key.to_encoded_point(false);
    let tagged_bytes = tagged.as_bytes();
    assert_eq!(
        tagged_bytes.len(),
        65,
        "uncompressed SEC1 point is 65 bytes"
    );
    assert_eq!(tagged_bytes[0], 0x04, "SEC1 uncompressed tag");
    let public_bytes: &[u8] = &tagged_bytes[1..];
    let public_b64 = BASE64_STANDARD.encode(public_bytes);

    let trust = TrustedPubkey {
        algorithm: "ecdsa-p256".to_string(),
        public: public_b64,
    };
    (sig_bytes, trust)
}

#[test]
fn verify_p256_ok() {
    let value: serde_json::Value = serde_json::from_str(FIXTURE_SIGNED).unwrap();
    let signed_at: DateTime<Utc> = value["meta"]["signedAt"].as_str().unwrap().parse().unwrap();
    let canonical = canonicalize(&value.to_string()).unwrap();

    let (sig, trust) = sign_p256(canonical.as_bytes());
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(canonical.as_bytes(), &sig, &[trust], now, window, None);
    assert!(result.is_ok(), "verify_p256_ok: {result:?}");
}

#[test]
fn verify_p256_accepts_high_s() {
    // TPM2_Sign emits ~50% high-s; verifier normalises before checking.
    let value: serde_json::Value = serde_json::from_str(FIXTURE_SIGNED).unwrap();
    let signed_at: DateTime<Utc> = value["meta"]["signedAt"].as_str().unwrap().parse().unwrap();
    let canonical = canonicalize(&value.to_string()).unwrap();

    let (sig, trust) = sign_p256(canonical.as_bytes());

    let mut malleable = sig;
    let s_be: [u8; 32] = sig[32..64].try_into().unwrap();
    let s_high = be_sub_32(&P256_N_BE, &s_be);
    malleable[32..64].copy_from_slice(&s_high);

    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(
        canonical.as_bytes(),
        &malleable,
        &[trust],
        now,
        window,
        None,
    );
    assert!(
        result.is_ok(),
        "high-s must verify (normalised internally): got {result:?}"
    );
}

#[test]
fn verify_rotation_cross_algorithm() {
    let value: serde_json::Value = serde_json::from_str(FIXTURE_SIGNED).unwrap();
    let signed_at: DateTime<Utc> = value["meta"]["signedAt"].as_str().unwrap().parse().unwrap();
    let canonical = canonicalize(&value.to_string()).unwrap();

    let (p256_sig, p256_trust) = sign_p256(canonical.as_bytes());

    let previous_ed25519_key = fresh_signing_key();
    let ed_trust = trust_root_for(&previous_ed25519_key);

    let trusted = vec![p256_trust, ed_trust];
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(canonical.as_bytes(), &p256_sig, &trusted, now, window, None);
    assert!(
        result.is_ok(),
        "p256 current + ed25519 previous - p256 sig must verify via first entry: {result:?}"
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

    // Malformed key falls through to BadSignature ("skip on decode failure").
    let err = verify_artifact(&bytes, &sig, &bad_key, now, window, None).unwrap_err();
    assert!(matches!(err, VerifyError::BadSignature));
}

#[test]
fn rejects_artifact_older_than_reject_before() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let freshness = Duration::from_secs(86_400);
    let reject_before = signed_at + ChronoDuration::seconds(60);
    let now = signed_at + ChronoDuration::seconds(10);

    let err = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        freshness,
        Some(reject_before),
    )
    .unwrap_err();

    match err {
        VerifyError::RejectedBeforeTimestamp {
            signed_at: got_signed_at,
            reject_before: got_rb,
        } => {
            assert_eq!(got_signed_at, signed_at);
            assert_eq!(got_rb, reject_before);
        }
        other => panic!("expected RejectedBeforeTimestamp, got: {other:?}"),
    }
}

#[test]
fn accepts_artifact_signed_at_after_reject_before() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let freshness = Duration::from_secs(86_400);
    let reject_before = signed_at - ChronoDuration::seconds(60);
    let now = signed_at + ChronoDuration::seconds(10);

    let fleet = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        freshness,
        Some(reject_before),
    )
    .expect("accepts artifact signed after rejectBefore");
    assert_eq!(fleet.schema_version, 1);
}

#[test]
fn reject_before_none_disables_the_gate() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let freshness = Duration::from_secs(86_400);
    let now = signed_at + ChronoDuration::minutes(30);

    let _fleet = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        freshness,
        None,
    )
    .expect("None means gate disabled");
}

/// Strict `<`: signed_at == reject_before is accepted.
#[test]
fn reject_before_exact_equal_is_accepted() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let freshness = Duration::from_secs(86_400);
    let reject_before = signed_at;
    let now = signed_at + ChronoDuration::seconds(10);

    let _fleet = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        freshness,
        Some(reject_before),
    )
    .expect("signed_at == reject_before must be accepted under strict < semantic");
}

/// `RejectedBeforeTimestamp` wins over `Stale` when both hold - alert
/// class must distinguish compromise from staleness.
#[test]
fn reject_before_takes_precedence_over_stale() {
    let (bytes, sig, trust, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let window = Duration::from_secs(60);
    let reject_before = signed_at + ChronoDuration::seconds(300);
    let now = signed_at + ChronoDuration::seconds(600);

    let err = verify_artifact(
        &bytes,
        &sig,
        std::slice::from_ref(&trust),
        now,
        window,
        Some(reject_before),
    )
    .unwrap_err();

    assert!(
        matches!(err, VerifyError::RejectedBeforeTimestamp { .. }),
        "compromise switch must win over routine staleness, got {err:?}"
    );
}
