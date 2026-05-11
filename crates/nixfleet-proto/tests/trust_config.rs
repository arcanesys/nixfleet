//! Round-trip tests for TrustConfig + KeySlot.

use nixfleet_proto::{KeySlot, TrustConfig, TrustedPubkey};

#[test]
fn trust_config_roundtrips_minimum_shape() {
    let json = r#"{
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": "AAAA" },
            "previous": null,
            "rejectBefore": null
        },
        "cacheKeys": [],
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
    assert!(cfg.cache_keys.is_empty());
    assert!(cfg.org_root_key.is_none());
}

#[test]
fn trust_config_omitted_cache_keys_defaults_to_empty() {
    let json = r#"{
        "schemaVersion": 1,
        "ciReleaseKey": { "current": null, "previous": null, "rejectBefore": null }
    }"#;
    let cfg: TrustConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.cache_keys.is_empty());
}

#[test]
fn trust_config_accepts_opaque_cache_key_strings() {
    // Proto stores key strings unparsed and forwards opaquely to nix.
    let json = r#"{
        "schemaVersion": 1,
        "ciReleaseKey": { "current": null, "previous": null, "rejectBefore": null },
        "cacheKeys": [
            "cache.example.com:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "attic:cache.example.com:BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB="
        ]
    }"#;
    let cfg: TrustConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.cache_keys.len(), 2);
    assert!(cfg.cache_keys[0].starts_with("cache.example.com:"));
    assert!(cfg.cache_keys[1].starts_with("attic:"));
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
        successor: None,
        retire_at: None,
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
        successor: None,
        retire_at: None,
    };
    assert!(slot.active_keys().is_empty());
}

#[test]
fn key_slot_active_keys_at_includes_successor_during_overlap() {
    let now = chrono::Utc::now();
    let retire_at = now + chrono::Duration::hours(24); // overlap window OPEN
    let slot = KeySlot {
        current: Some(TrustedPubkey {
            algorithm: "ed25519".into(),
            public: "AAAA".into(),
        }),
        previous: None,
        reject_before: None,
        successor: Some(TrustedPubkey {
            algorithm: "ed25519".into(),
            public: "CCCC".into(),
        }),
        retire_at: Some(retire_at),
    };
    let keys = slot.active_keys_at(now);
    assert_eq!(keys.len(), 2, "current + successor in overlap");
    assert_eq!(keys[0].public, "AAAA", "current first");
    assert_eq!(keys[1].public, "CCCC", "successor last");
}

#[test]
fn key_slot_active_keys_at_excludes_successor_post_retire() {
    let now = chrono::Utc::now();
    let retire_at = now - chrono::Duration::hours(1); // overlap window CLOSED
    let slot = KeySlot {
        current: Some(TrustedPubkey {
            algorithm: "ed25519".into(),
            public: "AAAA".into(),
        }),
        previous: None,
        reject_before: None,
        successor: Some(TrustedPubkey {
            algorithm: "ed25519".into(),
            public: "CCCC".into(),
        }),
        retire_at: Some(retire_at),
    };
    let keys = slot.active_keys_at(now);
    assert_eq!(
        keys.len(),
        1,
        "successor must NOT be trusted past retire_at"
    );
    assert_eq!(keys[0].public, "AAAA");
}

#[test]
fn key_slot_active_keys_at_treats_no_retire_at_as_no_overlap() {
    // Successor declared without retire_at - defensive: don't trust it.
    // The Nix-side schema asserts paired-options, so this state is
    // unreachable from the operator path; this test pins runtime
    // behaviour if a malformed trust.json sneaks in.
    let slot = KeySlot {
        current: Some(TrustedPubkey {
            algorithm: "ed25519".into(),
            public: "AAAA".into(),
        }),
        previous: None,
        reject_before: None,
        successor: Some(TrustedPubkey {
            algorithm: "ed25519".into(),
            public: "CCCC".into(),
        }),
        retire_at: None,
    };
    let keys = slot.active_keys_at(chrono::Utc::now());
    assert_eq!(
        keys.len(),
        1,
        "no retire_at → no overlap window → ignore successor"
    );
}

#[test]
fn trust_config_rejects_missing_schema_version() {
    let json = r#"{
        "ciReleaseKey": { "current": null, "previous": null, "rejectBefore": null }
    }"#;
    let err = serde_json::from_str::<TrustConfig>(json).unwrap_err();
    assert!(err.to_string().contains("schemaVersion"), "got: {err}");
}

/// Pins the JSON shape Nix scope modules emit when an operator sets
/// an org root key - the bare-string → struct promotion.
#[test]
fn trust_config_parses_populated_org_root_key_matching_nix_emission() {
    let json = r#"{
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": "AAAA" },
            "previous": null,
            "rejectBefore": null
        },
        "cacheKeys": [],
        "orgRootKey": {
            "current": { "algorithm": "ed25519", "public": "BBBB" },
            "previous": null,
            "rejectBefore": null
        }
    }"#;
    let cfg: TrustConfig = serde_json::from_str(json).unwrap();
    let org = cfg.org_root_key.as_ref().expect("orgRootKey set");
    let current = org.current.as_ref().expect("current pinned");
    assert_eq!(current.algorithm, "ed25519");
    assert_eq!(current.public, "BBBB");
    assert!(org.previous.is_none());
}
