//! The §18 elicitation artefact — a question posed as a live Fuaran tree plus a
//! typed answer contract, resolving to exactly one typed outcome. Three codecs:
//!
//! - the elicitation envelope (`{$elicitation, contract, default?, id,
//!   timeoutMs?, tree}`) — [`decode_elicitation`] / [`encode_elicitation`];
//! - the closed four-case outcome DU (Answered / Declined / TimedOut /
//!   Superseded) — [`decode_outcome`] / [`encode_outcome`];
//! - the answer-conformance validation (`{answer, contract}`) —
//!   [`decode_answer_doc`].
//!
//! Every object position is strict — undeclared keys are refused
//! (default-deny by shape) and the envelope evolves explicitly via
//! `$elicitation`, not by tolerance. Mirrors the shipped `fuaran-go` shapes
//! case-for-case; the closed value spaces and outcome cases are native `enum`s
//! with exhaustive `match`. The §18 error codes are kept OUT of the core six
//! (like §15's `FOREIGN_PROFILE`); structural faults reuse the core codes on the
//! same `{code, path, message}` envelope.

use crate::canonical::{
    JVal, escape_string, format_number, ordinal_cmp, parse, render_array, render_canonical,
    render_object,
};
use crate::ops::all_node_ids;
use crate::wire::{DecodeError, DecodeErrorCode, Node, decode_node, encode_node};

/// The elicitation format version this codec accepts.
pub const FORMAT_VERSION: &str = "1";

// ─── Error envelope ──────────────────────────────────────────────────────────

/// The error code an elicitation decode surfaces: one of the core six wire
/// codes (reused for structural faults) or a §18-only extension code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElicitErrorCode {
    /// One of the six canonical wire codes.
    Core(DecodeErrorCode),
    UnsupportedVersion,
    UndeclaredField,
    ContractEmpty,
    ContractDuplicateField,
    ContractUnknownNode,
    AnswerMissingField,
    AnswerUndeclaredField,
    AnswerTypeMismatch,
    AnswerOutOfSpace,
    DefaultNonconformant,
}

impl ElicitErrorCode {
    /// The wire-stable string every host emits verbatim.
    pub fn as_str(self) -> &'static str {
        match self {
            ElicitErrorCode::Core(c) => c.as_str(),
            ElicitErrorCode::UnsupportedVersion => "UNSUPPORTED_VERSION",
            ElicitErrorCode::UndeclaredField => "UNDECLARED_FIELD",
            ElicitErrorCode::ContractEmpty => "CONTRACT_EMPTY",
            ElicitErrorCode::ContractDuplicateField => "CONTRACT_DUPLICATE_FIELD",
            ElicitErrorCode::ContractUnknownNode => "CONTRACT_UNKNOWN_NODE",
            ElicitErrorCode::AnswerMissingField => "ANSWER_MISSING_FIELD",
            ElicitErrorCode::AnswerUndeclaredField => "ANSWER_UNDECLARED_FIELD",
            ElicitErrorCode::AnswerTypeMismatch => "ANSWER_TYPE_MISMATCH",
            ElicitErrorCode::AnswerOutOfSpace => "ANSWER_OUT_OF_SPACE",
            ElicitErrorCode::DefaultNonconformant => "DEFAULT_NONCONFORMANT",
        }
    }
}

/// A structured, recoverable elicitation-decode error.
#[derive(Debug, Clone, PartialEq)]
pub struct ElicitError {
    pub code: ElicitErrorCode,
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for ElicitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at {}: {}",
            self.code.as_str(),
            self.path,
            self.message
        )
    }
}

impl std::error::Error for ElicitError {}

