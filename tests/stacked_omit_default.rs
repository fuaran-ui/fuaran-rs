//! Phase 1585 — `Chart.stacked` is omit-at-default, and the explicit form is
//! still read.
//!
//! Until 1585 the encoder always emitted `stacked` while every host's decoder
//! already restored `false` on absence — a tolerance five hosts happened to
//! share rather than a stated rule. The IDL now declares the member
//! `omitDefault false`, so the omission is the CONTRACT: the encoder omits at
//! `false`, the decoder restores `false`, and the corpus's chart fixtures carry
//! the shorter bytes.
//!
//! The half worth testing is the one a corpus of re-emitted fixtures cannot
//! state, because every fixture in it now omits the member: that a document
//! carrying the OLD explicit `"stacked": false` still decodes to exactly the
//! same tree. This is the read-compat leg, and it lives here rather than in the
//! corpus runner because it is about bytes the corpus deliberately no longer
//! contains.

use fuaran_rs::wire::{decode_node, encode_node};

const OMITTED: &str = r#"{"id":"c1","kind":{"$type":"Chart","kind":"Bar","source":{"$type":"Static","value":[]},"xField":"quarter","yFields":["revenue"]}}"#;

const EXPLICIT_FALSE: &str = r#"{"id":"c1","kind":{"$type":"Chart","kind":"Bar","source":{"$type":"Static","value":[]},"stacked":false,"xField":"quarter","yFields":["revenue"]}}"#;

const EXPLICIT_TRUE: &str = r#"{"id":"c1","kind":{"$type":"Chart","kind":"Bar","source":{"$type":"Static","value":[]},"stacked":true,"xField":"quarter","yFields":["revenue"]}}"#;

#[test]
fn omitted_form_round_trips() {
    let node = decode_node(OMITTED).expect("the omitted form must decode");
    assert_eq!(
        encode_node(&node),
        OMITTED,
        "the canonical form carries no `stacked`"
    );
}

#[test]
fn explicit_false_decodes_identically() {
    // Read-compat: the pre-phase spelling reaches the same document. Two trees
    // that encode to the same canonical bytes ARE the same document, which is
    // the property every host is held to.
    let before = decode_node(EXPLICIT_FALSE).expect("the pre-phase form must still decode");
    let after = decode_node(OMITTED).expect("the omitted form must decode");
    assert_eq!(encode_node(&before), encode_node(&after));
    // …and the shared bytes are the SHORT ones: the explicit spelling is a §3.6
    // lenient accept, not a second canonical form.
    assert_eq!(encode_node(&before), OMITTED);
}

#[test]
fn true_is_still_carried() {
    let node = decode_node(EXPLICIT_TRUE).expect("`true` must decode");
    assert_eq!(
        encode_node(&node),
        EXPLICIT_TRUE,
        "`true` differs from the identity default, so it rides the wire"
    );
}
