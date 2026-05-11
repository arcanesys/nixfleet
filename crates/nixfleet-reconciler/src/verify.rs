//! Sidecar fetch + verify + freshness-gate.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use nixfleet_proto::{FleetResolved, Revocations, RolloutManifest, TrustedPubkey};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::time::Duration;
use thiserror::Error;

/// Signed sidecar under `ciReleaseKey`. Drives the shared
/// canonicalize → verify → freshness-gate pipeline.
pub trait SignedSidecar {
    fn schema_version(&self) -> u32;
    fn signed_at(&self) -> Option<DateTime<Utc>>;
}

impl SignedSidecar for FleetResolved {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
    fn signed_at(&self) -> Option<DateTime<Utc>> {
        self.meta.signed_at
    }
}

impl SignedSidecar for Revocations {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
    fn signed_at(&self) -> Option<DateTime<Utc>> {
        self.meta.signed_at
    }
}

impl SignedSidecar for RolloutManifest {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
    fn signed_at(&self) -> Option<DateTime<Utc>> {
        self.meta.signed_at
    }
}

const ACCEPTED_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("fleet.resolved parse failed: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("signature does not verify against any declared trust root")]
    BadSignature,

    #[error("artifact is unsigned (meta.signedAt is null)")]
    NotSigned,

    #[error("stale artifact: signedAt={signed_at}, now={now}, window={window:?}")]
    Stale {
        signed_at: DateTime<Utc>,
        now: DateTime<Utc>,
        window: Duration,
    },

    #[error(
        "future-dated artifact: signedAt={signed_at} is more than {slack_secs}s ahead of now={now} \
         (clock skew tolerance is {slack_secs}s; pre-signing suggests CI key compromise - \
         rotate via reject_before)"
    )]
    FutureDated {
        signed_at: DateTime<Utc>,
        now: DateTime<Utc>,
        slack_secs: i64,
    },

    #[error(
        "artifact signed at {signed_at} is older than reject_before {reject_before} (compromise switch)"
    )]
    RejectedBeforeTimestamp {
        signed_at: DateTime<Utc>,
        reject_before: DateTime<Utc>,
    },

    #[error("unsupported schemaVersion: {0} (accepted: 1)")]
    SchemaVersionUnsupported(u32),

    #[error("JCS re-canonicalization failed: {0}")]
    Canonicalize(#[source] anyhow::Error),

    #[error("unsupported signature algorithm: {algorithm} (supported: ed25519, ecdsa-p256)")]
    UnsupportedAlgorithm { algorithm: String },

    #[error("trusted pubkey material is malformed ({algorithm}): {reason}")]
    BadPubkeyEncoding { algorithm: String, reason: String },

    #[error("no trust roots configured for artifact verification")]
    NoTrustRoots,
}

/// Verify any signed sidecar (fleet.resolved / revocations / rollout
/// manifest). `trusted_keys` tried in declaration order, first match
/// wins; unsupported algorithms skipped silently for forward-compat.
/// `reject_before` is strict `<` - equality accepted.
pub fn verify_signed_sidecar<T: SignedSidecar + DeserializeOwned>(
    signed_bytes: &[u8],
    signature: &[u8],
    trusted_keys: &[TrustedPubkey],
    now: DateTime<Utc>,
    freshness_window: Duration,
    reject_before: Option<DateTime<Utc>>,
) -> Result<T, VerifyError> {
    let canonical = verify_signature_against_trust_roots(signed_bytes, signature, trusted_keys)?;
    finish_sidecar_verification(&canonical, now, freshness_window, reject_before)
}

/// Thin `FleetResolved` wrapper around `verify_signed_sidecar`.
pub fn verify_artifact(
    signed_bytes: &[u8],
    signature: &[u8],
    trusted_keys: &[TrustedPubkey],
    now: DateTime<Utc>,
    freshness_window: Duration,
    reject_before: Option<DateTime<Utc>>,
) -> Result<FleetResolved, VerifyError> {
    verify_signed_sidecar(
        signed_bytes,
        signature,
        trusted_keys,
        now,
        freshness_window,
        reject_before,
    )
}

