//! Trust root declarations. LOADBEARING: algorithm is a property of the key,
//! not the artifact - artifacts MUST NOT carry their own algorithm claim, or
//! an attacker could downgrade by lying about which algo signed the bytes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `algorithm` is `String` (not enum) for forward-compat. Unknown values
/// surface as `UnsupportedAlgorithm` at verify time. Today: ed25519, with
/// `public` as 32-byte base64 (padded).
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

    /// PEM-encoded fleet root CA cert. Offline-signed (operator workstation,
    /// file or Yubikey) and embedded in trust.json so verifiers anchor cert
    /// chains at a key the CP never holds at rest. `None` until the operator
    /// has run `nixfleet-trust-bootstrap`.
    #[serde(default)]
    pub root_ca_pem: Option<String>,

    /// PEM-encoded issuance CAs the fleet trusts to mint agent certs, each
    /// signed by `root_ca_pem`. Multiple entries support rotation overlap -
    /// agents accept any cert chain anchored at one of these intermediates.
    #[serde(default)]
    pub issuance_ca_pems: Vec<String>,
}

impl TrustConfig {
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;
}

/// LOADBEARING: `reject_before` is the compromise kill-switch - artifacts
/// signed before this timestamp are refused regardless of which key signed.
/// `successor` + `retire_at` declare a planned rotation: while
/// `now < retire_at` the successor's signature is accepted (overlap window);
/// past `retire_at` the reconciler emits `Action::RotateTrustRoot` so the
/// operator's tooling can promote `current -> previous`, `successor -> current`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySlot {
    #[serde(default)]
    pub current: Option<TrustedPubkey>,

    #[serde(default)]
    pub previous: Option<TrustedPubkey>,

    #[serde(default)]
    pub reject_before: Option<DateTime<Utc>>,

    /// Pre-announced next key. Accepted during the overlap window
    /// (`now < retire_at`). Must be set together with `retire_at` (Nix-side
    /// assertion in contracts/trust.nix). Promotion to `current` is operator-
    /// driven, never automated by CP.
    #[serde(default)]
    pub successor: Option<TrustedPubkey>,

    /// RFC 3339 deadline for rotation. Drives both the verifier's overlap-
    /// window check and the reconciler's `RotateTrustRoot` signal.
    #[serde(default)]
    pub retire_at: Option<DateTime<Utc>>,
}

impl KeySlot {
    /// Time-less view: `[current, previous]` (newer first). For schema-only
    /// inspection / fixtures. Verifiers should call [`Self::active_keys_at`] so
    /// successor is honored during the overlap window.
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

    /// LOADBEARING: `[current, previous, successor (if now < retire_at)]`.
    /// Verifiers iterate first-match-wins; this ordering lets the successor
    /// signature verify during the overlap window without forcing the
    /// operator to rotate `current` before the deadline. Outside the overlap
    /// it's identical to [`Self::active_keys`].
    pub fn active_keys_at(&self, now: DateTime<Utc>) -> Vec<TrustedPubkey> {
        let mut keys = self.active_keys();
        if let (Some(k), Some(retire_at)) = (&self.successor, self.retire_at)
            && now < retire_at
        {
            keys.push(k.clone());
        }
        keys
    }
}
