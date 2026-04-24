//! Round-trip tests for TrustConfig + KeySlot + AtticKeySlot.
//!
//! Shape authoritative per docs/trust-root-flow.md §3.4 + §7.4.

use nixfleet_proto::{AtticKeySlot, KeySlot, TrustConfig, TrustedPubkey};

#[test]
fn trust_config_roundtrips_minimum_shape() {
    let json = r#"{
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": "AAAA" },
            "previous": null,
            "rejectBefore": null
        },
        "atticCacheKey": null,
        "orgRootKey": null
    }"#;
    let cfg: TrustConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.schema_version, 1);
    assert_eq!(
        cfg.ci_release_key.current.as_ref().unwrap().algorithm,
        "ed25519"
    );
    assert!(cfg.ci_release_key.previous.is_none());
    assert!(cfg.ci_release_key.reject_before.is_none());
    assert!(cfg.attic_cache_key.is_none());
    assert!(cfg.org_root_key.is_none());
}

#[test]
fn key_slot_active_keys_returns_both_current_and_previous() {
    let slot = KeySlot {
        current: Some(TrustedPubkey {
            algorithm: "ed25519".into(),
            public: "AAAA".into(),
        }),
        previous: Some(TrustedPubkey {
            algorithm: "ecdsa-p256".into(),
            public: "BBBB".into(),
        }),
        reject_before: None,
    };
    let keys = slot.active_keys();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].algorithm, "ed25519");
    assert_eq!(keys[1].algorithm, "ecdsa-p256");
}

#[test]
fn key_slot_active_keys_skips_absent() {
    let slot = KeySlot {
        current: None,
        previous: None,
        reject_before: None,
    };
    assert!(slot.active_keys().is_empty());
}

#[test]
fn attic_key_slot_accepts_native_format() {
    let json = r#""attic:cache.example.com:AAAA""#;
    let slot: AtticKeySlot = serde_json::from_str(json).unwrap();
    assert_eq!(slot.0, "attic:cache.example.com:AAAA");
}

#[test]
fn trust_config_rejects_missing_schema_version() {
    let json = r#"{
        "ciReleaseKey": { "current": null, "previous": null, "rejectBefore": null }
    }"#;
    let err = serde_json::from_str::<TrustConfig>(json).unwrap_err();
    assert!(err.to_string().contains("schemaVersion"), "got: {err}");
}
