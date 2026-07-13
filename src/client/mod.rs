//! The client session: the target-agnostic heart of the browser-native
//! (`wasm32`) client host. A [`ClientSession`] holds a decoded wire tree plus
//! its live binding sources, renders the tree to HTML through the shared
//! server renderer, applies tree-ops that mutate the held tree, and writes
//! reactive state slots — the decode → render → drive loop a client runs in
//! the browser.
//!
//! This module is **pure Rust with no `wasm32`-specific code** — it is exercised
//! natively by `tests/client.rs`, and the WASM tier ([`wasm`], compiled only
//! for `target_arch = "wasm32"`) is a thin marshalling shim over the same
//! session type. The delivery mode this enables (per the README): the codec +
//! renderer ship as a `wasm32` module; a thin generic JS loader mounts the
//! rendered HTML and drives the session client-side.
//!
//! **Interaction model.** The server renderer emits inert HTML (no event
//! handlers ride the wire — closures are `"<closure>"` sentinels, §4). A client
//! drives interactivity by writing the reactive stores — [`set_state`] /
//! [`set_filter`] — and re-rendering, which is exactly what the wire's
//! *write-back default* prescribes (an omitted handler over a writable
//! `Binding.State` / `Binding.Filter` slot writes the change back to that slot,
//! WIRE_FORMAT.md §"Control write-back default"). Structural edits arrive as
//! [`TreeOp`]s through [`apply_op`], gated by the same total apply engine the
//! headless host uses. The loop never trusts the client with capabilities the
//! wire didn't grant.
//!
//! [`set_state`]: ClientSession::set_state
//! [`set_filter`]: ClientSession::set_filter
//! [`apply_op`]: ClientSession::apply_op

#[cfg(target_arch = "wasm32")]
pub mod wasm;

use crate::canonical::{JVal, parse, render_canonical};
use crate::ops::{ApplyError, apply};
use crate::render::BindingSources;
use crate::render::server::render_to_html;
use crate::wire::{DecodeError, Node, decode_node, decode_op};

/// A live client session over one wire tree. Holds the current tree + binding
/// sources; renders, applies ops, and writes reactive state.
pub struct ClientSession {
    tree: Node,
    sources: BindingSources,
}

impl ClientSession {
    /// Decode a canonical wire `Node` JSON into a new session. The tree is
    /// held as the session's current root; binding sources start empty (the
    /// host seeds them with [`set_state`](Self::set_state) /
    /// [`set_filter`](Self::set_filter) / [`set_query`](Self::set_query)).
    pub fn new(wire_json: &str) -> Result<Self, DecodeError> {
        let tree = decode_node(wire_json)?;
        Ok(ClientSession {
            tree,
            sources: BindingSources::default(),
        })
    }

    /// The current tree, re-encoded to canonical wire JSON — the round-trip
    /// exit point (persist, diff, or hand back to a headless peer).
    pub fn tree_json(&self) -> String {
        crate::wire::encode_node(&self.tree)
    }

    /// Render the current tree to a body-fragment HTML string against the live
    /// binding sources (the shared server renderer, so client and server
    /// produce byte-identical markup for the same tree + sources — the
    /// hydration-parity contract).
    pub fn render(&self) -> String {
        render_to_html(&self.tree, &self.sources)
    }

    /// Apply a canonical wire `TreeOp` JSON against the held tree. On success
    /// the session adopts the new tree; on any failure the held tree is
    /// untouched (the apply engine never partially mutates). The returned
    /// `Result` mirrors the headless host's apply surface.
    pub fn apply_op(&mut self, op_json: &str) -> Result<(), ClientError> {
        let op = decode_op(op_json).map_err(ClientError::Decode)?;
        let outcome = apply(&self.tree, &op).map_err(ClientError::Apply)?;
        self.tree = outcome.new_tree;
        Ok(())
    }

    /// Write a reactive `$state.<key>` slot from a JSON value (the write-back
    /// a decoded control performs when its handler is omitted). Re-render to
    /// observe the change. A non-JSON value is a [`ClientError::Decode`].
    pub fn set_state(&mut self, key: &str, value_json: &str) -> Result<(), ClientError> {
        let value = parse_value(value_json)?;
        self.sources.state.insert(key.to_string(), value);
        Ok(())
    }

    /// Write a `$filters.<name>` slot (the FilterStore write-back).
    pub fn set_filter(&mut self, name: &str, value_json: &str) -> Result<(), ClientError> {
        let value = parse_value(value_json)?;
        self.sources.filters.insert(name.to_string(), value);
        Ok(())
    }

    /// Seed a `$queries.<name>` result slot (host-fed data for a `Binding.Query`
    /// consumer — the fetch is a host concern; the tree owns only the name).
    pub fn set_query(&mut self, name: &str, value_json: &str) -> Result<(), ClientError> {
        let value = parse_value(value_json)?;
        self.sources.query_results.insert(name.to_string(), value);
        Ok(())
    }
}

/// A recoverable client-session failure: a malformed wire input, or a
/// structured apply-engine refusal.
#[derive(Debug, Clone)]
pub enum ClientError {
    Decode(DecodeError),
    Apply(ApplyError),
}

impl ClientError {
    /// A stable JSON envelope for the failure, shaped for the JS boundary:
    /// `{"error":{"class":"decode"|"apply","code":"…","path"?:"…","message":"…"}}`.
    pub fn to_json(&self) -> String {
        match self {
            ClientError::Decode(e) => {
                let path = render_canonical(&JVal::Str(e.path.clone()));
                let message = render_canonical(&JVal::Str(e.message.clone()));
                format!(
                    "{{\"error\":{{\"class\":\"decode\",\"code\":\"{}\",\"message\":{message},\"path\":{path}}}}}",
                    e.code.as_str()
                )
            }
            ClientError::Apply(e) => {
                let message = render_canonical(&JVal::Str(e.message.clone()));
                let batch = e
                    .batch_index
                    .map(|i| format!(",\"batchIndex\":{i}"))
                    .unwrap_or_default();
                format!(
                    "{{\"error\":{{\"class\":\"apply\",\"code\":\"{}\",\"message\":{message}{batch}}}}}",
                    e.code.as_str()
                )
            }
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Decode(e) => write!(f, "{e}"),
            ClientError::Apply(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Parse a JSON value string into a [`JVal`] for a store-write, mapping a parse
/// failure into the client `INVALID_JSON` decode error.
fn parse_value(value_json: &str) -> Result<JVal, ClientError> {
    parse(value_json).map_err(|e| {
        ClientError::Decode(DecodeError {
            code: crate::wire::DecodeErrorCode::InvalidJson,
            path: "$".to_string(),
            message: format!("state value is not valid JSON: {}", e.message),
            expected_shape: None,
        })
    })
}
