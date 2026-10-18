//! Phase 1962 — a multi-select `Select` carries `values` and no `value`.
//!
//! The corpus certifies the canonical bytes and the lenient drop of the empty
//! `Static` placeholder and a `State` binding. What it cannot state cheaply is
//! the boundary of the drop: a `value` on a multi-select is still DECODED
//! before it is dropped, so a malformed binding there refuses exactly as it
//! would anywhere else, while a well-formed one of any shape normalises away.

use fuaran_rs::wire::{decode_node, encode_node};

const CANONICAL: &str = r#"{"id":"s","kind":{"$type":"Select","label":"Pick","multiple":true,"source":{"$type":"Static","value":[]},"values":{"$type":"State","key":"picked"}}}"#;

fn with_value(value: &str) -> String {
    format!(
        r#"{{"id":"s","kind":{{"$type":"Select","label":"Pick","multiple":true,"source":{{"$type":"Static","value":[]}},"value":{value},"values":{{"$type":"State","key":"picked"}}}}}}"#
    )
}

#[test]
fn canonical_multi_select_round_trips_without_value() {
    let node = decode_node(CANONICAL).expect("the canonical form decodes");
    assert_eq!(encode_node(&node), CANONICAL);
}

#[test]
fn a_well_formed_value_on_a_multi_select_is_dropped() {
    for value in [
        r#"{"$type":"Static"}"#,
        r#"{"$type":"Static","value":null}"#,
        r#"{"$type":"Static","value":"eng"}"#,
        r#"{"$type":"State","defaultValue":"Auth","key":"category-primary"}"#,
    ] {
        let json = with_value(value);
        let node = decode_node(&json).unwrap_or_else(|e| panic!("{value} must decode: {e:?}"));
        assert_eq!(encode_node(&node), CANONICAL, "{value} normalises away");
    }
}

#[test]
fn a_malformed_value_on_a_multi_select_still_refuses() {
    for value in [r#"{"$type":"Nope"}"#, "42"] {
        let json = with_value(value);
        let err = decode_node(&json).expect_err("a malformed binding refuses");
        assert!(
            err.path.starts_with("$.kind.value"),
            "{value}: the refusal names the value slot: {err:?}"
        );
    }
}

#[test]
fn a_single_select_still_requires_value() {
    let json = r#"{"id":"s","kind":{"$type":"Select","label":"Pick","source":{"$type":"Static","value":[]}}}"#;
    let err = decode_node(json).expect_err("a single-select without value refuses");
    assert_eq!(err.code.as_str(), "MISSING_FIELD", "{err:?}");
    assert_eq!(err.path, "$.kind.value", "{err:?}");
}