/// LOADBEARING: `verify_strict` (not `verify`) - rejects malleable signatures
/// for root-of-trust keys.
fn verify_ed25519(
    canonical_bytes: &[u8],
    signature: &[u8],
    public_b64: &str,
) -> Result<(), VerifyError> {
    let public_bytes =
        BASE64_STANDARD
            .decode(public_b64)
            .map_err(|e| VerifyError::BadPubkeyEncoding {
                algorithm: "ed25519".into(),
                reason: format!("base64 decode failed: {e}"),
            })?;
    let public_array: [u8; 32] =
        public_bytes
            .try_into()
            .map_err(|v: Vec<u8>| VerifyError::BadPubkeyEncoding {
                algorithm: "ed25519".into(),
                reason: format!("expected 32 bytes, got {}", v.len()),
            })?;
    let verifying_key =
        VerifyingKey::from_bytes(&public_array).map_err(|e| VerifyError::BadPubkeyEncoding {
            algorithm: "ed25519".into(),
            reason: e.to_string(),
        })?;

    let sig_array: [u8; 64] = signature
        .try_into()
        .map_err(|_| VerifyError::BadSignature)?;
    let sig = Signature::from_bytes(&sig_array);

    verifying_key
        .verify_strict(canonical_bytes, &sig)
        .map_err(|_| VerifyError::BadSignature)
}

/// FOOTGUN: TPM2_Sign emits ~50% high-s ECDSA signatures; we MUST normalise
/// to low-s before verifying or every other lab signature fails as BadSignature.
/// Pubkey: 64-byte X||Y base64. Sig: 64-byte R||S.
fn verify_ecdsa_p256(
    canonical_bytes: &[u8],
    signature: &[u8],
    public_b64: &str,
) -> Result<(), VerifyError> {
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{Signature as P256Signature, VerifyingKey as P256VerifyingKey};
    use p256::EncodedPoint;

    let public_bytes =
        BASE64_STANDARD
            .decode(public_b64)
            .map_err(|e| VerifyError::BadPubkeyEncoding {
                algorithm: "ecdsa-p256".into(),
                reason: format!("base64 decode failed: {e}"),
            })?;
    if public_bytes.len() != 64 {
        return Err(VerifyError::BadPubkeyEncoding {
            algorithm: "ecdsa-p256".into(),
            reason: format!(
                "expected 64 bytes (X ‖ Y uncompressed), got {}",
                public_bytes.len()
            ),
        });
    }

    // 0x04 || X || Y SEC1 uncompressed.
    let mut tagged = [0u8; 65];
    tagged[0] = 0x04;
    tagged[1..].copy_from_slice(&public_bytes);
    let point = EncodedPoint::from_bytes(tagged).map_err(|e| VerifyError::BadPubkeyEncoding {
        algorithm: "ecdsa-p256".into(),
        reason: format!("SEC1 decode failed: {e}"),
    })?;
    let verifying_key = P256VerifyingKey::from_encoded_point(&point).map_err(|e| {
        VerifyError::BadPubkeyEncoding {
            algorithm: "ecdsa-p256".into(),
            reason: format!("not on curve / invalid point: {e}"),
        }
    })?;

    let sig = P256Signature::from_slice(signature).map_err(|_| VerifyError::BadSignature)?;
    let sig = sig.normalize_s().unwrap_or(sig);

    verifying_key
        .verify(canonical_bytes, &sig)
        .map_err(|_| VerifyError::BadSignature)
}

pub fn verify_revocations(
    signed_bytes: &[u8],
    signature: &[u8],
    trusted_keys: &[TrustedPubkey],
    now: DateTime<Utc>,
    freshness_window: Duration,
    reject_before: Option<DateTime<Utc>>,
) -> Result<Revocations, VerifyError> {
    verify_signed_sidecar(
        signed_bytes,
        signature,
        trusted_keys,
        now,
        freshness_window,
        reject_before,
    )
}

/// Verify a signed rollout manifest. Callers MUST also call
/// [`compute_rollout_id`] and assert it equals the advertised id.
pub fn verify_rollout_manifest(
    signed_bytes: &[u8],
    signature: &[u8],
    trusted_keys: &[TrustedPubkey],
    now: DateTime<Utc>,
    freshness_window: Duration,
    reject_before: Option<DateTime<Utc>>,
) -> Result<RolloutManifest, VerifyError> {
    verify_signed_sidecar(
        signed_bytes,
        signature,
        trusted_keys,
        now,
        freshness_window,
        reject_before,
    )
}

/// SHA-256 hex of JCS-canonical bytes of any serialisable value.
///
/// FOOTGUN: this is the **producer** path - caller has the parsed struct
/// and wants the canonical-hash of what they would emit. **Verifiers must
/// not use this**; re-serializing a parsed struct silently drops fields
/// the consumer's proto doesn't know about, breaking content-addressing
/// across schema versions even when the change is "additive" per
/// CONTRACTS §V Pattern A. Use [`canonical_hash_from_bytes`] for verify
/// paths that have the original received bytes.
pub fn compute_canonical_hash<T: serde::Serialize>(value: &T) -> Result<String, VerifyError> {
    let raw = serde_json::to_string(value)?;
    let canonical = nixfleet_canonicalize::canonicalize(&raw).map_err(VerifyError::Canonicalize)?;
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(hex_lowercase(&digest))
}

