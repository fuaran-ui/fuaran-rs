//! The constrained emitter (Phase 2452, WIRE_FORMAT §11.4) — the device side of a
//! panel, `no_std` + `alloc` over the Core spine.
//!
//! A device panel is static shape plus changing readings. The device never
//! constructs a node: the tree is authored and certified on a desktop by the tree
//! compiler ([`compile`], `std` only) and compiled in as an [`Image`] — the panel
//! document as constant canonical bytes, and the tables of node ids, insertable
//! fragments, declared writes, handlers and state keys. At run time the emitter
//! writes, per connection, a `ReplaceRoot` of that constant first; then a
//! `ReplaceBinding` for each READING, an `UpdateProp` for a declared property,
//! and `InsertChild` / `RemoveNode` for the fragments that come and go. Those are
//! the five op kinds of the `panel` profile and the only ones it writes.
//!
//! A reading is a `ReplaceBinding` on the reading's slot, never `UpdateState`:
//! on this wire `UpdateState` replaces a node's loading / empty / error
//! presentation (§3), and a binding is where a value lives (§11.4).
//!
//! Every op is canonical BY CONSTRUCTION: each writer spells its members in the
//! ordinal key order, every string through the Core spine's one escape, every
//! number as an integer within ±(2⁵³ − 1). A write names a TABLE entry rather than
//! a node id, so a device cannot target a node, slot or property the desktop did
//! not certify, and a write's value kind is fixed by the table, so an integer
//! reading can only ever carry an integer token.
//!
//! Inbound, the emitter decodes exactly one document — the server-driven client
//! event (`{"connId","nodeId","event","payload","lastSeq"}`, the shape this host's
//! driver and the `fuaran-go` driver read) — through the Core spine's reader under
//! declared limits ([`EVENT_LIMITS`]), and admits it only when its node and event
//! name a registered handler.
//!
//! Outbound frames are hash-chained ([`Emitter`]): each frame's op document is a
//! record of the Core spine's op-stream chain (`fuaran_core::chain`, canonical
//! profile), so the head commits to every frame before it and is what a device
//! signs on demand. A frame travels as the server-driven frame body
//! (`{"ops":[<op>],"seq":N}`, byte-identical to [`super::encode_frame_json`]),
//! and over a byte pipe ([`PipeChannel`], UART first) as one newline-terminated
//! line, with a reconnect-replay buffer bounded in BYTES so it can be sized to
//! RAM.

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use core::fmt::Write as _;

/// Who a frame is attributed to — the Core spine's actor, re-exported for a
/// device crate that depends on this one alone.
pub use fuaran_core::chain::Actor;
use fuaran_core::chain::{self, Anchor, ChainRecord, encode_actor};
use fuaran_core::json::{self, JVal, JsonLimits, JsonRefusal, escape_string};

#[cfg(feature = "std")]
mod compile;
#[cfg(feature = "std")]
pub use compile::{CompileError, CompiledPanel, PanelSpec, WriteSpec, compile};

/// The emitter profile revision this emitter claims (WIRE_FORMAT §11.4): the
/// corpus's `panel` lineage in its current spelling. `tests/conformance.rs` pins
/// it, and the subset below, against the corpus manifest's declaration.
pub const PROFILE_REVISION: &str = "panel@1.1";
/// The §15.1 profile the revision's documents are read under.
pub const PROFILE_WIRE: &str = "core@1.0";
/// The axes the claim makes: canonical bytes always, integers only.
pub const PROFILE_AXES: &[&str] = &["canonical-bytes", "integer-only"];
/// The declared subset's node kinds.
pub const SUBSET_KINDS: &[&str] = &[
    "Badge", "Box", "Button", "Callout", "Form", "Heading", "Metric",
];
/// The declared subset's form-field (control) kinds.
pub const SUBSET_FORM_FIELD_KINDS: &[&str] = &["DateTime", "Number"];
/// The declared subset's op kinds — every op this emitter can write.
pub const SUBSET_OPS: &[&str] = &[
    "InsertChild",
    "RemoveNode",
    "ReplaceBinding",
    "ReplaceRoot",
    "UpdateProp",
];

