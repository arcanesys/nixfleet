//! Sanity check: the exact `test-trust.json` shape emitted by
//! `tests/harness/fixtures/signed/default.nix` must deserialize with
//! `TrustConfig`. Smoke-tests the contract between the harness
//! fixture (Stream A output) and the proto crate (Stream C input)
//! before the full signed-roundtrip scenario lands.

use nixfleet_proto::TrustConfig;

#[test]
fn trust_json_from_fixture_shape_parses() {
    // Byte-identical to the HEREDOC in tests/harness/fixtures/signed/default.nix
    // (step 5) with a placeholder pubkey. If the fixture changes, update here.
    let raw = r#"
    {
      "schemaVersion": 1,
      "ciReleaseKey": {
        "current": { "algorithm": "ed25519", "public": "PLACEHOLDER_PUBKEY_BASE64" },
        "previous": null,
        "rejectBefore": null
      },
      "atticCacheKey": { "current": null },
      "orgRootKey": { "current": null }
    }
    "#;

    let parsed: TrustConfig = serde_json::from_str(raw).expect("trust.json parses");
    assert_eq!(parsed.schema_version, 1);
    assert!(parsed.ci_release_key.current.is_some());
}
