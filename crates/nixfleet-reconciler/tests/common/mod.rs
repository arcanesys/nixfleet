//! Shared fixture-triple runner: tests/fixtures/<cat>/<name>/{fleet,observed,expected}.json.

#![allow(dead_code)]

pub mod signing;

use chrono::{DateTime, Utc};
use nixfleet_proto::FleetResolved;
use nixfleet_reconciler::{Action, Observed, reconcile};

pub fn fixture_now() -> DateTime<Utc> {
    "2026-04-24T10:00:00Z".parse().unwrap()
}

fn load<T: serde::de::DeserializeOwned>(path: &str) -> T {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

pub fn run(fixture_path: &str) -> (Vec<Action>, Vec<Action>) {
    let dir = format!("tests/fixtures/{fixture_path}");
    let fleet: FleetResolved = load(&format!("{dir}/fleet.json"));
    let observed: Observed = load(&format!("{dir}/observed.json"));
    let expected: Vec<Action> = load(&format!("{dir}/expected.json"));
    let actual = reconcile(&fleet, &observed, fixture_now());
    (actual, expected)
}

pub fn assert_matches(actual: &[Action], expected: &[Action]) {
    assert_eq!(
        actual,
        expected,
        "reconcile produced {} actions, expected {}:\n  actual  = {actual:#?}\n  expected= {expected:#?}",
        actual.len(),
        expected.len()
    );
}