/// The largest integer magnitude a write may carry: integer identity on the wire
/// stops at 2⁵³, where the integer and double canonical layouts stop agreeing.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The kind of value a declared write carries. Fixed by the table, so a write's
/// value kind is certified on the desktop and enforced on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Int,
    Str,
    Bool,
}

/// A value a device writes: an integer, a string or a boolean — no fraction and
/// no exponent can be spelled, which is the `integer-only` axis by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value<'v> {
    Int(i64),
    Str(&'v str),
    Bool(bool),
}

impl Value<'_> {
    /// The value's kind.
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Int(_) => ValueKind::Int,
            Value::Str(_) => ValueKind::Str,
            Value::Bool(_) => ValueKind::Bool,
        }
    }
}

/// A subtree the device inserts and removes (an alarm row): its parent, its root
/// id and its canonical node document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fragment<'a> {
    pub parent: &'a str,
    pub id: &'a str,
    pub node: &'a str,
}

/// A declared write: the node it targets, the field it writes (a binding slot
/// for a reading, a property path for a property) and the kind of value it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Write<'a> {
    pub target: &'a str,
    pub field: &'a str,
    pub kind: ValueKind,
}

/// A registered handler: the node and the event name the server-driven client
/// event names. Index into [`Image::handlers`] is what an admitted event carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handler<'a> {
    pub node: &'a str,
    pub event: &'a str,
}

/// A compiled panel: everything the device knows about its tree, as constants.
/// Produced by the tree compiler ([`CompiledPanel::to_rust_source`]) and compiled
/// in; the device never builds one from parts it did not receive that way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image<'a> {
    /// The emitter profile revision the image was certified under.
    pub revision: &'a str,
    /// The panel document: canonical node bytes.
    pub root: &'a str,
    /// Every node id of the panel and of its fragments, document order.
    pub nodes: &'a [&'a str],
    /// The subtrees the device inserts and removes.
    pub fragments: &'a [Fragment<'a>],
    /// The readings: `ReplaceBinding` targets (`field` is the binding slot).
    pub readings: &'a [Write<'a>],
    /// The properties: `UpdateProp` targets (`field` is the property path).
    pub props: &'a [Write<'a>],
    /// The handler table — the set of events a device admits.
    pub handlers: &'a [Handler<'a>],
    /// Every `State` key the panel names, sorted.
    pub state_keys: &'a [&'a str],
}

/// Why a write was refused. Nothing is written on a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitError {
    /// No reading at that index.
    NoSuchReading(usize),
    /// No property at that index.
    NoSuchProp(usize),
    /// No fragment at that index.
    NoSuchFragment(usize),
    /// The value's kind is not the one the table declares for the write.
    WrongValueKind {
        expected: ValueKind,
        actual: ValueKind,
    },
    /// An integer outside ±(2⁵³ − 1), which has no canonical integer spelling.
    IntegerOutOfRange(i64),
}

fn write_value(out: &mut String, expected: ValueKind, v: Value<'_>) -> Result<(), EmitError> {
    if v.kind() != expected {
        return Err(EmitError::WrongValueKind {
            expected,
            actual: v.kind(),
        });
    }
    match v {
        Value::Int(i) => {
            if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&i) {
                return Err(EmitError::IntegerOutOfRange(i));
            }
            // Writing to a String cannot fail.
            let _ = write!(out, "{i}");
        }
        Value::Str(s) => out.push_str(&escape_string(s)),
        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
    }
    Ok(())
}

// ─── the op writers: canonical by construction ───────────────────────────────

/// `ReplaceRoot` of the compiled panel — the first frame of every connection.
pub fn replace_root(image: &Image<'_>) -> String {
    let mut out = String::with_capacity(image.root.len() + 32);
    out.push_str("{\"$type\":\"ReplaceRoot\",\"node\":");
    out.push_str(image.root);
    out.push('}');
    out
}

