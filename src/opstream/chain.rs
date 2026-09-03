//! The op-stream hash chain — SHA-256 over the canonical pre-image, identical
//! on every host (a Rust host's chain is byte-identical to an F#/TS host's, so
//! a chain is verifiable cross-runtime). This is the integrity substrate behind
//! the provenance / notarisation / relay demos: a tamper anywhere in a
//! historical record breaks the chain at that record, and [`verify_chain`]
//! names the first break.
//!
//! Pre-image (chain FORMAT version 2, `v` folded FIRST so the chain is
//! self-describing and a relabel is tamper-evident):
//!
//! ```text
//! hash[n] = sha256Hex( previousHash[n] ++ "|" ++
//!   {"seq":<sequence-1>,"actor":<actor>,"op":
//!     {"v":2,"op":<canonical op>,"ts":<unix s>,"promptId":<null|str>,"result":<result>}} )
//! ```
//!
//! `sequence` is the public 1-based value; the pre-image folds Core's 0-based
//! index (`sequence - 1`). Certified byte-for-byte against the shared
//! `chain-corpus.json` golden.

use crate::canonical::escape_string;
use crate::wire::{TreeOp, encode_op};

use super::sha256::sha256_hex;

/// The chain FORMAT version, folded first into every pre-image.
pub const CHAIN_FORMAT_VERSION: u32 = 2;

/// Sixty-four `'0'` characters — the `previous_hash` of every stream's
/// `sequence == 1` record.
pub fn genesis_previous_hash() -> String {
    "0".repeat(64)
}

/// The actor attributed with a record — folded into the integrity chain
/// (attribution is tamper-evident).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    Human {
        id: String,
    },
    Agent {
        model: String,
        version: String,
        id: String,
    },
}

/// The apply outcome an op-record carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpResult {
    Success,
    Failure { code: String, message: String },
}

/// Canonical encoding of the actor — field order pinned (`kind` first, then the
/// case fields), NOT Ordinal-sorted. That pinning is the contract: the same
/// bytes are folded into the linear chain's pre-image here and nested verbatim
/// into the DAG record's wire form (`crate::dag::record`) since Phase 1144, so
/// there is ONE canonical actor encoding in this host rather than two that can
/// drift. Public for that second consumer.
pub fn encode_actor(a: &Actor) -> String {
    match a {
        Actor::Human { id } => format!("{{\"kind\":\"human\",\"id\":{}}}", escape_string(id)),
        Actor::Agent { model, version, id } => format!(
            "{{\"kind\":\"agent\",\"model\":{},\"version\":{},\"id\":{}}}",
            escape_string(model),
            escape_string(version),
            escape_string(id)
        ),
    }
}

/// Canonical encoding of the apply outcome — lowercase tag.
fn encode_result(r: &OpResult) -> String {
    match r {
        OpResult::Success => "{\"kind\":\"success\"}".to_string(),
        OpResult::Failure { code, message } => format!(
            "{{\"kind\":\"failure\",\"code\":{},\"message\":{}}}",
            escape_string(code),
            escape_string(message)
        ),
    }
}

/// The provenance envelope — the opaque `op` payload the chain carries. Field
/// order `v` / op / ts / promptId / result is pinned.
fn encode_stream_entry(
    op: &TreeOp,
    timestamp_unix_seconds: i64,
    prompt_id: Option<&str>,
    result: &OpResult,
) -> String {
    let prompt = match prompt_id {
        None => "null".to_string(),
        Some(p) => escape_string(p),
    };
    format!(
        "{{\"v\":{CHAIN_FORMAT_VERSION},\"op\":{},\"ts\":{timestamp_unix_seconds},\"promptId\":{prompt},\"result\":{}}}",
        encode_op(op),
        encode_result(result)
    )
}

/// Compute the hash for an op-record. Verification re-derives this and
/// compares. `sequence` is the public 1-based value.
#[allow(clippy::too_many_arguments)]
pub fn compute_hash(
    previous_hash: &str,
    op: &TreeOp,
    sequence: u64,
    timestamp_unix_seconds: i64,
    actor: &Actor,
    prompt_id: Option<&str>,
    result: &OpResult,
) -> String {
    let payload = format!(
        "{{\"seq\":{},\"actor\":{},\"op\":{}}}",
        sequence.saturating_sub(1),
        encode_actor(actor),
        encode_stream_entry(op, timestamp_unix_seconds, prompt_id, result)
    );
    sha256_hex(&format!("{previous_hash}|{payload}"))
}

