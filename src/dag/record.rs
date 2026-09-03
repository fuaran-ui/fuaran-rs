//! The DAG-record wire form — the multi-parent op record a branching op-stream
//! persists (the substrate for fork / merge topology). A record nests the
//! canonical `TreeOp`, the typed `Actor` that authored it, its parent hashes
//! (0 = genesis, 1 = linear step, 2 = a merge node), and the merge outcome hash
//! + tombstone flag. Round-trip byte-identical to the shared `dag/` corpus.
//!
//! # The typed actor (Phase 1144), and what this host does NOT do
//!
//! The record's attribution member was a bare-string `userId` until Phase 1144
//! replaced it with the typed `actor` the linear chain has carried since Phase
//! 320 — the same `Human | Agent` value, in the same PINNED canonical encoding
//! ([`crate::opstream::encode_actor`]), nested verbatim exactly as the `op` is.
//! Top-level keys are Ordinal-sorted, so `actor` sorts to the FRONT where
//! `userId` sat at the back.
//!
//! The reference host folds that member into the DAG content address, at the
//! same position in the Phase-408 delimited envelope:
//!
//! ```text
//! …,"ts":<unix>,"userId":"alice",                      "promptId":…,"result":…   (408)
//! …,"ts":<unix>,"actor":{"kind":"human","id":"alice"}, "promptId":…,"result":…   (1144)
//! ```
//!
//! **Every DAG address was therefore re-minted, and pre-1144 addresses do not
//! carry forward** — a pre-1144 `hash` is not reproducible under the new
//! pre-image and is not a valid parent link for a post-1144 node.
//!
//! `fuaran-rs` is a CODEC host for this artefact: it mints no DAG content
//! address and verifies none (the only pre-images this crate computes are the
//! LINEAR chain's, in [`crate::opstream::chain`]), so `hash` is an opaque
//! string it round-trips. There was no Rust pre-image to re-derive; the
//! reference envelope above is recorded rather than implemented, and a Rust DAG
//! addresser would be a new capability rather than part of this adoption.
//!
//! Decoding is deliberately NOT dual-read: a pre-1144 `userId` envelope is
//! refused BY NAME rather than lifted to a `Human`, because a lift would mint a
//! record carrying a stored `hash` that no host can reproduce — turning a clear
//! refusal here into a silent verification failure somewhere else.

use crate::canonical::{JVal, render_canonical};
use crate::opstream::{Actor, encode_actor};
use crate::wire::{DecodeError, TreeOp, decode_op, encode_op};

/// A DAG op-record. Optional fields (`outcome_hash`, `prompt_id`) are omitted
/// on the wire when absent; the canonical key order is
/// `actor < hash < op < outcomeHash < parents < promptId < resultEnvelope <
/// streamId < timestamp < tombstoned`.
#[derive(Debug, Clone, PartialEq)]
pub struct DagRecord {
    /// Who authored the op — the typed actor (Phase 1144, replacing the bare
    /// `user_id`). Nested on the wire in its own pinned member order.
    pub actor: Actor,
    pub hash: String,
    pub op: TreeOp,
    /// The merged-tree outcome hash — present on a merge node (2 parents).
    pub outcome_hash: Option<String>,
    /// Parent record hashes (author order): `[]` genesis, `[p]` linear, `[a,b]`
    /// merge.
    pub parents: Vec<String>,
    pub prompt_id: Option<String>,
    /// The apply outcome, kept as its canonical wire form (`{"$type":"Success"}`
    /// / `{"$type":"Failure",…}`) so it round-trips faithfully.
    pub result_envelope: JVal,
    pub stream_id: String,
    pub timestamp: i64,
    pub tombstoned: bool,
}

fn require_str(fields: &JVal, key: &str) -> Result<String, DecodeError> {
    match fields.field(key) {
        Some(JVal::Str(s)) => Ok(s.clone()),
        _ => Err(decode_err(&format!("$.{key}"), "string")),
    }
}

fn decode_err(path: &str, expected: &str) -> DecodeError {
    DecodeError {
        code: crate::wire::DecodeErrorCode::WrongType,
        path: path.to_string(),
        message: format!("DAG record: expected {expected}"),
        expected_shape: Some(expected.to_string()),
    }
}

fn missing(path: &str, message: &str) -> DecodeError {
    DecodeError {
        code: crate::wire::DecodeErrorCode::MissingField,
        path: path.to_string(),
        message: message.to_string(),
        expected_shape: None,
    }
}

/// Read one of the actor's own fields, naming the defect. Never defaults: the
/// actor is inside the reference host's content address, so a guessed one
/// silently invalidates the record's own hash.
fn actor_str(v: &JVal, key: &str) -> Result<String, DecodeError> {
    match v.field(key) {
        Some(JVal::Str(s)) => Ok(s.clone()),
        Some(_) => Err(decode_err(&format!("$.actor.{key}"), "string")),
        None => Err(missing(
            &format!("$.actor.{key}"),
            &format!("DAG record: actor is missing required field '{key}'"),
        )),
    }
}