/// `ReplaceBinding` of reading `reading` to a `Static` value — a reading.
pub fn replace_binding(
    image: &Image<'_>,
    reading: usize,
    value: Value<'_>,
) -> Result<String, EmitError> {
    let w = image
        .readings
        .get(reading)
        .ok_or(EmitError::NoSuchReading(reading))?;
    let mut out =
        String::from("{\"$type\":\"ReplaceBinding\",\"binding\":{\"$type\":\"Static\",\"value\":");
    write_value(&mut out, w.kind, value)?;
    out.push_str("},\"slot\":");
    out.push_str(&escape_string(w.field));
    out.push_str(",\"target\":");
    out.push_str(&escape_string(w.target));
    out.push('}');
    Ok(out)
}

/// `UpdateProp` of property `prop` to `value`.
pub fn update_prop(image: &Image<'_>, prop: usize, value: Value<'_>) -> Result<String, EmitError> {
    let w = image.props.get(prop).ok_or(EmitError::NoSuchProp(prop))?;
    let mut out = String::from("{\"$type\":\"UpdateProp\",\"path\":");
    out.push_str(&escape_string(w.field));
    out.push_str(",\"target\":");
    out.push_str(&escape_string(w.target));
    out.push_str(",\"value\":");
    write_value(&mut out, w.kind, value)?;
    out.push('}');
    Ok(out)
}

/// `InsertChild` of fragment `fragment` under its declared parent.
pub fn insert_child(image: &Image<'_>, fragment: usize) -> Result<String, EmitError> {
    let f = image
        .fragments
        .get(fragment)
        .ok_or(EmitError::NoSuchFragment(fragment))?;
    let mut out = String::with_capacity(f.node.len() + f.parent.len() + 48);
    out.push_str("{\"$type\":\"InsertChild\",\"child\":");
    out.push_str(f.node);
    out.push_str(",\"parentId\":");
    out.push_str(&escape_string(f.parent));
    out.push('}');
    Ok(out)
}

/// `RemoveNode` of fragment `fragment`.
pub fn remove_node(image: &Image<'_>, fragment: usize) -> Result<String, EmitError> {
    let f = image
        .fragments
        .get(fragment)
        .ok_or(EmitError::NoSuchFragment(fragment))?;
    let mut out = String::from("{\"$type\":\"RemoveNode\",\"target\":");
    out.push_str(&escape_string(f.id));
    out.push('}');
    Ok(out)
}

// ─── frames and the chain ────────────────────────────────────────────────────

/// One outbound frame: its connection sequence (1-based, the reconnect-replay
/// key), its op document, and the chain link it made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq: u64,
    pub op: String,
    pub prev_hash: String,
    pub hash: String,
}

impl Frame {
    /// The server-driven frame body, `{"ops":[<op>],"seq":N}` — canonical, and
    /// byte-identical to the driver's `encode_frame_json` over the same op.
    pub fn body(&self) -> String {
        let mut out = String::with_capacity(self.op.len() + 32);
        out.push_str("{\"ops\":[");
        out.push_str(&self.op);
        out.push_str("],\"seq\":");
        let _ = write!(out, "{}", self.seq);
        out.push('}');
        out
    }

    /// This frame as a record of the Core spine's chain, for a verifier
    /// (`fuaran_core::chain::walk` from `Anchor::genesis()`). `actor` is the
    /// canonical actor text [`Emitter::actor`] answers.
    pub fn record(&self, actor: &str) -> ChainRecord {
        ChainRecord {
            seq: self.seq - 1,
            actor: actor.to_string(),
            op: self.op.clone(),
            prev_hash: self.prev_hash.clone(),
            hash: self.hash.clone(),
        }
    }
}

/// The per-connection emitter: it numbers frames and chains them.
#[derive(Debug, Clone)]
pub struct Emitter<'a> {
    image: &'a Image<'a>,
    actor: String,
    anchor: Anchor,
}

impl<'a> Emitter<'a> {
    /// A connection's emitter over a compiled image, attributing every frame to
    /// `actor`. The chain hangs from the canonical genesis.
    pub fn new(image: &'a Image<'a>, actor: &Actor) -> Self {
        Emitter {
            image,
            actor: encode_actor(actor),
            anchor: Anchor::genesis(),
        }
    }

