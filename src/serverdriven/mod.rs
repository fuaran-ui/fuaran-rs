//! The server-driven driver — the live-drive / Relay interactivity tier (the
//! Phoenix-LiveView model). The Rust server holds the tree + state, applies
//! `TreeOp`s in response to client events, and streams frame diffs — canonical
//! `TreeOp` lists — over a transport-neutral [`Channel`] to a thin generic
//! client; interactions round-trip to the server, and the server stays
//! render-runtime-free.
//!
//! The transport is behind the [`Channel`] seam (SSE first; WebSocket is an
//! optional, demand-gated backend that specialises the same seam), so a backend
//! swap never touches the driver. Every frame carries a per-connection `seq` —
//! the reconnect-replay key: a bounded per-connection buffer re-pushes frames
//! newer than the client's last-applied `seq` across a transport drop
//! ([`Connection::resync`]). Every driven op is vetted by the [`crate::gate`]
//! capability gate before it can touch the tree (FGP 3), and driver frames are
//! ops on the op-stream (FGP 5).
//!
//! The frame wire shape (`{"ops":[…],"seq":N}` / the SSE `id/event/data` framing)
//! is byte-interoperable with the `fuaran-go` `serverdriven` driver.

mod channel;
mod connection;
mod driver;
mod frame;

pub use channel::{Channel, ChannelError, InMemoryChannel, SseChannel};
pub use connection::{Connection, DEFAULT_REPLAY_BUFFER_CAPACITY};
pub use driver::{Handler, Reject, RejectReason, Session, op_gate_decision};
pub use frame::{Event, EventDecodeError, Frame, decode_event, encode_frame_json, encode_sse};
