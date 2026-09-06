//! Conditional visibility, predicate cases and the scalar selector (Phase 1535).
//!
//! Hand-built trees rather than corpus fixtures, and deliberately: two of the
//! corpus fixtures this phase added (`node-visible`, `switch-predicate`) carry a
//! `Binding.Expr`, which this host does not model on `main` — Phase 1534's Rust
//! leg is a separate branch. Reaching for the corpus here would make this suite
//! fail for a reason that is not about this phase. The bytes below are the same
//! SHAPES with `State` predicates in place of the `Expr` ones, so what is
//! asserted is this phase's own rule; the corpus legs join once 1534's Rust
//! branch lands.
//!
//! What is asserted is the RENDERING — which no round-trip can see, and which is
//! the whole of what this phase changed on a host that already decoded fine.

use std::collections::HashMap;

use fuaran_rs::canonical::JVal;
use fuaran_rs::render::{BindingSources, render_to_html};
use fuaran_rs::wire::{Node, decode_node};

fn node(json: &str) -> Node {
    decode_node(json).expect("test tree decodes")
}

fn sources(state: &[(&str, JVal)]) -> BindingSources {
    BindingSources {
        state: state
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect::<HashMap<_, _>>(),
        ..BindingSources::default()
    }
}

/// A three-child box; the middle child carries the predicate under test.
/// Siblings on both sides deliberately: a renderer that dropped the whole
/// container, or that stopped after the hidden child, would pass a single-node
/// test.
fn wrapped(middle: &str) -> String {
    format!(
        r#"{{"id":"root","kind":{{"$type":"Box","children":[
            {{"id":"before","kind":{{"$type":"Markdown","text":"BEFORE"}}}},
            {middle},
            {{"id":"after","kind":{{"$type":"Markdown","text":"AFTER"}}}}
        ],"layout":{{"$type":"Flex","direction":"Vertical","wrap":false}},"role":"Group"}}}}"#
    )
}

const SUBJECT: &str = r#"{"id":"subject","kind":{"$type":"Markdown","text":"SUBJECT"},
    "visible":{"$type":"State","key":"shown"}}"#;

#[test]
fn visible_false_removes_the_node_entirely() {
    let html = render_to_html(
        &node(&wrapped(SUBJECT)),
        &sources(&[("shown", JVal::Bool(false))]),
    );

    assert!(!html.contains("SUBJECT"), "the text is gone: {html}");
    // Removal is not concealment: no placeholder carries the id, and nothing
    // marks the absence.
    assert!(!html.contains("subject"), "no placeholder carries the id: {html}");
    assert!(!html.contains("aria-hidden"), "removal is not concealment: {html}");
    assert!(
        html.contains("BEFORE") && html.contains("AFTER"),
        "the siblings are untouched: {html}"
    );
}

#[test]
fn visible_true_renders_the_node() {
    let html = render_to_html(
        &node(&wrapped(SUBJECT)),
        &sources(&[("shown", JVal::Bool(true))]),
    );
    assert!(html.contains("SUBJECT"), "{html}");
}

#[test]
fn visible_true_renders_exactly_as_an_unconditional_node() {
    // The slot changes PRESENCE, never appearance: byte equality against the
    // same node with no predicate at all. A renderer that emitted a marker class
    // or a wrapper for a conditionally-visible node fails here rather than
    // passing quietly.
    let with_predicate = render_to_html(
        &node(&wrapped(SUBJECT)),
        &sources(&[("shown", JVal::Bool(true))]),
    );
    let unconditional = render_to_html(
        &node(&wrapped(
            r#"{"id":"subject","kind":{"$type":"Markdown","text":"SUBJECT"}}"#,
        )),
        &BindingSources::default(),
    );

    assert_eq!(with_predicate, unconditional, "presence, never appearance");
}

#[test]
fn an_unresolved_predicate_renders_the_node() {
    // A query result the host never furnished. `Binding::State` cannot express
    // this — its own rule resolves a default-less unwritten key to `false` — so
    // the fixture reaches for `Query`, which is exactly why FUARAN143 exists.
    let html = render_to_html(
        &node(&wrapped(
            r#"{"id":"subject","kind":{"$type":"Markdown","text":"SUBJECT"},
                "visible":{"$type":"Query","name":"flags.beta"}}"#,
        )),
        &BindingSources::default(),
    );

    assert!(
        html.contains("SUBJECT"),
        "a missing source must not hide content: {html}"
    );
}

