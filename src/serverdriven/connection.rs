//! One live connection binds a [`Session`] to a [`Channel`]. On each inbound
//! event it steps the driver, advances the op sequence, and pushes a [`Frame`]
//! when the step produced ops. A rejected step pushes nothing, leaves the session
//! unchanged, and **records the typed reject** (the always-on audit trail). Every
//! frame is buffered for reconnect-replay: a bounded per-connection buffer
//! re-pushes frames newer than the client's last-applied `seq` across a transport
//! drop ([`Connection::resync`]).

use super::channel::{Channel, ChannelError};
use super::driver::{Reject, Session};
use super::frame::{Event, Frame};

/// Bounds the per-connection reconnect-replay buffer, in frames. At capacity the
/// OLDEST frame is evicted, so a never-reconnecting client cannot grow server
/// memory without limit; a client reconnecting from behind the retained window
/// gets a partial replay.
pub const DEFAULT_REPLAY_BUFFER_CAPACITY: usize = 512;

/// Drives one [`Session`] through one [`Channel`], buffering frames for
/// reconnect-replay. Single-threaded per connection (a real transport serialises
/// a connection's inbound events).
pub struct Connection<C: Channel> {
    conn_id: String,
    session: Session,
    channel: C,
    seq: u64,
    buffer: Vec<Frame>,
    buffer_cap: usize,
    rejects: Vec<Reject>,
}

impl<C: Channel> Connection<C> {
    /// Bind a session to a channel with the default replay-buffer capacity.
    pub fn new(conn_id: impl Into<String>, session: Session, channel: C) -> Self {
        Connection {
            conn_id: conn_id.into(),
            session,
            channel,
            seq: 0,
            buffer: Vec::new(),
            buffer_cap: DEFAULT_REPLAY_BUFFER_CAPACITY,
            rejects: Vec::new(),
        }
    }

    /// Override the replay-buffer capacity (builder-style).
    pub fn with_replay_buffer_capacity(mut self, capacity: usize) -> Self {
        if capacity > 0 {
            self.buffer_cap = capacity;
        }
        self
    }

    pub fn conn_id(&self) -> &str {
        &self.conn_id
    }

    /// The current op sequence pushed to this connection.
    pub fn sequence(&self) -> u64 {
        self.seq
    }

    /// The current server-held session.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The channel this connection pushes through.
    pub fn channel(&self) -> &C {
        &self.channel
    }

    /// Every rejected step recorded, in order (the audit trail).
    pub fn rejects(&self) -> &[Reject] {
        &self.rejects
    }

    /// Step the connection with one inbound event: drive the session, advance the
    /// op sequence, and push a [`Frame`] when the step produced ops. A rejected
    /// step records the typed reject and changes nothing. An event addressed to a
    /// different `conn_id` is ignored.
    pub fn handle(&mut self, ev: &Event) -> Result<(), ChannelError> {
        if !ev.conn_id.is_empty() && ev.conn_id != self.conn_id {
            return Ok(());
        }
        match self.session.step(ev) {
            Err(reject) => {
                self.rejects.push(reject);
                Ok(())
            }
            Ok(ops) if ops.is_empty() => Ok(()), // legitimate no-op — no frame, no seq advance.
            Ok(ops) => {
                self.seq += 1;
                let frame = Frame { seq: self.seq, ops };
                self.buffer_frame(frame.clone());
                self.channel.push(&frame)
            }
        }
    }

    fn buffer_frame(&mut self, frame: Frame) {
        if self.buffer.len() >= self.buffer_cap && !self.buffer.is_empty() {
            self.buffer.remove(0);
        }
        self.buffer.push(frame);
    }

    /// Re-push every RETAINED buffered frame newer than `last_seq` — the
    /// transport-agnostic reconnect replay. A backend calls this when a client
    /// reconnects carrying its last-applied sequence, so a brief transport drop
    /// loses no state. The buffer is bounded: a client reconnecting from behind
    /// the retained window gets only the retained tail. Returns the number of
    /// frames replayed, or the first push error.
    pub fn resync(&mut self, last_seq: u64) -> Result<usize, ChannelError> {
        let to_replay: Vec<Frame> = self
            .buffer
            .iter()
            .filter(|f| f.seq > last_seq)
            .cloned()
            .collect();
        let mut replayed = 0;
        for frame in &to_replay {
            self.channel.push(frame)?;
            replayed += 1;
        }
        Ok(replayed)
    }
}