fn fail(code: ElicitErrorCode, path: impl Into<String>, message: impl Into<String>) -> ElicitError {
    ElicitError {
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn core(code: DecodeErrorCode, path: impl Into<String>, message: impl Into<String>) -> ElicitError {
    fail(ElicitErrorCode::Core(code), path, message)
}

/// Reroots a payload/tree `DecodeError` under a prefix (e.g. `$.tree`).
fn reroot(e: DecodeError, prefix: &str) -> ElicitError {
    core(e.code, format!("{prefix}{}", &e.path[1..]), e.message)
}

// ─── Value spaces (§18.1) ────────────────────────────────────────────────────

/// A decoded value space — the closed §18.1 vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub enum Space {
    IntRange { min: f64, max: f64 },
    FloatRange { min: f64, max: f64 },
    StringLen { min: f64, max: f64 },
    Enum { values: Vec<String> },
    AnyString,
}

/// One answer-contract field (§18.1). All five keys are required and strict.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub node_id: String,
    pub state_key: String,
    pub required: bool,
    pub space: Space,
}

/// The answer contract: a non-empty ordered field set.
#[derive(Debug, Clone, PartialEq)]
pub struct Contract {
    pub fields: Vec<Field>,
}

/// A decoded §18 envelope: the question tree + the answer contract + optional
/// default/timeout. Round-trip is byte-exact.
#[derive(Debug, Clone, PartialEq)]
pub struct Elicitation {
    pub id: String,
    pub tree: Node,
    pub contract: Contract,
    /// The raw answer object (name→scalar), or `None`.
    pub default: Option<JVal>,
    pub timeout_ms: Option<i64>,
}

/// One of the closed four-case §18.3 outcome shapes, correlated by
/// `elicitation_id`.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Answered {
        elicitation_id: String,
        answer: JVal,
    },
    Declined {
        elicitation_id: String,
    },
    TimedOut {
        elicitation_id: String,
    },
    Superseded {
        elicitation_id: String,
        by: Option<String>,
    },
}

// ─── JVal helpers ────────────────────────────────────────────────────────────

fn obj_fields(v: &JVal) -> Option<&[(String, JVal)]> {
    match v {
        JVal::Obj(f) => Some(f),
        _ => None,
    }
}

fn get<'a>(fields: &'a [(String, JVal)], key: &str) -> Option<&'a JVal> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Rejects any key on `fields` not in `declared`, first offender in Ordinal
/// order (== document order for canonical input).
fn strict_keys(
    fields: &[(String, JVal)],
    declared: &[&str],
    path: &str,
) -> Result<(), ElicitError> {
    let mut keys: Vec<&String> = fields.iter().map(|(k, _)| k).collect();
    keys.sort_by(|a, b| ordinal_cmp(a, b));
    for k in keys {
        if !declared.contains(&k.as_str()) {
            return Err(fail(
                ElicitErrorCode::UndeclaredField,
                format!("{path}.{k}"),
                format!("undeclared key '{k}'"),
            ));
        }
    }
    Ok(())
}

fn require_non_empty(
    fields: &[(String, JVal)],
    key: &str,
    path: &str,
) -> Result<String, ElicitError> {
    match get(fields, key) {
        None => Err(core(
            DecodeErrorCode::MissingField,
            format!("{path}.{key}"),
            format!("missing '{key}'"),
        )),
        Some(JVal::Str(s)) if !s.is_empty() => Ok(s.clone()),
        Some(_) => Err(core(
            DecodeErrorCode::WrongType,
            format!("{path}.{key}"),
            format!("'{key}' must be a non-empty string"),
        )),
    }
}

fn as_num(v: &JVal) -> Option<f64> {
    match v {
        JVal::Num(n) => Some(*n),
        _ => None,
    }
}

fn is_whole_int32(f: f64) -> bool {
    f == f.floor() && f >= i32::MIN as f64 && f <= i32::MAX as f64
}

// ─── Space decode / encode ───────────────────────────────────────────────────

