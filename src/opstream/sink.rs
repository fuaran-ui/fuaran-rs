//! The durable op-stream sink — the persistence half of the notarised-stream
//! capability. [`OpStreamSink`] is the append/replay contract (the Rust twin of
//! the cross-host `IOpStreamSink`); [`InMemorySink`] is the headlessly-testable
//! reference, and [`FileSink`] is a JSONL-backed implementation so a hash chain
//! survives a process (append, reopen, verify). A sink rejects a duplicate
//! sequence as a structural defect — query [`OpStreamSink::latest_sequence`]
//! before assigning one.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::canonical::{JVal, escape_string, parse, render_canonical, render_object};
use crate::wire::{decode_op, encode_op};

use super::chain::{Actor, OpRecord, OpResult};

/// A sink failure — a duplicate sequence, an I/O error, or a corrupt persisted
/// line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkError {
    DuplicateSequence(u64),
    Io(String),
    Parse(String),
}

impl std::fmt::Display for SinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SinkError::DuplicateSequence(seq) => write!(f, "duplicate sequence {seq}"),
            SinkError::Io(msg) => write!(f, "sink I/O error: {msg}"),
            SinkError::Parse(msg) => write!(f, "corrupt persisted record: {msg}"),
        }
    }
}

impl std::error::Error for SinkError {}

/// The durable op-stream sink contract. Synchronous — the in-memory sink answers
/// directly.
pub trait OpStreamSink {
    /// Append `record`; errors on a duplicate sequence.
    fn append(&mut self, record: &OpRecord) -> Result<(), SinkError>;
    /// The records with sequence in `[from, to]` inclusive, ascending.
    fn replay(&self, from: u64, to: u64) -> Vec<OpRecord>;
    /// The highest sequence observed; `0` when empty.
    fn latest_sequence(&self) -> u64;
}

fn in_range(records: &[OpRecord], from: u64, to: u64) -> Vec<OpRecord> {
    let mut out: Vec<OpRecord> = records
        .iter()
        .filter(|r| r.sequence >= from && r.sequence <= to)
        .cloned()
        .collect();
    out.sort_by_key(|r| r.sequence);
    out
}

fn latest(records: &[OpRecord]) -> u64 {
    records.iter().map(|r| r.sequence).max().unwrap_or(0)
}

/// The append-only in-memory reference sink.
#[derive(Debug, Clone, Default)]
pub struct InMemorySink {
    records: Vec<OpRecord>,
}

impl InMemorySink {
    pub fn new() -> Self {
        InMemorySink::default()
    }

    pub fn records(&self) -> &[OpRecord] {
        &self.records
    }
}

impl OpStreamSink for InMemorySink {
    fn append(&mut self, record: &OpRecord) -> Result<(), SinkError> {
        if self.records.iter().any(|r| r.sequence == record.sequence) {
            return Err(SinkError::DuplicateSequence(record.sequence));
        }
        self.records.push(record.clone());
        Ok(())
    }

    fn replay(&self, from: u64, to: u64) -> Vec<OpRecord> {
        in_range(&self.records, from, to)
    }

    fn latest_sequence(&self) -> u64 {
        latest(&self.records)
    }
}

/// A JSONL-backed sink: each record is one canonical-JSON line appended to a
/// file, so a chain reopened in a later process replays + verifies identically.
#[derive(Debug, Clone)]
pub struct FileSink {
    path: PathBuf,
    records: Vec<OpRecord>,
}

impl FileSink {
    /// Open (or create) a file-backed sink at `path`, reading any records already
    /// persisted there. Reopening the same path recovers the full chain.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, SinkError> {
        let path = path.into();
        let mut records = Vec::new();
        if path.exists() {
            let text = fs::read_to_string(&path).map_err(|e| SinkError::Io(e.to_string()))?;
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                records.push(parse_record(line)?);
            }
        }
        Ok(FileSink { path, records })
    }

    pub fn records(&self) -> &[OpRecord] {
        &self.records
    }
}

impl OpStreamSink for FileSink {
    fn append(&mut self, record: &OpRecord) -> Result<(), SinkError> {
        if self.records.iter().any(|r| r.sequence == record.sequence) {
            return Err(SinkError::DuplicateSequence(record.sequence));
        }
        let line = serialize_record(record);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| SinkError::Io(e.to_string()))?;
        writeln!(file, "{line}").map_err(|e| SinkError::Io(e.to_string()))?;
        self.records.push(record.clone());
        Ok(())
    }

    fn replay(&self, from: u64, to: u64) -> Vec<OpRecord> {
        in_range(&self.records, from, to)
    }

    fn latest_sequence(&self) -> u64 {
        latest(&self.records)
    }
}

// ─── Record (de)serialisation ────────────────────────────────────────────────

