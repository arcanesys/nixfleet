//! RFC-0002 §4 step 0 — fetch + verify + freshness-gate.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use nixfleet_proto::{FleetResolved, TrustedPubkey};
use std::time::Duration;
use thiserror::Error;

/// Accepted `schemaVersion` for this consumer.
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

/// Verify a signed `fleet.resolved` artifact per RFC-0002 §4 step 0.
///
/// # Trust root list
///
/// `trusted_keys` is a list to support [`CONTRACTS.md §II`]'s rotation
/// grace window — during a key rotation, the previous and current keys
/// are BOTH valid trust roots for up to 30 days. The verifier tries
/// each in declaration order; the first key whose algorithm is
/// supported AND whose `verify_strict` accepts the signature wins.
///
/// Entries with unsupported algorithms are skipped (with no error),
/// enabling forward compatibility: when `#18` amends §II to add e.g.
/// `p256`, an older verifier binary can still operate against a
/// mixed-algorithm `trust.ciReleaseKeys` list; it just only matches
/// the subset of keys whose algorithms it knows.
///
/// # Signature width
///
/// `signature` is a byte slice, not a fixed-size array. Per-algorithm
/// length validation happens inside the dispatcher. ed25519 expects
/// exactly 64 bytes (32-byte R || 32-byte s). A future `p256` branch
/// will decide whether to accept raw r||s (64 bytes) or DER-encoded
/// (variable) — for now, non-ed25519 algorithms bail with
/// `UnsupportedAlgorithm`.
pub fn verify_artifact(
    signed_bytes: &[u8],
    signature: &[u8],
    trusted_keys: &[TrustedPubkey],
    now: DateTime<Utc>,
    freshness_window: Duration,
) -> Result<FleetResolved, VerifyError> {
    if trusted_keys.is_empty() {
        return Err(VerifyError::NoTrustRoots);
    }

    // Step 1: parse as generic JSON so we can re-canonicalize it.
    let raw_str = std::str::from_utf8(signed_bytes).map_err(|e| {
        VerifyError::Parse(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e,
        )))
    })?;
    let _value: serde_json::Value = serde_json::from_str(raw_str)?;

    // Step 2: re-canonicalize via the pinned JCS library.
    let canonical =
        nixfleet_canonicalize::canonicalize(raw_str).map_err(VerifyError::Canonicalize)?;

    // Step 3: try each trust root. First matching signature wins.
    let mut attempted_any_supported = false;
    for key in trusted_keys {
        match key.algorithm.as_str() {
            "ed25519" => {
                attempted_any_supported = true;
                if verify_ed25519(canonical.as_bytes(), signature, &key.public).is_ok() {
                    return finish_verification(&canonical, now, freshness_window);
                }
            }
            "ecdsa-p256" => {
                attempted_any_supported = true;
                if verify_ecdsa_p256(canonical.as_bytes(), signature, &key.public).is_ok() {
                    return finish_verification(&canonical, now, freshness_window);
                }
            }
            _other => {
                // Unknown algorithm — skip this trust root (forward compat).
                // Only report UnsupportedAlgorithm if NO supported algorithm
                // appears in the list (below).
                continue;
            }
        }
    }

    if !attempted_any_supported {
        // The operator declared only unknown algorithms. Surface the first
        // unknown one so logs are actionable.
        return Err(VerifyError::UnsupportedAlgorithm {
            algorithm: trusted_keys[0].algorithm.clone(),
        });
    }
    Err(VerifyError::BadSignature)
}

/// Dispatched verification for ed25519. `verify_strict` rejects malleable
/// signatures (non-canonical R or `s >= L`) — required for root-of-trust
/// keys per CONTRACTS.md §II #1.
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

/// Dispatched verification for ECDSA P-256 (NIST curve) per #18 §II.
///
/// Public key encoding per CONTRACTS.md §II #1: 64 bytes `X ‖ Y`
/// (uncompressed SEC1 point with the `0x04` tag stripped), base64-encoded.
/// Signature encoding: 64 bytes `R ‖ S`, raw (no DER wrapping).
///
/// Low-s malleability rejection: ECDSA signatures on Weierstrass curves
/// are malleable — if `(r, s)` is valid, so is `(r, n − s)`. Canonical
/// p256 signatures have `s <= n / 2`. The `p256` crate's
/// `Signature::normalize_s()` returns `Some(normalized)` iff the input
/// was high-s; we reject any such signature outright. Required for
/// root-of-trust keys per the same hardening pattern as `verify_strict`
/// on ed25519.
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

    // Re-tag as a 65-byte SEC1 uncompressed point (0x04 || X || Y) for
    // the p256 decoder.
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

    // Reject malleable (high-s) signatures.
    if sig.normalize_s().is_some() {
        return Err(VerifyError::BadSignature);
    }

    verifying_key
        .verify(canonical_bytes, &sig)
        .map_err(|_| VerifyError::BadSignature)
}

/// Steps 4-6 after signature verification: type-parse, schema-gate, freshness.
fn finish_verification(
    canonical: &str,
    now: DateTime<Utc>,
    freshness_window: Duration,
) -> Result<FleetResolved, VerifyError> {
    // Step 4: type-parse.
    let fleet: FleetResolved = serde_json::from_str(canonical)?;

    // Step 5: schemaVersion gate.
    if fleet.schema_version != ACCEPTED_SCHEMA_VERSION {
        return Err(VerifyError::SchemaVersionUnsupported(fleet.schema_version));
    }

    // Step 6: freshness.
    let signed_at = fleet.meta.signed_at.ok_or(VerifyError::NotSigned)?;
    let window = ChronoDuration::from_std(freshness_window)
        .expect("freshness_window fits in i64 nanoseconds — multi-century windows are a bug");
    if now - signed_at > window {
        return Err(VerifyError::Stale {
            signed_at,
            now,
            window: freshness_window,
        });
    }

    Ok(fleet)
}