fn decode_space(raw: &JVal, path: &str) -> Result<Space, ElicitError> {
    let fields = obj_fields(raw)
        .ok_or_else(|| core(DecodeErrorCode::WrongType, path, "expected a space object"))?;
    let tag = match get(fields, "$type") {
        None => {
            return Err(core(
                DecodeErrorCode::MissingField,
                format!("{path}.$type"),
                "missing $type discriminator",
            ));
        }
        Some(JVal::Str(s)) => s.as_str(),
        Some(_) => {
            return Err(core(
                DecodeErrorCode::WrongType,
                format!("{path}.$type"),
                "$type must be a string",
            ));
        }
    };
    match tag {
        "intRange" | "floatRange" | "stringLen" => {
            strict_keys(fields, &["$type", "max", "min"], path)?;
            let min = get(fields, "min").and_then(as_num).ok_or_else(|| {
                core(
                    DecodeErrorCode::MissingField,
                    format!("{path}.min"),
                    "missing/invalid 'min'",
                )
            })?;
            let max = get(fields, "max").and_then(as_num).ok_or_else(|| {
                core(
                    DecodeErrorCode::MissingField,
                    format!("{path}.max"),
                    "missing/invalid 'max'",
                )
            })?;
            if min > max {
                return Err(core(DecodeErrorCode::WrongType, path, "min must be <= max"));
            }
            Ok(match tag {
                "intRange" => Space::IntRange { min, max },
                "floatRange" => Space::FloatRange { min, max },
                _ => Space::StringLen { min, max },
            })
        }
        "enum" => {
            strict_keys(fields, &["$type", "values"], path)?;
            let arr = match get(fields, "values") {
                Some(JVal::Arr(a)) if !a.is_empty() => a,
                _ => {
                    return Err(core(
                        DecodeErrorCode::WrongType,
                        format!("{path}.values"),
                        "enum.values must be a non-empty string array",
                    ));
                }
            };
            let mut values = Vec::with_capacity(arr.len());
            for item in arr {
                match item {
                    JVal::Str(s) => values.push(s.clone()),
                    _ => {
                        return Err(core(
                            DecodeErrorCode::WrongType,
                            format!("{path}.values"),
                            "enum.values must be strings",
                        ));
                    }
                }
            }
            Ok(Space::Enum { values })
        }
        "anyString" => {
            strict_keys(fields, &["$type"], path)?;
            Ok(Space::AnyString)
        }
        other => Err(core(
            DecodeErrorCode::UnknownDuCase,
            format!("{path}.$type"),
            format!("unrecognised value-space '{other}'"),
        )),
    }
}

fn encode_space(s: &Space) -> String {
    let range = |tag: &str, min: f64, max: f64| {
        let mut fields = vec![
            ("$type".to_string(), escape_string(tag)),
            ("max".to_string(), format_number(max)),
            ("min".to_string(), format_number(min)),
        ];
        render_object(&mut fields)
    };
    match s {
        Space::IntRange { min, max } => range("intRange", *min, *max),
        Space::FloatRange { min, max } => range("floatRange", *min, *max),
        Space::StringLen { min, max } => range("stringLen", *min, *max),
        Space::Enum { values } => {
            let items: Vec<String> = values.iter().map(|v| escape_string(v)).collect();
            let mut fields = vec![
                ("$type".to_string(), escape_string("enum")),
                ("values".to_string(), render_array(&items)),
            ];
            render_object(&mut fields)
        }
        Space::AnyString => {
            let mut fields = vec![("$type".to_string(), escape_string("anyString"))];
            render_object(&mut fields)
        }
    }
}

// ─── Contract decode ─────────────────────────────────────────────────────────

fn decode_field(raw: &JVal, path: &str) -> Result<Field, ElicitError> {
    let fields = obj_fields(raw)
        .ok_or_else(|| core(DecodeErrorCode::WrongType, path, "expected a field object"))?;
    strict_keys(
        fields,
        &["name", "nodeId", "required", "space", "stateKey"],
        path,
    )?;
    let name = require_non_empty(fields, "name", path)?;
    let node_id = require_non_empty(fields, "nodeId", path)?;
    let state_key = require_non_empty(fields, "stateKey", path)?;
    let required = match get(fields, "required") {
        Some(JVal::Bool(b)) => *b,
        _ => {
            return Err(core(
                DecodeErrorCode::WrongType,
                format!("{path}.required"),
                "required must be a boolean",
            ));
        }
    };
    let space_raw = get(fields, "space").ok_or_else(|| {
        core(
            DecodeErrorCode::MissingField,
            format!("{path}.space"),
            "missing 'space'",
        )
    })?;
    let space = decode_space(space_raw, &format!("{path}.space"))?;
    Ok(Field {
        name,
        node_id,
        state_key,
        required,
        space,
    })
}

