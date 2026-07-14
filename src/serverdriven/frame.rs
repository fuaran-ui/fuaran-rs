//! Server-driven frames + client events. A [`Frame`] is one server→client push:
//! the canonical `TreeOp` list one driver step produced, tagged with the
//! per-connection `seq` (the reconnect-replay key). The Rust host ships the
//! `TreeOp`s themselves — a conformant client re-renders by applying them with
//! the same apply engine every host ships — so the driver stays render-runtime-
//! free. The frame wire shape (`{"ops":[…],"seq":N}`) is byte-interoperable with
//! the `fuaran-go` driver's `EncodeFrameJSON` / `EncodeSSE`.

use crate::canonical::{JVal, parse};
use crate::wire::{TreeOp, encode_op};

/// One server→client push: the canonical `TreeOp`s of one driver step, tagged
/// with the per-connection op sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub seq: u64,
    pub ops: Vec<TreeOp>,
}

/// Render a frame as canonical JSON: `{"ops":[<TreeOp>,…],"seq":N}`. The op list
/// reuses the canonical op encoder (sorted keys, canonical numbers), and `"ops"`
/// sorts before `"seq"` under the Ordinal key order, so the body is itself
/// canonical — byte-identical to the go driver's `EncodeFrameJSON`.
pub fn encode_frame_json(frame: &Frame) -> String {
    let ops: Vec<String> = frame.ops.iter().map(encode_op).collect();
    format!("{{\"ops\":[{}],\"seq\":{}}}", ops.join(","), frame.seq)
}

/// Render a frame as one Server-Sent-Events wire frame: an `id:` line (the op
/// sequence — the reconnect `Last-Event-ID` key), the `patch` event type, the
/// single-line `data:` JSON body (the canonical body has no embedded newlines),
/// and the blank line that terminates the event.
pub fn encode_sse(frame: &Frame) -> String {
    format!(
        "id: {}\nevent: patch\ndata: {}\n\n",
        frame.seq,
        encode_frame_json(frame)
    )
}

/// One client→server interaction: the raw `(node_id, event, payload)` the client
/// sends — the driver does NOT trust it (see [`super::Session::step`]'s
/// legitimacy check). `last_seq` is the client's last-applied frame sequence,
/// threaded on every event so a reconnecting transport can drive resync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub conn_id: String,
    pub node_id: String,
    pub event: String,
    pub payload: String,
    pub last_seq: u64,
}

/// A malformed inbound client event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDecodeError {
    pub message: String,
}

fn str_field(v: &JVal, key: &str) -> String {
    match v.field(key) {
        Some(JVal::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Parse an inbound client event message. Client→server is a small control
/// message (not canonical wire): `{"connId","nodeId","event","payload","lastSeq"}`.
pub fn decode_event(raw: &str) -> Result<Event, EventDecodeError> {
    let root = parse(raw).map_err(|e| EventDecodeError { message: e.message })?;
    if !matches!(root, JVal::Obj(_)) {
        return Err(EventDecodeError {
            message: "expected a JSON object".to_string(),
        });
    }
    let last_seq = match root.field("lastSeq") {
        Some(JVal::Num(n)) if *n >= 0.0 => *n as u64,
        None => 0,
        _ => {
            return Err(EventDecodeError {
                message: "lastSeq must be a non-negative number".to_string(),
            });
        }
    };
    Ok(Event {
        conn_id: str_field(&root, "connId"),
        node_id: str_field(&root, "nodeId"),
        event: str_field(&root, "event"),
        payload: str_field(&root, "payload"),
        last_seq,
    })
}
