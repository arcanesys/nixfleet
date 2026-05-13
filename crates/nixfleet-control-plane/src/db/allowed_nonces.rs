//! In-memory view of the signed bootstrap-nonces allowlist. Lives in
//! AppState behind a RwLock so the polling task replaces it wholesale
//! per successful verify; readers (enrolment handler) take a read
//! lock and look up by nonce.
//!
//! Not a `db` module in the strict sense (no SQLite), but lives here
//! for namespace symmetry with `revocations` (which IS sqlite-backed).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use nixfleet_proto::{BootstrapNonceEntry, BootstrapNonces};

/// Lookup by nonce. Empty by default; replaced wholesale per poll.
#[derive(Debug, Default, Clone)]
pub struct AllowedNoncesView {
    /// `nonce` -> entry. Read by the enrolment handler.
    by_nonce: HashMap<String, BootstrapNonceEntry>,
}

impl AllowedNoncesView {
    pub fn from_artifact(artifact: BootstrapNonces) -> Self {
        let by_nonce = artifact
            .bootstrap_nonces
            .into_iter()
            .map(|e| (e.nonce.clone(), e))
            .collect();
        Self { by_nonce }
    }

    pub fn lookup(&self, nonce: &str) -> Option<&BootstrapNonceEntry> {
        self.by_nonce.get(nonce)
    }

    pub fn len(&self) -> usize {
        self.by_nonce.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_nonce.is_empty()
    }

    /// True iff `entry.expires_at >= now`. Defense-in-depth: the release
    /// tool prunes expired entries at sign time, but clock skew between
    /// CP and CI could let one through.
    pub fn entry_is_live(entry: &BootstrapNonceEntry, now: DateTime<Utc>) -> bool {
        entry.expires_at >= now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nixfleet_proto::Meta;

    fn meta_v1() -> Meta {
        Meta {
            schema_version: 1,
            signed_at: Some("2026-05-13T10:00:00Z".parse().unwrap()),
            ci_commit: None,
            signature_algorithm: Some("ecdsa-p256".into()),
        }
    }

    #[test]
    fn lookup_finds_declared_nonce() {
        let view = AllowedNoncesView::from_artifact(BootstrapNonces {
            schema_version: 1,
            bootstrap_nonces: vec![BootstrapNonceEntry {
                nonce: "abc".into(),
                hostname: "agent-01".into(),
                expires_at: "2026-05-14T00:00:00Z".parse().unwrap(),
                minted_at: None,
                minted_by: None,
            }],
            meta: meta_v1(),
        });
        let entry = view.lookup("abc").expect("declared nonce found");
        assert_eq!(entry.hostname, "agent-01");
    }

    #[test]
    fn lookup_returns_none_for_unknown_nonce() {
        let view = AllowedNoncesView::default();
        assert!(view.lookup("unknown").is_none());
    }

    #[test]
    fn entry_is_live_strict_inequality() {
        let entry = BootstrapNonceEntry {
            nonce: "abc".into(),
            hostname: "agent-01".into(),
            expires_at: "2026-05-13T10:00:00Z".parse().unwrap(),
            minted_at: None,
            minted_by: None,
        };
        assert!(AllowedNoncesView::entry_is_live(
            &entry,
            "2026-05-13T10:00:00Z".parse().unwrap()
        ));
        assert!(AllowedNoncesView::entry_is_live(
            &entry,
            "2026-05-13T09:59:59Z".parse().unwrap()
        ));
        assert!(!AllowedNoncesView::entry_is_live(
            &entry,
            "2026-05-13T10:00:01Z".parse().unwrap()
        ));
    }
}
