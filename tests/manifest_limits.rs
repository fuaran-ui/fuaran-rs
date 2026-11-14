//! Phase 2721 — the theme-manifest decoder reads under `WIRE_FORMAT.md` §21.1's
//! syntactic limits: JSON depth, string length, array length, object members and
//! document bytes (the node bounds do not apply: a manifest has no nodes). Each
//! bound is pinned twice — a manifest AT it decodes, a manifest ONE PAST it is
//! refused as `LIMIT_EXCEEDED` at `$` (§21.2 rules 1 and 2) — so neither an
//! off-by-one that refuses a conformant manifest nor one that admits a hostile
//! one can pass.

use fuaran_rs::limits::{MAX_ARRAY_LENGTH, MAX_DOCUMENT_BYTES, MAX_JSON_DEPTH, MAX_STRING_LENGTH};
use fuaran_rs::theme::manifest::{decode, try_decode};
use fuaran_rs::wire::DecodeErrorCode;

/// A DTCG tree `depth` objects deep with a scalar at the bottom. Only `{` and
/// `[` count toward §21.1's JSON depth, so the scalar adds nothing.
fn nested_objects(depth: usize) -> String {
    format!(
        "{}{{\"a\":1}}{}",
        "{\"a\":".repeat(depth - 1),
        "}".repeat(depth - 1)
    )
}

fn token_with_value(value: &str) -> String {
    format!("{{\"t\":{{\"$type\":\"color\",\"$value\":\"{value}\"}}}}")
}

fn manifest_with_roles(n: usize) -> String {
    let items = vec!["0"; n].join(",");
    format!("{{\"roles\":[{items}],\"tokens\":{{}}}}")
}

fn tree_with_members(n: usize) -> String {
    let members: Vec<String> = (0..n).map(|i| format!("\"k{i}\":0")).collect();
    format!("{{{}}}", members.join(","))
}

/// A manifest exactly `n` UTF-8 bytes long: a small tree, then insignificant
/// whitespace, so no other bound is near its edge.
fn document_of_bytes(n: usize) -> String {
    let head = "{\"t\":{\"$type\":\"color\",\"$value\":\"#000000\"}}";
    format!("{head}{}", " ".repeat(n - head.len()))
}

fn assert_edge(name: &str, at_bound: &str, past_it: &str, mentions: &str) {
    let at = try_decode(at_bound);
    assert!(
        at.is_ok(),
        "{name}: a manifest AT the bound was refused (§21.2 rule 1): {at:?}"
    );
    let e = try_decode(past_it).expect_err(&format!("{name}: one past the bound decoded"));
    assert_eq!(e.code, DecodeErrorCode::LimitExceeded, "{name}: {e}");
    assert_eq!(e.path, "$", "{name}: {e}");
    assert!(
        e.message.contains(mentions),
        "{name}: the refusal should name {mentions}, got {:?}",
        e.message
    );
    // The Option form is the same read: it refuses exactly what try_decode refuses.
    assert!(
        decode(past_it).is_none(),
        "{name}: decode admitted what try_decode refused"
    );
    assert!(
        decode(at_bound).is_some(),
        "{name}: decode refused what try_decode admitted"
    );
}

#[test]
fn json_depth_at_the_bound_and_one_past() {
    assert_edge(
        "JSON depth",
        &nested_objects(MAX_JSON_DEPTH),
        &nested_objects(MAX_JSON_DEPTH + 1),
        "MAX_JSON_DEPTH",
    );
}

#[test]
fn string_length_at_the_bound_and_one_past() {
    assert_edge(
        "string length",
        &token_with_value(&"x".repeat(MAX_STRING_LENGTH)),
        &token_with_value(&"x".repeat(MAX_STRING_LENGTH + 1)),
        "MAX_STRING_LENGTH",
    );
}

#[test]
fn string_length_is_counted_in_code_points() {
    // §21.6: an astral character is ONE code point and four UTF-8 bytes.
    let astral = "\u{1D11E}".repeat(MAX_STRING_LENGTH);
    assert!(try_decode(&token_with_value(&astral)).is_ok());
}

#[test]
fn array_length_at_the_bound_and_one_past() {
    assert_edge(
        "array length",
        &manifest_with_roles(MAX_ARRAY_LENGTH),
        &manifest_with_roles(MAX_ARRAY_LENGTH + 1),
        "MAX_ARRAY_LENGTH",
    );
}

#[test]
fn object_members_at_the_bound_and_one_past() {
    assert_edge(
        "object members",
        &tree_with_members(MAX_ARRAY_LENGTH),
        &tree_with_members(MAX_ARRAY_LENGTH + 1),
        "MAX_ARRAY_LENGTH",
    );
}

#[test]
fn document_bytes_at_the_bound_and_one_past() {
    assert_edge(
        "document bytes",
        &document_of_bytes(MAX_DOCUMENT_BYTES),
        &document_of_bytes(MAX_DOCUMENT_BYTES + 1),
        "33554432",
    );
}

#[test]
fn malformed_manifest_text_is_invalid_json_not_a_limit() {
    let e = try_decode("{\"tokens\":").expect_err("truncated JSON decoded");
    assert_eq!(e.code, DecodeErrorCode::InvalidJson, "{e}");
}
