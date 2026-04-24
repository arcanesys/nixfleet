//! Golden-file JCS test (`docs/CONTRACTS.md §III`).
//!
//! Asserts the canonicalizer produces byte-exact output matching
//! the committed golden. Runs on every push via pre-push
//! `cargo nextest run --workspace`. Any drift = signature contract
//! broken.

use nixfleet_canonicalize::canonicalize;

const GOLDEN_INPUT: &str = include_str!("fixtures/jcs-golden.json");
const GOLDEN_CANONICAL: &str = include_str!("fixtures/jcs-golden.canonical");

#[test]
fn jcs_golden_bytes_match() {
    let produced = canonicalize(GOLDEN_INPUT).expect("canonicalize golden input");
    assert_eq!(
        produced, GOLDEN_CANONICAL,
        "JCS output drifted from golden — signature contract broken"
    );
}

#[test]
fn canonicalize_is_idempotent() {
    let once = canonicalize(GOLDEN_INPUT).expect("canonicalize once");
    let twice = canonicalize(&once).expect("canonicalize canonical form");
    assert_eq!(once, twice, "canonical form must be a fixed point");
}

#[test]
fn reordering_input_does_not_change_canonical_output() {
    let reordered = r#"{"schemaVersion":1,"a":{"x":[3,1,2],"z":null,"y":true},"b":2}"#;
    let original = canonicalize(GOLDEN_INPUT).expect("canonicalize original");
    let shuffled = canonicalize(reordered).expect("canonicalize shuffled");
    assert_eq!(
        original, shuffled,
        "canonical output must be invariant under input key ordering"
    );
}

#[test]
fn invalid_json_is_rejected() {
    let result = canonicalize("{not json");
    assert!(result.is_err(), "invalid JSON must be rejected");
}

/// Cross-stream JCS byte-identity check.
///
/// Stream B (nixfleet-compliance) pins a 69-byte canonical reference at
/// `tests/typed-controls/fixtures/_jcs-golden.json`. That file is already
/// JCS-canonical (keys sorted, compact, no trailing newline). Feeding it
/// through Stream C's `nixfleet_canonicalize::canonicalize` MUST produce
/// identical bytes — otherwise Stream A's CI and Stream C's verifier would
/// compute different signing bytes from the same declaration, silently
/// breaking every signature at rotation time.
///
/// The fixture is vendored from Stream B at copy-time. If Stream B
/// changes theirs, this test fails loudly and the copy gets refreshed.
/// That failure IS the cross-stream drift alarm working.
#[test]
fn stream_b_jcs_golden_round_trips_byte_identical() {
    const STREAM_B_GOLDEN: &str = include_str!("fixtures/stream-b/jcs-golden.json");

    let produced = canonicalize(STREAM_B_GOLDEN).expect("canonicalize Stream B golden");

    assert_eq!(
        produced, STREAM_B_GOLDEN,
        "cross-stream JCS byte drift detected: our canonicalize produced different bytes than Stream B's pinned golden"
    );
    assert_eq!(
        produced.len(),
        69,
        "Stream B's golden is 69 bytes; we produced {}",
        produced.len()
    );
}
