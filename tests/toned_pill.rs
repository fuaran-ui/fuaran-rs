//! Phase 750 — `CellKindErased::TonedPill`, the one cell kind that survives the wire.
//!
//! The corpus pins the canonical round-trip, the three §16 shorthands and the reject's
//! code + path. This file pins what the corpus deliberately does not:
//!
//! * the THIRD tone-map alias (`tones` — the fixture exercises `toneMap`, and a host
//!   that wired only the one it was shown is non-conformant in a way no fixture would
//!   catch);
//! * the didactic CONTENT of the refusal, not merely its code and path — that it names
//!   the offending key and teaches the seven legal tones is the entire reason that
//!   fixture is in the corpus;
//! * the omit rule at every branch;
//! * that a closure `Pill` is left alone, which is the other half of the coercion;
//! * **the render**, which is the strongest evidence here and the reason this host's
//!   leg is worth more than a codec pass: a decoded tree this host did not produce
//!   paints a different tone per row, from data alone.

use fuaran_rs::render::{BindingSources, render_to_html};
use fuaran_rs::wire::{DecodeErrorCode, decode_node, encode_node};

/// The smallest grid document carrying `kind` as its one column's cell kind. The
/// surrounding keys are already in canonical order, so this doubles as the expected
/// bytes when the cell kind is itself canonical.
fn column(kind: &str) -> String {
    format!(
        r#"{{"id":"g1","kind":{{"$type":"DataGrid","columns":[{{"field":"status","kind":{kind},"label":"Status"}}],"source":{{"$type":"Static","value":"<opaque>"}}}}}}"#
    )
}

#[track_caller]
fn normalises_to(given: &str, want: &str) {
    let node = decode_node(&column(given)).unwrap_or_else(|e| panic!("decode {given}: {e:?}"));
    let got = encode_node(&node);
    assert_eq!(got, column(want), "cell kind {given} did not normalise");
}

// ─── The tone-map field aliases (§3.6) ───────────────────────────────────────

#[test]
fn every_tone_map_alias_normalises_to_map() {
    for alias in ["map", "toneMap", "tones"] {
        normalises_to(
            &format!(
                r#"{{"$type":"TonedPill","field":"status","{alias}":{{"Delayed":"Warning"}}}}"#
            ),
            r#"{"$type":"TonedPill","field":"status","map":{"Delayed":"Warning"}}"#,
        );
    }
}

#[test]
fn canonical_tone_map_wins_over_an_alias() {
    normalises_to(
        r#"{"$type":"TonedPill","field":"status","map":{"Delayed":"Warning"},"toneMap":{"Delayed":"Critical"}}"#,
        r#"{"$type":"TonedPill","field":"status","map":{"Delayed":"Warning"}}"#,
    );
}

// ─── The §16 `Pill`-tagged shorthand ─────────────────────────────────────────

#[test]
fn pill_tag_carrying_a_tone_map_coerces_to_toned_pill() {
    normalises_to(
        r#"{"$type":"Pill","field":"status","map":{"Delayed":"Warning"}}"#,
        r#"{"$type":"TonedPill","field":"status","map":{"Delayed":"Warning"}}"#,
    );
}

#[test]
fn a_closure_pill_is_untouched() {
    // The coercion keys off the tone map, so an ordinary closure `Pill` — which can
    // never carry one — still decodes to the closure sentinels.
    normalises_to(
        r#"{"$type":"Pill","labelFn":"<closure>","toneFn":"<closure>"}"#,
        r#"{"$type":"Pill","labelFn":"<closure>","toneFn":"<closure>"}"#,
    );
}

// ─── The Phase 460 omit rule on `default` ────────────────────────────────────

