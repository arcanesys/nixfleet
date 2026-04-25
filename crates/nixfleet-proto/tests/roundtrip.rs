//! Proto round-trip tests: parse -> re-serialize -> JCS canonicalize ->
//! assert byte-equality with golden.

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
        "FleetResolved round-trip is not JCS byte-identical to the canonical golden"
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

/// Sanity check against the Nix evaluator's real output.
#[test]
fn stream_b_empty_selector_parses_and_canonicalizes() {
    let input = load("stream-b/empty-selector-warns.resolved.json");

    let parsed: FleetResolved = serde_json::from_str(&input).expect("parse fleet fixture");

    // Spot-check a field that only the newer schema carries:
    assert!(parsed.channels.contains_key("stable"));
    let chan = &parsed.channels["stable"];
    assert!(chan.freshness_window > 0);
    assert!(chan.signing_interval_minutes > 0);

    let reserialized = serde_json::to_string(&parsed).expect("serialize");
    let canonical = canonicalize(&reserialized).expect("canonicalize");
    assert!(!canonical.is_empty());
}

#[test]
fn meta_signature_algorithm_absent_round_trips_as_none() {
    // docs/design/contracts.md §V Pattern A: absent ≡ "ed25519" within schemaVersion 1.
    // The fixture omits signatureAlgorithm; parse -> None; helper resolves
    // to "ed25519"; re-serialize keeps it absent (skip_serializing_if).
    let input = load("every-nullable.json");
    let parsed: FleetResolved = serde_json::from_str(&input).expect("parse");
    assert_eq!(parsed.meta.signature_algorithm, None);
    assert_eq!(
        parsed.meta.signature_algorithm_or_default(),
        "ed25519",
        "absent ≡ ed25519 default per CONTRACTS §V Pattern A",
    );

    let reserialized = serde_json::to_string(&parsed).expect("serialize");
    assert!(
        !reserialized.contains("\"signatureAlgorithm\""),
        "absent input must round-trip as absent (no field emitted): {reserialized}",
    );
}

#[test]
fn meta_signature_algorithm_some_round_trips_as_explicit_string() {
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
    let parsed: FleetResolved =
        serde_json::from_str(&injected).expect("unknown fields must parse (forward compat)");

    assert_eq!(parsed.schema_version, 1);
    assert_eq!(parsed.hosts.len(), 1);
}

#[test]
fn channel_freshness_window_duration_converts_minutes_to_seconds() {
    // freshness_window is MINUTES; helper guards against the 60× landmine.
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
