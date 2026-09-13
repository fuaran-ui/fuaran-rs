//! `WIRE_FORMAT.md` §24.8 — a bare `State` at an unwritten slot is UNRESOLVED
//! (Phase 1690).
//!
//! §24.1 says a `Binding.State` carrying a `defaultValue` resolves to it until
//! the slot is first written. §24.8 says what one carrying NONE resolves to,
//! which the specification left open and which five hosts had answered four
//! different ways: this host resolved the slot's typed PLACEHOLDER, so a
//! `Metric` labelled "Revenue" over a key nothing had written read `0` — a
//! figure indistinguishable from one the host furnished, where the truth is
//! that none was.
//!
//! `default_value` is the RESOLUTION default the decoder fills in whether or
//! not the document said anything; `default_declared` is the wire fact beside
//! it (Phase 1656). This arm reads the second, which is the whole reason the
//! pair exists.
//!
//! No corpus vector reaches this host: the family that pins §24.8
//! (`render-text.json`'s `bare-state-numeric-slot-unresolved`) has readers on
//! the reference host, `fuaran-py` and `fuaran-go`, and this host has none. So
//! the pin lives here, and it is asserted through `render_to_html` rather than
//! by calling the resolver — the fabricated `0` was only a defect because it
//! reached the page, and a unit test on the resolver would pass just as well if
//! the renderer stopped consuming it.

use std::collections::HashMap;

use fuaran_rs::canonical::JVal;
use fuaran_rs::render::{BindingSources, render_to_html};
use fuaran_rs::wire::decode_node;

/// A `Metric` whose value is a bare `State` — no declared default.
const BARE: &str = r#"{"id":"m","kind":{"$type":"Metric","label":"Revenue","value":{"$type":"State","key":"revenue"}}}"#;

/// The same `Metric` with a default the DOCUMENT declared.
const DECLARED: &str = r#"{"id":"m","kind":{"$type":"Metric","label":"Revenue","value":{"$type":"State","defaultValue":7,"key":"revenue"}}}"#;

fn render(doc: &str, state: Vec<(&str, JVal)>) -> String {
    let node = decode_node(doc).unwrap_or_else(|e| panic!("the fixture did not decode: {e:?}"));
    let mut map = HashMap::new();
    for (k, v) in state {
        map.insert(k.to_string(), v);
    }
    render_to_html(
        &node,
        &BindingSources {
            state: map,
            ..BindingSources::default()
        },
    )
}

/// The rule itself, and the leg that fails on the pre-1690 arm: nothing written,
/// nothing declared, so the slot renders its absence placeholder and never a
/// number.
#[test]
fn a_bare_state_at_an_unwritten_numeric_slot_renders_absence() {
    let html = render(BARE, vec![]);
    assert!(
        html.contains(r#"<div class="fuaran-metric-value">—</div>"#),
        "§24.8: a bare State at an unwritten slot is unresolved, so the slot shows its placeholder:\n{html}"
    );
    assert!(
        !html.contains(r#"<div class="fuaran-metric-value">0</div>"#),
        "a fabricated zero reached the page — the failure §24.8 exists to close:\n{html}"
    );
}

/// The ruling WIDENED nothing: a host value still resolves. Without this leg an
/// arm that answered `NotResolved` unconditionally would pass the test above.
#[test]
fn a_written_slot_still_resolves() {
    let html = render(BARE, vec![("revenue", JVal::Num(42.0))]);
    assert!(
        html.contains(r#"<div class="fuaran-metric-value">42</div>"#),
        "a host-furnished value must still reach the slot:\n{html}"
    );
}

/// And it NARROWED nothing: §24.1's declared-default rule is untouched, which is
/// the half this ruling is the other side of. An arm keyed on the value rather
/// than on `default_declared` passes the first two legs and fails this one at a
/// slot whose placeholder happens to equal the declaration.
#[test]
fn a_declared_default_still_resolves_and_a_written_value_still_beats_it() {
    let bare = render(DECLARED, vec![]);
    assert!(
        bare.contains(r#"<div class="fuaran-metric-value">7</div>"#),
        "§24.1: a declared default resolves with no host value at all:\n{bare}"
    );
    let written = render(DECLARED, vec![("revenue", JVal::Num(42.0))]);
    assert!(
        written.contains(r#"<div class="fuaran-metric-value">42</div>"#),
        "writing wins over defaulting — hydration re-resolves, it does not lose to authored data:\n{written}"
    );
}
