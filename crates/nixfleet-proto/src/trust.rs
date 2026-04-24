//! Trust root declarations (CONTRACTS.md §II).
//!
//! A [`TrustedPubkey`] pairs a signature algorithm with its public key
//! material. Per CONTRACTS.md §II, the algorithm is a property of the
//! key (not of the signed artifact) — a given private key produces
//! signatures in exactly one algorithm. The verifier matches
//! `(artifact, signature) → declared trust root → algorithm → verify
//! routine`; the artifact MUST NOT carry its own algorithm claim.
//!
//! # Rotation
//!
//! Callers pass a list of trust roots (`&[TrustedPubkey]`). The
//! verifier tries each in declaration order; first match wins. This
//! supports the `ciReleaseKey.previous` rotation grace window
//! described in CONTRACTS.md §II #1 — and, when a future PR amends
//! §II to allow non-ed25519 algorithms, it supports cross-algorithm
//! rotation (old `ed25519` plus new `p256` both valid for N days).
//!
//! # Algorithm extensibility
//!
//! The type is a flat struct with a `String` algorithm tag rather than
//! a Rust `enum`. Rationale: this crate mirrors the wire contract;
//! forward compatibility means the proto parses an unknown algorithm
//! without error (it's the verifier's job to reject), and an old
//! proto parsing a newer Nix-declared `{ "algorithm": "p256", ... }`
//! must not crash. Unknown algorithms surface as
//! `VerifyError::UnsupportedAlgorithm` at verify time, where they can
//! be logged with the algorithm name for operator visibility.

use serde::{Deserialize, Serialize};

/// A trust root: algorithm + public key material.
///
/// Currently supported algorithms:
/// - `ed25519`: `public` is the 32-byte Edwards-curve public key, base64
///   (standard alphabet, with padding) encoded.
///
/// Future algorithms (e.g. `p256`) extend this schema without a major
/// version bump; consumers ignore trust roots whose `algorithm` they
/// do not recognize (emitting an event) rather than refusing to start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedPubkey {
    /// Short algorithm name as declared in `fleet.nix`'s trust tree.
    pub algorithm: String,
    /// Base64-encoded public key bytes. Decoding is algorithm-specific
    /// and happens inside the verifier.
    pub public: String,
}
