//! RFC 8785 Appendix E conformance corpus.

use nixfleet_canonicalize::canonicalize;

const CASES: &[(&str, &str, &str)] = &[
    ("empty_object", "{}", "{}"),
    ("empty_array", "[]", "[]"),
    ("keys_sorted_simple", r#"{"a":1,"b":2}"#, r#"{"a":1,"b":2}"#),
    ("keys_unsorted", r#"{"b":2,"a":1}"#, r#"{"a":1,"b":2}"#),
    // Numeric keys sort as strings: "10" < "2".
    (
        "numeric_keys_sort_lexicographically",
        r#"{"2":"two","10":"ten","1":"one"}"#,
        r#"{"1":"one","10":"ten","2":"two"}"#,
    ),
    (
        "rfc8785_e1_arrays",
        r#"[56,{"d":true,"10":null,"1":[]}]"#,
        r#"[56,{"1":[],"10":null,"d":true}]"#,
    ),
    (
        "nested_objects_sort_recursively",
        r#"{"outer":{"z":1,"a":2}}"#,
        r#"{"outer":{"a":2,"z":1}}"#,
    ),
    (
        "control_chars_escaped",
        "{\"k\":\"\\b\\t\\n\"}",
        r#"{"k":"\b\t\n"}"#,
    ),
    // Forward slash is NOT escaped.
    (
        "forward_slash_not_escaped",
        r#"{"url":"http:\/\/example.com"}"#,
        r#"{"url":"http://example.com"}"#,
    ),
    (
        "primitives_round_trip",
        r#"{"t":true,"f":false,"n":null}"#,
        r#"{"f":false,"n":null,"t":true}"#,
    ),
];

#[test]
fn rfc8785_appendix_e_corpus() {
    let mut failures = Vec::new();
    for (name, input, expected) in CASES {
        let produced = match canonicalize(input) {
            Ok(p) => p,
            Err(err) => {
                failures.push(format!("{name}: canonicalize errored: {err:?}"));
                continue;
            }
        };
        if produced != *expected {
            failures.push(format!(
                "{name}:\n  input    = {input}\n  produced = {produced}\n  expected = {expected}",
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "RFC 8785 Appendix E corpus mismatches ({} of {}):\n{}",
        failures.len(),
        CASES.len(),
        failures.join("\n\n"),
    );
}

#[test]
fn corpus_canonical_outputs_are_fixed_points() {
    for (name, _input, expected) in CASES {
        let twice = canonicalize(expected)
            .unwrap_or_else(|err| panic!("{name}: re-canonicalize expected failed: {err:?}"));
        assert_eq!(
            twice, *expected,
            "{name}: canonical form is not a fixed point",
        );
    }
}