/// SHA-256 hex of JCS-canonical bytes, computed from raw input bytes.
/// `canonicalize` is idempotent on canonical input; running it here is
/// a defensive normaliser against transport-layer alterations
/// (whitespace, BOM, etc.). Critically, no parse step - fields the
/// caller's proto doesn't know about are preserved in the canonical
/// bytes. This is the verify-side function: same hash as the producer
/// computed regardless of any additive proto drift between them.
pub fn canonical_hash_from_bytes(bytes: &[u8]) -> Result<String, VerifyError> {
    let s = std::str::from_utf8(bytes).map_err(|err| {
        VerifyError::Canonicalize(anyhow::anyhow!("input not valid UTF-8: {err}"))
    })?;
    let canonical = nixfleet_canonicalize::canonicalize(s).map_err(VerifyError::Canonicalize)?;
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(hex_lowercase(&digest))
}

/// Producer-side rolloutId. See [`compute_canonical_hash`] caveat.
pub fn compute_rollout_id(manifest: &RolloutManifest) -> Result<String, VerifyError> {
    compute_canonical_hash(manifest)
}

/// Verify-side rolloutId - hashes the received manifest bytes.
/// Cross-version safe: an older verifier's proto missing fields the
/// producer added still computes the same hash because parsing is not
/// in the path.
pub fn rollout_id_from_bytes(bytes: &[u8]) -> Result<String, VerifyError> {
    canonical_hash_from_bytes(bytes)
}

fn hex_lowercase(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Parse → canonicalize → sig-verify. Returns canonical bytes.
fn verify_signature_against_trust_roots(
    signed_bytes: &[u8],
    signature: &[u8],
    trusted_keys: &[TrustedPubkey],
) -> Result<String, VerifyError> {
    if trusted_keys.is_empty() {
        return Err(VerifyError::NoTrustRoots);
    }

    let raw_str = std::str::from_utf8(signed_bytes).map_err(|e| {
        VerifyError::Parse(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e,
        )))
    })?;
    let _value: serde_json::Value = serde_json::from_str(raw_str)?;
    let canonical =
        nixfleet_canonicalize::canonicalize(raw_str).map_err(VerifyError::Canonicalize)?;

    let mut attempted_any_supported = false;
    for key in trusted_keys {
        match key.algorithm.as_str() {
            "ed25519" => {
                attempted_any_supported = true;
                if verify_ed25519(canonical.as_bytes(), signature, &key.public).is_ok() {
                    return Ok(canonical);
                }
            }
            "ecdsa-p256" => {
                attempted_any_supported = true;
                if verify_ecdsa_p256(canonical.as_bytes(), signature, &key.public).is_ok() {
                    return Ok(canonical);
                }
            }
            _other => continue,
        }
    }

    if !attempted_any_supported {
        return Err(VerifyError::UnsupportedAlgorithm {
            algorithm: trusted_keys[0].algorithm.clone(),
        });
    }
    Err(VerifyError::BadSignature)
}

/// Schema gate + `reject_before` + bidirectional freshness check
/// (past + future bound, both with `CLOCK_SKEW_SLACK_SECS` slack).
/// `reject_before` runs first so alerts can distinguish compromise
/// from staleness.
fn finish_sidecar_verification<T: SignedSidecar + DeserializeOwned>(
    canonical: &str,
    now: DateTime<Utc>,
    freshness_window: Duration,
    reject_before: Option<DateTime<Utc>>,
) -> Result<T, VerifyError> {
    let payload: T = serde_json::from_str(canonical)?;
    if payload.schema_version() != ACCEPTED_SCHEMA_VERSION {
        return Err(VerifyError::SchemaVersionUnsupported(
            payload.schema_version(),
        ));
    }

    let signed_at = payload.signed_at().ok_or(VerifyError::NotSigned)?;

    if let Some(rb) = reject_before {
        if signed_at < rb {
            return Err(VerifyError::RejectedBeforeTimestamp {
                signed_at,
                reject_before: rb,
            });
        }
    }

    let window = ChronoDuration::from_std(freshness_window)
        .expect("freshness_window fits in i64 nanoseconds - multi-century windows are a bug");
    let effective_window = window + ChronoDuration::seconds(CLOCK_SKEW_SLACK_SECS);
    let elapsed = now - signed_at;
    if elapsed > effective_window {
        return Err(VerifyError::Stale {
            signed_at,
            now,
            window: freshness_window,
        });
    }
    if -elapsed > ChronoDuration::seconds(CLOCK_SKEW_SLACK_SECS) {
        return Err(VerifyError::FutureDated {
            signed_at,
            now,
            slack_secs: CLOCK_SKEW_SLACK_SECS,
        });
    }

    Ok(payload)
}

pub const CLOCK_SKEW_SLACK_SECS: i64 = 60;
