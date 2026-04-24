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
