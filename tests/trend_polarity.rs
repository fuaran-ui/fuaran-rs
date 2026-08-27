//! Phase 867 — `Metric.trendPolarity` on the wire (WIRE_FORMAT.md §3.6.1).
//!
//! The corpus fixture `nodes/metric-inverted-polarity.json` certifies the happy
//! round trip and is where conformance is decided. These add the three things
//! that fixture deliberately cannot express, each of which is a claim about
//! bytes the corpus does NOT contain:
//!
//!  1. **The reserved case is refused.** `Neutral` is named in the spec as
//!     reserved and is therefore absent from the accepted set; no fixture can
//!     pin a spelling that does not appear in the corpus, and a reject fixture
//!     for it would be pinning a refusal the format might later withdraw.
//!  2. **Omitted-when-default is a property of the ENCODER**, not of one
//!     document. A corpus fixture proves a tree carrying the field round-trips;
//!     only a negative assertion proves a tree NOT carrying it stays byte-clean,
//!     which is what makes every pre-867 `Metric` unchanged.
//!  3. **The declaration survives without a trend** (§3.6.1 clause 4 — legal,
//!     and says nothing). Inert is not the same as droppable.

use fuaran_rs::wire::{DecodeErrorCode, NodeKind, TrendPolarity, decode_node, encode_node};

/// A `Metric` whose `trendPolarity` member is spliced in verbatim, so a test can
/// pass a spelling the type cannot construct.
fn metric(polarity_member: &str) -> String {
    format!(
        r#"{{"id":"m","kind":{{"$type":"Metric","label":"Avg wait","trend":{{"$type":"Static","value":-0.0734}}{polarity_member},"value":{{"$type":"Static","value":80}}}}}}"#
    )
}

#[test]
fn reserved_neutral_is_refused_and_the_hint_names_only_what_the_format_accepts() {
    let err = decode_node(&metric(r#","trendPolarity":"Neutral""#))
        .expect_err("`Neutral` is RESERVED, not accepted");
    assert_eq!(err.code, DecodeErrorCode::UnknownDuCase, "{err:?}");
    // The corpus states a bare-string enum's reject path as the SLOT
    // (`reject-unknown-tone` → `$.style.tone`) and the harness matches it as a
    // PREFIX. This host's `unknown_du_case` appends `.$type` uniformly across
    // every bare enum — `tone`, `weight`, `variant` and now this one — so the
    // emitted path is prefix-conformant and identical in shape to its siblings.
    // Asserted as the prefix the corpus states, plus the host's actual suffix,
    // so this test pins the real behaviour rather than an invented expectation
    // and would go red if this slot alone ever diverged from the family.
    assert!(
        err.path.starts_with("$.kind.trendPolarity"),
        "the reject path names the slot: {err:?}"
    );
    assert_eq!(err.path, "$.kind.trendPolarity.$type", "{err:?}");
    // The hint is the load-bearing half. Advertising `Neutral` would tell an
    // author to emit a spelling the format refuses; omitting it is what keeps a
    // later admission an ADDITION rather than a re-meaning of shipped bytes.
    let hint = format!("{err:?}");
    assert!(
        hint.contains("HigherIsBetter") && hint.contains("LowerIsBetter"),
        "the hint must name both canonical spellings: {hint}"
    );
    assert!(
        !hint.contains("Neutral") || hint.matches("Neutral").count() == 1,
        "the reserved spelling appears only as the offending value, never in the accepted list: {hint}"
    );
}

#[test]
fn the_boolean_spelling_the_slot_replaced_is_not_aliased_back_in() {
    // §3.6.1 refuses `inverted: bool`; an alias for either of its natural
    // spellings would reintroduce it under a canonical case.
    for spelling in ["Inverted", "Descending", "higherIsBetter", "true"] {
        match decode_node(&metric(&format!(r#","trendPolarity":"{spelling}""#))) {
            Ok(_) => panic!(
                "`{spelling}` must not decode — an alias would reinstate the refused boolean spelling"
            ),
            Err(err) => assert_eq!(
                err.code,
                DecodeErrorCode::UnknownDuCase,
                "`{spelling}` must be refused as an unknown case, not silently aliased: {err:?}"
            ),
        }
    }
}

#[test]
fn a_non_string_polarity_is_a_type_error_not_an_unknown_case() {
    let err = decode_node(&metric(r#","trendPolarity":true"#))
        .expect_err("the slot is a bare-string enum");
    assert_eq!(err.path, "$.kind.trendPolarity", "{err:?}");
}

#[test]
fn absent_polarity_decodes_to_higher_is_better_and_re_encodes_to_nothing() {
    let node = decode_node(&metric("")).expect("decodes");
    let NodeKind::Metric(spec) = &node.kind else {
        panic!("expected a Metric");
    };
    assert_eq!(spec.trend_polarity, TrendPolarity::HigherIsBetter);

    let re = encode_node(&node);
    assert!(
        !re.contains("trendPolarity"),
        "omitted-when-default: a pre-867 Metric is byte-unchanged by construction, not by luck: {re}"
    );
    assert_eq!(
        re,
        metric(""),
        "the whole document round-trips byte-identically"
    );
}

#[test]
fn a_declared_polarity_re_encodes_canonically() {
    let raw = metric(r#","trendPolarity":"LowerIsBetter""#);
    let node = decode_node(&raw).expect("decodes");
    assert_eq!(encode_node(&node), raw);
}

#[test]
fn an_inert_polarity_survives_the_round_trip() {
    // Clause 4 — a `Metric` with no `trend` that declares a polarity is legal
    // and says nothing. Dropping it on re-encode would silently rewrite an
    // author's document because this host judged the declaration pointless.
    let raw = r#"{"id":"m","kind":{"$type":"Metric","label":"Avg wait","trendPolarity":"LowerIsBetter","value":{"$type":"Static","value":80}}}"#;
    let node = decode_node(raw).expect("an inert declaration is legal");
    assert_eq!(encode_node(&node), raw);
}
