//! Phase 687 — the CLOSE of the migration window Phase 681 opened.
//!
//! 0.4.0 removed the ordinal from `InsertChild` and `MoveNode`: both append, and
//! `ReorderChildren` states order by naming child ids. Through the window every
//! decoder ACCEPTED AND IGNORED a legacy `position` / `newPosition` so the hosts
//! could adopt independently. Every host is now positionless and no emitter
//! produces the field, so the tolerance is withdrawn: it is a decode error,
//! named at its own path.
//!
//! The corpus fixtures (`reject-op-insertchild-retired-position`,
//! `reject-op-movenode-retired-newposition`) certify code + path. These add the
//! two things the corpus deliberately cannot: the didactic text — op-side reject
//! fixtures assert code and path only — and the cross-host ORDERING guarantee,
//! which no single fixture can express because its payload is well-formed apart
//! from the retired field by design.

use fuaran_rs::wire::{DecodeErrorCode, decode_op, encode_op};

#[test]
fn retired_position_refused_by_name() {
    for (raw, want_path) in [
        (
            r#"{"$type":"InsertChild","child":{"id":"n","kind":{"$type":"Markdown","text":"x"}},"parentId":"p","position":3}"#,
            "$.position",
        ),
        (
            r#"{"$type":"MoveNode","newParentId":"q","newPosition":2,"target":"n"}"#,
            "$.newPosition",
        ),
    ] {
        let err = decode_op(raw)
            .expect_err("the retired field was accepted — the migration window is closed");
        assert_eq!(err.code, DecodeErrorCode::WrongType, "input: {raw}");
        assert_eq!(err.path, want_path, "the error must name the retired field");
        // The didactic names what to reach for instead. A refusal that only says
        // "no" sends the author looking for a spelling.
        assert!(
            err.message.contains("ReorderChildren"),
            "message does not name ReorderChildren: {}",
            err.message
        );
    }
}

/// The retired field is named AHEAD of any other defect in the same op. Without
/// this ordering an author who also omitted a required field would fix that and
/// meet this one only on the next run. Fixed identically across all five hosts,
/// so which defect surfaces first is deterministic.
#[test]
fn retired_position_outranks_a_missing_required_field() {
    let err = decode_op(r#"{"$type":"InsertChild","position":0}"#)
        .expect_err("an op missing parentId AND carrying position decoded");
    assert_eq!(
        err.path, "$.position",
        "the retired field wins over the missing required field"
    );
}

/// The positionless form still decodes and re-encodes as the identity.
#[test]
fn positionless_form_round_trips() {
    const CURRENT: &str = r#"{"$type":"MoveNode","newParentId":"q","target":"n"}"#;
    let op = decode_op(CURRENT).expect("canonical MoveNode refused");
    assert_eq!(encode_op(&op), CURRENT);
}
