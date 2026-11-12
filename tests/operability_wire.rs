//! Phase 2517: the corpus's `operability/` family, wire section (WIRE_FORMAT §31.1, §31.2).
//!
//! This host's codec carries the `affordance` member on `Select`, `Button` and every form field,
//! and refuses a pattern the carrying kind's slot rule forbids. The family's `wire.roundTrip`
//! vectors must re-encode byte for byte and its `wire.reject` vectors must refuse with the recorded
//! code at the recorded path. The derivation and the matcher (§31.3–31.5) are not ported, which is
//! why the capability manifest declares the family pending rather than covered.

mod common;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::wire::{decode_node, encode_node};

fn vectors() -> JVal {
    let file = common::corpus_root()
        .join("operability")
        .join("operability-vectors.json");
    let raw = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("reading {}: {e}", file.display()));
    parse(&raw).expect("operability-vectors.json parses with the host's own JSON layer")
}

fn section(vectors: &JVal, name: &str) -> Vec<JVal> {
    let Some(JVal::Obj(_)) = vectors.field("wire") else {
        panic!("the family declares no wire section");
    };
    match vectors.field("wire").and_then(|w| w.field(name)) {
        Some(JVal::Arr(items)) => items.clone(),
        _ => panic!("the wire section declares no {name} array"),
    }
}

fn text(v: &JVal, key: &str) -> String {
    match v.field(key) {
        Some(JVal::Str(s)) => s.clone(),
        _ => panic!("vector member {key} is not a string"),
    }
}

#[test]
fn the_family_carries_both_wire_sections() {
    let v = vectors();
    assert!(!section(&v, "roundTrip").is_empty());
    assert!(!section(&v, "reject").is_empty());
}

#[test]
fn every_round_trip_vector_re_encodes_byte_for_byte() {
    for vector in section(&vectors(), "roundTrip") {
        let id = text(&vector, "id");
        let canonical = text(&vector, "canonical");
        let node = decode_node(&canonical).unwrap_or_else(|e| panic!("{id}: refused: {e:?}"));
        assert_eq!(encode_node(&node), canonical, "{id}");
    }
}

#[test]
fn every_reject_vector_refuses_with_the_recorded_code_and_path() {
    for vector in section(&vectors(), "reject") {
        let id = text(&vector, "id");
        let error = decode_node(&text(&vector, "input")).expect_err(&id);
        assert_eq!(
            error.code.to_string(),
            text(&vector, "expectedErrorCode"),
            "{id}"
        );
        let want = text(&vector, "expectedPath");
        assert!(
            error.path.starts_with(&want),
            "{id}: {} vs {want}",
            error.path
        );
    }
}
