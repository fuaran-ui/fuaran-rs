//! `WIRE_FORMAT.md` §20 decode determinism, and §7.1's integer-slot accept set.
//!
//! The corpus pins each §20 row with a reject fixture and `conformance.rs` runs
//! them. These are the assertions the corpus cannot carry: the CORRECTED TWIN
//! beside each refusal — a refusal-only suite passes on a decoder that refuses
//! everything — and the two rows whose divergence on this host was silent.
//!
//! This host answered four of the eight rows differently from the reference, and
//! could not see it from the inside: a lone surrogate became U+FFFD, a repeated
//! member took the last occurrence, content after the root value was ignored
//! outright, and the number grammar was Rust's rather than RFC 8259's.

use fuaran_rs::wire::decode_node;

fn refused(doc: &str) -> String {
    match decode_node(doc) {
        Ok(_) => panic!("expected {doc:.80} to be refused, but it decoded"),
        Err(e) => e.code.as_str().to_string(),
    }
}

fn accepted(doc: &str) {
    let r = decode_node(doc);
    assert!(r.is_ok(), "expected {doc:.80} to decode, got {r:?}");
}

fn markdown(text: &str) -> String {
    format!(r#"{{"id":"markdown-1","kind":{{"$type":"Markdown","text":"{text}"}}}}"#)
}

fn skeleton(rows: &str) -> String {
    format!(r#"{{"id":"skel-1","kind":{{"$type":"Skeleton","rows":{rows}}}}}"#)
}

// ── row 1: a repeated member ────────────────────────────────────────────────

#[test]
fn repeated_member_is_refused() {
    assert_eq!(
        refused(r#"{"id":"markdown-1","id":"smuggled","kind":{"$type":"Markdown","text":"x"}}"#),
        "INVALID_JSON"
    );
}

#[test]
fn repeated_member_is_refused_nested_too() {
    // The row binds every object, not the root one.
    assert_eq!(
        refused(r#"{"id":"n","kind":{"$type":"Markdown","text":"x","text":"y"}}"#),
        "INVALID_JSON"
    );
}

#[test]
fn the_same_key_in_sibling_objects_is_not_a_repeat() {
    // Non-vacuity: two objects each carrying "id" is the ordinary shape of every
    // tree, and a key set that was not scoped to its own object would refuse it.
    accepted(
        r#"{"id":"n","kind":{"$type":"Box","role":"Group","layout":{"$type":"Flex","direction":"Vertical","wrap":false},"children":[{"id":"a","kind":{"$type":"Markdown","text":"x"}},{"id":"b","kind":{"$type":"Markdown","text":"y"}}]}}"#,
    );
}

// ── row 2: content after the root value ─────────────────────────────────────

#[test]
fn content_after_the_root_value_is_refused() {
    assert_eq!(
        refused(
            r#"{"id":"markdown-1","kind":{"$type":"Markdown","text":"x"}} {"id":"second"}"#
        ),
        "INVALID_JSON"
    );
    // A surplus closing brace is the same row.
    assert_eq!(
        refused(r#"{"id":"markdown-1","kind":{"$type":"Markdown","text":"x"}}}"#),
        "INVALID_JSON"
    );
}

#[test]
fn trailing_whitespace_is_not_trailing_content() {
    accepted(&format!("  {}\n\t ", markdown("x")));
}

// ── row 3: the RFC 8259 number grammar ──────────────────────────────────────

#[test]
fn numbers_outside_the_rfc_8259_grammar_are_refused() {
    // Every one of these is accepted by at least one platform's float parser —
    // Rust's own accepts `+1`, `.5`, `1.` and `01` — which is why the grammar is
    // checked BEFORE the parse rather than delegated to it.
    for lit in ["+3", "03", "3.", "3e", "3e+", "0x10", "1.2.3"] {
        assert_eq!(refused(&skeleton(lit)), "INVALID_JSON", "for the token {lit}");
    }
    assert_eq!(refused(r#"{"id":"m","kind":{"$type":"Markdown","text":"x"},"tooltip":.5}"#), "INVALID_JSON");
}

#[test]
fn the_grammar_still_admits_every_well_formed_number() {
    // The corrected twins. A grammar check written too tightly refuses these,
    // and no reject fixture would notice.
    for lit in ["3", "0", "-3", "2147483647", "-2147483648"] {
        accepted(&skeleton(lit));
    }
}

#[test]
fn row_7_overflowing_exponent_is_still_accepted() {
    // The one row that ratifies an ACCEPT, sitting beside row 4 which refuses
    // the same three values written as bare literals. `1e999` is a well-formed
    // JSON number whose value is not representable, and IEEE-754 already says
    // what a finite decimal that overflows becomes.
    accepted(r#"{"id":"m","kind":{"$type":"Metric","label":"x","value":{"$type":"Static","value":1e999}}}"#);
}

// ── row 4: bare NaN / Infinity ──────────────────────────────────────────────

#[test]
fn bare_non_finite_literals_are_refused() {
    for lit in ["NaN", "Infinity", "-Infinity", "inf", "nan"] {
        assert_eq!(
            refused(&format!(
                r#"{{"id":"m","kind":{{"$type":"Markdown","text":"x"}},"tooltip":{lit}}}"#
            )),
            "INVALID_JSON",
            "for the bare literal {lit}"
        );
    }
}

#[test]
fn quoted_sentinels_are_untouched() {
    // §7's specified representation for a non-finite, at a float slot. Row 4
    // refuses the BARE spelling and nothing else.
    accepted(
        r#"{"id":"m","kind":{"$type":"Metric","label":"x","value":{"$type":"Static","value":"NaN"}}}"#,
    );
}

// ── row 5: a raw C0 control character ───────────────────────────────────────

#[test]
fn a_raw_control_character_inside_a_string_is_refused() {
    assert_eq!(refused(&markdown("Updated\thourly.")), "INVALID_JSON");
    assert_eq!(refused(&markdown("Updated\u{0000}hourly.")), "INVALID_JSON");
}

#[test]
fn the_escaped_spelling_of_a_control_character_is_accepted() {
    accepted(&markdown(r"Updated\thourly."));
}

// ── row 6: unpaired surrogates ──────────────────────────────────────────────

#[test]
fn unpaired_surrogate_escapes_are_refused() {
    for text in [
        r"Updated \ud83d hourly.",
        r"Updated \ude00 hourly.",
        r"\ud83d Updated \ude00 hourly.",
        // A high half followed by a NON-low \u escape: a check written as "a
        // high must be followed by another \u escape" passes this one and
        // leaves the class open.
        r"\ud83dA",
    ] {
        assert_eq!(refused(&markdown(text)), "INVALID_JSON", "for {text}");
    }
}

#[test]
fn a_well_formed_surrogate_pair_still_decodes() {
    // The corrected twin, and the case that matters most: this host used to
    // lower a lone half to U+FFFD, so refusing every escape would have looked
    // like a fix.
    accepted(&markdown(r"Updated 😀 hourly."));
}

#[test]
fn an_astral_character_written_literally_still_decodes() {
    accepted(&markdown("Updated \u{1F600} hourly."));
}

// ── §7.1: the integer-slot accept set ───────────────────────────────────────

#[test]
fn an_integral_float_spelling_decodes_at_an_integer_slot() {
    // `3.0` and `3` denote the same integer.
    accepted(&skeleton("3.0"));
}

#[test]
fn a_fraction_is_refused_at_an_integer_slot() {
    // Not truncated to 2. Truncating discards the author's value at a slot the
    // author chose to type as an integer, which §7.1 retires rather than
    // deprecates — there is no lenient profile under which it returns.
    assert_eq!(refused(&skeleton("2.5")), "WRONG_TYPE");
}

#[test]
fn a_value_outside_the_slot_width_is_refused() {
    // The measured row: the cast was implementation-defined, so the same bytes
    // became Int32.MinValue on one runtime and 1410065408 on another.
    for lit in ["1e10", "3000000000", "-3000000000", "1e400"] {
        assert_eq!(refused(&skeleton(lit)), "WRONG_TYPE", "for {lit}");
    }
}

#[test]
fn a_sentinel_string_is_refused_at_an_integer_slot() {
    // §7 widens FLOAT slots to the three sentinels and nothing else. An integer
    // has no non-finite form, so this is the case that makes the two accept sets
    // distinguishable rather than merely stated.
    assert_eq!(refused(&skeleton(r#""NaN""#)), "WRONG_TYPE");
    assert_eq!(refused(&skeleton("true")), "WRONG_TYPE");
}

// ── §2 rule 5: integer identity, and why this host needs no integer type ────

#[test]
fn integers_within_the_identity_range_re_encode_in_the_integer_layout() {
    // This host parses every number as an f64, and §2 rule 5 makes that
    // conformant rather than a limitation: the bound was chosen because the
    // integer and float canonical layouts AGREE exactly over ±(2^53−1). A double
    // holds every integer in that range, and rule 5's fixed-point window
    // (base-10 exponent ≤ 16) covers every one of them — so re-encoding produces
    // the integer spelling with no integer type in play.
    //
    // This asserts the property rather than assuming it, because it is what the
    // absence of an i64 arm here rests on.
    for lit in ["0", "1", "-1", "9007199254740991", "-9007199254740991", "42"] {
        let doc = format!(
            r#"{{"id":"c","kind":{{"$type":"Custom","componentId":"x","moduleId":"m","props":{{"n":{lit}}}}}}}"#
        );
        let node = decode_node(&doc).unwrap_or_else(|e| panic!("{lit} should decode: {e:?}"));
        let out = fuaran_rs::wire::encode_node(&node);
        assert!(
            out.contains(&format!(r#""n":{lit}"#)),
            "expected {lit} to re-encode in the integer layout, got {out}"
        );
    }
}
