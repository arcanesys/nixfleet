//! Bootstrap token + enrollment + renewal wire types (RFC-0003 §5).
//!
//! Phase 3 PR-5. Token format is JSON: `{version, claims, signature}`
//! where `signature` is a detached ed25519 signature over the JCS
//! canonical bytes of `claims` (the `nixfleet-canonicalize` crate
//! produces the same bytes consumers verify against). The org root
//! pubkey lives in `trust.json` under `orgRootKey.current`.
//!
//! All types are wire-only: no crypto primitives leak from this
//! module — the CP and `nixfleet-mint-token` consume them via the
//! issuance and signing helpers in their own crates.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// =====================================================================
// Bootstrap token — operator-minted, signed by org root key
// =====================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapToken {
    /// Schema version. Bump on incompatible claim changes; consumers
    /// MUST refuse unknown versions.
    pub version: u32,
    pub claims: TokenClaims,
    /// Base64-encoded ed25519 signature over the JCS canonical bytes
    /// of `claims`.
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenClaims {
    /// The hostname this token authorises enrollment for. CP rejects
    /// the enrollment if the CSR's CN doesn't match.
    pub hostname: String,
    /// SHA-256 fingerprint of the expected CSR public key, base64-
    /// encoded. Binds the token to a specific keypair so a leaked
    /// token can't be used with an attacker-controlled key.
    pub expected_pubkey_fingerprint: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Random 16-byte nonce, hex-encoded. Lets the CP detect token
    /// replay (in-memory replay set in PR-5; SQLite persistence in
    /// Phase 4).
    pub nonce: String,
}

// =====================================================================
// /v1/enroll — first-boot enrollment
// =====================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollRequest {
    /// Operator-minted bootstrap token (signed by org root key).
    pub token: BootstrapToken,
    /// PEM-encoded CSR (as `rcgen::CertificateSigningRequest::pem()`
    /// emits it). The CP validates the CSR's CN against
    /// `token.claims.hostname` and the CSR's pubkey against
    /// `token.claims.expected_pubkey_fingerprint`.
    pub csr_pem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResponse {
    /// PEM-encoded signed certificate. Agent writes to
    /// `--client-cert` path.
    pub cert_pem: String,
    /// Validity not-after, in case the agent wants to schedule
    /// renewal without re-parsing the cert.
    pub not_after: DateTime<Utc>,
}

// =====================================================================
// /v1/agent/renew — periodic cert renewal
// =====================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewRequest {
    /// PEM-encoded CSR. CP validates the CSR's CN matches the
    /// requesting agent's verified mTLS CN, and that the CSR's
    /// pubkey differs from the existing cert's pubkey (key rotation
    /// is the point of /renew).
    pub csr_pem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewResponse {
    pub cert_pem: String,
    pub not_after: DateTime<Utc>,
}