    /// The image this emitter writes.
    pub fn image(&self) -> &'a Image<'a> {
        self.image
    }

    /// The canonical actor text folded into every link.
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// The chain head: the hash of the last frame, or the genesis before the
    /// first. What a device signs on demand.
    pub fn head(&self) -> &str {
        &self.anchor.prev_hash
    }

    /// The sequence of the last frame written (0 before the first).
    pub fn seq(&self) -> u64 {
        self.anchor.seq
    }

    fn frame(&mut self, op: String) -> Frame {
        let (record, next) = chain::append(&self.anchor, &self.actor, &op);
        self.anchor = next;
        Frame {
            seq: record.seq + 1,
            op,
            prev_hash: record.prev_hash,
            hash: record.hash,
        }
    }

    /// The connection's first frame: the compiled panel. Also the answer to a
    /// client behind the replay window ([`Resync::Gap`]).
    pub fn replace_root(&mut self) -> Frame {
        let op = replace_root(self.image);
        self.frame(op)
    }

    /// A reading.
    pub fn reading(&mut self, reading: usize, value: Value<'_>) -> Result<Frame, EmitError> {
        let op = replace_binding(self.image, reading, value)?;
        Ok(self.frame(op))
    }

    /// A property write.
    pub fn prop(&mut self, prop: usize, value: Value<'_>) -> Result<Frame, EmitError> {
        let op = update_prop(self.image, prop, value)?;
        Ok(self.frame(op))
    }

    /// A fragment appears.
    pub fn insert(&mut self, fragment: usize) -> Result<Frame, EmitError> {
        let op = insert_child(self.image, fragment)?;
        Ok(self.frame(op))
    }

    /// A fragment goes.
    pub fn remove(&mut self, fragment: usize) -> Result<Frame, EmitError> {
        let op = remove_node(self.image, fragment)?;
        Ok(self.frame(op))
    }
}

// ─── the inbound event, under declared limits ────────────────────────────────

/// The limits the client event is read under: one object of scalar members (the
/// root is the only container admitted), at most eight members, strings of at
/// most 256 code points, 512 bytes in all. Tighter than §21's and therefore
/// deployment policy (§21.2 rule 6), never part of the profile.
pub const EVENT_LIMITS: JsonLimits = JsonLimits {
    depth: 1,
    string_length: Some(256),
    array_length: Some(0),
    object_members: Some(8),
    document_bytes: Some(512),
};

/// An admitted client event: the handler it names (an index into
/// [`Image::handlers`]), its payload and the client's last-applied sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    pub handler: usize,
    pub payload: String,
    pub last_seq: u64,
}

/// Why an inbound event was refused. Nothing runs on a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventRefusal {
    /// The reader refused: malformed text, or a declared limit passed.
    Read(JsonRefusal),
    /// The document is not an object.
    NotAnObject,
    /// A member has the wrong type (`lastSeq` not a non-negative safe integer,
    /// or a string member that is not a string).
    BadMember(&'static str),
    /// `nodeId` + `event` name no registered handler.
    UnknownHandler,
}

fn str_member(root: &JVal, key: &'static str) -> Result<String, EventRefusal> {
    match root.field(key) {
        None => Ok(String::new()),
        Some(JVal::Str(s)) => Ok(s.clone()),
        Some(_) => Err(EventRefusal::BadMember(key)),
    }
}

/// Decode a client event under `limits` (normally [`EVENT_LIMITS`]) and admit it
/// against the image's handler table.
pub fn decode_event(
    image: &Image<'_>,
    raw: &str,
    limits: &JsonLimits,
) -> Result<Inbound, EventRefusal> {
    let root = json::read(raw, limits).map_err(EventRefusal::Read)?;
    if !matches!(root, JVal::Obj(_)) {
        return Err(EventRefusal::NotAnObject);
    }
    let last_seq = match root.field("lastSeq") {
        None => 0,
        Some(JVal::Num(n)) => {
            let u = *n as u64;
            if *n < 0.0 || u as f64 != *n || u > MAX_SAFE_INTEGER as u64 {
                return Err(EventRefusal::BadMember("lastSeq"));
            }
            u
        }
        Some(_) => return Err(EventRefusal::BadMember("lastSeq")),
    };
    // connId is the transport's; it is type-checked and not otherwise read.
    str_member(&root, "connId")?;
    let node = str_member(&root, "nodeId")?;
    let event = str_member(&root, "event")?;
    let payload = str_member(&root, "payload")?;
    let handler = image
        .handlers
        .iter()
        .position(|h| h.node == node && h.event == event)
        .ok_or(EventRefusal::UnknownHandler)?;
    Ok(Inbound {
        handler,
        payload,
        last_seq,
    })
}

