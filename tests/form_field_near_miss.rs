//! The `FormField` rule-slot near-miss refusal — its didactic TEXT.
//!
//! The corpus carries `reject-formfield-near-miss-validation`, and a reject
//! fixture asserts the code and the `$`-rooted path only. The MESSAGE is
//! therefore a host-local obligation, and it is the whole point of the refusal:
//! a near miss that only says "no" sends the author looking for a spelling,
//! where naming the CONSEQUENCE says why the spelling matters. This host omitted
//! the trailing clause the reference hosts append — terser, not wrong, and
//! invisible to every gate. Pinning the whole string is what keeps the hosts
//! from drifting a word at a time.
//!
//! Two other things are pinned here because nothing else pins them: the two
//! spellings the corpus fixture does not exercise (a host wired to only the key
//! it was shown is non-conformant in a way no fixture catches), and the GRID
//! message's continued absence of a clause — the shared helper takes the
//! consequence as a parameter, so adding one to the form field is exactly the
//! edit that could add one to the grid by accident, and four other hosts pin
//! the grid's wording unchanged.

use fuaran_rs::wire::{DecodeErrorCode, decode_node};

/// The smallest form document carrying one field, with the field's members
/// spliced in. Canonical key order is irrelevant here — nothing re-encodes.
fn form(field_members: &str) -> String {
    format!(
        r#"{{"id":"f1","kind":{{"$type":"Form","fields":[{{"id":"email","kind":{{"$type":"Text"}},"label":"Work email",{field_members}}}],"onSubmit":{{"$type":"Dispatch"}},"submitLabel":"Save"}}}}"#
    )
}

#[test]
fn the_near_miss_refusal_names_what_the_silence_costs() {
    // All three rejected spellings, not only the one the corpus fixture carries.
    for spelling in ["validation", "constraints", "validate"] {
        let err = decode_node(&form(&format!(r#""{spelling}":{{}}"#))).expect_err(
            "the near-miss narrowing is not reaching this key — it decoded and constrains nothing",
        );
        assert_eq!(err.code, DecodeErrorCode::WrongType, "{spelling}");
        assert_eq!(
            err.path,
            format!("$.kind.fields[0].{spelling}"),
            "{spelling}"
        );
        assert_eq!(
            err.message,
            format!(
                "'{spelling}' is not part of the form field vocabulary — it would be ignored, \
                 not honoured, and the field would accept anything"
            ),
            "the didactic drifted from the reference hosts' wording"
        );
        assert_eq!(err.expected_shape.as_deref(), Some("rule"), "{spelling}");
    }
}

/// The grid's message carries NO trailing clause, and that is deliberate rather
/// than unfinished — the other hosts pin these bytes. It lives in this file
/// because the parameter that carries the form field's clause is the grid's too.
#[test]
fn the_grid_near_miss_message_stays_clause_free() {
    let err = decode_node(
        r#"{"id":"g1","kind":{"$type":"DataGrid","columns":[],"currentPage":2,"source":{"$type":"Static","value":[]}}}"#,
    )
    .expect_err("the grid near-miss narrowing is not reaching `currentPage`");
    assert_eq!(err.code, DecodeErrorCode::WrongType);
    assert_eq!(
        err.message,
        "'currentPage' is not part of the grid vocabulary — it would be ignored, not honoured"
    );
}
