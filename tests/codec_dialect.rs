//! Phase 2333: the generated codec and the dialect's codec hooks.
//!
//! The generator writes the uniform half of the codec (`gen_encode` /
//! `gen_decode` in `src/wire/generated.rs`); every policy position is a hook the
//! dialect declares. These tests hold the boundary: a plain field the IDL gains
//! is coded with no hand edit, a position no rule covers must be declared, and a
//! hook that names nothing or that the rules now cover is refused.

mod common;

fn corpus_idl() -> String {
    let corpus = common::corpus_root();
    std::fs::read_to_string(corpus.join("idl.json")).unwrap_or_else(|e| {
        panic!(
            "the corpus at '{}' holds no idl.json: {e}",
            corpus.display()
        )
    })
}

fn generate(idl: &str) -> Result<String, String> {
    fuaran_rs::wire::idl_codegen::generate(idl)
}

/// The first occurrence of `from` at or after the first occurrence of `after`,
/// replaced by `to`.
fn replace_after(idl: &str, after: &str, from: &str, to: &str) -> String {
    let start = idl
        .find(after)
        .unwrap_or_else(|| panic!("the probe's anchor is gone from idl.json: {after}"));
    let at = start
        + idl[start..]
            .find(from)
            .unwrap_or_else(|| panic!("no '{from}' after '{after}' in idl.json"));
    format!("{}{to}{}", &idl[..at], &idl[at + from.len()..])
}

/// A plain optional field reaches both halves of the generated codec: the
/// encoder writes it when present and the decoder reads it, so the field costs
/// no hand edit in the codec and cannot be silently left off the wire.
#[test]
fn a_plain_field_the_idl_gains_is_coded_by_the_generator() {
    let idl = corpus_idl();
    let moved = idl.replacen(
        "\"name\": \"label\",",
        "\"name\": \"probeField\", \"optionality\": {\"$type\": \"optional\"}, \"type\": {\"$type\": \"str\"}}, {\"name\": \"label\",",
        1,
    );
    assert_ne!(moved, idl, "the probe's anchor is gone from idl.json");
    let module = generate(&moved).unwrap_or_else(|e| panic!("the generator refused: {e}"));
    let (encode, decode) = module
        .split_once("pub(super) mod gen_decode")
        .expect("the generated module carries the decoder");
    assert!(
        encode.contains("f.push(field(\"probeField\", s(x)));"),
        "the generated encoder does not write the new field"
    );
    assert!(
        decode.contains("field_opt(path, fields, \"probeField\", &[], dec_string)"),
        "the generated decoder does not read the new field"
    );
}

/// A position with no uniform rule and no hook is refused, naming it — never
/// emitted as a guess.
#[test]
fn a_position_no_rule_covers_must_be_declared() {
    let idl = corpus_idl();
    let moved = idl.replacen(
        "\"name\": \"label\",",
        "\"name\": \"probeVar\", \"optionality\": {\"$type\": \"optional\"}, \"type\": {\"$type\": \"var\", \"name\": \"T\"}}, {\"name\": \"label\",",
        1,
    );
    let refusal = generate(&moved).expect_err("an uncoded position was generated");
    assert!(
        refusal.contains("encode Badge.probeVar") && refusal.contains("decode Badge.probeVar"),
        "the refusal does not name the uncovered position: {refusal}"
    );
}

/// The dialect's codec hooks are checked against the IDL: a hook naming a field
/// the IDL no longer has is refused, and so is a `Shape` hook the uniform rules
/// now cover. The unperturbed IDL must generate, so each refusal is the
/// perturbation's.
#[test]
fn the_generator_refuses_a_stale_codec_hook() {
    let idl = corpus_idl();
    generate(&idl).unwrap_or_else(|e| panic!("the corpus idl.json does not generate: {e}"));

    // `SrcSetEntry.width` renamed: its decode hook now names nothing. The record's
    // fields precede its name (keys are sorted) and its definition is the last
    // mention of the name, so the rename anchors on the last `width` before it.
    let record = idl
        .rfind("\"name\": \"SrcSetEntry\"")
        .expect("SrcSetEntry is in the IDL");
    let width = idl[..record]
        .rfind("\"name\": \"width\"")
        .expect("SrcSetEntry carries a width");
    let renamed = format!(
        "{}\"name\": \"widthPx\"{}",
        &idl[..width],
        &idl[width + "\"name\": \"width\"".len()..]
    );
    let refusal = generate(&renamed).expect_err("a hook naming no field was accepted");
    assert!(
        refusal.contains("SrcSetEntry.width") && refusal.contains("delete it"),
        "the refusal does not name the stale hook: {refusal}"
    );

    // `Tabs.activeIndex` made a plain optional binding: the uniform rules now code
    // it, so its `Shape` hook is stale.
    let plain = replace_after(
        &idl,
        "\"name\": \"activeIndex\"",
        "\"$type\": \"omitDefault\"",
        "\"$type\": \"optional\"",
    );
    let refusal = generate(&plain).expect_err("a stale Shape hook was accepted");
    assert!(
        refusal.contains("Tabs.activeIndex is stale"),
        "the refusal does not name the stale hook: {refusal}"
    );
}
