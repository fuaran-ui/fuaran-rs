//! Canonical-JSON encoding primitives the Fuaran UI wire format requires: number
//! form, key sort, and string escaping. The number form is the make-or-break of
//! any host (`WIRE_FORMAT.md` §5) — this is the first brick `fuaran-rs` ships.

mod float;

pub use float::format_finite_double;