// ─── the byte-pipe channel ───────────────────────────────────────────────────

/// A byte pipe — a UART first. One call writes one whole line.
pub trait BytePipe {
    type Error;
    fn write(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

/// What a resync did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resync {
    /// The client is current; nothing was written.
    Current,
    /// The retained frames after the client's sequence were re-written.
    Replayed(usize),
    /// The client is behind the retained window: nothing was written, and the
    /// device answers with a fresh `ReplaceRoot` ([`Emitter::replace_root`])
    /// followed by its current readings.
    Gap,
}

/// The server-driven channel specialised to a byte pipe: each frame is written as
/// its frame body and a newline (the canonical body holds no raw newline, so a
/// line is a frame), and retained for reconnect-replay until the retained op bytes
/// pass `replay_budget` — a budget in bytes, so it is sized to the device's RAM
/// rather than to a frame count. A frame larger than the whole budget is written
/// and not retained.
#[derive(Debug)]
pub struct PipeChannel<P: BytePipe> {
    pipe: P,
    replay: VecDeque<Frame>,
    retained: usize,
    replay_budget: usize,
}

/// The bytes a retained frame is charged: its op, its two hashes and a fixed
/// allowance for the frame's own fields.
fn retained_cost(f: &Frame) -> usize {
    f.op.len() + f.prev_hash.len() + f.hash.len() + 32
}

impl<P: BytePipe> PipeChannel<P> {
    pub fn new(pipe: P, replay_budget: usize) -> Self {
        PipeChannel {
            pipe,
            replay: VecDeque::new(),
            retained: 0,
            replay_budget,
        }
    }

    pub fn pipe(&self) -> &P {
        &self.pipe
    }

    /// The bytes currently retained for replay (never more than the budget).
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// The sequences currently retained, oldest first.
    pub fn retained_seqs(&self) -> impl Iterator<Item = u64> + '_ {
        self.replay.iter().map(|f| f.seq)
    }

    fn write_frame(pipe: &mut P, f: &Frame) -> Result<(), P::Error> {
        let mut line = f.body();
        line.push('\n');
        pipe.write(line.as_bytes())
    }

    /// Write a frame and retain it for replay.
    pub fn push(&mut self, frame: Frame) -> Result<(), P::Error> {
        Self::write_frame(&mut self.pipe, &frame)?;
        let cost = retained_cost(&frame);
        if cost > self.replay_budget {
            // Retaining it would evict everything and still pass the budget.
            self.replay.clear();
            self.retained = 0;
            return Ok(());
        }
        self.replay.push_back(frame);
        self.retained += cost;
        while self.retained > self.replay_budget {
            if let Some(old) = self.replay.pop_front() {
                self.retained -= retained_cost(&old);
            }
        }
        Ok(())
    }

    /// Re-write every retained frame newer than the client's `last_seq`, or say
    /// that the client is behind the window.
    pub fn resync(&mut self, last_seq: u64, newest: u64) -> Result<Resync, P::Error> {
        if last_seq >= newest {
            return Ok(Resync::Current);
        }
        match self.replay.front() {
            Some(oldest) if oldest.seq <= last_seq + 1 => {}
            _ => return Ok(Resync::Gap),
        }
        let mut n = 0;
        for f in self.replay.iter().filter(|f| f.seq > last_seq) {
            Self::write_frame(&mut self.pipe, f)?;
            n += 1;
        }
        Ok(Resync::Replayed(n))
    }
}
