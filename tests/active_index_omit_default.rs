//! Phase 1585 — `Tabs.activeIndex` is omit-at-default, and the explicit form is
//! still read.
//!
//! The sibling of `stacked_omit_default.rs`, landed one step behind it. Until
//! 1585 the encoder always emitted `activeIndex` while every host's decoder
//! already restored `Static 0` on absence — a tolerance five hosts happened to
//! share rather than a stated rule. The IDL now declares the member
//! `omitDefault Static{value=0}`, so the omission is the CONTRACT: the encoder
//! omits at that identity, the decoder restores it, and the corpus's tabs
//! fixtures carry the shorter bytes.
//!
//! The half worth testing is the one a corpus of re-emitted fixtures cannot
//! state, because every fixture carrying the identity now omits the member: that
//! a document carrying the OLD explicit `{"$type":"Static","value":0}` still
//! decodes to exactly the same tree. This is the read-compat leg, and it lives
//! here rather than in the corpus runner because it is about bytes the corpus
//! deliberately no longer contains.
//!
//! The last two cases are the ones a bool-valued member does not have. The
//! identity is one inhabitant of a union whose payload domain is unbounded, so
//! an omit test written on the tag alone would drop a document's authored tab —
//! and, for a writable binding, its write-back destination with it, leaving an
//! inert control.

use fuaran_rs::wire::{decode_node, encode_node};

const CHILD: &str = r#"{"id":"p1","kind":{"$type":"Markdown","text":"one"}}"#;

fn tabs(active_index: Option<&str>) -> String {
    match active_index {
        None => format!(r#"{{"id":"t1","kind":{{"$type":"Tabs","children":[{CHILD}]}}}}"#),
        Some(b) => format!(
            r#"{{"id":"t1","kind":{{"$type":"Tabs","activeIndex":{b},"children":[{CHILD}]}}}}"#
        ),
    }
}

#[test]
fn omitted_form_round_trips() {
    let omitted = tabs(None);
    let node = decode_node(&omitted).expect("the omitted form must decode");
    assert_eq!(
        encode_node(&node),
        omitted,
        "the canonical form carries no `activeIndex`"
    );
}

#[test]
fn explicit_identity_decodes_identically() {
    // Read-compat: the pre-phase spelling reaches the same document. Two trees
    // that encode to the same canonical bytes ARE the same document, which is
    // the property every host is held to.
    let omitted = tabs(None);
    let explicit = tabs(Some(r#"{"$type":"Static","value":0}"#));
    let before = decode_node(&explicit).expect("the pre-phase form must still decode");
    let after = decode_node(&omitted).expect("the omitted form must decode");
    assert_eq!(encode_node(&before), encode_node(&after));
    // …and the shared bytes are the SHORT ones: the explicit spelling is a §3.6
    // lenient accept, not a second canonical form.
    assert_eq!(encode_node(&before), omitted);
}

#[test]
fn a_static_carrying_another_index_is_still_carried() {
    let wire = tabs(Some(r#"{"$type":"Static","value":1}"#));
    let node = decode_node(&wire).expect("`Static 1` must decode");
    assert_eq!(
        encode_node(&node),
        wire,
        "`Static 1` differs from the identity default, so it rides the wire"
    );
}

#[test]
fn a_non_static_binding_is_still_carried() {
    // The `defaultValue` is part of the canonical spelling at an integer slot in
    // this host — a `State` binding there carries the slot's typed default, as
    // `nodes/controls-declarative.json` shows. That is unrelated to this phase;
    // what is being asserted is that the binding SURVIVES the omit test.
    let wire = tabs(Some(r#"{"$type":"State","defaultValue":0,"key":"pane"}"#));
    let node = decode_node(&wire).expect("a `State` binding must decode");
    assert_eq!(
        encode_node(&node),
        wire,
        "a writable binding is not the identity — dropping it would make the control inert"
    );
}
