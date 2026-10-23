//! Canonical-JSON encoding primitives the Fuaran UI wire format requires: number
//! form, key sort, and string escaping (`WIRE_FORMAT.md` §2), plus the JSON value
//! model and reader the decoder consumes.
//!
//! The layer is the Core spine's (`fuaran-core`, Phase 463): the value model, the
//! strict reader with the §20 decode-determinism rows, the canonical writer and
//! the number form are re-exported from it under the paths this module has always
//! published. What stays here is the UI wire's own half — its §21 limit VALUES and
//! the [`ParseError`] envelope a breach of them is reported in.

pub use fuaran_core::json::{
    JVal, escape_string, ordinal_cmp, render_array, render_canonical, render_object,
};
// The number form is a Core twin, so it lives behind the Core boundary and is
// re-exported here under its published paths.
pub use crate::core::number::{format_finite_double, format_number};

use fuaran_core::json::{JsonLimit, JsonLimits, JsonRefusal, read};

/// Structural parse failure carrying the byte offset where it was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
    /// True when this failure is a §21 resource-limit breach rather than a
    /// syntax error. §21.2 rule 2 forbids reporting a limit breach as
    /// `INVALID_JSON` — the input is well-formed and merely too large to walk,
    /// and calling it malformed sends the author to repair the wrong thing.
    /// `decode_node` reads this to choose between the two codes.
    pub limit: bool,
}

/// The §21.1 bounds this host's reader enforces on the way down (§21.2 rule
/// 4). An object's members are bounded by `MAX_ARRAY_LENGTH`, as they always
/// were here; the document size is checked by the decoder, before it reads.
const UI_LIMITS: JsonLimits = JsonLimits {
    depth: crate::limits::MAX_JSON_DEPTH,
    string_length: Some(crate::limits::MAX_STRING_LENGTH),
    array_length: Some(crate::limits::MAX_ARRAY_LENGTH),
    object_members: Some(crate::limits::MAX_ARRAY_LENGTH),
    document_bytes: None,
};

/// Parse a JSON document: exactly ONE top-level value, and nothing after it
/// (§20.2 row 2), every §20.2 row refused and every §21.1 bound enforced on the
/// way down.
pub fn parse(input: &str) -> Result<JVal, ParseError> {
    read(input, &UI_LIMITS).map_err(|r| match r {
        JsonRefusal::Malformed(e) => ParseError {
            message: e.message,
            offset: e.offset,
            limit: false,
        },
        JsonRefusal::LimitExceeded(b) => {
            let message = match b.limit {
                JsonLimit::Depth => {
                    format!(
                        "JSON nesting deeper than the wire limit MAX_JSON_DEPTH = {}",
                        b.value
                    )
                }
                JsonLimit::StringLength => format!(
                    "a string is longer than the wire limit MAX_STRING_LENGTH = {}",
                    b.value
                ),
                JsonLimit::ArrayLength => format!(
                    "an array is longer than the wire limit MAX_ARRAY_LENGTH = {}",
                    b.value
                ),
                JsonLimit::ObjectMembers => format!(
                    "an object has more members than the wire limit MAX_ARRAY_LENGTH = {}",
                    b.value
                ),
                JsonLimit::DocumentBytes => format!(
                    "the document is larger than the wire limit MAX_DOCUMENT_BYTES = {}",
                    b.value
                ),
            };
            ParseError {
                message,
                offset: b.offset,
                limit: true,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bounds_a_string_in_code_points() {
        // §21.6 — the unit is the code point, so an astral character costs ONE.
        // Counting UTF-8 bytes would charge it four, and it is the at-the-limit
        // astral case that then fails: the over-limit one passes under every
        // candidate unit, so a suite carrying only the refusal never notices.
        let at_limit = "\u{1D11E}".repeat(crate::limits::MAX_STRING_LENGTH);
        assert!(parse(&format!("\"{at_limit}\"")).is_ok());
        let over = "\u{1D11E}".repeat(crate::limits::MAX_STRING_LENGTH + 1);
        let e = parse(&format!("\"{over}\"")).unwrap_err();
        assert!(
            e.limit,
            "a limit breach must not be reported as a syntax error"
        );
        assert!(e.message.contains("MAX_STRING_LENGTH"));
    }

    #[test]
    fn a_malformed_text_is_not_a_breach() {
        let e = parse("{\"a\":1,\"a\":2}").unwrap_err();
        assert!(!e.limit);
    }
}