/// Decode the nested canonical actor object into the typed [`Actor`]. Every
/// defect is named — a non-object is `WRONG_TYPE`, a missing `kind` or case
/// field is `MISSING_FIELD`, and a `kind` outside the closed pair is
/// `UNKNOWN_DU_CASE`.
fn decode_actor(v: &JVal) -> Result<Actor, DecodeError> {
    let JVal::Obj(_) = v else {
        return Err(decode_err("$.actor", "canonical actor object"));
    };
    let kind = match v.field("kind") {
        Some(JVal::Str(k)) => k.clone(),
        Some(_) => return Err(decode_err("$.actor.kind", "string")),
        None => {
            return Err(missing(
                "$.actor.kind",
                "DAG record: actor is missing required field 'kind'",
            ));
        }
    };
    match kind.as_str() {
        "human" => Ok(Actor::Human {
            id: actor_str(v, "id")?,
        }),
        "agent" => Ok(Actor::Agent {
            model: actor_str(v, "model")?,
            version: actor_str(v, "version")?,
            id: actor_str(v, "id")?,
        }),
        other => Err(DecodeError {
            code: crate::wire::DecodeErrorCode::UnknownDuCase,
            path: "$.actor.kind".to_string(),
            message: format!("DAG record: unknown actor kind '{other}' (expected human | agent)"),
            expected_shape: Some("human | agent".to_string()),
        }),
    }
}

/// Decode a canonical DAG-record JSON.
pub fn decode_record(json: &str) -> Result<DagRecord, DecodeError> {
    let v = crate::canonical::parse(json).map_err(|e| DecodeError {
        code: crate::wire::DecodeErrorCode::InvalidJson,
        path: "$".to_string(),
        message: format!("DAG record is not valid JSON: {}", e.message),
        expected_shape: None,
    })?;
    let JVal::Obj(_) = &v else {
        return Err(decode_err("$", "JSON object"));
    };

    // A pre-1144 envelope is refused BY NAME, never lifted — see the module doc.
    let actor = match v.field("actor") {
        Some(a) => decode_actor(a)?,
        None if v.field("userId").is_some() => {
            return Err(missing(
                "$.actor",
                "DAG record: pre-1144 record — 'userId' was replaced by the typed 'actor', \
                 and DAG content addresses do not carry forward",
            ));
        }
        None => {
            return Err(missing(
                "$.actor",
                "DAG record: missing required field 'actor'",
            ));
        }
    };

    let op_jval = v.field("op").ok_or_else(|| decode_err("$.op", "TreeOp"))?;
    let op = decode_op(&render_canonical(op_jval))?;

    let parents = match v.field("parents") {
        Some(JVal::Arr(items)) => items
            .iter()
            .map(|item| match item {
                JVal::Str(s) => Ok(s.clone()),
                _ => Err(decode_err("$.parents[]", "hash string")),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(decode_err("$.parents", "array of hash strings")),
    };

    let result_envelope = v
        .field("resultEnvelope")
        .cloned()
        .ok_or_else(|| decode_err("$.resultEnvelope", "OpResultEnvelope object"))?;

    let timestamp = match v.field("timestamp") {
        Some(JVal::Num(n)) => *n as i64,
        _ => return Err(decode_err("$.timestamp", "unix-seconds number")),
    };
    let tombstoned = match v.field("tombstoned") {
        Some(JVal::Bool(b)) => *b,
        _ => return Err(decode_err("$.tombstoned", "boolean")),
    };

    Ok(DagRecord {
        actor,
        hash: require_str(&v, "hash")?,
        op,
        outcome_hash: match v.field("outcomeHash") {
            None => None,
            Some(JVal::Str(s)) => Some(s.clone()),
            Some(_) => return Err(decode_err("$.outcomeHash", "hash string")),
        },
        parents,
        prompt_id: match v.field("promptId") {
            None => None,
            Some(JVal::Str(s)) => Some(s.clone()),
            Some(_) => return Err(decode_err("$.promptId", "string")),
        },
        result_envelope,
        stream_id: require_str(&v, "streamId")?,
        timestamp,
        tombstoned,
    })
}

/// Encode a DAG record to canonical wire JSON (Ordinal key order, optionals
/// omitted when absent).
///
/// `render_object` sorts the top-level keys but passes each rendered VALUE
/// through verbatim, so the actor's pinned (deliberately unsorted) member order
/// survives — the same mechanism that already carries the nested `op`.
pub fn encode_record(record: &DagRecord) -> String {
    use crate::canonical::{escape_string, render_array};
    let mut fields: Vec<(String, String)> = vec![
        ("actor".to_string(), encode_actor(&record.actor)),
        ("hash".to_string(), escape_string(&record.hash)),
        ("op".to_string(), encode_op(&record.op)),
        (
            "parents".to_string(),
            render_array(
                &record
                    .parents
                    .iter()
                    .map(|p| escape_string(p))
                    .collect::<Vec<_>>(),
            ),
        ),
        (
            "resultEnvelope".to_string(),
            render_canonical(&record.result_envelope),
        ),
        ("streamId".to_string(), escape_string(&record.stream_id)),
        ("timestamp".to_string(), record.timestamp.to_string()),
        (
            "tombstoned".to_string(),
            if record.tombstoned { "true" } else { "false" }.to_string(),
        ),
    ];
    if let Some(outcome) = &record.outcome_hash {
        fields.push(("outcomeHash".to_string(), escape_string(outcome)));
    }
    if let Some(prompt) = &record.prompt_id {
        fields.push(("promptId".to_string(), escape_string(prompt)));
    }
    crate::canonical::render_object(&mut fields)
}
