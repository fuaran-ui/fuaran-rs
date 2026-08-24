//! The canonical decode-error envelope. Every conformant host surfaces the same
//! six codes (with a `$`-rooted path) for a malformed input, so the reject corpus
//! is host-neutral.

use std::fmt;

/// One of the six canonical decode-error codes the wire contract defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeErrorCode {
    InvalidJson,
    MissingField,
    WrongType,
    UnknownDuCase,
    WrongNodeKind,
    EmptyNodeId,
    /// A `WIRE_FORMAT.md` §21 resource limit was exceeded — the document is
    /// well-formed and merely too large to walk. Deliberately distinct from
    /// `InvalidJson`, which §21.2 rule 2 forbids using for this: calling a
    /// well-formed document malformed sends the author to repair the wrong
    /// thing.
    LimitExceeded,
}

impl DecodeErrorCode {
    /// The wire-stable string form (the code every host emits verbatim). The
    /// `match` is compile-time exhaustive — a new code cannot be added without a
    /// mapping here.
    pub fn as_str(self) -> &'static str {
        match self {
            DecodeErrorCode::LimitExceeded => "LIMIT_EXCEEDED",
            DecodeErrorCode::InvalidJson => "INVALID_JSON",
            DecodeErrorCode::MissingField => "MISSING_FIELD",
            DecodeErrorCode::WrongType => "WRONG_TYPE",
            DecodeErrorCode::UnknownDuCase => "UNKNOWN_DU_CASE",
            DecodeErrorCode::WrongNodeKind => "WRONG_NODE_KIND",
            DecodeErrorCode::EmptyNodeId => "EMPTY_NODE_ID",
        }
    }
}

impl fmt::Display for DecodeErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The structured, recoverable error a decode returns. `path` is the `$`-rooted
/// location of the fault (e.g. `"$.kind.text"`); the reject corpus asserts the
/// `code` plus a `path` prefix. `expected_shape` is the optional AI-recovery
/// hint (`WIRE_FORMAT.md` §6) — e.g. the valid cases of a `$type` position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    pub code: DecodeErrorCode,
    pub path: String,
    pub message: String,
    pub expected_shape: Option<String>,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.path, self.message)
    }
}

impl std::error::Error for DecodeError {}