#[test]
fn default_tone_omits_at_identity() {
    for given in [r#""default":"Default","#, r#""default":"Neutral","#] {
        // The aliased `Neutral` normalises to `Default` and THEN omits — two rules
        // composing, in that order.
        normalises_to(
            &format!(r#"{{"$type":"TonedPill",{given}"field":"s","map":{{"a":"Info"}}}}"#),
            r#"{"$type":"TonedPill","field":"s","map":{"a":"Info"}}"#,
        );
    }
}

#[test]
fn a_real_default_tone_survives() {
    normalises_to(
        r#"{"$type":"TonedPill","default":"Subdued","field":"s","map":{"a":"Info"}}"#,
        r#"{"$type":"TonedPill","default":"Subdued","field":"s","map":{"a":"Info"}}"#,
    );
}

#[test]
fn tone_aliases_apply_inside_the_map() {
    normalises_to(
        r#"{"$type":"TonedPill","field":"s","map":{"a":"Danger","b":"Positive","c":"Neutral"}}"#,
        r#"{"$type":"TonedPill","field":"s","map":{"a":"Critical","b":"Success","c":"Default"}}"#,
    );
}

// ─── The didactic refusal ────────────────────────────────────────────────────

#[test]
fn toned_pill_rejects() {
    let cases = [
        (
            r#"{"$type":"TonedPill","field":"status","map":{"Delayed":"Urgent"}}"#,
            DecodeErrorCode::UnknownDuCase,
            "$.kind.columns[0].kind.map.Delayed",
        ),
        (
            r#"{"$type":"TonedPill","field":"s","map":{"a":7}}"#,
            DecodeErrorCode::WrongType,
            "$.kind.columns[0].kind.map.a",
        ),
        (
            r#"{"$type":"TonedPill","map":{"a":"Info"}}"#,
            DecodeErrorCode::MissingField,
            "$.kind.columns[0].kind.field",
        ),
        (
            r#"{"$type":"TonedPill","field":"s"}"#,
            DecodeErrorCode::MissingField,
            "$.kind.columns[0].kind.map",
        ),
    ];
    for (kind, code, path) in cases {
        let err = decode_node(&column(kind)).expect_err("expected a refusal");
        assert_eq!(err.code, code, "{kind}");
        assert_eq!(err.path, path, "{kind}");
    }
}

#[test]
fn an_unknown_tone_map_value_is_refused_didactically() {
    let err = decode_node(&column(
        r#"{"$type":"TonedPill","field":"status","map":{"Delayed":"Urgent"}}"#,
    ))
    .expect_err("expected a refusal");
    // The offending KEY and value, in the terms the author wrote them — "one of your
    // tones is wrong" is not actionable when the map has nine entries.
    for want in ["Delayed", "Urgent"] {
        assert!(
            err.message.contains(want),
            "message {:?} does not name {want}",
            err.message
        );
    }
    // All seven legal names, so the author can fix it from the message alone.
    let expected = err.expected_shape.as_deref().unwrap_or("");
    for tone in [
        "Default", "Subdued", "Brand", "Success", "Warning", "Critical", "Info",
    ] {
        assert!(
            expected.contains(tone),
            "expected-shape {expected:?} does not teach {tone}"
        );
    }
}

// ─── The render ──────────────────────────────────────────────────────────────

/// A grid whose rows are embedded in the tree, so the render needs no host sources —
/// the point being that the TONE comes from the wire and nothing else.
const SHIPMENTS: &str = r#"{"id":"g","kind":{"$type":"DataGrid","columns":[{"field":"status","kind":{"$type":"TonedPill","default":"Subdued","field":"status","map":{"Cancelled":"Critical","Delayed":"Warning","On time":"Success"}},"label":"Status"}],"rowKeyField":"status","source":{"$type":"Transform","pipeline":[],"source":{"columns":{"status":{"validity":[true,true,true,true],"values":["On time","Delayed","Cancelled","Unknown"]}},"schema":[{"name":"status","type":"string"}]}}}}"#;

#[test]
fn toned_pill_paints_a_different_tone_per_row_from_the_wire_alone() {
    let node = decode_node(SHIPMENTS).expect("decodes");
    let html = render_to_html(&node, &BindingSources::default());
    // Mapped values each take their declared tone…
    for (label, tone) in [
        ("On time", "success"),
        ("Delayed", "warning"),
        ("Cancelled", "critical"),
    ] {
        let want =
            format!(r#"<span class="fuaran-grid-cell-pill fuaran-pill-{tone}">{label}</span>"#);
        assert!(html.contains(&want), "missing {want} in:\n{html}");
    }
    // …and the UNMAPPED value falls back to `default`, which is the case a parity
    // test misses most easily and the one a per-surface lookup copy gets wrong.
    assert!(
        html.contains(r#"<span class="fuaran-grid-cell-pill fuaran-pill-subdued">Unknown</span>"#),
        "unmapped value did not take the default tone:\n{html}"
    );
}

#[test]
fn an_absent_default_falls_back_to_the_identity_tone() {
    let node = decode_node(&SHIPMENTS.replace(r#""default":"Subdued","#, "").to_string())
        .expect("decodes");
    let html = render_to_html(&node, &BindingSources::default());
    assert!(
        html.contains(r#"<span class="fuaran-grid-cell-pill fuaran-pill-default">Unknown</span>"#),
        "absent `default` did not restore the identity tone:\n{html}"
    );
}

#[test]
fn a_row_missing_the_named_field_renders_an_empty_default_pill() {
    // The field-absent row: the projection is empty, so the label is empty and the
    // tone is the default — never a panic, never a dropped cell.
    let json = r#"{"id":"g","kind":{"$type":"DataGrid","columns":[{"field":"other","kind":{"$type":"TonedPill","field":"missing","map":{"a":"Info"}},"label":"Status"}],"rowKeyField":"other","source":{"$type":"Transform","pipeline":[],"source":{"columns":{"other":{"validity":[true],"values":["x"]}},"schema":[{"name":"other","type":"string"}]}}}}"#;
    let node = decode_node(json).expect("decodes");
    let html = render_to_html(&node, &BindingSources::default());
    assert!(
        html.contains(r#"<span class="fuaran-grid-cell-pill fuaran-pill-default"></span>"#),
        "field-absent row did not render an empty default pill:\n{html}"
    );
}
