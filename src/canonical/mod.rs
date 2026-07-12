//! Canonical-JSON encoding primitives the Fuaran UI wire format requires: number
//! form, key sort, and string escaping (`WIRE_FORMAT.md` §2), plus the hand-rolled
//! JSON value model + parser the decoder consumes. The number form is the
//! make-or-break of any host (§5) — it was the first brick `fuaran-rs` shipped.

mod float;
mod json;

pub use float::format_finite_double;
pub use json::{
    JVal, ParseError, escape_string, format_number, ordinal_cmp, parse, render_array,
    render_canonical, render_object,
};
