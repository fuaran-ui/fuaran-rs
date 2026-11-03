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
//!
//! [`emitter`] is the other end of a device panel (Phase 2452): a constrained
//! emitter (WIRE_FORMAT §11.4) that writes a desktop-compiled tree and the frames
//! after it in canonical bytes, and decodes only this driver's client event. It is
//! the one part of this module that builds `no_std` + `alloc`.

#[cfg(feature = "std")]
mod channel;
#[cfg(feature = "std")]
mod connection;
#[cfg(feature = "std")]
mod driver;
pub mod emitter;
#[cfg(feature = "std")]
mod frame;

#[cfg(feature = "std")]
pub use channel::{Channel, ChannelError, InMemoryChannel, SseChannel};
#[cfg(feature = "std")]
pub use connection::{Connection, DEFAULT_REJECT_TRAIL_CAPACITY, DEFAULT_REPLAY_BUFFER_CAPACITY};
#[cfg(feature = "std")]
pub use driver::{Handler, Reject, RejectReason, Session, op_gate_decision};
#[cfg(feature = "std")]
pub use frame::{Event, EventDecodeError, Frame, decode_event, encode_frame_json, encode_sse};
