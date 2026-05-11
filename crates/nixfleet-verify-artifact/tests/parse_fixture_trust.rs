//! Pin the harness `test-trust.json` shape against `TrustConfig`.

use nixfleet_proto::TrustConfig;

#[test]
fn trust_json_from_fixture_shape_parses() {
    // LOADBEARING: byte-identical to the harness HEREDOC - update both together.
    let raw = r#"
    {
      "schemaVersion": 1,
      "ciReleaseKey": {
        "current": { "algorithm": "ed25519", "public": "PLACEHOLDER_PUBKEY_BASE64" },
        "previous": null,
        "rejectBefore": null
      },
      "cacheKeys": [],
      "orgRootKey": { "current": null }
    }
    "#;

    let parsed: TrustConfig = serde_json::from_str(raw).expect("trust.json parses");
    assert_eq!(parsed.schema_version, 1);
    assert!(parsed.ci_release_key.current.is_some());
}
