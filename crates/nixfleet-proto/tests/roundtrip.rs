//! Proto round-trip tests.
//!
//! Byte-exact: parse → re-serialize through JCS canonicalizer →
//! assert bytes match the committed golden.

use nixfleet_canonicalize::canonicalize;
use nixfleet_proto::FleetResolved;

fn load(path: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/{path}"))
        .unwrap_or_else(|err| panic!("read fixture {path}: {err}"))
}

#[test]
fn every_nullable_roundtrips_byte_for_byte() {
    let input = load("every-nullable.json");
    let golden = load("every-nullable.canonical");

    let parsed: FleetResolved = serde_json::from_str(&input).expect("parse every-nullable.json");

    let reserialized = serde_json::to_string(&parsed).expect("serialize FleetResolved");
    let produced = canonicalize(&reserialized).expect("canonicalize reserialized");

    assert_eq!(
        produced, golden,
        "FleetResolved round-trip is not JCS byte-identical to Stream B-style emission"
    );
}

#[test]
fn signed_artifact_roundtrips_byte_for_byte() {
    let input = load("signed-artifact.json");
    let golden = load("signed-artifact.canonical");

    let parsed: FleetResolved = serde_json::from_str(&input).expect("parse signed-artifact.json");

    let reserialized = serde_json::to_string(&parsed).expect("serialize");
    let produced = canonicalize(&reserialized).expect("canonicalize");

    assert_eq!(produced, golden, "signed-artifact round-trip broken");

    let signed_at = parsed
        .meta
        .signed_at
        .expect("signed-artifact must have meta.signedAt populated");
    assert_eq!(signed_at.to_rfc3339(), "2026-04-24T10:00:00+00:00");
    assert_eq!(parsed.meta.ci_commit.as_deref(), Some("deadbeef"));
}

/// Sanity check against Stream B's real Nix output.
///
/// Copied from `tests/lib/mkFleet/fixtures/empty-selector-warns.resolved.json`
/// on branch `feat/mkfleet-promotion` (copy-time SHA locked in git log of
/// this file). If Stream B changes the schema, this test fails and we
/// re-copy + adjust proto types.
#[test]
fn stream_b_empty_selector_parses_and_canonicalizes() {
    let input = load("stream-b/empty-selector-warns.resolved.json");

    let parsed: FleetResolved = serde_json::from_str(&input).expect("parse Stream B fixture");

    // Spot-check a field that only Stream B's newer schema carries:
    assert!(parsed.channels.contains_key("stable"));
    let chan = &parsed.channels["stable"];
    assert!(chan.freshness_window > 0);
    assert!(chan.signing_interval_minutes > 0);

    // Round-trip must not panic or produce invalid JSON.
    let reserialized = serde_json::to_string(&parsed).expect("serialize");
    let canonical = canonicalize(&reserialized).expect("canonicalize");
    assert!(!canonical.is_empty());
}

#[test]
fn meta_signature_algorithm_none_round_trips_as_absent() {
    // Per #18's §I/§II amendment: `meta.signatureAlgorithm` is OPTIONAL
    // with default "ed25519" on the consumer side. The canonical way to
    // encode "use the default" is ABSENT (not `null`). Explicit algorithm
    // strings round-trip as themselves.
    let input = load("every-nullable.json");
    let parsed: FleetResolved = serde_json::from_str(&input).expect("parse");
    assert!(
        parsed.meta.signature_algorithm.is_none(),
        "every-nullable fixture has no signatureAlgorithm, parses as None"
    );

    let reserialized = serde_json::to_string(&parsed).expect("serialize");
    assert!(
        !reserialized.contains("signatureAlgorithm"),
        "None must not emit the field: {reserialized}"
    );
}

#[test]
fn meta_signature_algorithm_some_round_trips_as_explicit_string() {
    // Inject signatureAlgorithm: "ecdsa-p256" into a fixture; parse;
    // assert Some; re-serialize; assert the explicit string is present.
    let input = load("signed-artifact.json");
    let mut value: serde_json::Value = serde_json::from_str(&input).unwrap();
    value["meta"]["signatureAlgorithm"] = serde_json::json!("ecdsa-p256");

    let parsed: FleetResolved =
        serde_json::from_str(&value.to_string()).expect("parse with signatureAlgorithm");
    assert_eq!(
        parsed.meta.signature_algorithm.as_deref(),
        Some("ecdsa-p256")
    );

    let reserialized = serde_json::to_string(&parsed).expect("serialize");
    assert!(
        reserialized.contains(r#""signatureAlgorithm":"ecdsa-p256""#),
        "Some(\"ecdsa-p256\") must round-trip as explicit string: {reserialized}"
    );
}

#[test]
fn unknown_fields_at_any_level_are_ignored() {
    let input = load("every-nullable.json");
    let mut value: serde_json::Value = serde_json::from_str(&input).unwrap();
    value["futureTopLevelField"] = serde_json::json!("v2-preview");
    value["hosts"]["h1"]["unknownPerHostField"] = serde_json::json!(42);
    value["meta"]["unknownMetaField"] = serde_json::json!(true);

    let injected = serde_json::to_string(&value).unwrap();
    let parsed: FleetResolved = serde_json::from_str(&injected)
        .expect("unknown fields must parse (CONTRACTS §V forward compat)");

    assert_eq!(parsed.schema_version, 1);
    assert_eq!(parsed.hosts.len(), 1);
}

#[test]
fn channel_freshness_window_duration_converts_minutes_to_seconds() {
    // Unit landmine: `freshness_window` is declared in MINUTES by
    // `lib/mkFleet.nix` but the field name has no `_minutes` suffix.
    // The helper converts via u64 * 60 so callers cannot accidentally
    // pass the raw u32 into `Duration::from_secs` and shrink the
    // window 60×. The `every-nullable.json` and `signed-artifact.json`
    // fixtures both use `freshnessWindow: 180`, so 180 min → 10_800 s.
    use std::time::Duration;
    let input = load("every-nullable.json");
    let parsed: FleetResolved = serde_json::from_str(&input).expect("parse every-nullable.json");
    let stable = parsed
        .channels
        .get("stable")
        .expect("every-nullable fixture has a `stable` channel");
    assert_eq!(stable.freshness_window, 180);
    assert_eq!(
        stable.freshness_window_duration(),
        Duration::from_secs(10_800)
    );
}
