//! Trust root declarations.
//!
//! LOADBEARING: algorithm is a property of the key, not the artifact.
//! Verifier matches `(artifact, sig) → trust root → algorithm` — artifacts
//! MUST NOT carry their own algorithm claim (an attacker could otherwise
//! downgrade by lying about which algo signed the bytes).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `algorithm` is `String` (not enum) for forward-compat with future
/// algorithms. Unknown values surface as `UnsupportedAlgorithm` at verify
/// time. Today: ed25519 — `public` is 32-byte base64 (padded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedPubkey {
    pub algorithm: String,
    pub public: String,
}

/// Loaded from `/etc/nixfleet/{cp,agent}/trust.json`. Restart-only reload.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustConfig {
    pub schema_version: u32,

    pub ci_release_key: KeySlot,

    /// Forwarded opaquely to `nix.settings.trusted-public-keys`.
    #[serde(default)]
    pub cache_keys: Vec<String>,

    #[serde(default)]
    pub org_root_key: Option<KeySlot>,

    /// PEM-encoded fleet root CA cert. Offline-signed (operator
    /// workstation, file or Yubikey per D12) and embedded in trust.json
    /// so verifiers can anchor cert chains at a key the CP never holds
    /// at rest. `None` until the operator has run
    /// `nixfleet-trust-bootstrap`.
    #[serde(default)]
    pub root_ca_pem: Option<String>,

    /// PEM-encoded issuance CA chain. Each entry is signed by
    /// `root_ca_pem` and represents an issuance CA the fleet currently
    /// trusts to mint agent certs. Multiple entries during a rotation
    /// overlap window — agents accept any cert chain anchored at one
    /// of these intermediates. The TPM-bound issuance CA on the CP
    /// host appears here once it's bootstrapped.
    #[serde(default)]
    pub issuance_ca_pems: Vec<String>,
}

impl TrustConfig {
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;
}

/// LOADBEARING: `reject_before` is the compromise kill-switch — artifacts
/// signed before this timestamp are refused regardless of which key signed.
///
/// `successor` + `retire_at` declare a planned rotation in advance:
/// while `now < retire_at`, signatures from `successor` are accepted
/// (overlap window). Past `retire_at`, the reconciler emits
/// `Action::RotateTrustRoot` so the operator's tooling can rotate
/// `current → previous` and `successor → current` in the next fleet
/// commit. Closes nixfleet#63.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySlot {
    #[serde(default)]
    pub current: Option<TrustedPubkey>,

    #[serde(default)]
    pub previous: Option<TrustedPubkey>,

    #[serde(default)]
    pub reject_before: Option<DateTime<Utc>>,

    /// Pre-announced next key. Accepted by verifiers when
    /// `now < retire_at` (overlap window). Past `retire_at`, the
    /// reconciler emits `Action::RotateTrustRoot` — the actual
    /// promotion (`current → previous`, `successor → current`) is an
    /// out-of-band tooling step in fleet.nix, not an automated CP
    /// mutation. Paired with `retire_at` — both must be set together
    /// (Nix-side assertion in contracts/trust.nix).
    #[serde(default)]
    pub successor: Option<TrustedPubkey>,

    /// RFC 3339 deadline when the rotation should land. Drives both
    /// the verifier's overlap-window check (`now < retire_at` →
    /// successor accepted) and the reconciler's rotation-due signal
    /// (`now >= retire_at` + `successor.is_some()` → emit
    /// `Action::RotateTrustRoot`).
    #[serde(default)]
    pub retire_at: Option<DateTime<Utc>>,
}

impl KeySlot {
    /// Time-less view: returns `[current, previous]` (newer first).
    /// Use this from contexts that don't have a `now` parameter (test
    /// fixtures, schema-only inspection). Verifiers should call
    /// [`active_keys_at`] instead so the successor key is accepted
    /// during the overlap window.
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

    /// LOADBEARING: returns `[current, previous, successor (if
    /// now < retire_at)]`. Verifiers iterate first-match-wins; the
    /// order keeps the rotation grace window stable across calls,
    /// AND lets the successor's signature verify during the overlap
    /// window without requiring the operator to rotate `current`
    /// before the deadline.
    ///
    /// Outside the overlap (no `retire_at` set, or `now >=
    /// retire_at`), this is identical to [`active_keys`]. Once the
    /// reconciler emits `RotateTrustRoot` and the operator updates
    /// fleet.nix, the next tick sees the post-rotation slot and the
    /// successor-during-overlap path is no longer needed.
    pub fn active_keys_at(&self, now: DateTime<Utc>) -> Vec<TrustedPubkey> {
        let mut keys = self.active_keys();
        if let (Some(k), Some(retire_at)) = (&self.successor, self.retire_at) {
            if now < retire_at {
                keys.push(k.clone());
            }
        }
        keys
    }
}