fn decode_contract(raw: &JVal, path: &str) -> Result<Contract, ElicitError> {
    let fields = obj_fields(raw).ok_or_else(|| {
        core(
            DecodeErrorCode::WrongType,
            path,
            "expected a contract object",
        )
    })?;
    strict_keys(fields, &["fields"], path)?;
    let arr = match get(fields, "fields") {
        Some(JVal::Arr(a)) => a,
        _ => {
            return Err(core(
                DecodeErrorCode::WrongType,
                format!("{path}.fields"),
                "contract.fields must be an array",
            ));
        }
    };
    if arr.is_empty() {
        return Err(fail(
            ElicitErrorCode::ContractEmpty,
            format!("{path}.fields"),
            "the contract declares no fields",
        ));
    }
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let field_path = format!("{path}.fields[{i}]");
        let f = decode_field(item, &field_path)?;
        if seen.contains(&f.name) {
            return Err(fail(
                ElicitErrorCode::ContractDuplicateField,
                format!("{field_path}.name"),
                format!("duplicate field name '{}'", f.name),
            ));
        }
        seen.push(f.name.clone());
        out.push(f);
    }
    Ok(Contract { fields: out })
}

// ─── Answer conformance (§18.2 / §18.4) ──────────────────────────────────────

fn conforms_to_space(value: &JVal, space: &Space, path: &str) -> Result<(), ElicitError> {
    match space {
        Space::IntRange { min, max } => {
            let f = as_num(value).ok_or_else(|| {
                fail(
                    ElicitErrorCode::AnswerTypeMismatch,
                    path,
                    "expected an integer for an intRange field",
                )
            })?;
            if !is_whole_int32(f) {
                return Err(fail(
                    ElicitErrorCode::AnswerTypeMismatch,
                    path,
                    "value is not a 32-bit integer",
                ));
            }
            if f < *min || f > *max {
                return Err(fail(
                    ElicitErrorCode::AnswerOutOfSpace,
                    path,
                    "integer outside its intRange",
                ));
            }
        }
        Space::FloatRange { min, max } => {
            let f = as_num(value).ok_or_else(|| {
                fail(
                    ElicitErrorCode::AnswerTypeMismatch,
                    path,
                    "expected a number for a floatRange field",
                )
            })?;
            if f < *min || f > *max {
                return Err(fail(
                    ElicitErrorCode::AnswerOutOfSpace,
                    path,
                    "number outside its floatRange",
                ));
            }
        }
        Space::StringLen { min, max } => {
            let JVal::Str(str) = value else {
                return Err(fail(
                    ElicitErrorCode::AnswerTypeMismatch,
                    path,
                    "expected a string for a stringLen field",
                ));
            };
            let n = str.chars().count() as f64;
            if n < *min || n > *max {
                return Err(fail(
                    ElicitErrorCode::AnswerOutOfSpace,
                    path,
                    "string length outside its stringLen bound",
                ));
            }
        }
        Space::Enum { values } => {
            let JVal::Str(str) = value else {
                return Err(fail(
                    ElicitErrorCode::AnswerTypeMismatch,
                    path,
                    "expected a string for an enum field",
                ));
            };
            if !values.contains(str) {
                return Err(fail(
                    ElicitErrorCode::AnswerOutOfSpace,
                    path,
                    "string outside its enum",
                ));
            }
        }
        Space::AnyString => {
            if !matches!(value, JVal::Str(_)) {
                return Err(fail(
                    ElicitErrorCode::AnswerTypeMismatch,
                    path,
                    "expected a string for an anyString field",
                ));
            }
        }
    }
    Ok(())
}

