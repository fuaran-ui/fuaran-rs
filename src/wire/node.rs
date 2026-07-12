//! The typed UI tree node + the codec entry points. Stage-0 bootstrap: the node
//! carries only its stable identity; the closed per-kind model and the codec
//! bodies land with the roadmap floor.

use std::error::Error;
use std::fmt;

/// Marks the codec surface the stage-0 bootstrap declares but has not yet built.
/// The node/op codec is the roadmap "floor" tier; see `CLAUDE.md` and the fuaran
/// roadmap `fuaran-rs` bootstrap phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotImplemented;

impl fmt::Display for NotImplemented {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fuaran-rs: not implemented (stage-0 bootstrap; codec is roadmap floor work)")
    }
}

impl Error for NotImplemented {}

/// The typed UI tree node. The stage-0 bootstrap carries only the stable identity
/// field; the closed per-kind model lands with the codec.
///
/// `NodeKind` (the closed `$type` discriminator) will be modelled as a native Rust
/// `enum`, one variant per case — Rust recovers the compile-time exhaustiveness on
/// the per-kind `match` that the Go host had to trade away, so a newly-added kind
/// that misses a codec arm is a *build* error here, not a corpus-only catch (see
/// `CLAUDE.md`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
    pub id: String,
    // pub kind: NodeKind — not yet defined.
}

/// Decodes canonical wire JSON into a [`Node`]. Stub — see [`NotImplemented`].
///
/// The real signature will return the structured
/// [`DecodeError`](super::DecodeError) for malformed input; stage-0 returns
/// [`NotImplemented`] because no body exists yet.
pub fn decode_node(_canonical_json: &str) -> Result<Node, NotImplemented> {
    Err(NotImplemented)
}

/// Re-encodes a [`Node`] to canonical wire JSON, byte-identical to the shared
/// corpus. Stub — see [`NotImplemented`].
pub fn encode_node(_node: &Node) -> Result<String, NotImplemented> {
    Err(NotImplemented)
}
