//! The DAG-record wire form — the multi-parent op record a branching op-stream
//! persists (the substrate for fork / merge topology). A record nests the
//! canonical `TreeOp`, its parent hashes (0 = genesis, 1 = linear step, 2 = a
//! merge node), and the merge outcome hash + tombstone flag. Round-trip
//! byte-identical to the shared `dag/` corpus.

use crate::canonical::{JVal, render_canonical};
use crate::wire::{DecodeError, TreeOp, decode_op, encode_op};

/// A DAG op-record. Optional fields (`outcome_hash`, `prompt_id`) are omitted
/// on the wire when absent; the canonical key order is
/// `hash < op < outcomeHash < parents < promptId < resultEnvelope < streamId <
/// timestamp < tombstoned < userId`.
#[derive(Debug, Clone, PartialEq)]
pub struct DagRecord {
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
    pub user_id: String,
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
        user_id: require_str(&v, "userId")?,
    })
}

/// Encode a DAG record to canonical wire JSON (Ordinal key order, optionals
/// omitted when absent).
pub fn encode_record(record: &DagRecord) -> String {
    use crate::canonical::{escape_string, render_array};
    let mut fields: Vec<(String, String)> = vec![
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
        ("userId".to_string(), escape_string(&record.user_id)),
    ];
    if let Some(outcome) = &record.outcome_hash {
        fields.push(("outcomeHash".to_string(), escape_string(outcome)));
    }
    if let Some(prompt) = &record.prompt_id {
        fields.push(("promptId".to_string(), escape_string(prompt)));
    }
    crate::canonical::render_object(&mut fields)
}
