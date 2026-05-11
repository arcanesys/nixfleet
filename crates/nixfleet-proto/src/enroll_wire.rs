//! Bootstrap token + enrollment + renewal wire types. Tokens carry a
//! detached ed25519 signature over the JCS canonical bytes of `claims`,
//! verified against `orgRootKey.current` from `trust.json`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapToken {
    /// Consumers MUST refuse unknown versions.
    pub version: u32,
    pub claims: TokenClaims,
    /// Base64 ed25519 signature over JCS-canonical `claims`.
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenClaims {
    pub hostname: String,
    /// SHA-256 fingerprint of expected CSR pubkey (base64). Binds the
    /// token to a specific keypair.
    pub expected_pubkey_fingerprint: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Random 16-byte hex nonce. Backs replay detection.
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollRequest {
    pub token: BootstrapToken,
    /// PEM CSR. CP validates CN + pubkey against `token.claims`.
    pub csr_pem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResponse {
    pub cert_pem: String,
    pub not_after: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewRequest {
    /// PEM CSR. CP validates CN matches mTLS CN and pubkey differs from
    /// the existing cert's pubkey (rotation is the point of /renew).
    pub csr_pem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewResponse {
    pub cert_pem: String,
    pub not_after: DateTime<Utc>,
}

/// `POST /v1/agent/bootstrap-report` - anonymous, bootstrap-token-authed
/// event channel for failure modes the agent hits before it has a
/// client cert. CP validates the token signature against orgRootKey
/// (same path as `/v1/enroll`) but does NOT consume the nonce - the
/// agent should still be able to enroll on the next attempt with the
/// same token after the operator fixes whatever broke. Only specific
/// `ReportEvent` variants (`EnrollmentFailed`, `TrustError`) are
/// accepted; everything else is 422.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapEventRequest {
    pub token: BootstrapToken,
    pub agent_version: String,
    pub occurred_at: DateTime<Utc>,
    /// Carried opaquely; CP unwraps the discriminator to enforce the
    /// pre-cert-only allowlist and routes into the same per-host report
    /// store as mTLS-authed reports.
    pub event: serde_json::Value,
}
