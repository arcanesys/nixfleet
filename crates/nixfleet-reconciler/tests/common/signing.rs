//! Shared signing helpers for verify_* integration tests.

#![allow(dead_code)]

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::{DateTime, Utc};
use ed25519_dalek::ed25519::signature::rand_core::{OsRng, RngCore};
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_canonicalize::canonicalize;
use nixfleet_proto::TrustedPubkey;

pub const FIXTURE_SIGNED: &str =
    include_str!("../../../nixfleet-proto/tests/fixtures/signed-artifact.json");

/// Uses ed25519-dalek's pinned rand_core 0.6 OsRng so the same type also
/// satisfies `SigningKey::generate`'s `CryptoRngCore` bound. Workspace
/// `rand` (0.9) is intentionally not used here - those traits don't
/// interop with ed25519-dalek 2.
pub fn fresh_signing_key() -> SigningKey {
    let mut seed = [0u8; 32];
    OsRng.try_fill_bytes(&mut seed).expect("OS CSPRNG");
    SigningKey::from_bytes(&seed)
}

pub fn trust_root_for(signing_key: &SigningKey) -> TrustedPubkey {
    TrustedPubkey {
        algorithm: "ed25519".to_string(),
        public: BASE64_STANDARD.encode(signing_key.verifying_key().as_bytes()),
    }
}

/// Returns (signed_bytes, signature, trust_root, signed_at).
pub fn sign_artifact(json: &str) -> (Vec<u8>, [u8; 64], TrustedPubkey, DateTime<Utc>) {
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
