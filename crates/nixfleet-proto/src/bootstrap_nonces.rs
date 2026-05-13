//! `bootstrap-nonces.json` - signed sidecar declaring valid bootstrap-token
//! nonces. Same trust class as `fleet.resolved.json` and `revocations.json`
//! (signed by `ciReleaseKey`).
//!
//! Closes the replay-after-DB-wipe vector: CP refuses any `/v1/enroll`
//! whose token nonce is not in the signed allowlist. After a state.db
//! wipe, CP rebuilds replay protection from the signed artifact.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::fleet_resolved::Meta;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapNonces {
    pub schema_version: u32,
    /// Empty list is the steady state. Strict enforcement: any /v1/enroll
    /// whose nonce is not in this list is rejected with 401.
    pub bootstrap_nonces: Vec<BootstrapNonceEntry>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapNonceEntry {
    /// Token claim - matches `BootstrapToken.claims.nonce` exactly.
    pub nonce: String,
    /// Token claim must match this. Defends against mis-targeted token
    /// swap (token A presented as if it were for host B).
    pub hostname: String,
    /// Authoritative validity window. May be tighter than the token's own
    /// `expires_at` claim; cannot extend past it (the token's own claim
    /// is still checked separately).
    pub expires_at: DateTime<Utc>,
    /// Optional audit field.
    #[serde(default)]
    pub minted_at: Option<DateTime<Utc>>,
    /// Optional audit field.
    #[serde(default)]
    pub minted_by: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_v1() -> Meta {
        Meta {
            schema_version: 1,
            signed_at: Some("2026-05-13T10:00:00Z".parse().unwrap()),
            ci_commit: Some("abc12345".into()),
            signature_algorithm: Some("ecdsa-p256".into()),
        }
    }

    #[test]
    fn optional_audit_fields_default_to_none() {
        let json = r#"{
            "nonce": "1ed727e1f9c24e6ab87eb9693ba35e26",
            "hostname": "agent-01",
            "expiresAt": "2026-05-13T22:57:45Z"
        }"#;
        let entry: BootstrapNonceEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.nonce, "1ed727e1f9c24e6ab87eb9693ba35e26");
        assert_eq!(entry.hostname, "agent-01");
        assert_eq!(entry.minted_at, None);
        assert_eq!(entry.minted_by, None);
    }

    #[test]
    fn camelcase_on_wire() {
        let r = BootstrapNonces {
            schema_version: 1,
            bootstrap_nonces: vec![],
            meta: meta_v1(),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"schemaVersion\""));
        assert!(s.contains("\"bootstrapNonces\""));
    }
}
