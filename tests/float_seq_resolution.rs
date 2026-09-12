//! `resolve_float_seq`: a host-fed float sequence resolves to ONE ELEMENT PER
//! INPUT ELEMENT (Phase 1673).
//!
//! The `Json(Arr)` arm — the shape a `$state` / `$queries` slot arrives in — used
//! to be `filter_map(jval_number)`, so a non-numeric element was DROPPED and the
//! series silently SHORTENED. The identical array arriving as a typed
//! `Static`/`FloatSeq` payload kept its length, because that path reads each
//! element through `as_float`, which accepts the three quoted non-finite
//! sentinels §5 requires every host to emit.
//!
//! Two paths, one document, different series. The shorter one is the more
//! dangerous: a series index is a POSITION, so dropping element 1 does not leave
//! a gap at 1 — it slides every later reading one place left, and the chart then
//! shows the right values at the wrong places, looking entirely plausible.
//!
//! Asserted through `render_to_html` rather than by calling the resolver, on
//! purpose: the length is only a defect because it reaches the drawing, and a
//! unit test on the resolver would pass just as well if the renderer stopped
//! consuming it.

use std::collections::HashMap;

use fuaran_rs::canonical::JVal;
use fuaran_rs::render::{BindingSources, render_to_html};
use fuaran_rs::wire::decode_node;

/// A sparkline whose source is a `$state` slot — the host-fed `Json(Arr)` path.
const BOUND: &str =
    r#"{"id":"s","kind":{"$type":"Sparkline","source":{"$type":"State","key":"series"}}}"#;

fn num(n: f64) -> JVal {
    JVal::Num(n)
}

fn text(v: &str) -> JVal {
    JVal::Str(v.to_string())
}

fn series(items: Vec<JVal>) -> Option<JVal> {
    Some(JVal::Arr(items))
}

/// The same series as a typed `Static` payload — the corpus-exercised path, and
/// the oracle the bound path has to agree with.
fn statik(values: &str) -> String {
    let head = r#"{"id":"s","kind":{"$type":"Sparkline","source":{"$type":"Static","value":"#;
    format!("{head}{values}}}}}}}")
}

fn render(tree: &str, state: Option<JVal>) -> String {
    let node = decode_node(tree).expect("the test tree decodes");
    let mut sources = BindingSources::default();
    if let Some(v) = state {
        sources.state = HashMap::from([("series".to_string(), v)]);
    }
    render_to_html(&node, &sources)
}

/// The polyline's `points` attribute, verbatim.
fn points(html: &str) -> String {
    let at = html
        .find("points=\"")
        .unwrap_or_else(|| panic!("no polyline in the rendered sparkline:\n{html}"));
    let rest = &html[at + 8..];
    let end = rest.find('"').expect("the points attribute closes");
    rest[..end].to_string()
}

/// How many points the lowering put in the polyline. Deliberately independent of
/// how a coordinate is SPELLED — a non-finite coordinate renders as `0`, and
/// every assertion here is about a count or about two renders agreeing, never
/// about a particular number.
fn point_count(html: &str) -> usize {
    points(html).split(' ').filter(|p| !p.is_empty()).count()
}

#[test]
fn a_sentinel_element_keeps_its_place_instead_of_shortening_the_series() {
    // The defect, stated as its symptom: three in, three out.
    let html = render(BOUND, series(vec![num(1.0), text("NaN"), num(3.0)]));
    let n = point_count(&html);
    assert_eq!(n, 3, "a sentinel element must resolve to a point:\n{html}");
}

#[test]
fn an_element_that_is_neither_number_nor_sentinel_also_keeps_its_place() {
    // The rule is about LENGTH, not about which strings are blessed. `null`, an
    // object, a bool and an arbitrary string are all "no number here", and each
    // must still occupy its index.
    let junk = vec![
        JVal::Null,
        text("banana"),
        JVal::Bool(true),
        JVal::Obj(vec![]),
    ];
    for item in junk {
        let label = format!("{item:?}");
        let html = render(BOUND, series(vec![num(1.0), item, num(3.0)]));
        let n = point_count(&html);
        assert_eq!(n, 3, "element {label} shortened the series:\n{html}");
    }
}

#[test]
fn the_bound_path_and_the_static_path_agree_on_the_same_series() {
    // The property the rule exists for, and the one neither path can state
    // alone: one document, two routes, one series. The corpus pins the Static
    // route (`nodes/spark-nonfinite-sentinel.json`); nothing pinned the bound
    // one.
    let values = r#"[1,"NaN",3,"Infinity","-Infinity",5]"#;
    let via_static = render(&statik(values), None);
    let fed = vec![
        num(1.0),
        text("NaN"),
        num(3.0),
        text("Infinity"),
        text("-Infinity"),
        num(5.0),
    ];
    let via_state = render(BOUND, series(fed));
    assert_eq!(
        points(&via_state),
        points(&via_static),
        "the host-fed array and the typed Static payload produced different series"
    );
    assert_eq!(point_count(&via_static), 6);
}

#[test]
fn an_unresolved_source_is_still_the_empty_series() {
    // The rule widened what an ELEMENT may be; it must not have widened what an
    // absent SOURCE means. No state seeded ⇒ not-resolved ⇒ the em-dash element,
    // which carries no polyline at all.
    let html = render(BOUND, None);
    let empty = html.contains("fuaran-sparkline-empty");
    assert!(
        empty,
        "an unresolved source stays the empty sparkline:\n{html}"
    );
    assert!(!html.contains("points=\""), "{html}");
}

#[test]
fn the_probe_can_go_red() {
    // `point_count` agreeing with the expectation proves nothing unless it can
    // disagree. A genuinely shorter series must count shorter — otherwise every
    // assertion above would hold against the very defect this file certifies is
    // gone.
    let two = render(BOUND, series(vec![num(1.0), num(3.0)]));
    let three = render(BOUND, series(vec![num(1.0), num(2.0), num(3.0)]));
    assert_eq!(point_count(&two), 2);
    assert_eq!(point_count(&three), 3);
    assert_ne!(two, three);
}