/// Runs the §18.4 answer validation: undeclared answer keys (Ordinal order),
/// then each contract field in declaration order (missing-required, then
/// type-vs-space, then in-space). Returns the first `ANSWER_*` error.
fn validate_answer(
    answer: &[(String, JVal)],
    contract: &Contract,
    prefix: &str,
) -> Result<(), ElicitError> {
    let declared: Vec<&str> = contract.fields.iter().map(|f| f.name.as_str()).collect();
    let mut keys: Vec<&String> = answer.iter().map(|(k, _)| k).collect();
    keys.sort_by(|a, b| ordinal_cmp(a, b));
    for k in keys {
        if !declared.contains(&k.as_str()) {
            return Err(fail(
                ElicitErrorCode::AnswerUndeclaredField,
                format!("{prefix}.{k}"),
                format!("undeclared answer key '{k}'"),
            ));
        }
    }
    for f in &contract.fields {
        match get(answer, &f.name) {
            None => {
                if f.required {
                    return Err(fail(
                        ElicitErrorCode::AnswerMissingField,
                        format!("{prefix}.{}", f.name),
                        format!("required answer field '{}' is absent", f.name),
                    ));
                }
            }
            Some(value) => conforms_to_space(value, &f.space, &format!("{prefix}.{}", f.name))?,
        }
    }
    Ok(())
}

// ─── Envelope decode / encode ────────────────────────────────────────────────

fn parse_obj(text: &str) -> Result<JVal, ElicitError> {
    let raw = parse(text).map_err(|e| {
        core(
            DecodeErrorCode::InvalidJson,
            "$",
            format!("input is not valid JSON: {}", e.message),
        )
    })?;
    if obj_fields(&raw).is_none() {
        return Err(core(
            DecodeErrorCode::WrongType,
            "$",
            "expected an object at $",
        ));
    }
    Ok(raw)
}

/// Run the §18.4 decode + validation pipeline, failing fast with one structured
/// error. Returns the typed envelope on acceptance.
pub fn decode_elicitation(text: &str) -> Result<Elicitation, ElicitError> {
    let raw = parse_obj(text)?;
    let fields = obj_fields(&raw).expect("checked object");
    // 2 — undeclared envelope keys.
    strict_keys(
        fields,
        &[
            "$elicitation",
            "contract",
            "default",
            "id",
            "timeoutMs",
            "tree",
        ],
        "$",
    )?;
    // 3 — version tag.
    match get(fields, "$elicitation") {
        None => {
            return Err(core(
                DecodeErrorCode::MissingField,
                "$.$elicitation",
                "missing '$elicitation' format tag",
            ));
        }
        Some(JVal::Str(v)) if v == FORMAT_VERSION => {}
        Some(JVal::Str(v)) => {
            return Err(fail(
                ElicitErrorCode::UnsupportedVersion,
                "$.$elicitation",
                format!("unsupported elicitation version '{v}'"),
            ));
        }
        Some(_) => {
            return Err(core(
                DecodeErrorCode::WrongType,
                "$.$elicitation",
                "$elicitation must be a string",
            ));
        }
    }
    // 4 — id.
    let id = require_non_empty(fields, "id", "$")?;
    // 5 — tree.
    let tree_raw = get(fields, "tree")
        .ok_or_else(|| core(DecodeErrorCode::MissingField, "$.tree", "missing 'tree'"))?;
    let tree = decode_node(&render_canonical(tree_raw)).map_err(|e| reroot(e, "$.tree"))?;
    // 6 — contract (structure/shape/duplicate, then tree membership).
    let contract_raw = get(fields, "contract").ok_or_else(|| {
        core(
            DecodeErrorCode::MissingField,
            "$.contract",
            "missing 'contract'",
        )
    })?;
    let contract = decode_contract(contract_raw, "$.contract")?;
    let ids = all_node_ids(&tree);
    for (i, f) in contract.fields.iter().enumerate() {
        if !ids.contains(&f.node_id) {
            return Err(fail(
                ElicitErrorCode::ContractUnknownNode,
                format!("$.contract.fields[{i}].nodeId"),
                format!("field nodeId '{}' names no node in the tree", f.node_id),
            ));
        }
    }
    // 7 — timeoutMs.
    let timeout_ms = match get(fields, "timeoutMs") {
        None => None,
        Some(v) => {
            let ok = as_num(v).filter(|f| *f >= 1.0 && *f == f.floor());
            match ok {
                Some(f) => Some(f as i64),
                None => {
                    return Err(core(
                        DecodeErrorCode::WrongType,
                        "$.timeoutMs",
                        "timeoutMs must be an integer >= 1",
                    ));
                }
            }
        }
    };
    // 8 — default (conformance → DEFAULT_NONCONFORMANT).
    let default = match get(fields, "default") {
        None => None,
        Some(v) => {
            let d = obj_fields(v).ok_or_else(|| {
                fail(
                    ElicitErrorCode::DefaultNonconformant,
                    "$.default",
                    "default must be an answer object",
                )
            })?;
            validate_answer(d, &contract, "$.default")
                .map_err(|e| fail(ElicitErrorCode::DefaultNonconformant, e.path, e.message))?;
            Some(v.clone())
        }
    };

    Ok(Elicitation {
        id,
        tree,
        contract,
        default,
        timeout_ms,
    })
}