/// One record in the op-stream: the op + its provenance + the chain links.
/// (`PartialEq` only — the op's structured-JSON payloads carry `f64`s, so the
/// wire model is not `Eq`.)
#[derive(Debug, Clone, PartialEq)]
pub struct OpRecord {
    pub sequence: u64,
    pub op: TreeOp,
    pub timestamp_unix_seconds: i64,
    pub actor: Actor,
    pub prompt_id: Option<String>,
    pub result: OpResult,
    pub previous_hash: String,
    pub hash: String,
}

impl OpRecord {
    /// Recompute this record's hash from its own fields (the value the chain
    /// verifier compares against `hash`).
    pub fn recompute_hash(&self) -> String {
        compute_hash(
            &self.previous_hash,
            &self.op,
            self.sequence,
            self.timestamp_unix_seconds,
            &self.actor,
            self.prompt_id.as_deref(),
            &self.result,
        )
    }
}

/// The first integrity break a chain walk finds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationError {
    /// Records are not in contiguous ascending 1-based order.
    OutOfOrder {
        expected_sequence: u64,
        actual_sequence: u64,
    },
    /// `record.previous_hash` does not match the prior record's `hash`.
    PreviousHashMismatch {
        sequence: u64,
        expected: String,
        actual: String,
    },
    /// `record.hash` does not recompute from the record's fields (a tamper).
    HashMismatch {
        sequence: u64,
        expected: String,
        actual: String,
    },
}

/// An append-only op-stream: a hash-chained sequence of op-records. The
/// integrity substrate behind provenance / notarisation / relay.
#[derive(Debug, Clone, Default)]
pub struct OpStream {
    records: Vec<OpRecord>,
}

impl OpStream {
    /// A fresh, empty stream (its next record links to the genesis hash).
    pub fn new() -> Self {
        OpStream {
            records: Vec::new(),
        }
    }

    /// Build a stream from existing records (e.g. decoded from persistence),
    /// without re-verifying — call [`verify`](Self::verify) to check integrity.
    pub fn from_records(records: Vec<OpRecord>) -> Self {
        OpStream { records }
    }

    pub fn records(&self) -> &[OpRecord] {
        &self.records
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The current chain head (the last record's hash, or genesis when empty).
    pub fn head(&self) -> String {
        self.records
            .last()
            .map(|r| r.hash.clone())
            .unwrap_or_else(genesis_previous_hash)
    }

    /// Append an op with its provenance, computing the chain links. Returns the
    /// new record's hash (the new head).
    pub fn append(
        &mut self,
        op: TreeOp,
        timestamp_unix_seconds: i64,
        actor: Actor,
        prompt_id: Option<String>,
        result: OpResult,
    ) -> String {
        let previous_hash = self.head();
        let sequence = self.records.len() as u64 + 1;
        let hash = compute_hash(
            &previous_hash,
            &op,
            sequence,
            timestamp_unix_seconds,
            &actor,
            prompt_id.as_deref(),
            &result,
        );
        self.records.push(OpRecord {
            sequence,
            op,
            timestamp_unix_seconds,
            actor,
            prompt_id,
            result,
            previous_hash,
            hash: hash.clone(),
        });
        hash
    }

    /// Verify the whole chain from genesis: contiguous sequence, linked
    /// previous-hashes, and each hash recomputing to its stored value. Returns
    /// the first break, or `Ok(())` on a clean chain. A tamper to any historical
    /// record surfaces here.
    pub fn verify(&self) -> Result<(), VerificationError> {
        verify_chain(&self.records)
    }
}

/// Verify a record sequence from genesis. Returns the first break.
pub fn verify_chain(records: &[OpRecord]) -> Result<(), VerificationError> {
    let mut previous_hash = genesis_previous_hash();
    for (i, record) in records.iter().enumerate() {
        let expected_sequence = i as u64 + 1;
        if record.sequence != expected_sequence {
            return Err(VerificationError::OutOfOrder {
                expected_sequence,
                actual_sequence: record.sequence,
            });
        }
        if record.previous_hash != previous_hash {
            return Err(VerificationError::PreviousHashMismatch {
                sequence: record.sequence,
                expected: previous_hash,
                actual: record.previous_hash.clone(),
            });
        }
        let recomputed = record.recompute_hash();
        if recomputed != record.hash {
            return Err(VerificationError::HashMismatch {
                sequence: record.sequence,
                expected: recomputed,
                actual: record.hash.clone(),
            });
        }
        previous_hash = record.hash.clone();
    }
    Ok(())
}
