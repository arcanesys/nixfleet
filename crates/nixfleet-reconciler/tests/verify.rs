//! Step 0 — signature verification + freshness window.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_canonicalize::canonicalize;
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

/// Build a signed fleet.resolved artifact from JSON source.
///
/// Returns (signed_bytes, signature, pubkey, signed_at).
fn sign_artifact(json: &str) -> (Vec<u8>, [u8; 64], ed25519_dalek::VerifyingKey, DateTime<Utc>) {
    let signing_key = fresh_signing_key();
    let pubkey = signing_key.verifying_key();

    let value: serde_json::Value = serde_json::from_str(json).expect("parse");
    let signed_at: DateTime<Utc> = value["meta"]["signedAt"]
        .as_str()
        .expect("fixture must have meta.signedAt set")
        .parse()
        .expect("parse RFC 3339");

    let reserialized = serde_json::to_string(&value).unwrap();
    let canonical = canonicalize(&reserialized).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();

    (canonical.into_bytes(), sig, pubkey, signed_at)
}

const FIXTURE_SIGNED: &str = include_str!("../../nixfleet-proto/tests/fixtures/signed-artifact.json");

#[test]
fn verify_ok_returns_fleet() {
    let (bytes, sig, pubkey, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let result = verify_artifact(&bytes, &sig, &pubkey, now, window);

    let fleet = result.expect("verify_ok");
    assert_eq!(fleet.schema_version, 1);
    assert!(fleet.hosts.contains_key("h1"));
}

#[test]
fn verify_bad_signature() {
    let (bytes, mut sig, pubkey, signed_at) = sign_artifact(FIXTURE_SIGNED);
    sig[0] ^= 0xFF;
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(&bytes, &sig, &pubkey, now, window).unwrap_err();
    assert!(matches!(err, VerifyError::BadSignature));
}

#[test]
fn verify_stale() {
    let (bytes, sig, pubkey, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let now = signed_at + ChronoDuration::hours(4);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(&bytes, &sig, &pubkey, now, window).unwrap_err();
    assert!(matches!(err, VerifyError::Stale { signed_at: Some(_), .. }));
}

#[test]
fn verify_unsigned() {
    let json = include_str!("../../nixfleet-proto/tests/fixtures/every-nullable.json");

    let signing_key = fresh_signing_key();
    let pubkey = signing_key.verifying_key();
    let canonical = canonicalize(json).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();

    let now = Utc::now();
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(canonical.as_bytes(), &sig, &pubkey, now, window).unwrap_err();
    assert!(matches!(err, VerifyError::Stale { signed_at: None, .. }));
}

#[test]
fn verify_unsupported_schema() {
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE_SIGNED).unwrap();
    value["schemaVersion"] = serde_json::json!(2);
    let json = value.to_string();

    let signing_key = fresh_signing_key();
    let pubkey = signing_key.verifying_key();
    let canonical = canonicalize(&json).expect("canonicalize");
    let sig = signing_key.sign(canonical.as_bytes()).to_bytes();

    let signed_at: DateTime<Utc> = value["meta"]["signedAt"].as_str().unwrap().parse().unwrap();
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(canonical.as_bytes(), &sig, &pubkey, now, window).unwrap_err();
    assert!(matches!(err, VerifyError::SchemaVersionUnsupported(2)));
}

#[test]
fn verify_malformed_json() {
    let signing_key = fresh_signing_key();
    let pubkey = signing_key.verifying_key();
    let bytes = b"{not json";
    let sig = [0u8; 64];

    let err = verify_artifact(bytes, &sig, &pubkey, Utc::now(), Duration::from_secs(60))
        .unwrap_err();
    assert!(matches!(err, VerifyError::Parse(_)));
}

#[test]
fn verify_tampered_payload() {
    let (bytes, sig, pubkey, signed_at) = sign_artifact(FIXTURE_SIGNED);
    let mut tampered = bytes.clone();
    if let Some(byte) = tampered.iter_mut().find(|b| **b == b'"') {
        *byte = b'_';
    }
    let now = signed_at + ChronoDuration::minutes(30);
    let window = Duration::from_secs(3 * 3600);

    let err = verify_artifact(&tampered, &sig, &pubkey, now, window).unwrap_err();
    assert!(
        matches!(err, VerifyError::Parse(_) | VerifyError::BadSignature),
        "got {err:?}"
    );
}
