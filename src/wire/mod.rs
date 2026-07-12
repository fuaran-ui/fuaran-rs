//! The canonical `Node` / `TreeOp` codec surface of the `fuaran-rs` host. Stage-0
//! bootstrap: the error envelope is defined; the decode/encode bodies are the
//! roadmap floor.

mod node;
mod result;

pub use node::{Node, NotImplemented, decode_node, encode_node};
pub use result::{DecodeError, DecodeErrorCode};
