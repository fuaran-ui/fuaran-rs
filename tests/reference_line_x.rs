//! Phase 2654 — `ChartAnnotation.ReferenceLineX` shares `ReferenceLine`'s
//! finite-value narrowing at decode.
//!
//! The x reference line names a place on a continuous x axis exactly as
//! `ReferenceLine` names one on the value axis, and a non-finite value names
//! none: it would enter the x domain and take every tick and mark to NaN with
//! it. The reference host refuses it at decode, as `WRONG_TYPE` at the value
//! slot, in the same arm as `ReferenceLine`. These tests hold this host to that
//! until the corpus carries a reject vector for the case.

use fuaran_rs::wire::{DecodeErrorCode, decode_node, encode_node};

fn chart(value: &str) -> String {
    format!(
        r#"{{"id":"c1","kind":{{"$type":"Chart","annotations":[{{"$type":"ReferenceLineX","value":{value}}}],"kind":"Scatter","source":{{"$type":"Static","value":[]}},"xField":"x","yFields":["y"]}}}}"#
    )
}

#[test]
fn a_finite_x_reference_line_round_trips() {
    let src = chart("50");
    let node = decode_node(&src).expect("a finite x reference line must decode");
    assert_eq!(encode_node(&node), src);
}

#[test]
fn a_non_finite_x_reference_line_is_refused_at_its_value() {
    for sentinel in [r#""NaN""#, r#""Infinity""#, r#""-Infinity""#] {
        let err = decode_node(&chart(sentinel))
            .expect_err("a non-finite x reference line must be refused");
        assert_eq!(err.code, DecodeErrorCode::WrongType, "value {sentinel}");
        assert_eq!(err.path, "$.kind.annotations[0].value", "value {sentinel}");
    }
}