#[test]
fn a_hidden_node_takes_its_whole_subtree_with_it() {
    let html = render_to_html(
        &node(&wrapped(
            r#"{"id":"panel","kind":{"$type":"Box","children":[
                {"id":"buried","kind":{"$type":"Markdown","text":"BURIED"}}
            ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"},
            "visible":{"$type":"State","key":"shown"}}"#,
        )),
        &sources(&[("shown", JVal::Bool(false))]),
    );

    assert!(!html.contains("BURIED"), "the subtree goes too: {html}");
    assert!(
        html.contains("BEFORE") && html.contains("AFTER"),
        "the siblings do not: {html}"
    );
}

// ── predicate cases ─────────────────────────────────────────────────────────

/// A switch whose predicate case precedes a `match` case that would ALSO select.
/// This is the one shape that catches a host batching all its matches ahead of
/// all its predicates (or the reverse).
const MIXED_SWITCH: &str = r#"{"id":"sw","kind":{"$type":"Switch","cases":[
    {"child":{"id":"by-predicate","kind":{"$type":"Markdown","text":"BY PREDICATE"}},
     "when":{"$type":"State","key":"flag"}},
    {"child":{"id":"by-match","kind":{"$type":"Markdown","text":"BY MATCH"}},"match":"a"}
],"default":{"id":"sw-default","kind":{"$type":"Markdown","text":"DEFAULT"}},"stateKey":"view"}}"#;

#[test]
fn first_match_wins_runs_over_the_authored_order() {
    let html = render_to_html(
        &node(MIXED_SWITCH),
        &sources(&[("flag", JVal::Bool(true)), ("view", JVal::Str("a".into()))]),
    );

    assert!(html.contains("BY PREDICATE"), "authored order decides: {html}");
    assert!(!html.contains("BY MATCH"), "the later match does not pre-empt: {html}");
}

#[test]
fn a_match_case_wins_once_the_predicate_declines() {
    let html = render_to_html(
        &node(MIXED_SWITCH),
        &sources(&[("flag", JVal::Bool(false)), ("view", JVal::Str("a".into()))]),
    );

    assert!(html.contains("BY MATCH"), "{html}");
    assert!(!html.contains("BY PREDICATE"), "{html}");
}

#[test]
fn nothing_selecting_falls_through_to_the_default() {
    let html = render_to_html(
        &node(MIXED_SWITCH),
        &sources(&[
            ("flag", JVal::Bool(false)),
            ("view", JVal::Str("no-such-case".into())),
        ]),
    );

    assert!(html.contains("DEFAULT"), "{html}");
}

#[test]
fn a_when_only_switch_needs_no_selector() {
    let json = r#"{"id":"sw","kind":{"$type":"Switch","cases":[
        {"child":{"id":"ready","kind":{"$type":"Markdown","text":"READY"}},
         "when":{"$type":"State","key":"form.valid"}}
    ],"default":{"id":"sw-default","kind":{"$type":"Markdown","text":"NOT READY"}},"stateKey":""}}"#;

    assert!(
        render_to_html(&node(json), &sources(&[("form.valid", JVal::Bool(true))])).contains("READY"),
    );
    assert!(
        render_to_html(&node(json), &sources(&[("form.valid", JVal::Bool(false))]))
            .contains("NOT READY"),
    );
}

// ── the decode refusals ─────────────────────────────────────────────────────

#[test]
fn a_case_carrying_both_match_and_when_is_refused() {
    let json = r#"{"id":"x","kind":{"$type":"Switch","cases":[
        {"child":{"id":"c","kind":{"$type":"Markdown","text":"hi"}},
         "match":"a","when":{"$type":"State","key":"flag"}}
    ],"default":{"id":"d","kind":{"$type":"Markdown","text":"no"}},"stateKey":"view"}}"#;

    let err = decode_node(json).expect_err("both is refused");
    assert_eq!(err.path, "$.kind.cases[0].when", "{err:?}");
}

#[test]
fn a_case_carrying_neither_is_refused() {
    let json = r#"{"id":"x","kind":{"$type":"Switch","cases":[
        {"child":{"id":"c","kind":{"$type":"Markdown","text":"hi"}}}
    ],"default":{"id":"d","kind":{"$type":"Markdown","text":"no"}},"stateKey":"view"}}"#;

    let err = decode_node(json).expect_err("neither is refused");
    assert_eq!(err.path, "$.kind.cases[0].match", "{err:?}");
}