fn encode_field(f: &Field) -> String {
    let mut fields = vec![
        ("name".to_string(), escape_string(&f.name)),
        ("nodeId".to_string(), escape_string(&f.node_id)),
        (
            "required".to_string(),
            if f.required { "true" } else { "false" }.to_string(),
        ),
        ("space".to_string(), encode_space(&f.space)),
        ("stateKey".to_string(), escape_string(&f.state_key)),
    ];
    render_object(&mut fields)
}

/// Re-encode an envelope to canonical wire JSON (byte-exact round-trip). Keys
/// sort Ordinal: `$elicitation` < `contract` < `default` < `id` < `timeoutMs`
/// < `tree`.
pub fn encode_elicitation(e: &Elicitation) -> String {
    let field_objs: Vec<String> = e.contract.fields.iter().map(encode_field).collect();
    let mut contract_fields = vec![("fields".to_string(), render_array(&field_objs))];
    let contract = render_object(&mut contract_fields);

    let mut fields = vec![
        ("$elicitation".to_string(), escape_string(FORMAT_VERSION)),
        ("contract".to_string(), contract),
        ("id".to_string(), escape_string(&e.id)),
        ("tree".to_string(), encode_node(&e.tree)),
    ];
    if let Some(default) = &e.default {
        fields.push(("default".to_string(), render_canonical(default)));
    }
    if let Some(timeout) = e.timeout_ms {
        fields.push(("timeoutMs".to_string(), timeout.to_string()));
    }
    render_object(&mut fields)
}

// ─── Outcome decode / encode (§18.3) ─────────────────────────────────────────

