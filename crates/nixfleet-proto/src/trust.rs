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

use chrono::{DateTime, Utc};
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

/// Trust configuration loaded from `/etc/nixfleet/{cp,agent}/trust.json`.
///
/// Shape authoritative per [`docs/trust-root-flow.md §3.4`][flow]. Materialised
/// by the NixOS scope modules from `config.nixfleet.trust`, consumed by CP
/// and agent binaries at startup.
///
/// Reload model: restart-only (§7.1). No SIGHUP, no inotify.
///
/// Field order here is human-readable (schema first, then the three trust
/// categories), not JCS-sorted. `trust.json` is a consumer-local file,
/// never a signed artifact, so serde's declaration-order serialisation
/// does not need to match `nixfleet_canonicalize` output.
///
/// [flow]: ../../../docs/trust-root-flow.md
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustConfig {
    /// Required. Bumped only on breaking schema changes; binaries refuse
    /// to start on unknown versions (§7.4). The wire-protocol schema for
    /// `fleet.resolved` is separate (see `fleet_resolved::Meta`).
    pub schema_version: u32,

    pub ci_release_key: KeySlot,

    #[serde(default)]
    pub attic_cache_key: Option<AtticKeySlot>,

    #[serde(default)]
    pub org_root_key: Option<KeySlot>,
}

impl TrustConfig {
    /// The only `schemaVersion` value this crate parses. Binaries match on
    /// this after deserialisation and refuse unknown versions.
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;
}

/// A single trust-root slot with current/previous rotation grace.
///
/// `reject_before` is the compromise switch — artifacts whose `signedAt`
/// is older than this timestamp are refused regardless of which key
/// signed them (§7.2). Enforcement lives in `verify_artifact`, not here.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySlot {
    #[serde(default)]
    pub current: Option<TrustedPubkey>,

    #[serde(default)]
    pub previous: Option<TrustedPubkey>,

    #[serde(default)]
    pub reject_before: Option<DateTime<Utc>>,
}

impl KeySlot {
    /// Returns the active key list for this slot. Both `current` and
    /// `previous` are returned unconditionally when present.
    ///
    /// Order is `[current, previous]`. First-match callers (including
    /// `verify_artifact`'s iteration) see the newer key first, which is
    /// load-bearing for the rotation semantics in
    /// [`docs/trust-root-flow.md §6`][flow].
    ///
    /// Signature per coordinator's context update: no `now` parameter;
    /// `reject_before` filtering happens inside `verify_artifact`.
    ///
    /// [flow]: ../../../docs/trust-root-flow.md
    pub fn active_keys(&self) -> Vec<TrustedPubkey> {
        let mut keys = Vec::with_capacity(2);
        if let Some(k) = &self.current {
            keys.push(k.clone());
        }
        if let Some(k) = &self.previous {
            keys.push(k.clone());
        }
        keys
    }
}

/// Attic cache key in the attic-native string format `"attic:<host>:<base64>"`.
///
/// Typed as an opaque newtype because Stream B's `modules/_trust.nix`
/// currently keeps the attic key flat (CONTRACTS.md §II #2 has not yet
/// been migrated to the `{algorithm, public}` shape that §II #1 uses).
/// Migrates to `KeySlot<AtticPubkey>` when §II #2 gains that treatment.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub struct AtticKeySlot(pub String);
