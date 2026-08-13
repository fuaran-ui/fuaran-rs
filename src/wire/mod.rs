//! The canonical `Node` / `TreeOp` codec of the `fuaran-rs` host: the typed
//! wire model ([`model`]), the structural decoder ([`decode_node`] /
//! [`decode_op`] — six-code [`DecodeError`] envelope, `$`-rooted paths), and
//! the byte-stable canonical encoder ([`encode_node`] / [`encode_op`]).
//! Certified against the shared conformance corpus by `tests/conformance.rs`.

mod decode;
mod encode;
mod model;
mod result;

pub(crate) use decode::coerce;
pub(crate) use decode::live_data_source;
pub use decode::{decode_node, decode_op};
pub use encode::{encode_node, encode_op};
pub use model::*;
pub use result::{DecodeError, DecodeErrorCode};