/// Decode an outcome document. Decoding does NOT check contract conformance
/// (the outcome does not carry the contract).
pub fn decode_outcome(text: &str) -> Result<Outcome, ElicitError> {
    let raw = parse_obj(text)?;
    let fields = obj_fields(&raw).expect("checked object");
    let tag = match get(fields, "$type") {
        None => {
            return Err(core(
                DecodeErrorCode::MissingField,
                "$.$type",
                "missing $type discriminator",
            ));
        }
        Some(JVal::Str(s)) => s.as_str(),
        Some(_) => {
            return Err(core(
                DecodeErrorCode::WrongType,
                "$.$type",
                "$type must be a string",
            ));
        }
    };
    let declared: &[&str] = match tag {
        "Answered" => &["$type", "answer", "elicitationId"],
        "Declined" | "TimedOut" => &["$type", "elicitationId"],
        "Superseded" => &["$type", "by", "elicitationId"],
        other => {
            return Err(core(
                DecodeErrorCode::UnknownDuCase,
                "$.$type",
                format!("unrecognised outcome '{other}'"),
            ));
        }
    };
    strict_keys(fields, declared, "$")?;
    let elicitation_id = require_non_empty(fields, "elicitationId", "$")?;
    match tag {
        "Answered" => {
            let answer_raw = get(fields, "answer").ok_or_else(|| {
                core(
                    DecodeErrorCode::MissingField,
                    "$.answer",
                    "Answered outcome missing 'answer'",
                )
            })?;
            if obj_fields(answer_raw).is_none() {
                return Err(core(
                    DecodeErrorCode::WrongType,
                    "$.answer",
                    "answer must be an object",
                ));
            }
            Ok(Outcome::Answered {
                elicitation_id,
                answer: answer_raw.clone(),
            })
        }
        "Declined" => Ok(Outcome::Declined { elicitation_id }),
        "TimedOut" => Ok(Outcome::TimedOut { elicitation_id }),
        _ => {
            let by = match get(fields, "by") {
                None => None,
                Some(JVal::Str(s)) => Some(s.clone()),
                Some(_) => {
                    return Err(core(
                        DecodeErrorCode::WrongType,
                        "$.by",
                        "by must be a string",
                    ));
                }
            };
            Ok(Outcome::Superseded { elicitation_id, by })
        }
    }
}

/// Re-encode an outcome to canonical wire JSON (byte-exact).
pub fn encode_outcome(o: &Outcome) -> String {
    let mut fields = match o {
        Outcome::Answered {
            elicitation_id,
            answer,
        } => vec![
            ("$type".to_string(), escape_string("Answered")),
            ("answer".to_string(), render_canonical(answer)),
            ("elicitationId".to_string(), escape_string(elicitation_id)),
        ],
        Outcome::Declined { elicitation_id } => vec![
            ("$type".to_string(), escape_string("Declined")),
            ("elicitationId".to_string(), escape_string(elicitation_id)),
        ],
        Outcome::TimedOut { elicitation_id } => vec![
            ("$type".to_string(), escape_string("TimedOut")),
            ("elicitationId".to_string(), escape_string(elicitation_id)),
        ],
        Outcome::Superseded { elicitation_id, by } => {
            let mut f = vec![
                ("$type".to_string(), escape_string("Superseded")),
                ("elicitationId".to_string(), escape_string(elicitation_id)),
            ];
            if let Some(by) = by {
                f.push(("by".to_string(), escape_string(by)));
            }
            f
        }
    };
    render_object(&mut fields)
}

// ─── Answer conformance document (§18.4) ─────────────────────────────────────

/// Run the elicitation-answer conformance document `{"answer":…,"contract":…}`:
/// decode the contract, then validate the answer. The document carries no tree,
/// so `CONTRACT_UNKNOWN_NODE` does not apply. `Ok(())` on acceptance.
pub fn decode_answer_doc(text: &str) -> Result<(), ElicitError> {
    let raw = parse_obj(text)?;
    let fields = obj_fields(&raw).expect("checked object");
    strict_keys(fields, &["answer", "contract"], "$")?;
    let contract_raw = get(fields, "contract").ok_or_else(|| {
        core(
            DecodeErrorCode::MissingField,
            "$.contract",
            "missing 'contract'",
        )
    })?;
    let contract = decode_contract(contract_raw, "$.contract")?;
    let answer_raw = get(fields, "answer").ok_or_else(|| {
        core(
            DecodeErrorCode::MissingField,
            "$.answer",
            "missing 'answer'",
        )
    })?;
    let answer = obj_fields(answer_raw).ok_or_else(|| {
        core(
            DecodeErrorCode::WrongType,
            "$.answer",
            "answer must be an object",
        )
    })?;
    validate_answer(answer, &contract, "$.answer")
}
