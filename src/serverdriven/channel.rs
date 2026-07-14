//! The transport seam. The driver pushes server→client [`Frame`]s through a
//! [`Channel`] — NO transport type leaks into the driver, so a WebSocket backend
//! is a drop-in swap for the SSE one (the "channel is a seam" posture applied to
//! the live connection). The seam is per-connection (one channel = one live
//! connection); a multiplexing backend owns the `conn_id → Channel` registry
//! above this seam.
//!
//! The inbound (client→server) direction is the caller feeding events to
//! [`super::Connection::handle`] — a real transport's receive loop (an SSE POST
//! endpoint, a WS message handler) calls it. Keeping the trait a push-only sink
//! avoids a callback registry and keeps every backend a plain value.

use super::frame::{Frame, encode_sse};

/// A transport push failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelError {
    pub message: String,
}

/// The transport seam a backend implements — the server→client push side.
pub trait Channel {
    /// Send a frame to the connected client.
    fn push(&mut self, frame: &Frame) -> Result<(), ChannelError>;
    /// Tear the connection down.
    fn close(&mut self) -> Result<(), ChannelError>;
}

/// The headlessly-testable reference channel the SSE / WS backends specialise —
/// records every frame pushed (assert against it).
#[derive(Debug, Clone, Default)]
pub struct InMemoryChannel {
    pushed: Vec<Frame>,
    closed: bool,
}

impl InMemoryChannel {
    pub fn new() -> Self {
        InMemoryChannel::default()
    }

    /// Every frame pushed to the client, in order.
    pub fn pushed(&self) -> &[Frame] {
        &self.pushed
    }

    /// Whether [`Channel::close`] has been called.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl Channel for InMemoryChannel {
    fn push(&mut self, frame: &Frame) -> Result<(), ChannelError> {
        self.pushed.push(frame.clone());
        Ok(())
    }

    fn close(&mut self) -> Result<(), ChannelError> {
        self.closed = true;
        Ok(())
    }
}

/// The SSE backend: it renders each pushed frame to its Server-Sent-Events wire
/// form (`id:` / `event: patch` / `data:` / blank line) and records the bytes a
/// real SSE endpoint would write to the response stream. The stdlib-only host
/// carries the encoder + the recorded stream; wiring it to an HTTP response is
/// the deployment's concern.
#[derive(Debug, Clone, Default)]
pub struct SseChannel {
    stream: String,
    closed: bool,
}

impl SseChannel {
    pub fn new() -> Self {
        SseChannel::default()
    }

    /// The accumulated SSE wire stream (every pushed frame, concatenated).
    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl Channel for SseChannel {
    fn push(&mut self, frame: &Frame) -> Result<(), ChannelError> {
        self.stream.push_str(&encode_sse(frame));
        Ok(())
    }

    fn close(&mut self) -> Result<(), ChannelError> {
        self.closed = true;
        Ok(())
    }
}
