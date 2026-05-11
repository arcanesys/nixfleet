//! `revocations.json` - signed agent-cert revocation sidecar. Same
//! trust class as `fleet.resolved.json` (signed by `ciReleaseKey`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::fleet_resolved::Meta;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Revocations {
    pub schema_version: u32,
    /// Empty list is the steady state.
    pub revocations: Vec<RevocationEntry>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RevocationEntry {
    pub hostname: String,
    /// Any cert for `hostname` with `notBefore` strictly older than this
    /// is rejected at mTLS handshake time.
    pub not_before: DateTime<Utc>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub revoked_by: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_v1() -> Meta {
        Meta {
            schema_version: 1,
            signed_at: Some("2026-04-28T10:00:00Z".parse().unwrap()),
            ci_commit: Some("abc12345".into()),
            signature_algorithm: Some("ed25519".into()),
        }
    }

    #[test]
    fn empty_revocations_round_trip() {
        let r = Revocations {
            schema_version: 1,
            revocations: vec![],
            meta: meta_v1(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let parsed: Revocations = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn revocation_entry_round_trip() {
        let r = Revocations {
            schema_version: 1,
            revocations: vec![RevocationEntry {
                hostname: "old-laptop".into(),
                not_before: "2026-04-26T00:00:00Z".parse().unwrap(),
                reason: Some("decommissioned".into()),
                revoked_by: Some("operator".into()),
            }],
            meta: meta_v1(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let parsed: Revocations = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn revocation_entry_optional_fields_default_to_none() {
        let json = r#"{
            "hostname": "old-laptop",
            "notBefore": "2026-04-26T00:00:00Z"
        }"#;
        let entry: RevocationEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.hostname, "old-laptop");
        assert!(entry.reason.is_none());
        assert!(entry.revoked_by.is_none());
    }
}