// ─── The actor's ONE encoding (Phase 1653) ───────────────────────────────────
//
// This file used to render the actor through `render_object`, which Ordinal-
// SORTS its keys and so emitted `{"id","kind","model","version"}` — where
// `chain::encode_actor` emits the pinned `{"kind","model","version","id"}` that
// the hash pre-image folds in and the DAG record nests verbatim. Two encodings
// of one value in one module is the drift Phase 1168 avoided in the DAG record
// by EXPOSING the canonical encoder rather than writing a second one, and this
// is the same fix.
//
// # The readability question, answered before the change (it is the first task,
// # not an afterthought)
//
// The sink's output is PERSISTED, so a change here changes bytes already on
// disk, and the honest question is whether lines written under the old order
// must still read back. They must, and they do — established rather than
// assumed:
//
//  * The reader is key-addressed, not positional. `deserialize_record` and
//    `parse_actor` go through `JVal::field`, so member order is not a fact
//    either of them can observe. `a_sink_line_in_the_pre_1653_key_order_still_
//    parses` in `tests/opstream.rs` pins that against a literal old line rather
//    than leaving it to inspection.
//  * The line's own bytes are not hashed. `chain::compute_hash` builds its
//    pre-image from `encode_actor` — the pinned encoder — and never from the
//    serialised line, so no chain verification depends on this order and old
//    and new lines verify identically in one file.
//
// So there is no migration, no dual-read path, and no version marker: the two
// orders are the same document, and after this change the host emits one of
// them instead of two.
//
// One same-class twin is deliberately NOT changed here: `result_json` below has
// the identical shape against `chain`'s private `encode_result`. Unifying it
// means publishing a second function from `chain`, which is a public-surface
// decision this phase was not asked to take.
fn actor_json(a: &Actor) -> String {
    crate::opstream::encode_actor(a)
}

fn result_json(r: &OpResult) -> String {
    match r {
        OpResult::Success => {
            let mut f = vec![("kind".to_string(), escape_string("success"))];
            render_object(&mut f)
        }
        OpResult::Failure { code, message } => {
            let mut f = vec![
                ("kind".to_string(), escape_string("failure")),
                ("code".to_string(), escape_string(code)),
                ("message".to_string(), escape_string(message)),
            ];
            render_object(&mut f)
        }
    }
}

/// Serialise a record to one canonical-JSON line (Ordinal-sorted keys).
fn serialize_record(r: &OpRecord) -> String {
    let prompt = match &r.prompt_id {
        None => "null".to_string(),
        Some(p) => escape_string(p),
    };
    let mut fields = vec![
        ("actor".to_string(), actor_json(&r.actor)),
        ("hash".to_string(), escape_string(&r.hash)),
        ("op".to_string(), encode_op(&r.op)),
        ("previousHash".to_string(), escape_string(&r.previous_hash)),
        ("promptId".to_string(), prompt),
        ("result".to_string(), result_json(&r.result)),
        ("sequence".to_string(), r.sequence.to_string()),
        ("ts".to_string(), r.timestamp_unix_seconds.to_string()),
    ];
    render_object(&mut fields)
}

fn parse_str(v: &JVal, key: &str, ctx: &str) -> Result<String, SinkError> {
    match v.field(key) {
        Some(JVal::Str(s)) => Ok(s.clone()),
        _ => Err(SinkError::Parse(format!("{ctx}: missing string '{key}'"))),
    }
}

fn parse_actor(v: &JVal) -> Result<Actor, SinkError> {
    match v.field("kind") {
        Some(JVal::Str(k)) if k == "agent" => Ok(Actor::Agent {
            model: parse_str(v, "model", "actor")?,
            version: parse_str(v, "version", "actor")?,
            id: parse_str(v, "id", "actor")?,
        }),
        Some(JVal::Str(k)) if k == "human" => Ok(Actor::Human {
            id: parse_str(v, "id", "actor")?,
        }),
        _ => Err(SinkError::Parse("actor: unknown kind".to_string())),
    }
}

fn parse_result(v: &JVal) -> Result<OpResult, SinkError> {
    match v.field("kind") {
        Some(JVal::Str(k)) if k == "failure" => Ok(OpResult::Failure {
            code: parse_str(v, "code", "result")?,
            message: parse_str(v, "message", "result")?,
        }),
        Some(JVal::Str(k)) if k == "success" => Ok(OpResult::Success),
        _ => Err(SinkError::Parse("result: unknown kind".to_string())),
    }
}

/// Parse a persisted line back into an [`OpRecord`].
fn parse_record(line: &str) -> Result<OpRecord, SinkError> {
    let root = parse(line).map_err(|e| SinkError::Parse(e.message))?;
    let sequence = match root.field("sequence") {
        Some(JVal::Num(n)) => *n as u64,
        _ => return Err(SinkError::Parse("missing 'sequence'".to_string())),
    };
    let timestamp_unix_seconds = match root.field("ts") {
        Some(JVal::Num(n)) => *n as i64,
        _ => return Err(SinkError::Parse("missing 'ts'".to_string())),
    };
    let prompt_id = match root.field("promptId") {
        Some(JVal::Str(s)) => Some(s.clone()),
        Some(JVal::Null) | None => None,
        _ => return Err(SinkError::Parse("bad 'promptId'".to_string())),
    };
    let actor = parse_actor(
        root.field("actor")
            .ok_or_else(|| SinkError::Parse("missing 'actor'".to_string()))?,
    )?;
    let result = parse_result(
        root.field("result")
            .ok_or_else(|| SinkError::Parse("missing 'result'".to_string()))?,
    )?;
    let op_raw = root
        .field("op")
        .ok_or_else(|| SinkError::Parse("missing 'op'".to_string()))?;
    let op = decode_op(&render_canonical(op_raw))
        .map_err(|e| SinkError::Parse(format!("op decode: {}", e.message)))?;
    Ok(OpRecord {
        sequence,
        op,
        timestamp_unix_seconds,
        actor,
        prompt_id,
        result,
        previous_hash: parse_str(&root, "previousHash", "record")?,
        hash: parse_str(&root, "hash", "record")?,
    })
}
