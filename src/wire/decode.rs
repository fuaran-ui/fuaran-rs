//! The structural decoder: canonical wire JSON → the typed tree, storage-shape
//! erased (`WIRE_FORMAT.md` §1). Every wire-shape violation surfaces a
//! structured, recoverable [`DecodeError`] — never a panic — carrying one of
//! the six codes plus a `$`-rooted dotted path (§6), path-for-path with the
//! reference hosts so the reject corpus is host-neutral.
//!
//! Closure-bearing slots (§4) decode to presence markers that re-encode to the
//! `"<closure>"` sentinel; opaque `Binding.Static` payloads (§5) decode to the
//! faithful parsed value whose non-primitive forms re-encode as `"<opaque>"` —
//! keeping the round-trip byte-stable.
//!
//! Phase 2333: the per-owner decoders are GENERATED (`gen_decode` in
//! `generated.rs`) from the IDL, and their dispatch over every union's
//! discriminators with them. This file keeps the readers they call, the
//! host-modelled types' decoders (the compute layer, typed `Static` payloads),
//! the two context-taking unions (`Binding` over its `StaticSlot`,
//! `FormFieldKind` over its auto-bind context, each matching exhaustively over
//! its generated tag enum), and the policy hooks `dialect.rs` declares:
//! leniency, aliases, refusals and checks.

use std::collections::BTreeMap;

use crate::canonical::{JVal, parse};

use super::generated::gen_decode::*;
use super::model::*;
use super::result::{DecodeError, DecodeErrorCode};

pub(super) type DResult<T> = Result<T, DecodeError>;

pub(super) const OPAQUE: &str = "<opaque>";

// ─── Error constructors ──────────────────────────────────────────────────────

pub(super) fn make_error(
    code: DecodeErrorCode,
    path: impl Into<String>,
    message: impl Into<String>,
    expected_shape: Option<String>,
) -> DecodeError {
    DecodeError {
        code,
        path: path.into(),
        message: message.into(),
        expected_shape,
    }
}

pub(super) fn missing_field(path: &str, key: &str, expected: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::MissingField,
        format!("{path}.{key}"),
        format!("missing required field '{key}'"),
        Some(expected.to_string()),
    )
}

pub(super) fn wrong_type(path: &str, expected: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::WrongType,
        path,
        format!("expected {expected}"),
        Some(expected.to_string()),
    )
}

/// An unrecognised case at a `$type`-DISCRIMINATED position: the document carries a
/// literal `"$type"` member and its value is not a known case. WIRE_FORMAT.md §6 —
/// "`$type` appears literally in the path when the discriminator is at fault" — so the
/// reported path names that member.
/// The hint lists the union's cases, read from the generated discriminator set,
/// so it cannot name fewer cases than the decoder accepts.
pub(super) fn unknown_du_case(path: &str, got: &str, cases: &[&str]) -> DecodeError {
    make_error(
        DecodeErrorCode::UnknownDuCase,
        format!("{path}.$type"),
        format!("unknown discriminator '{got}'"),
        Some(cases.join(" | ")),
    )
}

/// An unrecognised case at a BARE ENUM position: a plain JSON string in a named field,
/// with no `$type` member anywhere in the document (`style.tone`, `kind.trendPolarity`,
/// `accessibility.liveRegion`, …). Same code — §6's `UNKNOWN_DU_CASE` covers "a `$type`
/// discriminator (or bare-enum string)" — but the path is the FIELD's own, no suffix.
///
/// Phase 1073: this helper did not exist, so `decode_bare_enum!` routed every bare enum
/// through `unknown_du_case` and reported `$.style.tone.$type`, naming a JSON member the
/// document does not contain and cannot be repaired at. It survived because the
/// conformance harness prefix-matches `expectedPath`. Note `channel.direction` was
/// already bare here — written out longhand rather than through the macro — so this host
/// was internally inconsistent as well as divergent. Do not route a bare enum through
/// `unknown_du_case`.
pub(super) fn unknown_enum_case(path: &str, got: &str, expected: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::UnknownDuCase,
        path,
        format!("unknown discriminator '{got}'"),
        Some(expected.to_string()),
    )
}

pub(super) fn null_not_representable(path: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::WrongType,
        path,
        "null is not representable in the Fuaran wire model — omit the field instead",
        None,
    )
}

// ─── AST require-helpers ─────────────────────────────────────────────────────

pub(super) type Fields = [(String, JVal)];

/// Lenient AI-ingest (§3.6, generalised): a `Static` envelope wrapped around a
/// PLAIN scalar unwraps before the scalar readers — the inverse of the
/// bare-scalar-in-Binding-slot confusion, applied at every plain-scalar
/// position in one place. Unambiguous: at a plain-scalar position the envelope
/// has exactly one reading. Objects that are NOT a well-formed Static envelope
/// pass through untouched and fail with the normal error.
pub(super) fn unwrap_static_envelope(j: &JVal) -> &JVal {
    if let JVal::Obj(fields) = j
        && let Some(JVal::Str(t)) = get(fields, "$type")
        && t == "Static"
        && let Some(inner) = get(fields, "value")
    {
        return inner;
    }
    j
}

pub(super) fn as_obj<'a>(path: &str, j: &'a JVal) -> DResult<&'a Fields> {
    match j {
        JVal::Obj(fields) => Ok(fields),
        _ => Err(wrong_type(path, "JSON object")),
    }
}

pub(super) fn as_str<'a>(path: &str, j: &'a JVal) -> DResult<&'a str> {
    match unwrap_static_envelope(j) {
        JVal::Str(s) => Ok(s),
        _ => Err(wrong_type(path, "JSON string")),
    }
}

pub(super) fn as_bool(path: &str, j: &JVal) -> DResult<bool> {
    match unwrap_static_envelope(j) {
        JVal::Bool(b) => Ok(*b),
        _ => Err(wrong_type(path, "JSON boolean")),
    }
}

pub(super) fn as_float(path: &str, j: &JVal) -> DResult<f64> {
    match unwrap_static_envelope(j) {
        JVal::Num(n) => Ok(*n),
        JVal::Str(s) if s == "NaN" => Ok(f64::NAN),
        JVal::Str(s) if s == "Infinity" => Ok(f64::INFINITY),
        JVal::Str(s) if s == "-Infinity" => Ok(f64::NEG_INFINITY),
        _ => Err(wrong_type(
            path,
            "JSON number (or 'NaN' / 'Infinity' / '-Infinity' sentinel string)",
        )),
    }
}

/// The width of every typed integer slot this format declares (§7.1). A
/// different bound from §2 rule 5's ±(2⁵³−1), answering a different question:
/// that one is where integer IDENTITY stops in an untyped payload position,
/// this one is the width of the SLOT, and a value the slot cannot hold has
/// nowhere to land.
pub(super) const INT_SLOT_MIN: f64 = -2_147_483_648.0;
pub(super) const INT_SLOT_MAX: f64 = 2_147_483_647.0;

/// The §7.1 integer-slot accept set: a finite JSON number with no fractional
/// part, inside the signed 32-bit range.
///
/// `2.0` decodes as `2` — the two denote the same integer, and refusing the
/// first refuses a document whose intent is unambiguous, for its spelling.
/// `2.5` is a `WRONG_TYPE` rather than the `trunc()` this host used to apply,
/// which silently discarded the author's value at a slot the author chose to
/// type as an integer. And `1e10` is a `WRONG_TYPE` rather than a cast: that is
/// the row that was measured, since `as i64` saturates here and the equivalent
/// is implementation-defined elsewhere, so the same bytes became
/// `Int32.MinValue` on one runtime and `1410065408` on another.
pub(super) fn as_int(path: &str, j: &JVal) -> DResult<i64> {
    match unwrap_static_envelope(j) {
        JVal::Num(n) => {
            if !n.is_finite() {
                Err(wrong_type(
                    path,
                    "a finite integral JSON number within the signed 32-bit range \
                     (an integer slot has no non-finite form)",
                ))
            } else if n.trunc() != *n {
                Err(wrong_type(
                    path,
                    "a finite integral JSON number within the signed 32-bit range \
                     (an integer slot holds no fraction, and truncating would discard \
                     a value the author typed)",
                ))
            } else if *n < INT_SLOT_MIN || *n > INT_SLOT_MAX {
                Err(wrong_type(
                    path,
                    "a finite integral JSON number within the signed 32-bit range \
                     (a value the slot cannot hold is not a value to be reinterpreted)",
                ))
            } else {
                Ok(*n as i64)
            }
        }
        _ => Err(wrong_type(path, "JSON number (integer)")),
    }
}

pub(super) fn as_arr<'a>(path: &str, j: &'a JVal) -> DResult<&'a [JVal]> {
    match j {
        JVal::Arr(items) => Ok(items),
        _ => Err(wrong_type(path, "JSON array")),
    }
}

pub(super) fn get<'a>(fields: &'a Fields, key: &str) -> Option<&'a JVal> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

pub(super) fn req<'a>(
    path: &str,
    fields: &'a Fields,
    key: &str,
    expected: &str,
) -> DResult<&'a JVal> {
    get(fields, key).ok_or_else(|| missing_field(path, key, expected))
}

pub(super) fn disc<'a>(path: &str, fields: &'a Fields) -> DResult<&'a str> {
    match get(fields, "$type") {
        None => Err(missing_field(
            path,
            "$type",
            "DU object must carry a '$type' discriminator string",
        )),
        Some(JVal::Str(s)) => Ok(s),
        Some(_) => Err(wrong_type(
            &format!("{path}.$type"),
            "JSON string discriminator",
        )),
    }
}

pub(super) fn req_string(
    path: &str,
    fields: &Fields,
    key: &str,
    expected: &str,
) -> DResult<String> {
    let v = req(path, fields, key, expected)?;
    Ok(as_str(&format!("{path}.{key}"), v)?.to_string())
}

pub(super) fn req_int(path: &str, fields: &Fields, key: &str, expected: &str) -> DResult<i64> {
    let v = req(path, fields, key, expected)?;
    as_int(&format!("{path}.{key}"), v)
}

pub(super) fn opt_string(path: &str, fields: &Fields, key: &str) -> DResult<Option<String>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(as_str(&format!("{path}.{key}"), v)?.to_string())),
    }
}

pub(super) fn opt_float(path: &str, fields: &Fields, key: &str) -> DResult<Option<f64>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(as_float(&format!("{path}.{key}"), v)?)),
    }
}

pub(super) fn opt_bool(path: &str, fields: &Fields, key: &str) -> DResult<Option<bool>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(as_bool(&format!("{path}.{key}"), v)?)),
    }
}

/// A closure sentinel slot: presence maps to `Some(Closure)`, absence to `None`
/// (Phases 423/426 — an omitted handler arms the renderer's write-back default).
pub(super) fn opt_closure(fields: &Fields, key: &str) -> Option<Closure> {
    get(fields, key).map(|_| Closure)
}

// ─── Lenient-ingest field-name aliases (decode-only; WIRE_FORMAT.md §3.6) ─────
//
// A curated set of foreign field names decode to the canonical slot when they
// denote the same concept at the same semantics (a model wrote `href` for a
// Navigate `route`, etc.). The canonical name always WINS when both are present;
// a re-encode always normalises back to the canonical name (aliases never
// appear in the schema or the conformance corpus). Mirrors the F# reference's
// `requireFieldAliased` / `optFieldAliased`.

/// Resolve a canonical field, falling back to the first present alias.
pub(super) fn get_aliased<'a>(
    fields: &'a Fields,
    canonical: &str,
    aliases: &[&str],
) -> Option<&'a JVal> {
    get(fields, canonical).or_else(|| aliases.iter().find_map(|a| get(fields, a)))
}

/// Require a canonical field or one of its aliases; the nested path always uses
/// the canonical name (matching the F# reference).
pub(super) fn req_aliased<'a>(
    path: &str,
    fields: &'a Fields,
    canonical: &str,
    aliases: &[&str],
    expected: &str,
) -> DResult<&'a JVal> {
    get_aliased(fields, canonical, aliases).ok_or_else(|| missing_field(path, canonical, expected))
}

pub(super) fn req_string_aliased(
    path: &str,
    fields: &Fields,
    canonical: &str,
    aliases: &[&str],
    expected: &str,
) -> DResult<String> {
    let v = req_aliased(path, fields, canonical, aliases, expected)?;
    Ok(as_str(&format!("{path}.{canonical}"), v)?.to_string())
}

// Prominence intent survives: Strong/Bold→Loud, Subtle/Muted→Quiet. The
// `emphasis` name is a same-name cross-vocabulary collision (style ENUM here
// vs behavioural BOOL on Fact/LabelValueRow) and models cross it in both
// directions: a bool in the enum slot projects one-to-one (true ⇒ Loud,
// false ⇒ Normal). The bool sites' direction lives in `decode_emphasis_flag`.
pub(super) fn decode_emphasis(path: &str, j: &JVal) -> DResult<Emphasis> {
    let s = match unwrap_static_envelope(j) {
        JVal::Bool(true) => return Ok(Emphasis::Loud),
        JVal::Bool(false) => return Ok(Emphasis::Normal),
        JVal::Str(s) => s,
        _ => return Err(wrong_type(path, "JSON string (Emphasis)")),
    };
    if let Some(v) = Emphasis::from_wire(s) {
        return Ok(v);
    }
    match s.as_str() {
        "Strong" | "Bold" => Ok(Emphasis::Loud),
        "Subtle" | "Muted" => Ok(Emphasis::Quiet),
        _ => Err(unknown_enum_case(
            path,
            s,
            &Emphasis::WIRE_NAMES.join(" | "),
        )),
    }
}

/// The behavioural `emphasis` BOOL (Fact / LabelValueRow) — the other half of
/// the same-name collision with the `Emphasis` style enum. Booleans pass
/// through; the enum AND its aliases project one-to-one (Loud/Strong/Bold ⇒
/// true, Normal/Quiet/Subtle/Muted ⇒ false); any other string is the didactic
/// reject naming both vocabularies.
pub(super) fn decode_emphasis_flag(path: &str, j: &JVal) -> DResult<bool> {
    match unwrap_static_envelope(j) {
        JVal::Bool(b) => Ok(*b),
        JVal::Str(s) => match s.as_str() {
            "Loud" | "Strong" | "Bold" => Ok(true),
            "Normal" | "Quiet" | "Subtle" | "Muted" => Ok(false),
            other => Err(make_error(
                DecodeErrorCode::WrongType,
                path.to_string(),
                format!(
                    "expected JSON boolean, got '{other}' — this `emphasis` is a BOOL (is this an emphasised row/fact?); the Emphasis style enum (Quiet|Normal|Loud) lives on style/Metric.emphasis. Write true or false"
                ),
                Some("JSON boolean".to_string()),
            )),
        },
        _ => Err(wrong_type(path, "JSON boolean")),
    }
}

// ─── Strict JVal decode (rule 12 — structured JSON positions) ────────────────

pub(super) fn decode_jval(path: &str, j: &JVal) -> DResult<JVal> {
    match j {
        JVal::Null => Err(null_not_representable(path)),
        JVal::Str(_) | JVal::Bool(_) | JVal::Num(_) => Ok(j.clone()),
        JVal::Arr(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                out.push(decode_jval(&format!("{path}[{i}]"), item)?);
            }
            Ok(JVal::Arr(out))
        }
        JVal::Obj(fields) => {
            let mut out = Vec::with_capacity(fields.len());
            for (k, v) in fields {
                out.push((k.clone(), decode_jval(&format!("{path}.{k}"), v)?));
            }
            Ok(JVal::Obj(out))
        }
    }
}

/// A `TextSource.I18n` argument bag — discriminated BY INSPECTION (§5, Phase 1661).
///
/// An object carrying a `$type` member is a BINDING and decodes as one, so an
/// unrecognised case and a known case missing a required member refuse at the
/// argument's own path rather than passing as an unrecognised object literal;
/// every other JSON value is the LITERAL argument, rule-12 strict.
///
/// A tagged `Static` is read here rather than left to `decode_binding`, so the
/// two spellings of a literal agree. A PRESENT value collapses to
/// `I18nArg::Literal` under the same strict decoder the bare spelling takes —
/// one payload position under two spellings cannot have two null postures, and
/// routing it through the binding decoder would additionally put it on the
/// `StaticValue::Ast` rule-11 path. A missing or null value stays a binding:
/// absence is structural (Phase 677), has no bare spelling, and `decode_binding`
/// is already this host's one implementation of it.
pub(super) fn decode_i18n_args(path: &str, j: &JVal) -> DResult<Vec<(String, I18nArg)>> {
    let fields = as_obj(path, j)?;
    let mut out = Vec::with_capacity(fields.len());
    for (k, v) in fields {
        let arg_path = format!("{path}.{k}");
        let arg = match v {
            JVal::Obj(arg_fields) if get(arg_fields, "$type").is_some() => {
                match (get(arg_fields, "$type"), get(arg_fields, "value")) {
                    (Some(JVal::Str(t)), Some(raw))
                        if t == "Static" && !matches!(raw, JVal::Null) =>
                    {
                        I18nArg::Literal(decode_jval(&format!("{arg_path}.value"), raw)?)
                    }
                    _ => I18nArg::Bound(decode_binding(&arg_path, v)?),
                }
            }
            literal => I18nArg::Literal(decode_jval(&arg_path, literal)?),
        };
        out.push((k.clone(), arg));
    }
    Ok(out)
}

// ─── Compute layer (Core-style string errors) ────────────────────────────────

pub(super) type CResult<T> = Result<T, String>;

pub(super) fn ast_kind(j: &JVal) -> &'static str {
    match j {
        JVal::Str(_) => "string",
        JVal::Num(_) => "number",
        JVal::Bool(_) => "bool",
        JVal::Arr(_) => "array",
        JVal::Obj(_) => "object",
        JVal::Null => "null",
    }
}

pub(super) fn c_obj(j: &JVal) -> CResult<&Fields> {
    match j {
        JVal::Obj(fields) => Ok(fields),
        _ => Err(format!("malformed: expected object, got {}", ast_kind(j))),
    }
}

pub(super) fn c_field<'a>(fields: &'a Fields, key: &str) -> CResult<&'a JVal> {
    get(fields, key).ok_or_else(|| format!("missing field: {key}"))
}

pub(super) fn c_str(j: &JVal) -> CResult<String> {
    match j {
        JVal::Str(s) => Ok(s.clone()),
        _ => Err(format!("malformed: expected string, got {}", ast_kind(j))),
    }
}

pub(super) fn c_arr(j: &JVal) -> CResult<&[JVal]> {
    match j {
        JVal::Arr(items) => Ok(items),
        _ => Err(format!("malformed: expected array, got {}", ast_kind(j))),
    }
}

/// The coercion-bridge integer reader, held to the same §7.1 accept set as
/// `as_int`. A second reader with a looser rule would be an undeclared divergent
/// entry point under §20.1 — the coercion bridge is reached from
/// `TreeOp::UpdateProp`, which is wire input like any other.
pub(super) fn c_int(j: &JVal) -> CResult<i64> {
    match j {
        JVal::Num(n) if !n.is_finite() => {
            Err("malformed: expected a finite integer, got a non-finite number".to_string())
        }
        JVal::Num(n) if n.trunc() != *n => Err(format!(
            "malformed: expected an integer, got {n} — an integer slot holds no fraction"
        )),
        JVal::Num(n) if *n < INT_SLOT_MIN || *n > INT_SLOT_MAX => Err(format!(
            "malformed: {n} is outside the signed 32-bit range a typed integer slot can hold"
        )),
        JVal::Num(n) => Ok(*n as i64),
        _ => Err(format!("malformed: expected int, got {}", ast_kind(j))),
    }
}

pub(super) fn c_str_field(fields: &Fields, key: &str) -> CResult<String> {
    c_str(c_field(fields, key)?)
}

pub(super) fn c_str_list(j: &JVal) -> CResult<Vec<String>> {
    c_arr(j)?.iter().map(c_str).collect()
}

pub(super) fn c_enum<T: Copy>(
    value: &str,
    parse_wire: fn(&str) -> Option<T>,
    names: &[&str],
) -> CResult<T> {
    parse_wire(value).ok_or_else(|| {
        format!(
            "unknown column type '{value}'; expected one of: {}",
            names.join(", ")
        )
    })
}

pub(super) fn decode_cell_lit(j: &JVal) -> CResult<Cell> {
    let fields = c_obj(j).map_err(|_| "malformed: lit: expected object".to_string())?;
    let tag = match get(fields, "$type") {
        Some(JVal::Str(s)) => s.clone(),
        _ => return Err("missing field: lit.$type".to_string()),
    };
    if tag == "Null" {
        return Ok(Cell::Null);
    }
    let mismatch = || format!("column 'lit': expected {tag} value, got value");
    let v = get(fields, "value").ok_or_else(mismatch)?;
    match (tag.as_str(), v) {
        ("Int", JVal::Num(n)) => Ok(Cell::Int(n.trunc() as i64)),
        ("Float", JVal::Num(n)) => Ok(Cell::Float(*n)),
        ("Bool", JVal::Bool(b)) => Ok(Cell::Bool(*b)),
        ("Str", JVal::Str(s)) => Ok(Cell::Str(s.clone())),
        ("Date", JVal::Str(s)) => Ok(Cell::Date(s.clone())),
        ("Timestamp", JVal::Str(s)) => Ok(Cell::Timestamp(s.clone())),
        // Phase 2001: decimal TEXT, canonicalised on read; an integer token (the
        // int32 range a literal int holds) is exact and read too; anything else,
        // a fractional number token included, is refused.
        ("Decimal", JVal::Str(s)) => crate::core::decimal::try_canonical(s)
            .map(Cell::Decimal)
            .ok_or_else(mismatch),
        ("Decimal", JVal::Num(n))
            if *n == n.trunc() && *n >= INT_SLOT_MIN && *n <= INT_SLOT_MAX =>
        {
            Ok(Cell::Decimal((*n as i64).to_string()))
        }
        _ => Err(mismatch()),
    }
}

/// The largest integer a JSON number token carries exactly — the bound past
/// which a whole-valued token has been through a float (Phase 2001).
const INT53_MAX: f64 = 9_007_199_254_740_992.0;

pub(super) fn decode_column_cell(col_name: &str, ty: ColumnType, v: &JVal) -> CResult<Cell> {
    let mismatch = || {
        format!(
            "column '{col_name}': expected {} value, got {}",
            ty.as_str(),
            ast_kind(v)
        )
    };
    match (ty, v) {
        (ColumnType::Int, JVal::Num(n)) => Ok(Cell::Int(n.trunc() as i64)),
        (ColumnType::Float, JVal::Num(n)) => Ok(Cell::Float(*n)),
        (ColumnType::Bool, JVal::Bool(b)) => Ok(Cell::Bool(*b)),
        (ColumnType::Str, JVal::Str(s)) => Ok(Cell::Str(s.clone())),
        (ColumnType::Date, JVal::Str(s)) => Ok(Cell::Date(s.clone())),
        // Phase 2001: a decimal column reads a STRING of decimal text and
        // canonicalises it; it reads a whole-valued number token within 2^53 as
        // its digits, and refuses a fractional token or one past 2^53 — that
        // value has been through a float, and an exact type cannot vouch for it.
        (ColumnType::Decimal, JVal::Str(s)) => crate::core::decimal::try_canonical(s)
            .map(Cell::Decimal)
            .ok_or_else(|| {
                format!(
                    "malformed: column '{col_name}': a decimal value must be decimal text — an optional '-', digits, and an optional '.' followed by digits, with no exponent, sign '+', separator or white space"
                )
            }),
        (ColumnType::Decimal, JVal::Num(n)) if *n == n.trunc() && n.abs() <= INT53_MAX => {
            Ok(Cell::Decimal((*n as i64).to_string()))
        }
        _ => match ty.timestamp_digits() {
            Some(digits) => {
                decode_instant(col_name, ty, digits, v).unwrap_or_else(|| Err(mismatch()))
            }
            None => Err(mismatch()),
        },
    }
}

/// The epoch seconds the canonical instant text spells: `0000-01-01T00:00:00Z`
/// and `9999-12-31T23:59:59Z`.
const MIN_EPOCH_SECOND: i64 = -62_167_219_200;
const MAX_EPOCH_SECOND: i64 = 253_402_300_799;

/// A timestamp column's present value at a unit holding `digits` fraction
/// digits (WIRE_FORMAT §3.6.25, fuaran-core#422). TEXT is read only in its
/// canonical form, with a fraction no finer than the unit (a coarser instant
/// widens in unchanged — one instant has one text). An EPOCH NUMBER
/// (whole-valued, |x| < 9e15) is read in a `timestamp` column by magnitude, as
/// Core Phase 94 pinned it (>= 1e11 => milliseconds, truncated to the second),
/// and in a sub-second column in its own unit with no guess; either way it must
/// name an instant in the years 0000-9999. `None` for a value of another kind.
fn decode_instant(col_name: &str, ty: ColumnType, digits: u32, v: &JVal) -> Option<CResult<Cell>> {
    match v {
        JVal::Str(s) => Some(match instant_fraction_digits(s) {
            None => Err(format!(
                "malformed: {col_name}: a {} value must be canonical ISO-8601 text, YYYY-MM-DDThh:mm:ssZ or YYYY-MM-DDThh:mm:ss.FZ, naming a moment that exists",
                ty.as_str()
            )),
            Some(n) if n > digits => Err(format!(
                "malformed: {col_name}: a {} value's fraction is finer than the column's unit holds: {} or finer is needed",
                ty.as_str(),
                ColumnType::timestamp_of_digits(n).as_str()
            )),
            Some(_) => Ok(Cell::Timestamp(s.clone())),
        }),
        JVal::Num(n) if *n == n.trunc() && n.abs() < 9e15 => {
            let i = *n as i64;
            let (second, fraction) = if digits == 0 {
                (
                    if i.abs() >= 100_000_000_000 {
                        i / 1000
                    } else {
                        i
                    },
                    0,
                )
            } else {
                let scale = 10_i64.pow(digits);
                (i.div_euclid(scale), i.rem_euclid(scale))
            };
            if !(MIN_EPOCH_SECOND..=MAX_EPOCH_SECOND).contains(&second) {
                return Some(Err(format!(
                    "malformed: {col_name}: the epoch {i} names an instant outside the years 0000-9999 the canonical timestamp spells"
                )));
            }
            let whole = iso_of_epoch_seconds(second);
            let text = if fraction == 0 {
                whole
            } else {
                let frac = format!("{fraction:0width$}", width = digits as usize);
                format!("{}.{}Z", &whole[..19], frac.trim_end_matches('0'))
            };
            Some(Ok(Cell::Timestamp(text)))
        }
        _ => None,
    }
}

/// Render an epoch-seconds instant as the canonical ISO-8601 UTC timestamp
/// string. Pure integer civil-from-days arithmetic — deterministic and
/// clock-free; negative epochs (pre-1970) are handled.
pub(super) fn iso_of_epoch_seconds(secs: i64) -> String {
    let days = {
        let d = secs / 86_400;
        if secs % 86_400 < 0 { d - 1 } else { d }
    };
    let sod = secs - days * 86_400;
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        sod / 3600,
        sod % 3600 / 60,
        sod % 60
    )
}

pub(super) fn decode_col_schema(j: &JVal) -> CResult<Vec<SchemaEntry>> {
    c_arr(j)?.iter().map(decode_schema_entry).collect()
}

/// One schema entry (WIRE_FORMAT §3.6.26): `name` and `type`, then the metadata
/// members when present. `unit` is read through the unit algebra — so it
/// re-encodes in its one canonical text — and REFUSED, naming the column and
/// the reason, where the algebra does not parse it; `label` and `description`
/// must be strings; `ext` must be an object of strings, and an empty one states
/// nothing. A member the entry does not define is read past.
fn decode_schema_entry(e: &JVal) -> CResult<SchemaEntry> {
    let fields = c_obj(e)?;
    let name = c_str_field(fields, "name")?;
    let ty = c_str_field(fields, "type")?;
    let column_type = c_enum(&ty, ColumnType::from_wire, ColumnType::WIRE_NAMES)?;
    let text = |member: &str| -> CResult<Option<String>> {
        match get(fields, member) {
            None => Ok(None),
            Some(JVal::Str(s)) => Ok(Some(s.clone())),
            Some(other) => Err(format!(
                "malformed: {name}: schema entry member '{member}' must be a string, got {}",
                ast_kind(other)
            )),
        }
    };
    let mut entry = SchemaEntry::new(name.clone(), column_type);
    if let Some(unit) = text("unit")? {
        entry.unit = Some(crate::core::unit::parse(&unit).map_err(|r| {
            format!(
                "malformed: {name}: unit '{unit}' is not a unit the algebra parses: {}",
                r.text()
            )
        })?);
    }
    entry.label = text("label")?;
    entry.description = text("description")?;
    match get(fields, "ext") {
        None => {}
        Some(JVal::Obj(members)) => {
            for (key, value) in members {
                let JVal::Str(value) = value else {
                    return Err(format!(
                        "malformed: {name}: schema entry extension member '{key}' must be a string, got {}",
                        ast_kind(value)
                    ));
                };
                entry.ext.insert(key.clone(), value.clone());
            }
        }
        Some(other) => {
            return Err(format!(
                "malformed: {name}: schema entry member 'ext' must be an object, got {}",
                ast_kind(other)
            ));
        }
    }
    Ok(entry)
}

/// Lenient-ingest (Core Phases 88 + 94) — the `(values, validity)` parts of a
/// column element: a BARE JSON array is the "just the data" shorthand (an
/// all-present mask is synthesised — the wire has no JSON null, so a bare
/// array can only mean every cell present); a wrapped object carrying `values`
/// but no `validity` is the same all-present statement. The full wrapped form
/// stays canonical.
pub(super) fn column_parts<'a>(
    name: &str,
    col: &'a JVal,
) -> CResult<(&'a [JVal], std::borrow::Cow<'a, [JVal]>)> {
    use std::borrow::Cow;
    if let JVal::Arr(xs) = col {
        return Ok((xs, Cow::Owned(vec![JVal::Bool(true); xs.len()])));
    }
    let fields = c_obj(col)?;
    let values = c_arr(c_field(fields, "values").map_err(|e| format!("{name}: {e}"))?)?;
    match get(fields, "validity") {
        None => Ok((values, Cow::Owned(vec![JVal::Bool(true); values.len()]))),
        Some(v) => Ok((values, Cow::Borrowed(c_arr(v)?))),
    }
}

pub(super) fn decode_data_column(
    columns: &Fields,
    name: &str,
    ty: ColumnType,
) -> CResult<DataColumn> {
    let col = get(columns, name).ok_or_else(|| format!("missing field: columns.{name}"))?;
    let (values, validity) = column_parts(name, col)?;
    let validity: &[JVal] = &validity;
    if values.len() != validity.len() {
        return Err(format!(
            "column '{name}': values/validity length mismatch ({} vs {})",
            values.len(),
            validity.len()
        ));
    }
    let mut cells = Vec::with_capacity(values.len());
    for (value, present) in values.iter().zip(validity) {
        match present {
            JVal::Bool(false) => cells.push(Cell::Null),
            JVal::Bool(true) => cells.push(decode_column_cell(name, ty, value)?),
            other => {
                return Err(format!(
                    "malformed: {name}.validity: expected bool, got {}",
                    ast_kind(other)
                ));
            }
        }
    }
    Ok(DataColumn {
        name: name.to_string(),
        column_type: ty,
        cells,
    })
}

/// Lenient-ingest (Core Phase 88) — infer one column's type from its cells.
/// Pinned deterministic rules: all-int numerics => int, any fractional =>
/// float, all-bool => bool, all-string => string — never date/timestamp
/// (temporal types require a declared schema). Empty / mixed is a didactic
/// reject naming the explicit-schema remedy.
pub(super) fn infer_column_type(name: &str, values: &[JVal]) -> CResult<ColumnType> {
    if values.is_empty() {
        return Err(format!(
            "malformed: {name}: cannot infer a column type from an empty / all-null column — declare it in an explicit \"schema\" array"
        ));
    }
    let mut saw_int = false;
    let mut saw_float = false;
    let mut saw_bool = false;
    let mut saw_str = false;
    let mut saw_other = false;
    for v in values {
        match v {
            JVal::Num(n) if *n == n.trunc() => saw_int = true,
            JVal::Num(_) => saw_float = true,
            JVal::Bool(_) => saw_bool = true,
            JVal::Str(_) => saw_str = true,
            _ => saw_other = true,
        }
    }
    match (saw_int, saw_float, saw_bool, saw_str, saw_other) {
        (true, false, false, false, false) => Ok(ColumnType::Int),
        (_, true, false, false, false) => Ok(ColumnType::Float),
        (false, false, true, false, false) => Ok(ColumnType::Bool),
        (false, false, false, true, false) => Ok(ColumnType::Str),
        _ => Err(format!(
            "malformed: {name}: cannot infer a single column type from mixed cell kinds — declare it in an explicit \"schema\" array"
        )),
    }
}

pub(super) fn decode_data_source(j: &JVal) -> CResult<DataSource> {
    let fields = c_obj(j)?;
    // Lenient-ingest (Core Phase 88): `schema` may be omitted on an EMBEDDED
    // source (inferred per column, Ordinal key order); a `ref` source still
    // requires it. The canonical encoder always emits the explicit schema.
    let declared = match get(fields, "schema") {
        Some(s) => Some(decode_col_schema(s)?),
        None => None,
    };
    if let Some(r) = get(fields, "ref") {
        if declared.is_none() {
            return Err(
                "malformed: a ref source requires an explicit \"schema\" array — there are no cells to infer column types from"
                    .to_string(),
            );
        }
        return Ok(DataSource::Ref { name: c_str(r)? });
    }
    let cols_obj = c_obj(c_field(fields, "columns")?)?;
    let schema = match declared {
        Some(schema) => schema,
        None => {
            let mut names: Vec<&String> = cols_obj.iter().map(|(k, _)| k).collect();
            names.sort();
            let mut out = Vec::with_capacity(names.len());
            for name in names {
                let col =
                    get(cols_obj, name).ok_or_else(|| format!("missing field: columns.{name}"))?;
                let (values, _) = column_parts(name, col)?;
                out.push(SchemaEntry::new(
                    name.clone(),
                    infer_column_type(name, values)?,
                ));
            }
            out
        }
    };
    let columns = schema
        .iter()
        .map(|e| decode_data_column(cols_obj, &e.name, e.column_type))
        .collect::<CResult<Vec<_>>>()?;
    Ok(DataSource::Embedded { schema, columns })
}

/// Every direct sub-expression of `e`, so the three walks below share one
/// definition of the shape and cannot disagree about which arms recurse.
/// The `params` slot shared by `Binding::Transform` and `Binding::Expr` —
/// ONE decoder, because §3.3.2 makes it the SAME slot following the same rules,
/// and two copies would drift on the lenient form below.
///
/// Lenient AI-ingest (§3.6): a `{name: <Binding>}` MAP is accepted alongside
/// the canonical `[{from, name}]` array — normalised to the array form sorted
/// by name (the reference host's map iteration order). Omitted when empty.
pub(super) fn decode_binding_params(
    path: &str,
    fields: &Fields,
) -> DResult<Option<Vec<TransformParam>>> {
    match get(fields, "params") {
        None => Ok(None),
        Some(JVal::Obj(map_fields)) => {
            let mut entries: Vec<(&String, &JVal)> =
                map_fields.iter().map(|(k, v)| (k, v)).collect();
            entries.sort_by_key(|(k, _)| *k);
            let mut out = Vec::with_capacity(entries.len());
            for (name, from_j) in entries {
                let from = decode_binding(&format!("{path}.params.{name}.from"), from_j)?;
                out.push(TransformParam {
                    name: name.clone(),
                    from,
                });
            }
            Ok(Some(out))
        }
        Some(v) => {
            let items = as_arr(&format!("{path}.params"), v)?;
            let p = format!("{path}.params[]");
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(decode_transform_param(&p, item)?);
            }
            Ok(Some(out))
        }
    }
}

pub(super) fn expr_children(e: &ColExpr) -> Vec<&ColExpr> {
    match e {
        ColExpr::Col { .. } | ColExpr::Param { .. } | ColExpr::Lit { .. } => vec![],
        ColExpr::Binary { left, right, .. } => vec![left, right],
        ColExpr::Not { expr } | ColExpr::Cast { expr, .. } | ColExpr::IsNull { expr } => vec![expr],
        ColExpr::Coalesce { exprs } | ColExpr::Apply { args: exprs, .. } => exprs.iter().collect(),
        ColExpr::Case { cases, else_expr } => {
            let mut out: Vec<&ColExpr> = Vec::with_capacity(cases.len() * 2 + 1);
            for arm in cases {
                out.push(&arm.when);
                out.push(&arm.then);
            }
            out.push(else_expr);
            out
        }
        ColExpr::InList { subject, items } => {
            let mut out: Vec<&ColExpr> = vec![subject];
            out.extend(items.iter());
            out
        }
        ColExpr::InParam { subject, .. } => vec![subject],
        ColExpr::Quotient {
            dividend, divisor, ..
        } => vec![dividend, divisor],
        ColExpr::Rounded { expr, .. } | ColExpr::Convert { expr, .. } => vec![expr],
    }
}

/// The `ColExpr` node count of one expression — the subject of §21.8's
/// `MaxExprNodes`, which is counted per EXPRESSION rather than per document.
pub(super) fn count_expr_nodes(e: &ColExpr) -> usize {
    1 + expr_children(e)
        .into_iter()
        .map(count_expr_nodes)
        .sum::<usize>()
}

/// §21.8's expression-node bound over the expressions a `Binding::Transform`
/// PIPELINE embeds (Phase 1662) — the first breach, as the `(index, slot)` of
/// the offending step member, or `None`.
///
/// `MAX_EXPR_NODES` bounded `Binding::Expr` alone until now, which made it
/// bypassable by wrapping the expression in a Transform: a `derive`'s
/// expression and a `filter`'s predicate reach the same evaluator and carried
/// no ceiling on any host.
///
/// `Filter` and `Derive` are the whole surface — the only `TransformStep` arms
/// carrying a `ColExpr`; a `Join` / `Union` operand is a `DataSource` (embedded
/// table or named ref), never another pipeline — so there is no recursive axis
/// to descend. The match is written arm-by-arm with no catch-all so that a new
/// expression-bearing step is a build error here rather than a silent hole.
///
/// Same budget, counted per EMBEDDED EXPRESSION, refused with `LIMIT_EXCEEDED`
/// at the path of the offending `pred` / `expr` member so an author is told
/// which STEP to come back under. The first breach wins. `count_expr_nodes` is
/// reused rather than re-derived — its figure is exactly what this bound wants,
/// and the `col` verdict `Binding::Expr` also needs is irrelevant here, since a
/// `col` is perfectly ordinary in a pipeline expression.
pub(super) fn first_pipeline_expr_breach(
    pipeline: &[TransformStep],
) -> Option<(usize, &'static str)> {
    pipeline.iter().enumerate().find_map(|(i, step)| {
        let (slot, expr) = match step {
            TransformStep::Filter { pred } => ("pred", pred),
            TransformStep::Derive { expr, .. } => ("expr", expr),
            TransformStep::Project { .. }
            | TransformStep::GroupBy { .. }
            | TransformStep::Join { .. }
            | TransformStep::Window { .. }
            | TransformStep::Pivot { .. }
            | TransformStep::Unpivot { .. }
            | TransformStep::Sort { .. }
            | TransformStep::Distinct
            | TransformStep::Limit { .. }
            | TransformStep::Union { .. }
            | TransformStep::Intersect { .. }
            | TransformStep::Except { .. } => return None,
        };
        (count_expr_nodes(expr) > crate::limits::MAX_EXPR_NODES).then_some((i, slot))
    })
}

/// The first `col` reference reachable in `e`, if any (§3.3.2 refusal 1).
pub(super) fn first_col_reference(e: &ColExpr) -> Option<&str> {
    if let ColExpr::Col { name } = e {
        return Some(name);
    }
    expr_children(e).into_iter().find_map(first_col_reference)
}

/// The first param name `e` references that `bound` does not carry, if any
/// (§3.3.2 refusal 2). `InParam`'s name is a param too — it is the LIST
/// spelling of the same reference, so leaving it out would admit an unbound
/// membership test through the one arm that reads a param without being one.
pub(super) fn first_unbound_param<'a>(e: &'a ColExpr, bound: &[&str]) -> Option<&'a str> {
    let named = match e {
        ColExpr::Param { name } | ColExpr::InParam { name, .. } => Some(name.as_str()),
        _ => None,
    };
    if let Some(name) = named
        && !bound.contains(&name)
    {
        return Some(name);
    }
    expr_children(e)
        .into_iter()
        .find_map(|child| first_unbound_param(child, bound))
}

pub(super) fn decode_col_expr(j: &JVal) -> CResult<ColExpr> {
    let fields = c_obj(j)?;
    let tag = c_str_field(fields, "$type")?;
    match tag.as_str() {
        "col" => Ok(ColExpr::Col {
            name: c_str_field(fields, "name")?,
        }),
        "param" => Ok(ColExpr::Param {
            name: c_str_field(fields, "name")?,
        }),
        "lit" => Ok(ColExpr::Lit {
            cell: decode_cell_lit(c_field(fields, "cell")?)?,
        }),
        "binary" => {
            let op = c_str_field(fields, "op")?;
            let op = c_enum(&op, BinOp::from_wire, BinOp::WIRE_NAMES)?;
            let left = decode_col_expr(c_field(fields, "left")?)?;
            let right = decode_col_expr(c_field(fields, "right")?)?;
            Ok(ColExpr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            })
        }
        "not" => Ok(ColExpr::Not {
            expr: Box::new(decode_col_expr(c_field(fields, "expr")?)?),
        }),
        "coalesce" => {
            let exprs = c_arr(c_field(fields, "exprs")?)?
                .iter()
                .map(decode_col_expr)
                .collect::<CResult<Vec<_>>>()?;
            Ok(ColExpr::Coalesce { exprs })
        }
        "case" => {
            let cases = c_arr(c_field(fields, "cases")?)?
                .iter()
                .map(|c| {
                    let cf = c_obj(c)?;
                    Ok(CaseArm {
                        when: decode_col_expr(c_field(cf, "when")?)?,
                        then: decode_col_expr(c_field(cf, "then")?)?,
                    })
                })
                .collect::<CResult<Vec<_>>>()?;
            let else_expr = decode_col_expr(c_field(fields, "else")?)?;
            Ok(ColExpr::Case {
                cases,
                else_expr: Box::new(else_expr),
            })
        }
        "cast" => {
            let ty = c_str_field(fields, "type")?;
            let column_type = c_enum(&ty, ColumnType::from_wire, ColumnType::WIRE_NAMES)?;
            Ok(ColExpr::Cast {
                column_type,
                expr: Box::new(decode_col_expr(c_field(fields, "expr")?)?),
            })
        }
        // Lenient-ingest (Core Phases 93/94): `call` and `fn` alias `apply`
        // (same fn/args fields); an `expr` field stands in for a one-element
        // `args`.
        "apply" | "call" | "fn" => {
            let f = c_str_field(fields, "fn")?;
            let func = c_enum(&f, ScalarFn::from_wire, ScalarFn::WIRE_NAMES)?;
            let args = expr_args(fields)?;
            Ok(ColExpr::Apply { func, args })
        }
        "in" => {
            let subject = decode_col_expr(c_field(fields, "expr")?)?;
            // Exactly one of `items` (literal list) / `param` (a bound
            // multi-select list param) — Core Phase 91.
            match (get(fields, "items"), get(fields, "param")) {
                (Some(items_j), None) => {
                    let items = c_arr(items_j)?
                        .iter()
                        .map(decode_col_expr)
                        .collect::<CResult<Vec<_>>>()?;
                    Ok(ColExpr::InList {
                        subject: Box::new(subject),
                        items,
                    })
                }
                (None, Some(p)) => Ok(ColExpr::InParam {
                    subject: Box::new(subject),
                    name: c_str(p)?,
                }),
                (Some(_), Some(_)) => Err(
                    "malformed: in: give exactly ONE of \"items\" (a literal list) or \"param\" (a multi-select list param), not both"
                        .to_string(),
                ),
                (None, None) => Err("missing field: items".to_string()),
            }
        }
        "isNull" => Ok(ColExpr::IsNull {
            expr: Box::new(decode_col_expr(c_field(fields, "expr")?)?),
        }),
        // Phase 2001: the two exact-number forms, each carrying a rounding.
        "quotient" => Ok(ColExpr::Quotient {
            dividend: Box::new(decode_col_expr(c_field(fields, "dividend")?)?),
            divisor: Box::new(decode_col_expr(c_field(fields, "divisor")?)?),
            rounding: decode_rounding(c_field(fields, "rounding")?)?,
        }),
        "rounded" => Ok(ColExpr::Rounded {
            expr: Box::new(decode_col_expr(c_field(fields, "expr")?)?),
            rounding: decode_rounding(c_field(fields, "rounding")?)?,
        }),
        // Phase 2208: the operand brought to a unit, read by the unit algebra.
        "convert" => {
            let text = c_str(c_field(fields, "unit")?)?;
            let unit = crate::core::unit::parse(&text)
                .map_err(|_| format!("malformed: convert: '{text}' is not a unit"))?;
            Ok(ColExpr::Convert {
                expr: Box::new(decode_col_expr(c_field(fields, "expr")?)?),
                unit,
            })
        }
        // Lenient-ingest (Core Phase 93): expression-level string-predicate
        // spellings — {"$type":"contains","expr":X,"other":Y} (also
        // left/right) denotes exactly the canonical binary form.
        tag @ ("contains" | "startsWith" | "endsWith") => {
            let op = match tag {
                "contains" => BinOp::Contains,
                "startsWith" => BinOp::StartsWith,
                _ => BinOp::EndsWith,
            };
            let left = decode_col_expr(c_field_aliased(fields, "left", "expr")?)?;
            let right = decode_col_expr(c_field_aliased(fields, "right", "other")?)?;
            Ok(ColExpr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            })
        }
        // Lenient-ingest (Core Phase 94): flat logical spellings — a variadic
        // `exprs` list left-folds into the nested binary form (and/or are
        // associative); the two-operand left/right form maps one-to-one.
        tag @ ("and" | "or") => {
            let op = if tag == "and" { BinOp::And } else { BinOp::Or };
            match get(fields, "exprs") {
                Some(exprs_j) => {
                    let exprs = c_arr(exprs_j)?
                        .iter()
                        .map(decode_col_expr)
                        .collect::<CResult<Vec<_>>>()?;
                    let mut iter = exprs.into_iter();
                    let Some(first) = iter.next() else {
                        return Err(format!(
                            "malformed: {tag}.exprs: expected a non-empty array"
                        ));
                    };
                    Ok(iter.fold(first, |acc, e| ColExpr::Binary {
                        op,
                        left: Box::new(acc),
                        right: Box::new(e),
                    }))
                }
                None => {
                    let left = decode_col_expr(c_field_aliased(fields, "left", "expr")?)?;
                    let right = decode_col_expr(c_field_aliased(fields, "right", "other")?)?;
                    Ok(ColExpr::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    })
                }
            }
        }
        // Lenient-ingest (Core Phase 94): flat comparison spellings.
        tag @ ("eq" | "ne" | "lt" | "le" | "gt" | "ge") => {
            let op = match tag {
                "eq" => BinOp::Eq,
                "ne" => BinOp::Ne,
                "lt" => BinOp::Lt,
                "le" => BinOp::Le,
                "gt" => BinOp::Gt,
                _ => BinOp::Ge,
            };
            let left = decode_col_expr(c_field_aliased(fields, "left", "expr")?)?;
            let right = decode_col_expr(c_field_aliased(fields, "right", "other")?)?;
            Ok(ColExpr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            })
        }
        // Lenient-ingest (Core Phase 94): flat scalar-fn spellings — a bare
        // fn-name node ({"$type":"lower","expr":X} / {"$type":"concat",
        // "args":[...]}) denotes Apply (the fn-name vocabulary is disjoint
        // from the node kinds, so the mapping is one-to-one).
        other => match ScalarFn::from_wire(other) {
            Some(func) => Ok(ColExpr::Apply {
                func,
                args: expr_args(fields)?,
            }),
            None => Err(format!(
                "unknown column type '{other}'; expected one of: col, lit, param, binary, not, coalesce, case, cast, apply, in, isNull, quotient, rounded"
            )),
        },
    }
}

/// A rounding policy (Phase 2001): `mode` one of the seven spellings — an
/// unknown one refused naming them — and `scale` an integer slot, a literal or
/// a `{"$param":"<name>"}` object.
pub(super) fn decode_rounding(j: &JVal) -> CResult<Rounding> {
    let fields = c_obj(j)?;
    let mode = c_str_field(fields, "mode")?;
    let mode = RoundingMode::from_wire(&mode).ok_or_else(|| {
        format!(
            "unknown rounding mode '{mode}'; expected one of: {}",
            RoundingMode::WIRE_NAMES.join(", ")
        )
    })?;
    let scale = match c_field(fields, "scale")? {
        JVal::Obj(slot) => match get(slot, "$param") {
            Some(JVal::Str(name)) => RoundingScale::Param(name.clone()),
            Some(_) => {
                return Err(
                    "malformed: \"$param\" must be a JSON string (rounding scale)".to_string(),
                );
            }
            None => {
                return Err(
                    "malformed: rounding scale: an object here is a parameter slot and must carry \"$param\""
                        .to_string(),
                );
            }
        },
        other => RoundingScale::Lit(c_int(other)?),
    };
    Ok(Rounding { scale, mode })
}

/// One of the canonical field or its observed alias — Core Phase 92's `fieldAliased`.
/// Exactly one: BOTH present is refused as ambiguous rather than resolved to the
/// canonical. The two spellings can carry different values, so there is no reading of
/// the document that says which the author meant, and a silent winner turns an author's
/// mistake into a document that decodes to something they did not write. This matches
/// the reference codec and every sibling host; until 0.28.0 this host let the canonical
/// win, which was the one behaviour of the family it did not share.
pub(super) fn c_field_aliased<'a>(
    fields: &'a Fields,
    canonical: &str,
    alias: &str,
) -> CResult<&'a JVal> {
    match (get(fields, canonical), get(fields, alias)) {
        (Some(_), Some(_)) => Err(format!(
            "malformed: give \"{canonical}\" (canonical) or \"{alias}\" (alias), not both"
        )),
        (Some(v), None) | (None, Some(v)) => Ok(v),
        (None, None) => Err(format!("missing field: {canonical}")),
    }
}

/// The `args` of an apply-shaped node: the canonical array, or a lone `expr`
/// field standing in for a one-element list (Core Phase 94).
pub(super) fn expr_args(fields: &Fields) -> CResult<Vec<ColExpr>> {
    match get(fields, "args") {
        Some(args_j) => c_arr(args_j)?
            .iter()
            .map(decode_col_expr)
            .collect::<CResult<Vec<_>>>(),
        None => Ok(vec![decode_col_expr(c_field(fields, "expr")?)?]),
    }
}

pub(super) fn decode_col_pair(j: &JVal) -> CResult<ColPair> {
    let fields = c_obj(j)?;
    Ok(ColPair {
        a: c_str_field(fields, "a")?,
        b: c_str_field(fields, "b")?,
    })
}

pub(super) fn decode_agg(j: &JVal) -> CResult<Agg> {
    let fields = c_obj(j)?;
    // Lenient-ingest (Core Phase 92) — entry aliases: `as` for `name`, `op`
    // for `fn`, `column` for `of`; `avg` aliases `mean` (the SQL prior).
    let name = c_str(c_field_aliased(fields, "name", "as")?)?;
    let f = c_str(c_field_aliased(fields, "fn", "op")?)?;
    let func = match f.as_str() {
        "avg" => AggFn::Mean,
        _ => c_enum(&f, AggFn::from_wire, AggFn::WIRE_NAMES)?,
    };
    let of = c_str(c_field_aliased(fields, "of", "column")?)?;
    Ok(Agg { name, func, of })
}

pub(super) fn decode_order(j: &JVal) -> CResult<SortKey> {
    let fields = c_obj(j)?;
    // Core Phase 92 admitted `column` as an alias of `col`; 0.28.0 SWAPPED which of the
    // two is canonical — a member whose only honest name is "the column" is spelled out
    // in full — so both still decode, `column` re-encodes, and giving both is refused.
    // Direction is one of `dir` (canonical asc|desc), boolean `descending`, or
    // `direction`; a directionless entry is the SQL default (asc). Only "desc" sorts
    // descending; anything else reads ascending.
    let col = c_str(c_field_aliased(fields, "column", "col")?)?;
    let dir = match (
        get(fields, "dir"),
        get(fields, "descending"),
        get(fields, "direction"),
    ) {
        (Some(d), _, _) | (_, _, Some(d)) => {
            if c_str(d)? == "desc" {
                SortDir::Desc
            } else {
                SortDir::Asc
            }
        }
        (None, Some(JVal::Bool(b)), None) => {
            if *b {
                SortDir::Desc
            } else {
                SortDir::Asc
            }
        }
        (None, Some(_), None) => {
            return Err("malformed: \"descending\" must be a JSON boolean".to_string());
        }
        (None, None, None) => SortDir::Asc,
    };
    Ok(SortKey { col, dir })
}

pub(super) fn decode_transform_step(j: &JVal) -> CResult<TransformStep> {
    let fields = c_obj(j)?;
    let tag = c_str_field(fields, "$type")?;
    match tag.as_str() {
        "filter" => {
            // Lenient-ingest (Core Phases 89/93): `predicate` aliases `pred`;
            // the flat prior {"$type":"filter","column":C,"op":O,"param":P |
            // "value":V} coerces to the canonical nested predicate.
            if let Some(pred_j) = get(fields, "pred").or_else(|| get(fields, "predicate")) {
                return Ok(TransformStep::Filter {
                    pred: decode_col_expr(pred_j)?,
                });
            }
            let (Some(col_j), Some(op_j)) = (get(fields, "column"), get(fields, "op")) else {
                return Err(
                    "malformed: a filter step carries \"pred\" (a $type-discriminated expression) — or the flat short form {\"column\":…,\"op\":…,\"param\":…|\"value\":…}"
                        .to_string(),
                );
            };
            let col = c_str(col_j)?;
            let op_tag = c_str(op_j)?;
            let op = c_enum(&op_tag, BinOp::from_wire, BinOp::WIRE_NAMES)?;
            let right = match (get(fields, "param"), get(fields, "value")) {
                (Some(p), None) => ColExpr::Param { name: c_str(p)? },
                (None, Some(v)) => ColExpr::Lit {
                    cell: match v {
                        JVal::Str(s) => Cell::Str(s.clone()),
                        JVal::Num(n) if *n == n.trunc() => Cell::Int(*n as i64),
                        JVal::Num(n) => Cell::Float(*n),
                        JVal::Bool(b) => Cell::Bool(*b),
                        _ => {
                            return Err(
                                "malformed: flat filter step: \"value\" must be a scalar (string/int/float/bool)"
                                    .to_string(),
                            );
                        }
                    },
                },
                _ => {
                    return Err(
                        "malformed: flat filter step: {column, op} needs \"param\" (a pipeline param name) or \"value\" (a scalar literal) as the right-hand side"
                            .to_string(),
                    );
                }
            };
            Ok(TransformStep::Filter {
                pred: ColExpr::Binary {
                    op,
                    left: Box::new(ColExpr::Col { name: col }),
                    right: Box::new(right),
                },
            })
        }
        "project" => {
            // 0.28.0 — `columns` canonical, `cols` a decode alias, both-present refused.
            let cols = c_arr(c_field_aliased(fields, "columns", "cols")?)?
                .iter()
                .map(decode_col_pair)
                .collect::<CResult<Vec<_>>>()?;
            Ok(TransformStep::Project { cols })
        }
        "derive" => Ok(TransformStep::Derive {
            name: c_str_field(fields, "name")?,
            expr: decode_col_expr(c_field(fields, "expr")?)?,
        }),
        "groupBy" => {
            // Lenient-ingest (Core Phase 92): `by` (the pandas prior) aliases
            // `keys`; `aggregations` aliases `aggs`.
            let keys = c_str_list(c_field_aliased(fields, "keys", "by")?)?;
            let aggs = c_arr(c_field_aliased(fields, "aggs", "aggregations")?)?
                .iter()
                .map(decode_agg)
                .collect::<CResult<Vec<_>>>()?;
            Ok(TransformStep::GroupBy { keys, aggs })
        }
        "join" => {
            let source = decode_data_source(c_field(fields, "source")?)?;
            let on = c_arr(c_field(fields, "on")?)?
                .iter()
                .map(decode_col_pair)
                .collect::<CResult<Vec<_>>>()?;
            let how = c_str_field(fields, "how")?;
            let how = c_enum(&how, JoinKind::from_wire, JoinKind::WIRE_NAMES)?;
            Ok(TransformStep::Join { source, on, how })
        }
        "window" => {
            let partition_by = c_str_list(c_field(fields, "partitionBy")?)?;
            let order_by = c_arr(c_field(fields, "orderBy")?)?
                .iter()
                .map(decode_order)
                .collect::<CResult<Vec<_>>>()?;
            let f = c_str_field(fields, "fn")?;
            // Legacy alias: `cumSum` is the pre-rename wire tag for
            // `cumulSum`; normalises on re-encode.
            let func = match f.as_str() {
                "cumSum" => WindowFn::CumulSum,
                _ => c_enum(&f, WindowFn::from_wire, WindowFn::WIRE_NAMES)?,
            };
            let of = c_str_field(fields, "of")?;
            let alias = c_str_field(fields, "as")?;
            Ok(TransformStep::Window {
                partition_by,
                order_by,
                func,
                of,
                alias,
            })
        }
        "pivot" => {
            let index = c_str_list(c_field(fields, "index")?)?;
            let on = c_str_field(fields, "on")?;
            let values = c_str_field(fields, "values")?;
            let agg = c_str_field(fields, "agg")?;
            let agg = c_enum(&agg, AggFn::from_wire, AggFn::WIRE_NAMES)?;
            Ok(TransformStep::Pivot {
                index,
                on,
                values,
                agg,
            })
        }
        "unpivot" => Ok(TransformStep::Unpivot {
            id_vars: c_str_list(c_field(fields, "idVars")?)?,
            value_vars: c_str_list(c_field(fields, "valueVars")?)?,
        }),
        "sort" => {
            // Lenient-ingest (Core Phase 92): `keys` (the SQL ORDER-BY-list
            // prior) aliases `by`.
            let by = c_arr(c_field_aliased(fields, "by", "keys")?)?
                .iter()
                .map(decode_order)
                .collect::<CResult<Vec<_>>>()?;
            Ok(TransformStep::Sort { by })
        }
        "distinct" => Ok(TransformStep::Distinct),
        "limit" => Ok(TransformStep::Limit {
            // Lenient-ingest (Core Phase 92): `count` aliases `n`; an absent
            // `offset` is unambiguously 0.
            n: c_int(c_field_aliased(fields, "n", "count")?)?,
            offset: match get(fields, "offset") {
                Some(o) => c_int(o)?,
                None => 0,
            },
        }),
        "union" => Ok(TransformStep::Union {
            source: decode_data_source(c_field(fields, "source")?)?,
        }),
        "intersect" => Ok(TransformStep::Intersect {
            source: decode_data_source(c_field(fields, "source")?)?,
        }),
        "except" => Ok(TransformStep::Except {
            source: decode_data_source(c_field(fields, "source")?)?,
        }),
        other => Err(format!(
            "unknown column type '{other}'; expected one of: filter, project, derive, groupBy, join, window, pivot, unpivot, sort, distinct, limit, union, intersect, except"
        )),
    }
}

pub(super) fn decode_pipeline(j: &JVal) -> CResult<Vec<TransformStep>> {
    match j {
        JVal::Arr(items) => items.iter().map(decode_transform_step).collect(),
        _ => Err("malformed: pipeline: expected a JSON array of transform steps".to_string()),
    }
}

pub(super) fn decode_invoke_args(path: &str, j: &JVal) -> DResult<Vec<InvokeArg>> {
    let items = match j {
        JVal::Arr(items) => items,
        _ => return Err(wrong_type(path, "JSON array of invoke args")),
    };
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let p = format!("{path}[{i}]");
        let fields = as_obj(&p, item)?;
        out.push(InvokeArg {
            addr: req_string(&p, fields, "addr", "invoke arg addr string")?,
            value: req_string(&p, fields, "value", "invoke arg value string")?,
        });
    }
    Ok(out)
}

// ─── Typed Static payload slots (Phase 429) ──────────────────────────────────

/// Which typed `Static` payload shape a binding slot carries (§5); `Untyped` is
/// the faithful-AST residual boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StaticSlot {
    Untyped,
    /// A `Binding<string>` slot. The parsed payload still rides as
    /// `StaticValue::Ast`, so nothing downstream and no encoded byte changes —
    /// what the slot adds is the TYPE CHECK the reference host has always
    /// applied (`bindingGeneric<string> path requireString ""`). §3.6's
    /// bare-scalar coercion is about SHAPE; the slot's own `'T` still governs
    /// the value, which is why `"hidden": "yes"` must be refused even though
    /// `"label": "Home"` is sanctioned shorthand.
    Str,
    /// A `Binding<bool>` slot — the `Str` reasoning at the other scalar type.
    Bool,
    /// A `Binding<float>` slot. §7's float admission is `{ JSON number } ∪
    /// { "NaN", "Infinity", "-Infinity" }` — those three tokens EXACTLY and
    /// case-sensitively, which is why the sentinel-bearing round-trip fixtures
    /// keep decoding while `"nan"` and `"lots"` do not. The parsed payload
    /// still rides as `StaticValue::Ast`, preserving the sentinel STRING so
    /// re-encode is byte-identical; what the slot adds is the type check the
    /// reference host has always applied (`bindingGeneric<float> requireFloat`).
    Float,
    /// A `Binding<int>` slot. §7 admits `{ JSON number }` ONLY, truncating via
    /// integer cast — an integer slot has no non-finite form, so `"NaN"` here
    /// is `WRONG_TYPE` where at a `Float` slot it is the sentinel.
    Int,
    Options,
    StringOpt,
    StringList,
    FloatSeq,
    Markers,
    /// The fuaran#665 grid / chart row source — an array of row objects.
    Rows,
    /// The 0.2.0 `FormFieldKind.Range` dual-thumb `(min, max)` pair.
    FloatPair,
    /// The 0.7.0 `FormFieldKind.DateRange` ordered `(from, to)` ISO-8601 pair.
    StringPair,
}

impl StaticSlot {
    /// The typed placeholder an absent / unparseable `State.defaultValue`
    /// falls back to — byte-for-byte with the reference hosts.
    fn placeholder(self) -> StaticValue {
        match self {
            StaticSlot::Untyped => StaticValue::Ast(JVal::Str(OPAQUE.to_string())),
            // The reference host's typed fallbacks (`""` / `false`), so an
            // absent `State.defaultValue` at a scalar slot agrees byte-for-byte.
            StaticSlot::Str => StaticValue::Ast(JVal::Str(String::new())),
            StaticSlot::Bool => StaticValue::Ast(JVal::Bool(false)),
            // The reference host's typed numeric fallbacks (`0.0` / `0`). This
            // host carries one numeric AST node, and the canonical number form
            // renders both as `0`, so the two agree byte-for-byte on the wire.
            StaticSlot::Float | StaticSlot::Int => StaticValue::Ast(JVal::Num(0.0)),
            StaticSlot::Options => StaticValue::Options(vec![SelectOption {
                value: OPAQUE.to_string(),
                label: TextSource::Literal(OPAQUE.to_string()),
            }]),
            StaticSlot::StringOpt => StaticValue::StringOpt(Some(OPAQUE.to_string())),
            StaticSlot::StringList => StaticValue::StringList(vec![OPAQUE.to_string()]),
            StaticSlot::FloatSeq => StaticValue::FloatSeq(vec![]),
            StaticSlot::Markers => StaticValue::Markers(vec![]),
            StaticSlot::Rows => StaticValue::Rows(vec![]),
            StaticSlot::FloatPair => StaticValue::FloatPair(0.0, 0.0),
            StaticSlot::StringPair => StaticValue::StringPair(String::new(), String::new()),
        }
    }

    /// Parse a `Static.value` / `State.defaultValue` payload for this slot.
    /// Read-compat (§5): the legacy `"<opaque>"` sentinel and pre-typed `null`
    /// both decode to typed placeholders / empties at the typed slots.
    fn parse(self, path: &str, v: &JVal) -> DResult<StaticValue> {
        match self {
            StaticSlot::Untyped => Ok(StaticValue::Ast(v.clone())),
            StaticSlot::Str => {
                as_str(path, v)?;
                Ok(StaticValue::Ast(unwrap_static_envelope(v).clone()))
            }
            StaticSlot::Bool => {
                as_bool(path, v)?;
                Ok(StaticValue::Ast(unwrap_static_envelope(v).clone()))
            }
            // `as_float` / `as_int` ARE §7's admission rules, and they already
            // unwrap a `Static` envelope found at a plain-scalar position — so
            // both arms §3.6 sanctions (the envelope and the bare scalar) reach
            // the same typed test. The unwrapped node is cloned rather than the
            // parsed `f64` re-boxed, so a sentinel string round-trips as itself.
            StaticSlot::Float => {
                as_float(path, v)?;
                Ok(StaticValue::Ast(unwrap_static_envelope(v).clone()))
            }
            StaticSlot::Int => {
                as_int(path, v)?;
                Ok(StaticValue::Ast(unwrap_static_envelope(v).clone()))
            }
            StaticSlot::Options => match v {
                JVal::Null => Ok(StaticValue::Options(vec![])),
                JVal::Str(s) if s == OPAQUE => Ok(self.placeholder()),
                _ => {
                    let items = as_arr(path, v)?;
                    let mut out = Vec::with_capacity(items.len());
                    for (i, item) in items.iter().enumerate() {
                        out.push(decode_select_option(&format!("{path}[{i}]"), item)?);
                    }
                    Ok(StaticValue::Options(out))
                }
            },
            StaticSlot::StringOpt => match v {
                JVal::Null => Ok(StaticValue::StringOpt(None)),
                JVal::Str(s) => Ok(StaticValue::StringOpt(Some(s.clone()))),
                _ => Err(wrong_type(path, "JSON string or null (string option)")),
            },
            StaticSlot::StringList => match v {
                JVal::Null => Ok(StaticValue::StringList(vec![])),
                JVal::Str(s) if s == OPAQUE => Ok(self.placeholder()),
                _ => {
                    let items = as_arr(path, v)?;
                    let mut out = Vec::with_capacity(items.len());
                    for (i, item) in items.iter().enumerate() {
                        out.push(as_str(&format!("{path}[{i}]"), item)?.to_string());
                    }
                    Ok(StaticValue::StringList(out))
                }
            },
            StaticSlot::FloatSeq => match v {
                JVal::Null => Ok(StaticValue::FloatSeq(vec![])),
                JVal::Str(s) if s == OPAQUE => Ok(StaticValue::FloatSeq(vec![])),
                _ => {
                    let items = as_arr(path, v)?;
                    let mut out = Vec::with_capacity(items.len());
                    for (i, item) in items.iter().enumerate() {
                        out.push(as_float(&format!("{path}[{i}]"), item)?);
                    }
                    Ok(StaticValue::FloatSeq(out))
                }
            },
            StaticSlot::Markers => match v {
                JVal::Null => Ok(StaticValue::Markers(vec![])),
                JVal::Str(s) if s == OPAQUE => Ok(StaticValue::Markers(vec![])),
                _ => {
                    let items = as_arr(path, v)?;
                    let mut out = Vec::with_capacity(items.len());
                    for (i, item) in items.iter().enumerate() {
                        out.push(decode_map_marker(&format!("{path}[{i}]"), item)?);
                    }
                    Ok(StaticValue::Markers(out))
                }
            },
            // fuaran#665 — the typed row source. The legacy `"<opaque>"` sentinel
            // decodes to the empty feed indefinitely (read-compat: that *was* the
            // whole value it carried), as does a pre-typed `null`. A non-object
            // row element is a named decode error.
            StaticSlot::Rows => match v {
                JVal::Null => Ok(StaticValue::Rows(vec![])),
                JVal::Str(s) if s == OPAQUE => Ok(StaticValue::Rows(vec![])),
                _ => {
                    let items = as_arr(path, v)?;
                    let mut out = Vec::with_capacity(items.len());
                    for (i, item) in items.iter().enumerate() {
                        let p = format!("{path}[{i}]");
                        match item {
                            JVal::Obj(cells) => out.push(cells.clone()),
                            _ => return Err(wrong_type(&p, "row object")),
                        }
                    }
                    Ok(StaticValue::Rows(out))
                }
            },
            StaticSlot::FloatPair => match v {
                // Canonical: the bare `{min, max}` object (Phase 423 shape);
                // lenient: a two-element `[min, max]` array (§3.6 coercion).
                JVal::Obj(pf) => {
                    let (Some(min_j), Some(max_j)) = (get(pf, "min"), get(pf, "max")) else {
                        return Err(wrong_type(path, "object with min and max numbers"));
                    };
                    let min = as_float(&format!("{path}.min"), min_j)?;
                    let max = as_float(&format!("{path}.max"), max_j)?;
                    Ok(StaticValue::FloatPair(min, max))
                }
                JVal::Arr(items) if items.len() == 2 => {
                    let a = as_float(&format!("{path}[0]"), &items[0])?;
                    let b = as_float(&format!("{path}[1]"), &items[1])?;
                    Ok(StaticValue::FloatPair(a, b))
                }
                _ => Err(wrong_type(
                    path,
                    "range pair ({min, max} object or [min, max] array)",
                )),
            },
            StaticSlot::StringPair => {
                // Didactic domain rule: a LITERAL pair must be ordered. Same-variant
                // ISO-8601 strings sort lexicographically in chronological order, so
                // Rust's byte-wise `str` Ord is an ordinal compare — total for every
                // variant, no date parsing, no locale, no dependency. Only a literal
                // pair is checked; a bound pair's ordering is a runtime concern.
                let ordered = |from: String, to: String| -> DResult<StaticValue> {
                    if from > to {
                        return Err(make_error(
                            DecodeErrorCode::WrongType,
                            path,
                            format!(
                                "date-range start '{from}' is after end '{to}' — a DateTimeRange pair \
                                 is ordered (from <= to); ISO-8601 strings of one variant compare \
                                 lexicographically, so swap the two values"
                            ),
                            Some(
                                "ordered ISO-8601 pair ({\"from\": <iso>, \"to\": <iso>} with from <= to)"
                                    .to_string(),
                            ),
                        ));
                    }
                    Ok(StaticValue::StringPair(from, to))
                };
                match v {
                    // Canonical: the bare `{from, to}` object (no `Static`
                    // envelope); lenient: a two-element `[from, to]` array.
                    JVal::Obj(pf) => {
                        let (Some(from_j), Some(to_j)) = (get(pf, "from"), get(pf, "to")) else {
                            return Err(wrong_type(
                                path,
                                "object with from and to ISO-8601 strings",
                            ));
                        };
                        let from = as_str(&format!("{path}.from"), from_j)?.to_string();
                        let to = as_str(&format!("{path}.to"), to_j)?.to_string();
                        ordered(from, to)
                    }
                    JVal::Arr(items) if items.len() == 2 => {
                        let a = as_str(&format!("{path}[0]"), &items[0])?.to_string();
                        let b = as_str(&format!("{path}[1]"), &items[1])?.to_string();
                        ordered(a, b)
                    }
                    _ => Err(wrong_type(
                        path,
                        "date-range pair ({from, to} object or [from, to] array)",
                    )),
                }
            }
        }
    }
}

// ─── Binding (recursive) ─────────────────────────────────────────────────────

/// Phase 815 — organic-demand leniencies for the Transform `source` slot, both
/// observed cross-family (claude, gemini, kimi — the Tier-D pilot, 2026-08-13):
/// models bind a derived value to a Transform whose source is
/// `{"$type":"State","defaultValue":[{row},…]}`. Two universal priors,
/// accommodated as typed data at THIS host bridge, before the columnar codec
/// sees the value (the fuaran#633 `Bound`-unwrap precedent — no Core change,
/// no wire-spec change, no new key). Mirror of the F#
/// `normaliseTransformSource`:
///   1. a `State`/`Static`/`Bound` binding WRAPPER around the data unwraps to
///      its `defaultValue`/`value` (initial-snapshot semantics — a LIVE
///      state-sourced Transform is deliberately not this);
///   2. ROW-MAJOR data (an array of row objects) transposes to the canonical
///      columnar `{"columns": …}` shape — FIRST-row key set (sorted ordinal,
///      the F# Map ordering), absent cells null. Canonical columnar and `ref`
///      sources pass through untouched, so existing fixtures stay
///      byte-identical.
///
/// Ragged / mixed-type rows may still fail downstream (the mixed-type
/// didactic) — deliberately not special-cased. A wrapper carrying neither
/// `defaultValue` nor `value` stays UNCHANGED and fails the columnar decode.
pub(super) fn normalise_transform_source(j: &JVal) -> JVal {
    let unwrapped: &JVal = match j {
        JVal::Obj(fields) => match get(fields, "$type") {
            Some(JVal::Str(t)) if t == "State" || t == "Static" || t == "Bound" => {
                match get(fields, "defaultValue").or_else(|| get(fields, "value")) {
                    Some(inner) => inner,
                    None => j,
                }
            }
            _ => j,
        },
        _ => j,
    };
    transpose_row_major(unwrapped)
}

/// Phase 815/818 — the row-major half of the source normalisation, shared with
/// the LIVE-source snapshot derivation: an array of row objects transposes to
/// the canonical columnar `{"columns": …}` shape (FIRST-row key set, sorted
/// ordinal; absent cells null). Anything else passes through untouched.
pub(crate) fn transpose_row_major(unwrapped: &JVal) -> JVal {
    match unwrapped {
        JVal::Arr(rows) => match rows.first() {
            Some(JVal::Obj(first)) => {
                let mut keys: Vec<&String> = first.iter().map(|(k, _)| k).collect();
                keys.sort();
                keys.dedup();
                let cols: Vec<(String, JVal)> = keys
                    .iter()
                    .map(|k| {
                        let cells: Vec<JVal> = rows
                            .iter()
                            .map(|row| match row {
                                JVal::Obj(rf) => get(rf, k.as_str()).cloned().unwrap_or(JVal::Null),
                                _ => JVal::Null,
                            })
                            .collect();
                        ((*k).clone(), JVal::Arr(cells))
                    })
                    .collect();
                JVal::Obj(vec![("columns".to_string(), JVal::Obj(cols))])
            }
            _ => unwrapped.clone(),
        },
        other => other.clone(),
    }
}

/// Phase 818 — the empty embedded table: the initial snapshot of a live source
/// that carries no data yet (a Selection with no default, a Query) — the
/// pipeline evaluates over zero rows and the node renders its empty state.
pub(crate) fn empty_embedded_source() -> DataSource {
    DataSource::Embedded {
        schema: vec![],
        columns: vec![],
    }
}

/// Phase 818 — materialise a live source's carried / resolved data as the
/// snapshot `DataSource` (row-major transpose, then the columnar decode) — the
/// render-time twin of the decode-time `initial` derivation.
pub(crate) fn live_data_source(data: &JVal) -> Result<DataSource, String> {
    decode_data_source(&transpose_row_major(data))
}

pub(super) fn decode_binding(path: &str, j: &JVal) -> DResult<Binding> {
    decode_binding_slot(path, j, StaticSlot::Untyped)
}

pub(super) fn decode_binding_slot(path: &str, j: &JVal, slot: StaticSlot) -> DResult<Binding> {
    // Lenient AI-ingest shape coercion (§3.6): a bare JSON array (`options:
    // ["A","B"]`, the HTML-select prior) or bare SCALAR (`fraction: 0.9`)
    // where a Binding is expected is accepted as `Static` with that value —
    // unambiguous, since every Binding case is a `$type`-discriminated object.
    // Objects stay strict (an object without `$type` is more plausibly a
    // mistyped binding); `null` stays strict (ambiguous with absent).
    match j {
        JVal::Arr(_) | JVal::Str(_) | JVal::Num(_) | JVal::Bool(_) => {
            let value = slot.parse(path, j)?;
            return Ok(Binding::Static { value });
        }
        _ => {}
    }
    let fields = as_obj(path, j)?;
    let tag = disc(path, fields)?;
    // Lenient AI-ingest: the `TextSource.Bound` wrapper convention
    // transferred to a bare-Binding slot — `{"$type":"Bound","binding":X}`
    // carries exactly one payload field, so the unwrap is one-to-one.
    // Decode-only; the canonical encoder never wraps bare-Binding slots.
    if tag == "Bound" {
        let inner = req(path, fields, "binding", "the wrapped Binding object")?;
        return decode_binding_slot(&format!("{path}.binding"), inner, slot);
    }
    match BindingTag::of(tag) {
        Some(BindingTag::Static) => {
            // Phase 677 — absence is structural: a MISSING `value` means the
            // binding carries none, and the legacy `"value": null` spelling
            // normalises to the same thing (§16 shorthand) by routing to the very
            // same per-slot parse, so the two cannot disagree.
            let null = JVal::Null;
            let v = get(fields, "value").unwrap_or(&null);
            let value = slot.parse(&format!("{path}.value"), v)?;
            Ok(Binding::Static { value })
        }
        Some(BindingTag::Query) => {
            let name = req_string(path, fields, "name", "query name string")?;
            // Field aliases: deps / dependencies → dependsOn.
            let depends_on = match get_aliased(fields, "dependsOn", &["deps", "dependencies"]) {
                None => None,
                Some(v) => {
                    let items = as_arr(&format!("{path}.dependsOn"), v)?;
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        out.push(as_str(&format!("{path}.dependsOn[]"), item)?.to_string());
                    }
                    Some(out)
                }
            };
            Ok(Binding::Query { name, depends_on })
        }
        Some(BindingTag::Filter) => {
            let name = req_string(path, fields, "name", "filter name string")?;
            // 0.2.0 — optional `defaultValue`, decoded through the slot's typed
            // static parser (mirroring `State.defaultValue`); a parse failure
            // reads as absent, matching the reference host's leniency.
            let default_value = get(fields, "defaultValue")
                .and_then(|v| slot.parse(&format!("{path}.defaultValue"), v).ok());
            Ok(Binding::Filter {
                name,
                default_value,
            })
        }
        Some(BindingTag::Selection) => {
            let node_id = req_string(path, fields, "nodeId", "selection NodeId string")?;
            // 0.2.9 — optional `defaultValue` (the Filter.defaultValue
            // convention); 0.2.10 — optional `field` (the declarative
            // row-field projection).
            let default_value = get(fields, "defaultValue")
                .and_then(|v| slot.parse(&format!("{path}.defaultValue"), v).ok());
            let field = get(fields, "field").and_then(|v| match v {
                JVal::Str(s) => Some(s.clone()),
                _ => None,
            });
            Ok(Binding::Selection {
                node_id,
                default_value,
                field,
            })
        }
        Some(BindingTag::State) => {
            let key = req_string(path, fields, "key", "state key string")?;
            // Field aliases: initialValue / default → defaultValue.
            //
            // Phase 1656 — §5's absent-`State.defaultValue` posture, which is
            // now stated normatively rather than left to rule 4 by inference:
            // absence OMITS, and the three spellings of absence are the member
            // missing, the member present as `null`, and either alias present
            // as `null`. All three yield NO declaration; the encoder writes the
            // member only where one was declared.
            //
            // Two facts are carried apart, and that is what lets both halves be
            // right at once. `default_declared` is the WIRE fact — did the
            // document say anything. `default_value` is the RESOLUTION default
            // (§3.3) — what an unwritten key yields, which at a numeric slot is
            // `0` and at a bool one `false`, and which §3.6's `visible` rule
            // depends on to remove a node whose unwritten predicate resolves
            // false. Conflating them cost a byte-level round trip before Phase
            // 1499: with only the value, an undeclared default at a NUMERIC or
            // BOOL slot was indistinguishable from a declared zero, so
            // `{"$type":"State","key":k}` re-encoded as
            // `{"$type":"State","defaultValue":0,"key":k}`.
            //
            // An UNREADABLE declared default reads as undeclared too, which is
            // the reference host's answer and the reason `static_is_absent` is
            // no longer consulted here. The alternative — keep the declaration
            // and encode the slot's placeholder — writes a value the document
            // did not carry over one it did, which is the same fabrication in a
            // different position, and at an absent-sentinel slot it would emit
            // a member with no value at all. Corpus:
            // `nodes/state-absent-default` (the five typed slots),
            // `lenient/lenient-1656-state-default-null` (both null spellings),
            // `reject/reject-state-default-without-key` (the optionality, the
            // right way round); `tests/state_seeding.rs` holds the seeding half.
            let raw = get_aliased(fields, "defaultValue", &["initialValue", "default"])
                .filter(|v| !matches!(v, JVal::Null));
            let parsed = raw.and_then(|v| slot.parse(&format!("{path}.defaultValue"), v).ok());
            Ok(Binding::State {
                key,
                default_value: parsed.clone().unwrap_or_else(|| slot.placeholder()),
                default_declared: parsed.is_some(),
            })
        }
        Some(BindingTag::Computed) => Ok(Binding::Computed),
        // Phase 765 — the host-furnished INSTANT is never on the wire. The
        // instant is already the wire-shaped string, so a decoded reader
        // receives it as-is (the sibling hosts' identity projection; this
        // closure-free host has nothing to erase).
        //
        // Phase 1533 — the declared `grain` is the one wire field, optional,
        // absent meaning `Second`. Present-but-unreadable is a REFUSAL rather
        // than a silent fallback to the default: a document that names a grain
        // the host cannot honour must not be rendered at a neighbouring
        // resolution in silence.
        Some(BindingTag::Now) => {
            let grain = match get(fields, "grain") {
                None => None,
                Some(v) => Some(decode_time_grain(&format!("{path}.grain"), v)?),
            };
            Ok(Binding::Now { grain })
        }
        Some(BindingTag::I18n) => {
            let key = req_string(path, fields, "key", "i18n key string")?;
            let args = match get(fields, "args") {
                None => None,
                Some(v) => {
                    let arg_fields = as_obj(&format!("{path}.args"), v)?;
                    let mut out = Vec::with_capacity(arg_fields.len());
                    for (k, arg) in arg_fields {
                        out.push((k.clone(), decode_binding(&format!("{path}.args.{k}"), arg)?));
                    }
                    Some(out)
                }
            };
            Ok(Binding::I18n { key, args })
        }
        Some(BindingTag::Local) => {
            let initial_j = req(path, fields, "initialFrom", "Local InitialFrom Binding")?;
            let initial_from =
                decode_binding_slot(&format!("{path}.initialFrom"), initial_j, slot)?;
            let flush_on = match get(fields, "flushOn") {
                None => LocalFlushTrigger::OnBlur,
                Some(v) => decode_local_flush_trigger(&format!("{path}.flushOn"), v)?,
            };
            // WIRE_FORMAT.md §3.3.3 — the buffer's own codec REPLACES the
            // identity on both sides: `format` renders through it, `parse`
            // inverts it. The admitted set is therefore the `Format` cases with
            // a TOTAL, LOCALE-INDEPENDENT inverse, and today that is `Number`
            // alone. Every other case is refused with a stated reason rather
            // than by omission — `Currency` prepends a locale-chosen symbol,
            // `Date`'s styles are locale renditions with no parse, and
            // `Percent` (the one that looks admissible) needs a ×100 scale
            // whose IEEE round trip is not exact, so admitting it would mean
            // specifying a rounding to the bit on every host.
            let codec = match get(fields, "codec") {
                None => None,
                Some(v) => {
                    let codec_path = format!("{path}.codec");
                    let format = decode_format(&codec_path, v)?;
                    if !matches!(format, Format::Number { .. }) {
                        return Err(wrong_type(
                            &codec_path,
                            "a Format with a total, locale-independent inverse — Number alone,                              since whatever the buffer renders it must also parse back from what                              the reader typed",
                        ));
                    }
                    Some(format)
                }
            };
            let on_commit = opt_closure(fields, "onCommit");
            let commit_to = opt_string(path, fields, "commitTo")?;
            // Mutually exclusive, and a refusal rather than a precedence rule:
            // the wire cannot carry the closure — it is `"<closure>"` and
            // nothing more — so a host honouring `onCommit` and a host
            // honouring `commitTo` would write to different places from
            // identical bytes.
            if on_commit.is_some() && commit_to.is_some() {
                return Err(wrong_type(
                    &format!("{path}.commitTo"),
                    "exactly one of 'onCommit' and 'commitTo' — the wire cannot carry the                      closure, so two hosts would write to different places from identical bytes",
                ));
            }
            Ok(Binding::Local {
                codec,
                commit_to,
                flush_on,
                initial_from: Box::new(initial_from),
                on_commit,
            })
        }
        Some(BindingTag::Format) => {
            let source_j = req(path, fields, "source", "Binding<number> source object")?;
            let source = decode_binding(&format!("{path}.source"), source_j)?;
            let fmt_j = req(path, fields, "format", "Format DU object")?;
            let format = decode_format(&format!("{path}.format"), fmt_j)?;
            let loc_j = req(path, fields, "locale", "LocaleSource DU object")?;
            let locale = decode_locale_source(&format!("{path}.locale"), loc_j)?;
            Ok(Binding::Format {
                format,
                locale,
                source: Box::new(source),
            })
        }
        Some(BindingTag::Transform) => {
            let src_j = req(path, fields, "source", "Transform DataSource object")?;
            let pipe_j = req(path, fields, "pipeline", "Transform pipeline array")?;
            // Phase 815 — normalise the organic-demand source shapes (Static /
            // Bound wrapper / row-major array) before the columnar decode sees
            // them.
            // Phase 818 — a binding-shaped source (State / Selection / Query
            // `$type`) is PRESERVED as `TransformSource::Live`: the decoded
            // binding re-encodes verbatim (one wire dialect) and the runtime
            // re-evaluates the pipeline against it, falling back to the
            // decode-time `initial` snapshot derived from the binding's
            // carried default data. A State wrapper carrying NO data — the bare
            // `{"$type":"State","key":k}` — is a live source over the EMPTY
            // initial snapshot (§16), and a State wrapper's carried data is
            // snapshot-decoded here so the ragged-rows didactic stays
            // byte-identical.
            let live_tag = match src_j {
                JVal::Obj(sf) => match get(sf, "$type") {
                    Some(JVal::Str(t)) if t == "State" || t == "Selection" || t == "Query" => {
                        Some(t.as_str())
                    }
                    _ => None,
                },
                _ => None,
            };
            let source = match live_tag {
                None => {
                    let src_n = normalise_transform_source(src_j);
                    TransformSource::Data(decode_data_source(&src_n).map_err(|e| {
                        make_error(
                            DecodeErrorCode::WrongType,
                            format!("{path}.source"),
                            e,
                            None,
                        )
                    })?)
                }
                Some(tag) => {
                    // The carried data rides the Rows slot, so the preserved
                    // binding's defaultValue re-encodes faithfully.
                    let b =
                        decode_binding_slot(&format!("{path}.source"), src_j, StaticSlot::Rows)?;
                    let carried = match src_j {
                        JVal::Obj(sf) => get(sf, "defaultValue"),
                        _ => None,
                    };
                    // Phase 1656 — a `null` member is a SPELLING OF ABSENCE (§5),
                    // so it carries nothing. It did not read that way before: a
                    // raw `JVal::Null` is `Some` and is not an empty array, so it
                    // fell to the snapshot branch and was decoded as carried data.
                    // The reference host never had the defect because it reads the
                    // DECODED binding's default, where every spelling of absence
                    // has already collapsed to one value; this reads the raw
                    // member, so it has to name them.
                    let has_carried = matches!(carried, Some(v) if !matches!(v, JVal::Null));
                    // §24.4 (slot seeding) — an EMPTY carried array carries no
                    // ROWS, and that is a live source with an empty initial
                    // snapshot, not a malformed document. A declared default
                    // fills the SLOT, so a reader may legitimately declare
                    // `defaultValue: []` and be seeded by a sibling that
                    // declares the rows. Sending it through the columnar decode
                    // instead accuses it: a bare `[]` has no first row to take a
                    // key set from, so it does not transpose and surfaces as
                    // `WRONG_TYPE … expected object, got array` against a
                    // document the reference hosts decode. This is the same
                    // empty-table start the Selection / Query branch below
                    // already takes, on the one input where State needs it too.
                    let empty_carried = matches!(carried, Some(JVal::Arr(rows)) if rows.is_empty());
                    // §16 — an ABSENT `defaultValue` takes the same arm. The bare
                    // `{"$type":"State","key":k}` is a live source over the empty
                    // initial snapshot, exactly as a Selection / Query source
                    // already was. It surfaced the columnar codec's missing-field
                    // didactic until now, which was correct while nothing else
                    // could fill the slot; under §24.4 a sibling reader's
                    // declaration fills it, so the refusal was rejecting the most
                    // direct spelling of "I read this key and carry no data of my
                    // own" — the one FUARAN106's remedy text tells an author to
                    // write. The two spellings say one thing; the empty array
                    // stays the answer for a genuinely empty live collection
                    // rather than a workaround for a wrapper this decoder would
                    // not accept bare.
                    if tag == "State" && (empty_carried || !has_carried) {
                        // Phase 1656 — the absence override that used to sit
                        // here is RETIRED. It substituted
                        // `StaticValue::Ast(JVal::Null)` for the Rows slot's own
                        // placeholder so that `static_is_absent` would omit the
                        // member again on re-encode; since the case carries the
                        // wire fact separately (`default_declared`, Phase 1499)
                        // the decoder already clears it for an absent default and
                        // the encoder already reads the declaration alone, so the
                        // substitution bought nothing and cost the RESOLUTION
                        // default — a bare rows source resolved to a null AST
                        // rather than to the empty feed §24.4 gives it.
                        TransformSource::Live {
                            binding: Box::new(b),
                            initial: empty_embedded_source(),
                        }
                    } else if tag == "State" {
                        // The carried data IS the initial snapshot; a decode
                        // failure (ragged / mixed-type rows) surfaces the same
                        // didactic the 815 snapshot decode raised.
                        let src_n = normalise_transform_source(src_j);
                        let initial = decode_data_source(&src_n).map_err(|e| {
                            make_error(
                                DecodeErrorCode::WrongType,
                                format!("{path}.source"),
                                e,
                                None,
                            )
                        })?;
                        TransformSource::Live {
                            binding: Box::new(b),
                            initial,
                        }
                    } else {
                        // Selection / Query — a tabular carried default seeds
                        // the initial snapshot; anything else starts from the
                        // empty table (runtime evaluation stays loud on a
                        // non-tabular live value, never here).
                        let initial = match src_j {
                            JVal::Obj(sf) => get(sf, "defaultValue")
                                .and_then(|dv| decode_data_source(&transpose_row_major(dv)).ok())
                                .unwrap_or_else(empty_embedded_source),
                            _ => empty_embedded_source(),
                        };
                        TransformSource::Live {
                            binding: Box::new(b),
                            initial,
                        }
                    }
                }
            };
            let pipeline = decode_pipeline(pipe_j).map_err(|e| {
                make_error(
                    DecodeErrorCode::WrongType,
                    format!("{path}.pipeline"),
                    e,
                    None,
                )
            })?;
            // Phase 1662 — §21.8's expression-node bound over the pipeline's own
            // embedded expressions, at DECODE and not at validation: a document
            // that decodes must not be able to name an unbounded evaluation.
            if let Some((i, slot)) = first_pipeline_expr_breach(&pipeline) {
                return Err(make_error(
                    DecodeErrorCode::LimitExceeded,
                    format!("{path}.pipeline[{i}].{slot}"),
                    format!(
                        "expression exceeds the maximum of {} expression nodes (WIRE_FORMAT 21.8)",
                        crate::limits::MAX_EXPR_NODES
                    ),
                    Some(format!(
                        "at most {} ColExpr nodes in one pipeline expression",
                        crate::limits::MAX_EXPR_NODES
                    )),
                ));
            }
            let params = match get(fields, "params") {
                None => None,
                // Lenient AI-ingest (§3.6): a `{name: <Binding>}` MAP is
                // accepted alongside the canonical `[{from, name}]` array —
                // normalised to the array form sorted by name (the reference
                // host's map iteration order).
                Some(JVal::Obj(map_fields)) => {
                    let mut entries: Vec<(&String, &JVal)> =
                        map_fields.iter().map(|(k, v)| (k, v)).collect();
                    entries.sort_by_key(|(k, _)| *k);
                    let mut out = Vec::with_capacity(entries.len());
                    for (name, from_j) in entries {
                        let from = decode_binding(&format!("{path}.params.{name}.from"), from_j)?;
                        out.push(TransformParam {
                            name: name.clone(),
                            from,
                        });
                    }
                    Some(out)
                }
                Some(v) => {
                    let items = as_arr(&format!("{path}.params"), v)?;
                    let p = format!("{path}.params[]");
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        let pf = as_obj(&p, item)?;
                        let name = req_string(&p, pf, "name", "param name string")?;
                        let from_j = req(&p, pf, "from", "param source Binding")?;
                        let from = decode_binding(&format!("{p}.from"), from_j)?;
                        out.push(TransformParam { name, from });
                    }
                    Some(out)
                }
            };
            Ok(Binding::Transform {
                params,
                pipeline,
                source,
            })
        }
        // Phase 1534 — ONE scalar expression evaluated to ONE value (§3.3.2).
        // The expression is the SAME `ColExpr` algebra a `Transform` pipeline
        // step carries, decoded by the same codec, so there is one algebra to
        // specify, certify and teach rather than two that drift apart.
        Some(BindingTag::Expr) => {
            let expr_j = req(path, fields, "expr", "ColExpr object")?;
            let expr_path = format!("{path}.expr");
            let expr = decode_col_expr(expr_j)
                .map_err(|e| make_error(DecodeErrorCode::WrongType, expr_path.clone(), e, None))?;
            // §21.8 — the evaluation bound, counted per EXPRESSION rather than
            // per document. Its scope is `Binding::Expr` and nothing else: a
            // `ColExpr` inside a pipeline is deliberately not covered.
            let nodes = count_expr_nodes(&expr);
            if nodes > crate::limits::MAX_EXPR_NODES {
                return Err(make_error(
                    DecodeErrorCode::LimitExceeded,
                    expr_path,
                    format!(
                        "expression carries {nodes} nodes, above the {} a single Binding.Expr may hold",
                        crate::limits::MAX_EXPR_NODES
                    ),
                    None,
                ));
            }
            let params = decode_binding_params(path, fields)?;
            // The two refusals, both because an `Expr` HAS NO ROW. Left
            // admitted, each would decode to an expression whose evaluation
            // could only ever fail, once per render, on every host — so they
            // are decode-time and unconditional rather than resolution-time.
            if let Some(offender) = first_col_reference(&expr) {
                return Err(wrong_type(
                    &expr_path,
                    &format!(
                        "no column reference — a Binding.Expr evaluates against its params alone                          and has no frame for '{offender}' to read; the remedy is a different                          BINDING, not a different spelling, and Binding.Transform is the case                          that supplies the frame"
                    ),
                ));
            }
            let bound: Vec<&str> = params
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|p| p.name.as_str())
                .collect();
            // Statically decidable HERE where it is not for `Transform`, whose
            // unbound filter params are PRUNED under the deliberate "unset chip
            // ⇒ no constraint" leniency: an `Expr` has no step to prune and no
            // rows to fall back on, so an unbound reference has no value it
            // could ever take.
            if let Some(unbound) = first_unbound_param(&expr, &bound) {
                return Err(wrong_type(
                    &expr_path,
                    &format!(
                        "every referenced param to be bound by the binding's own params list —                          '{unbound}' is not"
                    ),
                ));
            }
            Ok(Binding::Expr { expr, params })
        }
        Some(BindingTag::Invoke) => {
            let capability_id = req_string(path, fields, "capabilityId", "capability id string")?;
            let args_j = req(path, fields, "args", "invoke args array")?;
            let args = decode_invoke_args(&format!("{path}.args"), args_j)?;
            Ok(Binding::Invoke {
                capability_id,
                args,
            })
        }
        None => Err(unknown_du_case(path, tag, BINDING_TAGS)),
    }
}

pub(super) fn req_binding_slot(
    path: &str,
    fields: &Fields,
    key: &str,
    expected: &str,
    slot: StaticSlot,
) -> DResult<Binding> {
    let v = req(path, fields, key, expected)?;
    decode_binding_slot(&format!("{path}.{key}"), v, slot)
}

pub(super) fn opt_binding_slot(
    path: &str,
    fields: &Fields,
    key: &str,
    slot: StaticSlot,
) -> DResult<Option<Binding>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(decode_binding_slot(
            &format!("{path}.{key}"),
            v,
            slot,
        )?)),
    }
}

pub(super) fn req_text_source(
    path: &str,
    fields: &Fields,
    key: &str,
    expected: &str,
) -> DResult<TextSource> {
    let v = req(path, fields, key, expected)?;
    decode_text_source(&format!("{path}.{key}"), v)
}

/// Phase 1537 — fail when a `Confirm` is reachable from `action`. Confirmation
/// is bounded at ONE question: a dialogue that answers a dialogue is a modal
/// stack the reader cannot escape, and it expresses no intent a single question
/// does not.
///
/// It walks the DECODED action rather than raw JSON, and descends `Chain`,
/// because a chain is otherwise a hiding place — a check written against the
/// continuation's immediate `$type` passes a nested confirm one level down.
pub(super) fn refuse_nested_confirm(action: &Action, path: &str) -> DResult<()> {
    match action {
        Action::Confirm { .. } => Err(wrong_type(
            path,
            "no Confirm inside another Confirm's continuation — confirmation is bounded at one \
             question",
        )),
        Action::Chain(ops) => {
            for (i, inner) in ops.iter().enumerate() {
                refuse_nested_confirm(inner, &format!("{path}.ops[{i}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

// ─── Controls (FormFieldKind, decoded in its auto-bind context) ───────────────

/// Phase 596 — the auto-bind context for a control's ABSENT `value` slot. One
/// rule across the whole control vocabulary: every control may omit `value`; a
/// filter chip auto-binds `$filters.<name>` (0.2.0) and a form field
/// auto-binds `$state.<field id>` with the slot's typed placeholder as the
/// State default (0.2.1).
#[derive(Clone, Copy)]
pub(super) enum ControlAutoBind<'a> {
    FilterChip(&'a str),
    FormFieldId(&'a str),
}

impl ControlAutoBind<'_> {
    /// The synthesised binding for an absent `value` slot.
    fn auto_binding(self, placeholder: StaticValue) -> Binding {
        match self {
            ControlAutoBind::FilterChip(name) => Binding::Filter {
                name: name.to_string(),
                default_value: None,
            },
            ControlAutoBind::FormFieldId(id) => Binding::State {
                // The auto-binding is SYNTHESISED, so it declares nothing.
                default_declared: false,
                key: id.to_string(),
                default_value: placeholder,
            },
        }
    }
}

/// The typed placeholders for the 0.2.1 form-field auto-bind, pinned by the
/// reference implementations and the `form-declarative-minimal` fixture:
/// empty string / `0` / `false` / null-choice / `{min 0, max 0}` / ISO-empty
/// date / ISO-empty `{from, to}` pair.
pub(super) mod control_value_defaults {
    use super::*;

    pub fn text() -> StaticValue {
        StaticValue::Ast(JVal::Str(String::new()))
    }
    pub fn number() -> StaticValue {
        StaticValue::Ast(JVal::Num(0.0))
    }
    pub fn checkbox() -> StaticValue {
        StaticValue::Ast(JVal::Bool(false))
    }
    pub fn choice() -> StaticValue {
        StaticValue::StringOpt(None)
    }
    pub fn range() -> StaticValue {
        StaticValue::FloatPair(0.0, 0.0)
    }
    pub fn date_time() -> StaticValue {
        StaticValue::Ast(JVal::Str(String::new()))
    }
    pub fn date_time_range() -> StaticValue {
        StaticValue::StringPair(String::new(), String::new())
    }
    /// Phase 1121 — the EMPTY LIST. The token list is ordered and the order is
    /// the reader's, so an auto-bound token field starts with no chips rather
    /// than with a placeholder one.
    pub fn tokens() -> StaticValue {
        StaticValue::StringList(Vec::new())
    }
    /// Phase 1130 — `Rating` shares `Number`'s zero placeholder: an auto-bound
    /// rating starts unrated.
    pub fn rating() -> StaticValue {
        StaticValue::Ast(JVal::Num(0.0))
    }
    /// Phase 1130 — the unset swatch. A native colour input substitutes its own
    /// default when handed nothing, and `#000000` is that default's wire
    /// spelling — the one `#rrggbb` form the control can hold.
    pub fn color() -> StaticValue {
        StaticValue::Ast(JVal::Str("#000000".to_string()))
    }
}

/// `#rrggbb` — six hexadecimal digits after a `#`, either case
/// (WIRE_FORMAT.md §3.6.17). Deliberately narrower than CSS: it is the one
/// shape a native colour input can hold or return.
pub fn is_hex_colour(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

/// Phase 1811 — the `variant` of a `DateTime` / `DateTimeRange` field, read
/// through the §16 `Time` / `TimeRange` alias rule. Under the canonical tag (or
/// the pre-rename alias) `variant` is required as it always was. Under the
/// time-alias tag the alias SUPPLIES `Time` when the member is absent, and an
/// explicit member beside it must agree — a `$type` of `Time` carrying
/// `variant: "Date"` is refused as ambiguous rather than resolved to either, the
/// 0.28.0 column-member posture applied to a `$type`.
pub(super) fn temporal_variant(
    path: &str,
    fields: &Fields,
    tag: &str,
    time_alias: &str,
) -> DResult<DateTimeVariant> {
    let variant_path = format!("{path}.variant");
    if tag != time_alias {
        let v = req(path, fields, "variant", "DateTimeVariant")?;
        return decode_date_time_variant(&variant_path, v);
    }
    match get(fields, "variant") {
        None => Ok(DateTimeVariant::Time),
        Some(v) => match decode_date_time_variant(&variant_path, v)? {
            DateTimeVariant::Time => Ok(DateTimeVariant::Time),
            _ => Err(wrong_type(
                &variant_path,
                &format!(
                    "the variant Time, or no variant at all — a $type of {time_alias} already fixes the variant to Time, and a different one beside it is ambiguous"
                ),
            )),
        },
    }
}

pub(super) fn decode_form_field_kind(
    auto_bind: ControlAutoBind<'_>,
    path: &str,
    j: &JVal,
) -> DResult<FormFieldKind> {
    let fields = as_obj(path, j)?;
    // Phase 2177 — every form-field kind carries its change handler as
    // `onChange`, Checkbox and Toggle included.
    let on_change = opt_closure(fields, "onChange");
    // Value slot: present ⇒ typed decode; absent ⇒ the context's auto-binding
    // — Filter(name) on a chip, State(field id, typed placeholder) on a form
    // field (Phase 596).
    let value_or = |slot: StaticSlot, placeholder: StaticValue| -> DResult<Binding> {
        match get(fields, "value") {
            Some(v) => decode_binding_slot(&format!("{path}.value"), v, slot),
            None => Ok(auto_bind.auto_binding(placeholder)),
        }
    };
    let tag = disc(path, fields)?;
    // The Date/Time spellings name the temporal controls; `temporal_variant`
    // reads which one was written.
    let canonical = match tag {
        "Date" | "Time" => "DateTime",
        "DateRange" | "TimeRange" => "DateTimeRange",
        t => t,
    };
    match FormFieldKindTag::of(canonical) {
        Some(FormFieldKindTag::Text) => Ok(FormFieldKind::Text {
            value: value_or(StaticSlot::Str, control_value_defaults::text())?,
            on_change,
        }),
        Some(FormFieldKindTag::Number) => Ok(FormFieldKind::Number {
            value: value_or(StaticSlot::Float, control_value_defaults::number())?,
            on_change,
        }),
        Some(FormFieldKindTag::Checkbox) => Ok(FormFieldKind::Checkbox {
            value: value_or(StaticSlot::Bool, control_value_defaults::checkbox())?,
            on_change,
        }),
        // Phase 766 — the switch affordance: Checkbox's mechanics under a
        // distinct tag.
        Some(FormFieldKindTag::Toggle) => Ok(FormFieldKind::Toggle {
            value: value_or(StaticSlot::Bool, control_value_defaults::checkbox())?,
            on_change,
        }),
        Some(FormFieldKindTag::Choice) => Ok(FormFieldKind::Choice {
            options: req_binding_slot(
                path,
                fields,
                "options",
                "Choice options binding",
                StaticSlot::Options,
            )?,
            value: value_or(StaticSlot::StringOpt, control_value_defaults::choice())?,
            on_change,
        }),
        // Phase 1113 - the searchable form of `Choice`, sharing its value contract
        // exactly: a document migrating between the two changes its `$type` and
        // nothing else, so the auto-bind placeholder is `choice()`'s too.
        // `allowFreeText` is NOT truthiness-coerced - a lenient read would widen
        // the field on `"no"` and `"false"` alike.
        Some(FormFieldKindTag::Combobox) => Ok(FormFieldKind::Combobox {
            options: req_binding_slot(
                path,
                fields,
                "options",
                "Combobox options binding",
                StaticSlot::Options,
            )?,
            value: value_or(StaticSlot::StringOpt, control_value_defaults::choice())?,
            allow_free_text: opt_bool(path, fields, "allowFreeText")?
                .unwrap_or(defaults::FORM_FIELD_KIND_COMBOBOX_ALLOW_FREE_TEXT),
            on_change,
        }),
        // 0.2.0 — the dual-thumb numeric range (absorbed FilterKind.RangeFilter).
        // The canonical Static pair rides as the BARE `{min, max}` object (no
        // `$type`) — accept it before the generic binding dispatch.
        Some(FormFieldKindTag::Range) => Ok(FormFieldKind::Range {
            value: match get(fields, "value") {
                Some(JVal::Obj(pf))
                    if get(pf, "$type").is_none()
                        && get(pf, "min").is_some()
                        && get(pf, "max").is_some() =>
                {
                    Binding::Static {
                        value: StaticSlot::FloatPair
                            .parse(&format!("{path}.value"), &JVal::Obj(pf.clone()))?,
                    }
                }
                _ => value_or(StaticSlot::FloatPair, control_value_defaults::range())?,
            },
            min: opt_float(path, fields, "min")?,
            max: opt_float(path, fields, "max")?,
            step: opt_float(path, fields, "step")?,
            on_change,
        }),
        Some(FormFieldKindTag::RangedNumber) => Ok(FormFieldKind::RangedNumber {
            value: value_or(StaticSlot::Float, control_value_defaults::number())?,
            min: opt_float(path, fields, "min")?,
            max: opt_float(path, fields, "max")?,
            step: opt_float(path, fields, "step")?,
            on_change,
        }),
        Some(FormFieldKindTag::SegmentedChoice) => {
            let options = req_binding_slot(
                path,
                fields,
                "options",
                "SegmentedChoice options binding",
                StaticSlot::Options,
            )?;
            // Omit-at-default (§3.6): absent restores the IDL's declared default
            // `Horizontal` — declared since Phase 2300, so the encoder omits it
            // too, as `Tabs.orientation` always did.
            let orientation = match get(fields, "orientation") {
                None => super::generated::defaults::FORM_FIELD_KIND_SEGMENTED_CHOICE_ORIENTATION,
                Some(v) => decode_orientation(&format!("{path}.orientation"), v)?,
            };
            let value = value_or(StaticSlot::StringOpt, control_value_defaults::choice())?;
            Ok(FormFieldKind::SegmentedChoice {
                options,
                orientation,
                value,
                on_change,
            })
        }
        Some(FormFieldKindTag::TextArea) => Ok(FormFieldKind::TextArea {
            rows: req_int(path, fields, "rows", "textarea row count integer")?,
            value: value_or(StaticSlot::Str, control_value_defaults::text())?,
            on_change,
        }),
        // Phase 1811 — `DateTime` is canonical. `Date` is the pre-rename spelling,
        // kept as a §16 lenient alias by the reference host's D8 ruling; `Time` is
        // the invented spelling the rename exists to make findable — a
        // `DateTime{variant:"Time"}` reached for by intent, so the alias SUPPLIES
        // the variant when absent and REFUSES a disagreeing one beside it. All
        // three re-encode canonical.
        Some(FormFieldKindTag::DateTime) => {
            let value = value_or(StaticSlot::Str, control_value_defaults::date_time())?;
            let variant = temporal_variant(path, fields, tag, "Time")?;
            Ok(FormFieldKind::DateTime {
                value,
                variant,
                min: opt_string(path, fields, "min")?,
                max: opt_string(path, fields, "max")?,
                step: opt_float(path, fields, "step")?,
                on_change,
            })
        }
        // Phase 1811 — `DateTimeRange` is canonical; `DateRange` (pre-rename) and
        // `TimeRange` (invented, fixes `variant` to `Time`) are its §16 lenient
        // aliases on exactly the `DateTime` rule above.
        Some(FormFieldKindTag::DateTimeRange) => {
            // The canonical Static pair rides as the BARE `{from, to}` object (no
            // `$type`) — accept it before the generic binding dispatch, exactly as
            // `Range` does above. The `value_or` fallback is what carries BOTH
            // lenient forms: `decode_binding_slot` routes a bare two-element array
            // and a `Static` envelope into `slot.parse`.
            let value = match get(fields, "value") {
                Some(JVal::Obj(pf))
                    if get(pf, "$type").is_none()
                        && get(pf, "from").is_some()
                        && get(pf, "to").is_some() =>
                {
                    Binding::Static {
                        value: StaticSlot::StringPair
                            .parse(&format!("{path}.value"), &JVal::Obj(pf.clone()))?,
                    }
                }
                _ => value_or(
                    StaticSlot::StringPair,
                    control_value_defaults::date_time_range(),
                )?,
            };
            let variant = temporal_variant(path, fields, tag, "TimeRange")?;
            Ok(FormFieldKind::DateTimeRange {
                value,
                variant,
                min: opt_string(path, fields, "min")?,
                max: opt_string(path, fields, "max")?,
                step: opt_float(path, fields, "step")?,
                on_change,
            })
        }
        // Phase 1121 — the multi-token input. Every member is OPTIONAL, and
        // `allowFreeText` omits at TRUE (the opposite polarity to `Combobox`,
        // whose option source is required). The one decode refusal is the
        // control that cannot exist: free text denied AND no suggestion source,
        // so no gesture could ever put a token in. It is refused at
        // `allowFreeText` rather than at `suggestions`, because the member that
        // was written is the one that names the impossible state — an absent
        // `suggestions` is the ordinary open token box.
        Some(FormFieldKindTag::Tokens) => {
            let suggestions = opt_binding_slot(path, fields, "suggestions", StaticSlot::Options)?;
            let allow_free_text = opt_bool(path, fields, "allowFreeText")?
                .unwrap_or(defaults::FORM_FIELD_KIND_TOKENS_ALLOW_FREE_TEXT);
            if !allow_free_text && suggestions.is_none() {
                return Err(wrong_type(
                    &format!("{path}.allowFreeText"),
                    "a Tokens field admitting no free text needs a suggestion source — with \
                     neither, no gesture could ever put a token into it",
                ));
            }
            Ok(FormFieldKind::Tokens {
                value: value_or(StaticSlot::StringList, control_value_defaults::tokens())?,
                suggestions,
                allow_free_text,
                on_change,
            })
        }
        // Phase 1130 — the score. `max` IS the scale, so it is required and a
        // value below 1 is refused rather than clamped: a scale with no
        // positions names a control that cannot exist. Note the asymmetry the
        // corpus pins — the SCALE is refused here and the VALUE is not, because
        // a bound value is invisible to a decoder and a rule enforced only on
        // literals would be two rules wearing one name.
        Some(FormFieldKindTag::Rating) => {
            let max = req_int(path, fields, "max", "rating scale integer")?;
            if max < 1 {
                return Err(wrong_type(
                    &format!("{path}.max"),
                    "a rating scale of at least 1 — a scale with no positions has nothing to \
                     draw and no keystroke that could change anything",
                ));
            }
            Ok(FormFieldKind::Rating {
                value: value_or(StaticSlot::Float, control_value_defaults::rating())?,
                max,
                // Governs ENTRY granularity, never display: a host must not
                // quantise a resolved value to it.
                allow_half: opt_bool(path, fields, "allowHalf")?
                    .unwrap_or(defaults::FORM_FIELD_KIND_RATING_ALLOW_HALF),
                on_change,
            })
        }
        // Phase 1130 — the swatch. Only the STATIC case is judged here, and the
        // split is recorded rather than hidden: a State / Query / Selection
        // binding carries its text from outside the document, where a decoder
        // cannot see it. The same rule is owed by the pre-emit validator and by
        // the server-side submission floor.
        Some(FormFieldKindTag::Color) => {
            let value = value_or(StaticSlot::Str, control_value_defaults::color())?;
            if let Binding::Static {
                value: StaticValue::Ast(JVal::Str(literal)),
            } = &value
                && !is_hex_colour(literal)
            {
                return Err(wrong_type(
                    &format!("{path}.value"),
                    "a `#rrggbb` colour — the one shape a native colour input can hold, so a \
                     literal outside it names a colour this control could never carry",
                ));
            }
            Ok(FormFieldKind::Color { value, on_change })
        }
        // The hint is DERIVED from the canonical vocabulary, never hand-typed —
        // the hand-typed form had already drifted (Phase 746).
        None => Err(unknown_du_case(path, tag, CANONICAL_FORM_FIELD_KINDS)),
    }
}

/// The rule slot's rejected spellings. Small and enumerated for the same reason
/// the grid's set is: tolerance of unknown keys is right for a field a future
/// profile may add and wrong for a near miss of one that exists, because the tree
/// then decodes and renders while constraining nothing.
pub(super) const FORM_FIELD_NEAR_MISSES: &[(&str, &str)] = &[
    ("validation", "rule"),
    ("constraints", "rule"),
    ("validate", "rule"),
];

/// What the silence costs at this position, appended to the refusal message —
/// the same shape as `A11Y_NEAR_MISS_CONSEQUENCE`, and pinned to the reference
/// hosts' wording, as is the `form` vocabulary label the call site passes
/// (Phase 1659 — this host and `fuaran-py` both said `form field`, and both
/// moved). A near-missed rule slot does not merely go unread: the field
/// still renders, and it constrains nothing at all.
pub(super) const FORM_FIELD_NEAR_MISS_CONSEQUENCE: &str = ", and the field would accept anything";

// ─── Tone maps, near misses, dates ───────────────────────────────────────────

/// The tone-map field names a `TonedPill` cell accepts, canonical first. `map` is
/// the shortest honest name for a value→tone dictionary and the least descriptive.
pub(super) const TONE_MAP_KEYS: [&str; 3] = ["map", "toneMap", "tones"];

/// Phase 750 — a `TonedPill`'s `map`: a string-keyed object whose VALUES are
/// `ToneVariant`s. Routed through `decode_tone_variant` per entry, so the §3.6 tone aliases
/// work inside the map exactly as they do at a `tone` field; a second, private tone
/// reader here is precisely how this position would come to accept a vocabulary the
/// `tone` field does not.
///
/// The refusal is RE-ISSUED rather than passed through. `unknown_du_case` reports at
/// `<path>.$type` with "unknown discriminator", and a map value is neither a
/// discriminator nor at a `$type` key — so the raw error names a path the document
/// does not contain, which is actively misleading for a fixture whose whole purpose
/// is didactic. The re-issue keeps the code and the seven legal names and points at
/// the offending KEY, because "one of your tones is wrong" is not an actionable
/// report when the map has nine entries. A non-string value is a `WRONG_TYPE` and
/// already reports at the right path, so it passes through untouched.
pub(super) fn decode_tone_map(path: &str, j: &JVal) -> DResult<BTreeMap<String, ToneVariant>> {
    let fields = as_obj(path, j)?;
    let mut map = BTreeMap::new();
    for (key, v) in fields {
        let entry_path = format!("{path}.{key}");
        let tone = decode_tone_variant(&entry_path, v).map_err(|e| {
            if e.code != DecodeErrorCode::UnknownDuCase {
                return e;
            }
            let got = match v {
                JVal::Str(s) => s.as_str(),
                _ => "",
            };
            make_error(
                DecodeErrorCode::UnknownDuCase,
                &entry_path,
                format!("tone-map value '{got}' for '{key}' is not a ToneVariant"),
                Some(ToneVariant::WIRE_NAMES.join(" | ")),
            )
        })?;
        map.insert(key.clone(), tone);
    }
    Ok(map)
}

/// Decode-time didactics for the grid-behaviour family's NEAR MISSES (Phase
/// 860's charter, its rejected-spellings deliverable).
///
/// Every shape in the tables below decoded SILENTLY before: §2 rule 2 tolerates
/// unknown keys, so a model that reached for the wrong name got a tree that
/// decoded, validated and rendered while the declaration did nothing — the
/// fake-affordance failure in a new guise, and tolerance is what hid it. The
/// narrowing is an ENUMERATED set with an unambiguous canonical form each; rule
/// 2 holds for everything else.
/// The RETIRED positional slot on `InsertChild` / `MoveNode` (Phase 687, closing
/// the window Phase 681 opened).
///
/// Phase 681 removed the field and every host then ACCEPTED AND IGNORED it so
/// each could adopt independently. Silence was the whole mechanism: this decoder
/// reads named fields and ignores the rest, so *not reading it* was the
/// tolerance. That is also why closing the window cannot be done by deletion —
/// there was never a read to delete, and the field would go on decoding silently
/// forever. The close is an explicit refusal BY NAME, on the `check_near_misses`
/// pattern and for its reason: a key that no-ops is worse than one that fails,
/// because the op decodes, applies, and puts the node somewhere other than where
/// the ordinal asked.
///
/// Called BEFORE the required-field reads, mirroring the `FormField` near-miss
/// ordering, so an op carrying both a retired ordinal and some other defect names
/// the ordinal. The ordering is identical in all five hosts, so which defect
/// surfaces first is deterministic.
pub(super) fn retired_positional_field(
    path: &str,
    fields: &Fields,
    name: &str,
    op_kind: &str,
) -> DResult<()> {
    if get(fields, name).is_some() {
        return Err(make_error(
            DecodeErrorCode::WrongType,
            format!("{path}.{name}"),
            format!(
                "'{name}' was removed from the wire format — {op_kind} appends, and order is stated by naming ids with ReorderChildren"
            ),
            Some("a Batch of the structural op followed by ReorderChildren".to_string()),
        ));
    }
    Ok(())
}

pub(super) fn check_near_misses(
    path: &str,
    fields: &Fields,
    candidates: &[(&str, &str)],
) -> DResult<()> {
    check_near_misses_in(path, fields, candidates, "grid", "")
}

/// The near-miss check, with the vocabulary NAMED. The message says which
/// vocabulary the key is not part of, so a form field's near miss does not report
/// itself against the grid's — the refusal is didactic, and a didactic message
/// that names the wrong vocabulary sends the author to the wrong document.
/// `consequence` is an optional trailing clause naming what the silence costs in
/// that particular vocabulary (Phase 959) — the refusal is didactic, and the
/// didactic is sharper when it says what was LOST, not only what was ignored.
/// Empty for the grid, whose message the four other hosts pin unchanged.
pub(super) fn check_near_misses_in(
    path: &str,
    fields: &Fields,
    candidates: &[(&str, &str)],
    vocabulary: &str,
    consequence: &str,
) -> DResult<()> {
    for (found, canonical) in candidates {
        if get(fields, found).is_some() {
            return Err(make_error(
                DecodeErrorCode::WrongType,
                format!("{path}.{found}"),
                format!(
                    "'{found}' is not part of the {vocabulary} vocabulary — it would be ignored, not honoured{consequence}"
                ),
                Some((*canonical).to_string()),
            ));
        }
    }
    Ok(())
}

/// The `Accessibility` trait's near-miss set (Phase 959 — the Phase 863
/// discipline applied to the §3.1 trait).
///
/// Rule 2's tolerance of unknown keys is right for a slot a future profile may
/// add and wrong for a near miss of one that exists. That silence is sharper
/// here than anywhere else in the vocabulary, for a reason peculiar to this
/// trait: it has **no visible output**. A mislabelled column is on screen; an
/// ignored `ariaLabel` looks identical to an honoured one from every side, so
/// the refusal is the only feedback that can ever arrive.
///
/// Refused rather than aliased. `ariaLabel` IS an unambiguous synonym, so
/// admission turns on §16's other half — a shorthand earns its place by being a
/// genuine assist to the emitting model, and a six-character key rename is not
/// one. `live` settles it: the HTML idiom it comes from also spells a BOOLEAN,
/// so an alias would bind a possibly-boolean prior onto a closed token set.
///
/// `live` and `ariaLabel` are named by MEASURED evidence (6 and 1 emissions
/// against `liveRegion`'s 12 and `label`'s 44, across 12,722 language-tier
/// emissions); the rest of their families ride in with them. Declaration order
/// is identical in all five hosts, so which defect surfaces first is
/// deterministic.
pub(super) const A11Y_NEAR_MISSES: &[(&str, &str)] = &[
    (
        "aria-label",
        "label — the accessible name, a Binding<string> (a bare string is the §3.6 shorthand)",
    ),
    (
        "ariaLabel",
        "label — the accessible name, a Binding<string> (a bare string is the §3.6 shorthand)",
    ),
    (
        "aria-labelledby",
        "labelledBy — the id of a sibling node whose text carries the name",
    ),
    (
        "ariaLabelledBy",
        "labelledBy — the id of a sibling node whose text carries the name",
    ),
    (
        "labelledby",
        "labelledBy — the slot name is camelCase on the wire, not the ARIA attribute spelling",
    ),
    (
        "aria-describedby",
        "describedBy — the id of a sibling node whose text carries the description",
    ),
    (
        "ariaDescribedBy",
        "describedBy — the id of a sibling node whose text carries the description",
    ),
    (
        "describedby",
        "describedBy — the slot name is camelCase on the wire, not the ARIA attribute spelling",
    ),
    ("aria-role", "role — the ARIA role NAME as a bare string"),
    ("ariaRole", "role — the ARIA role NAME as a bare string"),
    (
        "aria-live",
        "liveRegion — the closed token set \"polite\" / \"assertive\" / \"off\"",
    ),
    (
        "ariaLive",
        "liveRegion — the closed token set \"polite\" / \"assertive\" / \"off\"",
    ),
    (
        "live",
        "liveRegion — the closed token set \"polite\" / \"assertive\" / \"off\"",
    ),
    (
        "liveregion",
        "liveRegion — the closed token set \"polite\" / \"assertive\" / \"off\"",
    ),
    (
        "aria-hidden",
        "hidden — a Binding<bool> (a bare bool is the §3.6 shorthand)",
    ),
    (
        "ariaHidden",
        "hidden — a Binding<bool> (a bare bool is the §3.6 shorthand)",
    ),
];

/// What the silence costs at this position, appended to the refusal message.
pub(super) const A11Y_NEAR_MISS_CONSEQUENCE: &str =
    ", and the intent would reach assistive technology as nothing at all";

/// Named by the census row itself. Deliberately NOT aliased to `editable:
/// false`: an inverting alias that guesses wrong makes a read-only column
/// editable.
pub(super) const COLUMN_NEAR_MISSES: &[(&str, &str)] = &[(
    "readOnly",
    "editable: false — the column flag NARROWS the grid's editable capability",
)];

/// The grid-level rejected spellings, walked in declaration order so which
/// defect surfaces first is deterministic across hosts.
pub(super) const GRID_NEAR_MISSES: &[(&str, &str)] = &[
    // The sharpest of them: a LITERAL page number is not expressible at all,
    // because the position lives in State so a control can move it.
    (
        "currentPage",
        "pageStateKey — the page POSITION lives in State as {\"page\": N} so the pager can move it; a literal page number is not expressible",
    ),
    (
        "page",
        "pageStateKey — the page POSITION lives in State as {\"page\": N} so the pager can move it; a literal page number is not expressible",
    ),
    (
        "pageIndex",
        "pageStateKey — the page POSITION lives in State as {\"page\": N}, 1-based (not a zero-based index)",
    ),
    (
        "sortable",
        "sortStateKey on the grid + sortable on each COLUMN — grid-wide sortable is the staticRows spelling; a data-bound grid narrows per column",
    ),
    (
        "onEdit",
        "editStateKey — the edit DESTINATION is a State key on the grid; onEdit is a per-cell host closure and carries no destination across the wire",
    ),
    (
        "behaviour",
        "sibling fields on the grid (sortStateKey / pageStateKey / pageSize / editStateKey / defaultSort) — grid behaviour is not a nested record",
    ),
    (
        "behavior",
        "sibling fields on the grid (sortStateKey / pageStateKey / pageSize / editStateKey / defaultSort) — grid behaviour is not a nested record",
    ),
];

/// `true` when `text` is a canonical ISO-8601 date the temporal axis can place —
/// `YYYY-MM-DD`, optionally followed by `T…` whose time-of-day is discarded.
///
/// STRICT by shape AND by calendar: four digits, two, two, both hyphens, a month
/// in 1–12 and a day the month actually has. A locale spelling (`15/01/2026`) and
/// a bare year are both refused — admitting either would be the string-sniffing
/// the temporal axis exists to avoid.
pub(super) fn is_canonical_iso_day(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if b.len() > 10 && b[10] != b'T' {
        return false;
    }
    let digits = |start: usize, len: usize| -> Option<i64> {
        let mut acc: i64 = 0;
        for byte in &b[start..start + len] {
            if !byte.is_ascii_digit() {
                return None;
            }
            acc = acc * 10 + i64::from(byte - b'0');
        }
        Some(acc)
    };
    let (Some(y), Some(m), Some(d)) = (digits(0, 4), digits(5, 2), digits(8, 2)) else {
        return false;
    };
    if !(1..=12).contains(&m) {
        return false;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let last = match m {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=last).contains(&d)
}

pub(super) fn decode_box_role(path: &str, j: &JVal) -> DResult<BoxRole> {
    let s = as_str(path, j)?;
    BoxRole::from_wire(s)
        .ok_or_else(|| unknown_enum_case(path, s, "Group | Card | Dashboard | Separator"))
}

/// A `FragmentArg` map entry: `SlotArg` carries a subtree; any other
/// discriminator reads as a value scalar (Int | Float | Bool | Str).
pub(super) fn decode_fragment_arg(path: &str, j: &JVal) -> DResult<FragmentArg> {
    let fields = as_obj(path, j)?;
    let tag = disc(path, fields)?;
    if tag == "SlotArg" {
        let tree_j = req(path, fields, "tree", "SlotArg tree Node")?;
        let tree = decode_node_ast(&format!("{path}.tree"), tree_j)?;
        Ok(FragmentArg::Slot {
            tree: Box::new(tree),
        })
    } else if SCALAR_TAGS.contains(&tag) {
        Ok(FragmentArg::Value(decode_fragment_scalar(path, j)?))
    } else {
        Err(unknown_du_case(path, tag, FRAGMENT_ARG_TAGS))
    }
}

pub(super) fn decode_fragment_args(path: &str, j: &JVal) -> DResult<Vec<(String, FragmentArg)>> {
    let fields = as_obj(path, j)?;
    let mut out = Vec::with_capacity(fields.len());
    for (key, value) in fields {
        out.push((
            key.clone(),
            decode_fragment_arg(&format!("{path}.{key}"), value)?,
        ));
    }
    Ok(out)
}

// ─── The generated codec's helpers ───────────────────────────────────────────

/// A required field (or one of its foreign spellings), decoded at the canonical
/// field's path. The canonical name wins when both are present.
pub(super) fn field_req<T>(
    path: &str,
    fields: &Fields,
    key: &str,
    aliases: &[&str],
    expected: &str,
    f: impl FnOnce(&str, &JVal) -> DResult<T>,
) -> DResult<T> {
    let v = req_aliased(path, fields, key, aliases, expected)?;
    f(&format!("{path}.{key}"), v)
}

/// An optional field: absent is `None`.
pub(super) fn field_opt<T>(
    path: &str,
    fields: &Fields,
    key: &str,
    aliases: &[&str],
    f: impl FnOnce(&str, &JVal) -> DResult<T>,
) -> DResult<Option<T>> {
    match get_aliased(fields, key, aliases) {
        None => Ok(None),
        Some(v) => f(&format!("{path}.{key}"), v).map(Some),
    }
}

/// An omit-at-default field: absent is the IDL's default.
pub(super) fn field_or<T>(
    path: &str,
    fields: &Fields,
    key: &str,
    aliases: &[&str],
    default: T,
    f: impl FnOnce(&str, &JVal) -> DResult<T>,
) -> DResult<T> {
    match get_aliased(fields, key, aliases) {
        None => Ok(default),
        Some(v) => f(&format!("{path}.{key}"), v),
    }
}

/// A list: each element at `path[i]`.
pub(super) fn dec_list<T>(
    path: &str,
    j: &JVal,
    f: impl Fn(&str, &JVal) -> DResult<T>,
) -> DResult<Vec<T>> {
    let items = as_arr(path, j)?;
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        out.push(f(&format!("{path}[{i}]"), item)?);
    }
    Ok(out)
}

/// A map: each entry at `path.key`, in document order.
pub(super) fn dec_map<T>(
    path: &str,
    j: &JVal,
    f: impl Fn(&str, &JVal) -> DResult<T>,
) -> DResult<Vec<(String, T)>> {
    let entries = as_obj(path, j)?;
    let mut out = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        out.push((k.clone(), f(&format!("{path}.{k}"), v)?));
    }
    Ok(out)
}

pub(super) fn dec_string(path: &str, j: &JVal) -> DResult<String> {
    Ok(as_str(path, j)?.to_string())
}

/// A closure slot: its presence is the whole fact (the sentinel carries nothing).
pub(super) fn dec_closure(_path: &str, _j: &JVal) -> DResult<Closure> {
    Ok(Closure)
}

/// A bare-string enum: its wire spelling, else one of its foreign spellings,
/// else `UNKNOWN_DU_CASE` at the field's own path.
pub(super) fn decode_enum<T: Copy>(
    path: &str,
    j: &JVal,
    label: &str,
    from_wire: fn(&str) -> Option<T>,
    names: &[&str],
    aliases: &[(&str, T)],
) -> DResult<T> {
    let s = match j {
        JVal::Str(s) => s,
        _ => return Err(wrong_type(path, &format!("JSON string ({label})"))),
    };
    from_wire(s)
        .or_else(|| aliases.iter().find(|(a, _)| a == s).map(|(_, case)| *case))
        .ok_or_else(|| unknown_enum_case(path, s, &names.join(" | ")))
}

/// An unknown `NodeKind` discriminator is `WRONG_NODE_KIND` (§6), its hint every
/// kind grouped by category.
pub(super) fn wrong_node_kind(path: &str, other: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::WrongNodeKind,
        format!("{path}.$type"),
        format!("unknown NodeKind discriminator '{other}'"),
        Some(WRONG_NODE_KIND_HINT.to_string()),
    )
}

// ─── Decode hooks (declared in dialect.rs) ───────────────────────────────────
//
// A field hook is `fn(path, fields) -> DResult<_>` returning the member(s) its
// field maps to; an owner hook decodes the whole owner; a check refuses after (or
// before) the uniform reads; an enter hook holds a guard for the owner's decode.

/// Enums that read through a `Static` envelope like a plain scalar does.
fn decode_enum_unwrapped<T: Copy>(
    path: &str,
    j: &JVal,
    from_wire: fn(&str) -> Option<T>,
    names: &[&str],
) -> DResult<T> {
    let s = as_str(path, j)?;
    from_wire(s).ok_or_else(|| unknown_enum_case(path, s, &names.join(" | ")))
}

pub(super) fn decode_hash_strictness(path: &str, j: &JVal) -> DResult<HashStrictness> {
    decode_enum_unwrapped(
        path,
        j,
        HashStrictness::from_wire,
        HashStrictness::WIRE_NAMES,
    )
}

pub(super) fn decode_host_effect(path: &str, j: &JVal) -> DResult<HostEffect> {
    decode_enum_unwrapped(path, j, HostEffect::from_wire, HostEffect::WIRE_NAMES)
}

pub(super) fn decode_determinism_source(path: &str, j: &JVal) -> DResult<DeterminismSource> {
    decode_enum_unwrapped(
        path,
        j,
        DeterminismSource::from_wire,
        DeterminismSource::WIRE_NAMES,
    )
}

pub(super) fn decode_channel_direction(path: &str, j: &JVal) -> DResult<ChannelDirection> {
    decode_enum_unwrapped(
        path,
        j,
        ChannelDirection::from_wire,
        ChannelDirection::WIRE_NAMES,
    )
}

/// `Custom.exposedNodeIds` — an absent list is the empty list.
pub(super) fn decode_exposed_node_ids(path: &str, fields: &Fields) -> DResult<Vec<NodeId>> {
    field_or(path, fields, "exposedNodeIds", &[], Vec::new(), |p, j| {
        dec_list(p, j, dec_string)
    })
}

/// `FragmentDecl.effect` — absent is the pure deterministic class.
pub(super) fn decode_fragment_effect(path: &str, fields: &Fields) -> DResult<EffectClass> {
    field_or(
        path,
        fields,
        "effect",
        &[],
        EffectClass::PURE_DETERMINISTIC,
        decode_effect_class,
    )
}

/// `FragmentRef.args` — an absent map is the empty map.
pub(super) fn decode_fragment_ref_args(
    path: &str,
    fields: &Fields,
) -> DResult<Vec<(String, FragmentArg)>> {
    field_or(path, fields, "args", &[], Vec::new(), decode_fragment_args)
}

/// `Mount.inputs` — an absent map is the empty map.
pub(super) fn decode_mount_inputs(
    path: &str,
    fields: &Fields,
) -> DResult<Vec<(String, FragmentArg)>> {
    field_or(
        path,
        fields,
        "inputs",
        &[],
        Vec::new(),
        decode_fragment_args,
    )
}

/// `Switch.on` — absent, the compact `stateKey` spelling names a State binding
/// (required in that case). An explicit `on` decodes untyped.
pub(super) fn decode_switch_on(path: &str, fields: &Fields) -> DResult<Binding> {
    match get(fields, "on") {
        Some(v) => decode_binding(&format!("{path}.on"), v),
        None => Ok(Binding::State {
            key: req_string(path, fields, "stateKey", "Switch stateKey string")?,
            default_value: StaticValue::Ast(JVal::Null),
            default_declared: false,
        }),
    }
}

/// `Switch.stateKey` — read by `decode_switch_on`, which it folds into.
pub(super) fn decode_switch_state_key(_path: &str, _fields: &Fields) -> DResult<()> {
    Ok(())
}

/// `SwitchCase` — exactly one of `match` (a string) and `when` (a bool binding),
/// flattened into the case record.
pub(super) fn decode_switch_case_folded(path: &str, j: &JVal) -> DResult<SwitchCase> {
    let cf = as_obj(path, j)?;
    let condition = match (get(cf, "match"), get(cf, "when")) {
        (Some(_), Some(_)) => {
            return Err(wrong_type(
                &format!("{path}.when"),
                "exactly one of 'match' and 'when' — a precedence rule between \
                 them would have to be agreed on every host for a document \
                 nobody meant to write",
            ));
        }
        (None, Some(when_j)) => SwitchCondition::When(decode_binding_slot(
            &format!("{path}.when"),
            when_j,
            StaticSlot::Bool,
        )?),
        _ => SwitchCondition::Match(req_string(path, cf, "match", "Switch case match string")?),
    };
    let child = field_req(
        path,
        cf,
        "child",
        &[],
        "Switch case child Node",
        decode_node_ast,
    )?;
    Ok(SwitchCase { child, condition })
}

/// `Switch.autoAdvanceMs` — a positive whole interval; an absent key is off.
pub(super) fn check_switch(path: &str, _fields: &Fields, spec: &SwitchSpec) -> DResult<()> {
    match spec.auto_advance_ms {
        Some(ms) if ms < 1 => Err(wrong_type(
            &format!("{path}.autoAdvanceMs"),
            "a positive millisecond interval — an absent key is already the \
             spelling for off",
        )),
        _ => Ok(()),
    }
}

/// An integer-binding slot omitted at the literal binding `0` (the IDL's
/// `omitDefault Static{value=0}`, which the defaults table does not render):
/// absent is that literal; present decodes at an integer slot.
fn int_binding_or_zero(path: &str, fields: &Fields, key: &str) -> DResult<Binding> {
    match get(fields, key) {
        None => Ok(Binding::Static {
            value: StaticValue::Ast(JVal::Num(0.0)),
        }),
        Some(v) => decode_binding_slot(&format!("{path}.{key}"), v, StaticSlot::Int),
    }
}

/// `Tabs.activeIndex` — absent is the literal binding `0` (omit-at-default by
/// contract); present decodes at an integer slot.
pub(super) fn decode_tabs_active_index(path: &str, fields: &Fields) -> DResult<Binding> {
    int_binding_or_zero(path, fields, "activeIndex")
}

/// `Stepper.activeStep` — the same slot shape as `Tabs.activeIndex`, declared
/// omit-at-default `Static{value=0}` by Phase 2300.
pub(super) fn decode_stepper_active_step(path: &str, fields: &Fields) -> DResult<Binding> {
    int_binding_or_zero(path, fields, "activeStep")
}

/// `Accessibility.role` — the hosted ARIA role rides as its token string.
pub(super) fn decode_aria_role(path: &str, fields: &Fields) -> DResult<Option<String>> {
    opt_string(path, fields, "role")
}

/// `SelectOption.label` / `MapMarker.label` — read as a full `TextSource`.
pub(super) fn decode_text_label(path: &str, fields: &Fields) -> DResult<TextSource> {
    req_text_source(path, fields, "label", "label TextSource")
}

/// `SelectOption` — a bare JSON string is the option whose value and label are
/// that string.
pub(super) fn select_option_shorthand(_path: &str, j: &JVal) -> Option<DResult<SelectOption>> {
    match j {
        JVal::Str(s) => Some(Ok(SelectOption {
            value: s.clone(),
            label: TextSource::Literal(s.clone()),
        })),
        _ => None,
    }
}

/// `Format` — `Date` is the foreign spelling of `DateTime`.
pub(super) fn format_date_alias(path: &str, j: &JVal) -> Option<DResult<Format>> {
    match j {
        JVal::Obj(fields) if matches!(get(fields, "$type"), Some(JVal::Str(t)) if t == "Date") => {
            Some(decode_format_date_time(path, fields))
        }
        _ => None,
    }
}

/// `CellFormat` — `Date` is the foreign spelling of `DateTime`.
pub(super) fn cell_format_date_alias(path: &str, j: &JVal) -> Option<DResult<CellFormat>> {
    match j {
        JVal::Obj(fields) if matches!(get(fields, "$type"), Some(JVal::Str(t)) if t == "Date") => {
            Some(decode_cell_format_date_time(path, fields))
        }
        _ => None,
    }
}

/// `CellKindErased` — a `Pill` carrying a tone map is the declarative pill:
/// "pill" is the WORD for the thing, so a tone rule arrives tagged `Pill` more
/// often than `TonedPill`, and the closure `Pill` can never carry a map.
pub(super) fn pill_with_tone_map(path: &str, j: &JVal) -> Option<DResult<CellKindErased>> {
    match j {
        JVal::Obj(fields)
            if matches!(get(fields, "$type"), Some(JVal::Str(t)) if t == "Pill")
                && TONE_MAP_KEYS.iter().any(|k| get(fields, k).is_some()) =>
        {
            Some(decode_cell_kind_erased_toned_pill(path, fields))
        }
        _ => None,
    }
}

/// `CellKindErased.TonedPill.map` — the lookup table, under its canonical key or
/// a foreign one, each entry a tone.
pub(super) fn decode_toned_pill_map(
    path: &str,
    fields: &Fields,
) -> DResult<BTreeMap<String, ToneVariant>> {
    let map_j = req_aliased(
        path,
        fields,
        TONE_MAP_KEYS[0],
        &TONE_MAP_KEYS[1..],
        "TonedPill value→ToneVariant map",
    )?;
    decode_tone_map(&format!("{path}.map"), map_j)
}

/// `CellKindErased.ButtonGroup.buttons` — each button object contributes its
/// label; the click handler is the sentinel.
pub(super) fn decode_button_group(path: &str, fields: &Fields) -> DResult<Vec<TextSource>> {
    field_req(path, fields, "buttons", &[], "button group list", |p, j| {
        dec_list(p, j, |bp, item| {
            let bf = as_obj(bp, item)?;
            req_text_source(bp, bf, "label", "button label TextSource")
        })
    })
}

/// `BoxLayout` — a `Grid` naming neither a column count nor a template is `Auto`.
pub(super) fn grid_without_columns(path: &str, j: &JVal) -> Option<DResult<BoxLayout>> {
    match j {
        JVal::Obj(fields)
            if matches!(get(fields, "$type"), Some(JVal::Str(t)) if t == "Grid")
                && get_aliased(fields, "cols", &["columns"]).is_none() =>
        {
            match opt_string(path, fields, "templateColumns") {
                Ok(None) => Some(Ok(BoxLayout::Auto)),
                Ok(Some(_)) => None,
                Err(e) => Some(Err(e)),
            }
        }
        _ => None,
    }
}

/// `BoxLayout.Grid.cols` — absent beside a template is one column.
pub(super) fn decode_grid_cols(path: &str, fields: &Fields) -> DResult<i64> {
    field_or(path, fields, "cols", &["columns"], 1, as_int)
}

/// `BoxLayout.Masonry.cols` — a positive column count.
pub(super) fn check_masonry(path: &str, _fields: &Fields, layout: &BoxLayout) -> DResult<()> {
    match layout {
        BoxLayout::Masonry { cols, .. } if *cols <= 0 => Err(wrong_type(
            &format!("{path}.cols"),
            "JSON number (positive integer column count)",
        )),
        _ => Ok(()),
    }
}

/// A `Shape` case's and `Drawing`'s `style` — absent is the default style.
pub(super) fn decode_style_or_default(path: &str, fields: &Fields) -> DResult<DrawStyle> {
    field_or(
        path,
        fields,
        "style",
        &[],
        DrawStyle::default(),
        decode_draw_style,
    )
}

/// `TextSource.Bound.binding` — read untyped: the bound value is rendered as
/// text whatever it carries.
pub(super) fn decode_text_bound(path: &str, fields: &Fields) -> DResult<Box<Binding>> {
    field_req(
        path,
        fields,
        "binding",
        &[],
        "Binding<string> object",
        decode_binding,
    )
    .map(Box::new)
}

/// `TextSource.I18n.args` — absent is the empty bag; each argument is
/// discriminated by inspection (`decode_i18n_args`).
pub(super) fn decode_text_i18n_args(
    path: &str,
    fields: &Fields,
) -> DResult<Vec<(String, I18nArg)>> {
    field_or(path, fields, "args", &[], Vec::new(), decode_i18n_args)
}

/// `Fact.emphasis` — the behavioural bool, which a model also spells with the
/// `Emphasis` style tokens.
pub(super) fn decode_fact_emphasis(path: &str, fields: &Fields) -> DResult<bool> {
    field_or(
        path,
        fields,
        "emphasis",
        &[],
        defaults::FACT_EMPHASIS,
        decode_emphasis_flag,
    )
}

/// `LabelValueRow.emphasis` — as `Fact.emphasis`.
pub(super) fn decode_row_emphasis(path: &str, fields: &Fields) -> DResult<bool> {
    field_or(
        path,
        fields,
        "emphasis",
        &[],
        defaults::LABEL_VALUE_ROW_EMPHASIS,
        decode_emphasis_flag,
    )
}

/// `SrcSetEntry.width` — Phase 1080: a POSITIVE integer pixel width. A `0w`
/// candidate is not a small image but one a client can never select.
pub(super) fn decode_src_set_width(path: &str, fields: &Fields) -> DResult<i64> {
    let width_j = req(path, fields, "width", "positive intrinsic pixel width")?;
    match width_j {
        JVal::Num(n) if *n > 0.0 && n.fract() == 0.0 => Ok(*n as i64),
        _ => Err(wrong_type(
            &format!("{path}.width"),
            "JSON number (positive integer pixel width)",
        )),
    }
}

/// `Skeleton.rows` — the §21.9 bound.
pub(super) fn check_skeleton(path: &str, _fields: &Fields, spec: &SkeletonSpec) -> DResult<()> {
    if spec.rows > crate::limits::MAX_SKELETON_ROWS {
        return Err(make_error(
            DecodeErrorCode::LimitExceeded,
            format!("{path}.rows"),
            format!(
                "skeleton rows {} exceeds the maximum of {} (WIRE_FORMAT 21.9)",
                spec.rows,
                crate::limits::MAX_SKELETON_ROWS
            ),
            Some(format!(
                "at most {} rows on one Skeleton",
                crate::limits::MAX_SKELETON_ROWS
            )),
        ));
    }
    Ok(())
}

/// `FieldRule` — a rule must constrain something, and its length bounds must
/// not be inverted.
pub(super) fn check_field_rule(path: &str, _fields: &Fields, rule: &FieldRule) -> DResult<()> {
    if rule.format.is_none()
        && rule.pattern.is_none()
        && rule.min_length.is_none()
        && rule.max_length.is_none()
        && rule.compare.is_none()
    {
        return Err(make_error(
            DecodeErrorCode::WrongType,
            path.to_string(),
            "a rule that constrains nothing is a defect, not a no-op — declare at least one of \
             format / pattern / minLength / maxLength / compare, or omit 'rule' entirely"
                .to_string(),
            Some("FieldRule with at least one constraint slot".to_string()),
        ));
    }
    if let (Some(lo), Some(hi)) = (rule.min_length, rule.max_length)
        && lo > hi
    {
        return Err(make_error(
            DecodeErrorCode::WrongType,
            path.to_string(),
            format!(
                "minLength {lo} is above maxLength {hi} — an inverted length bound admits no value \
                 at all, so the field could never be submitted"
            ),
            Some("minLength <= maxLength".to_string()),
        ));
    }
    Ok(())
}

/// `FormField` — the near-miss refusals, before any field is read.
pub(super) fn check_form_field_near_misses(path: &str, fields: &Fields) -> DResult<()> {
    check_near_misses_in(
        path,
        fields,
        FORM_FIELD_NEAR_MISSES,
        "form",
        FORM_FIELD_NEAR_MISS_CONSEQUENCE,
    )
}

/// `FormField.kind` — an absent control value auto-binds to the field id.
pub(super) fn decode_form_field_control(path: &str, fields: &Fields) -> DResult<FormFieldKind> {
    let id = req_string_aliased(path, fields, "id", &["name"], "form field id string")?;
    let kind_j = req(path, fields, "kind", "FormFieldKind")?;
    decode_form_field_kind(
        ControlAutoBind::FormFieldId(&id),
        &format!("{path}.kind"),
        kind_j,
    )
}

/// `FilterSpec.kind` — an absent control value auto-binds to the chip name.
pub(super) fn decode_filter_control(path: &str, fields: &Fields) -> DResult<FormFieldKind> {
    let name = req_string(path, fields, "name", "filter name string")?;
    let kind_j = req(path, fields, "kind", "FormFieldKind control")?;
    decode_form_field_kind(
        ControlAutoBind::FilterChip(&name),
        &format!("{path}.kind"),
        kind_j,
    )
}

/// The `Filters` kind — its variant holds the `items` list directly.
pub(super) fn decode_filters_kind(path: &str, j: &JVal) -> DResult<Vec<FilterSpec>> {
    let fields = as_obj(path, j)?;
    field_req(path, fields, "items", &[], "Filters items list", |p, j| {
        dec_list(p, j, decode_filter_spec)
    })
}

/// `FileUpload` — a named destination is non-empty, and each ceiling positive.
pub(super) fn check_file_upload(
    path: &str,
    _fields: &Fields,
    spec: &FileUploadSpec,
) -> DResult<()> {
    if spec.destination.as_deref() == Some("") {
        return Err(wrong_type(
            &format!("{path}.destination"),
            "a registered destination name — an absent member is already the spelling for \
             an upload that streams nowhere",
        ));
    }
    for (key, n) in [("maxBytes", spec.max_bytes), ("maxFiles", spec.max_files)] {
        if matches!(n, Some(v) if v <= 0) {
            return Err(wrong_type(
                &format!("{path}.{key}"),
                "a positive integer ceiling — an absent member is already the spelling for \
                 an upload that declares no ceiling",
            ));
        }
    }
    Ok(())
}

/// `ColumnErased` — the grid vocabulary's near misses, before any field is read.
pub(super) fn check_column_near_misses(path: &str, fields: &Fields) -> DResult<()> {
    check_near_misses(path, fields, COLUMN_NEAR_MISSES)
}

/// `DefaultSort.column` — a non-negative header index.
pub(super) fn check_default_sort(path: &str, _fields: &Fields, ds: &DefaultSort) -> DResult<()> {
    if ds.column < 0 {
        return Err(wrong_type(
            &format!("{path}.column"),
            "JSON number (non-negative integer header index)",
        ));
    }
    Ok(())
}

/// `DataGrid` — the grid-behaviour near misses, before any field is read.
pub(super) fn check_grid_near_misses(path: &str, fields: &Fields) -> DResult<()> {
    check_near_misses(path, fields, GRID_NEAR_MISSES)
}

/// `DataGrid.pageSize` — a page holds at least one row.
pub(super) fn check_grid(path: &str, _fields: &Fields, spec: &GridSpec) -> DResult<()> {
    if matches!(spec.page_size, Some(n) if n < 1) {
        return Err(wrong_type(
            &format!("{path}.pageSize"),
            "JSON number (integer page size of 1 or more)",
        ));
    }
    Ok(())
}

/// `ChartAnnotationX.Date` — the date is the address the marker is drawn at.
pub(super) fn check_annotation_date(
    path: &str,
    _fields: &Fields,
    x: &ChartAnnotationX,
) -> DResult<()> {
    match x {
        ChartAnnotationX::Date(iso) if !is_canonical_iso_day(iso) => Err(wrong_type(
            &format!("{path}.iso"),
            "a canonical ISO-8601 date (YYYY-MM-DD, optionally followed by a time) naming a real calendar day — an event marker's date is the address it is drawn at, and an unreadable one would place the marker at 1970-01-01 and drag the axis back with it",
        )),
        _ => Ok(()),
    }
}

const BACKWARDS_BAND: &str = "an ORDERED pair — a range band runs from its lower value to its upper one, and this pair runs backwards; swapping the ends silently would draw a band the author did not describe";

/// `ChartAnnotationRange.ValueRange` — finite ends, in order.
pub(super) fn check_value_range(
    path: &str,
    _fields: &Fields,
    r: &ChartAnnotationRange,
) -> DResult<()> {
    if let ChartAnnotationRange::ValueRange { from, to } = r {
        for (slot, v) in [("from", *from), ("to", *to)] {
            if !v.is_finite() {
                return Err(wrong_type(
                    &format!("{path}.{slot}"),
                    "a FINITE JSON number — a range band's end names a place on the value axis, and NaN / Infinity names none; give the value in the axis's own units, or drop the annotation",
                ));
            }
        }
        if from > to {
            return Err(wrong_type(path, BACKWARDS_BAND));
        }
    }
    Ok(())
}

/// `ChartAnnotationRange.XRange` — two dates run forwards.
pub(super) fn check_x_range(path: &str, _fields: &Fields, r: &ChartAnnotationRange) -> DResult<()> {
    if let ChartAnnotationRange::XRange {
        from: ChartAnnotationX::Date(a),
        to: ChartAnnotationX::Date(b),
    } = r
        && a > b
    {
        return Err(wrong_type(
            path,
            "an ORDERED pair — a range band runs from its earlier date to its later one, and this pair runs backwards; swapping the ends silently would draw a band the author did not describe",
        ));
    }
    Ok(())
}

/// `ChartAnnotation.ReferenceLine.value` — a finite place on the value axis.
pub(super) fn check_reference_line(
    path: &str,
    _fields: &Fields,
    a: &ChartAnnotation,
) -> DResult<()> {
    match a {
        ChartAnnotation::ReferenceLine { value, .. } if !value.is_finite() => Err(wrong_type(
            &format!("{path}.value"),
            "a FINITE JSON number — a reference line names a place on the value axis, and NaN / Infinity names none; give the value in the axis's own units, or drop the annotation",
        )),
        _ => Ok(()),
    }
}

/// `StyleDelta` — a patch naming nothing is a defect.
pub(super) fn check_style_delta(path: &str, _fields: &Fields, delta: &StyleDelta) -> DResult<()> {
    if delta.is_empty() {
        return Err(wrong_type(
            path,
            "StyleDelta naming at least one field (direction | emphasis | role | tone | voice | weight)",
        ));
    }
    Ok(())
}

/// `Accessibility` — the ARIA near misses, before any field is read.
pub(super) fn check_a11y_near_misses(path: &str, fields: &Fields) -> DResult<()> {
    check_near_misses_in(
        path,
        fields,
        A11Y_NEAR_MISSES,
        "accessibility",
        A11Y_NEAR_MISS_CONSEQUENCE,
    )
}

/// `Action.SetState` — exactly one of `value` (a literal) and `valueFrom` (a
/// binding read at dispatch).
pub(super) fn decode_set_state(path: &str, fields: &Fields) -> DResult<Action> {
    let key = req_string(path, fields, "key", "state key string")?;
    match (get(fields, "value"), get(fields, "valueFrom")) {
        (Some(_), Some(_)) => Err(make_error(
            DecodeErrorCode::WrongType,
            format!("{path}.valueFrom"),
            "SetState carries both 'value' and 'valueFrom' — exactly one is allowed: 'value' is a literal JSON value written verbatim; 'valueFrom' derives the written value from a Binding at dispatch time; remove one".to_string(),
            None,
        )),
        (None, None) => Err(make_error(
            DecodeErrorCode::MissingField,
            format!("{path}.value"),
            "missing required field 'value' — provide 'value' (a literal JSON value) or 'valueFrom' (a Binding evaluated at dispatch time)".to_string(),
            None,
        )),
        (Some(v), None) => Ok(Action::SetState {
            key,
            value: Some(decode_jval(&format!("{path}.value"), v)?),
            value_from: None,
        }),
        (None, Some(b)) => Ok(Action::SetState {
            key,
            value: None,
            value_from: Some(Box::new(decode_binding(&format!("{path}.valueFrom"), b)?)),
        }),
    }
}

/// `Action.Print` — the payload-free action: any member beside `$type` is refused.
pub(super) fn decode_print(path: &str, fields: &Fields) -> DResult<Action> {
    let mut extras: Vec<&str> = fields
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| *k != "$type")
        .collect();
    if !extras.is_empty() {
        extras.sort_unstable();
        return Err(wrong_type(
            &format!("{path}.{}", extras[0]),
            "no member beside $type — Print takes no payload",
        ));
    }
    Ok(Action::Print)
}

/// `Action.Confirm` — no confirmation may be nested inside another.
pub(super) fn check_confirm(path: &str, _fields: &Fields, a: &Action) -> DResult<()> {
    if let Action::Confirm {
        on_confirm,
        on_cancel,
        ..
    } = a
    {
        refuse_nested_confirm(on_confirm, &format!("{path}.onConfirm"))?;
        if let Some(cancel) = on_cancel {
            refuse_nested_confirm(cancel, &format!("{path}.onCancel"))?;
        }
    }
    Ok(())
}

/// `TreeOp.InsertChild` — the retired positional slot is refused by name.
pub(super) fn check_insert_child(path: &str, fields: &Fields) -> DResult<()> {
    retired_positional_field(path, fields, "position", "InsertChild")
}

/// `TreeOp.MoveNode` — the retired positional slot is refused by name.
pub(super) fn check_move_node(path: &str, fields: &Fields) -> DResult<()> {
    retired_positional_field(path, fields, "newPosition", "MoveNode")
}

/// `Node` — the §21 node-depth and total-node bounds, entered on the way DOWN;
/// the guard pops in `Drop`, so the count is right on every error path.
pub(super) fn enter_node(path: &str) -> DResult<crate::limits::NodeGuard> {
    crate::limits::NodeGuard::enter().map_err(|b| limit_error(path, b))
}

/// `TreeOp` — the op axis, counted separately and held to the same ceiling
/// (§21.5: `Batch` makes the op decoder self-recursive).
pub(super) fn enter_op(path: &str) -> DResult<crate::limits::OpGuard> {
    crate::limits::OpGuard::enter().map_err(|b| limit_error(path, b))
}

/// `TreeItem` — a tree item's own nesting bound.
pub(super) fn enter_tree_item(path: &str) -> DResult<crate::limits::TreeItemGuard> {
    crate::limits::TreeItemGuard::enter().map_err(|b| limit_error(path, b))
}

/// `DataGrid.source` — the rows feed decodes through the `Rows` payload slot
/// (fuaran#665): an authored feed is typed data, not the opaque sentinel.
pub(super) fn decode_grid_source(path: &str, fields: &Fields) -> DResult<Binding> {
    req_binding_slot_aliased(path, fields, "source", &["data", "rows"], StaticSlot::Rows)
}

/// `Chart.source` — the rows feed decodes through the `Rows` payload slot.
pub(super) fn decode_chart_source(path: &str, fields: &Fields) -> DResult<Binding> {
    req_binding_slot_aliased(path, fields, "source", &["data"], StaticSlot::Rows)
}

/// `Select.value` — the selection is an optional string: "no selection" is a
/// value, so the payload decodes through `StringOpt`. Phase 1962 — the `value`
/// rule (WIRE_FORMAT.md §3.2), a relation between two sibling keys the IDL's
/// per-field optionality cannot state: a single-select REQUIRES `value`; a
/// multi-select carries `values` and no `value`. A `value` on a multi-select is
/// decoded as any binding is (so a malformed one still refuses) and then
/// dropped — a §16 lenient accept that normalises it to absent, since the
/// control never reads it. A malformed `multiple` is its own defect, so the
/// presence rule is not applied on top of it (`value` decodes if present, is
/// not demanded).
pub(super) fn decode_select_value(path: &str, fields: &Fields) -> DResult<Option<Binding>> {
    match get(fields, "multiple") {
        Some(JVal::Bool(true)) => {
            opt_binding_slot(path, fields, "value", StaticSlot::StringOpt)?;
            Ok(None)
        }
        None | Some(JVal::Bool(false)) => Ok(Some(req_binding_slot_aliased(
            path,
            fields,
            "value",
            &[],
            StaticSlot::StringOpt,
        )?)),
        Some(_) => opt_binding_slot(path, fields, "value", StaticSlot::StringOpt),
    }
}

fn req_binding_slot_aliased(
    path: &str,
    fields: &Fields,
    key: &str,
    aliases: &[&str],
    slot: StaticSlot,
) -> DResult<Binding> {
    field_req(path, fields, key, aliases, "Binding object", |p, j| {
        decode_binding_slot(p, j, slot)
    })
}

/// `Node.id` — a node id names something, so an empty one is `EMPTY_NODE_ID`.
pub(super) fn decode_node_id(path: &str, fields: &Fields) -> DResult<NodeId> {
    let id = field_req(path, fields, "id", &[], "Node id string", dec_string)?;
    if id.is_empty() {
        return Err(make_error(
            DecodeErrorCode::EmptyNodeId,
            format!("{path}.id"),
            "Node id is empty",
            Some("non-empty string".to_string()),
        ));
    }
    Ok(id)
}

/// `Node.state` — absent is the empty behaviour.
pub(super) fn decode_node_state(path: &str, fields: &Fields) -> DResult<StateBehaviour> {
    field_or(
        path,
        fields,
        "state",
        &[],
        StateBehaviour::default(),
        decode_state_behaviour,
    )
}

/// `Node.style` — absent is the default style.
pub(super) fn decode_node_style(path: &str, fields: &Fields) -> DResult<SemanticStyle> {
    field_or(
        path,
        fields,
        "style",
        &[],
        SemanticStyle::default(),
        decode_semantic_style,
    )
}

// ─── Public surface ──────────────────────────────────────────────────────────

pub(super) fn invalid_json(parse_message: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::InvalidJson,
        "$",
        format!("input is not valid JSON: {parse_message}"),
        Some("well-formed JSON object per the canonical-JSON shape".to_string()),
    )
}

/// Turn a parse failure into a decode error, honouring §21.2 rule 2: a resource
/// limit breach is `LIMIT_EXCEEDED`, never `INVALID_JSON`. Only the parser knows
/// which of the two happened, so it flags the distinction rather than leaving it
/// to be re-derived from a message string.
pub(super) fn parse_failure(e: &crate::canonical::ParseError) -> DecodeError {
    if e.limit {
        make_error(
            DecodeErrorCode::LimitExceeded,
            "$".to_string(),
            e.message.clone(),
            Some(format!(
                "a document nesting no more than {} levels deep",
                crate::limits::MAX_JSON_DEPTH
            )),
        )
    } else {
        invalid_json(&e.message)
    }
}

/// A §21 walk-bound refusal at `path`.
pub(super) fn limit_error(path: &str, breach: crate::limits::LimitBreach) -> DecodeError {
    make_error(
        DecodeErrorCode::LimitExceeded,
        path.to_string(),
        breach.message(),
        Some(breach.expected()),
    )
}

/// The §21.7 total-document ceiling, checked BEFORE parsing.
///
/// One comparison on the input's length. Deferring it would allocate the
/// document twice for no benefit, and it is the only §21 limit that bounds the
/// document's TOTAL rather than the shape of the walk — the five structural
/// bounds compose multiplicatively and admit a hundred-gigabyte document that
/// satisfies every one of them individually.
///
/// `&str` is UTF-8 by construction, so `len()` IS the measured unit and there
/// is nothing to convert. The path is `$`: the breach is a property of the
/// document, not of a position in it.
pub(super) fn document_bytes_error(json: &str) -> Option<DecodeError> {
    if json.len() <= crate::limits::MAX_DOCUMENT_BYTES {
        return None;
    }
    Some(make_error(
        DecodeErrorCode::LimitExceeded,
        "$".to_string(),
        format!(
            "document is {} UTF-8 bytes, over the {}-byte ceiling",
            json.len(),
            crate::limits::MAX_DOCUMENT_BYTES
        ),
        Some(format!(
            "a document of at most {} UTF-8 bytes",
            crate::limits::MAX_DOCUMENT_BYTES
        )),
    ))
}

/// Decode a canonical-JSON `Node` payload into the storage-shape typed tree.
pub fn decode_node(json: &str) -> Result<Node, DecodeError> {
    if let Some(e) = document_bytes_error(json) {
        return Err(e);
    }
    match parse(json) {
        Ok(ast) => {
            crate::limits::reset_walk();
            decode_node_ast("$", &ast)
        }
        Err(e) => Err(parse_failure(&e)),
    }
}

/// Decode a canonical-JSON `TreeOp` payload into the storage-shape typed op.
pub fn decode_op(json: &str) -> Result<TreeOp, DecodeError> {
    if let Some(e) = document_bytes_error(json) {
        return Err(e);
    }
    match parse(json) {
        Ok(ast) => {
            crate::limits::reset_walk();
            decode_tree_op_ast("$", &ast)
        }
        Err(e) => Err(parse_failure(&e)),
    }
}

// ─── Coercion bridge (apply-engine UpdateProp) ───────────────────────────────
//
// `TreeOp.UpdateProp` carries a structured `JVal` payload; the apply engine
// pours it into a typed spec field. These helpers run the matching per-type
// decoder over the payload; failures surface a plain message string the apply
// engine reframes into a `KindMismatch` ApplyError. Mirrors the reference
// hosts' coercion bridge.

pub(crate) mod coerce {
    use super::*;

    type C<T> = Result<T, String>;

    fn via<T>(v: &JVal, dec: impl Fn(&str, &JVal) -> DResult<T>) -> C<T> {
        dec("$value", v).map_err(|e| e.message)
    }

    pub fn int(v: &JVal) -> C<i64> {
        match v {
            JVal::Num(n) => Ok(n.trunc() as i64),
            _ => Err("expected a JSON number (integer)".to_string()),
        }
    }

    pub fn float(v: &JVal) -> C<f64> {
        match v {
            JVal::Num(n) => Ok(*n),
            _ => Err("expected a JSON number".to_string()),
        }
    }

    pub fn boolean(v: &JVal) -> C<bool> {
        match v {
            JVal::Bool(b) => Ok(*b),
            _ => Err("expected a JSON boolean".to_string()),
        }
    }

    pub fn string(v: &JVal) -> C<String> {
        match v {
            JVal::Str(s) => Ok(s.clone()),
            _ => Err("expected a JSON string".to_string()),
        }
    }

    pub fn text_source(v: &JVal) -> C<TextSource> {
        via(v, decode_text_source)
    }

    pub fn binding(v: &JVal) -> C<Binding> {
        via(v, decode_binding)
    }

    pub fn cell_format(v: &JVal) -> C<CellFormat> {
        via(v, decode_cell_format)
    }

    pub fn column_width(v: &JVal) -> C<ColumnWidth> {
        via(v, decode_column_width)
    }

    pub fn orientation(v: &JVal) -> C<Orientation> {
        via(v, decode_orientation)
    }

    pub fn tone(v: &JVal) -> C<ToneVariant> {
        via(v, decode_tone_variant)
    }

    pub fn weight(v: &JVal) -> C<StyleWeight> {
        via(v, decode_style_weight)
    }

    pub fn emphasis(v: &JVal) -> C<Emphasis> {
        via(v, decode_emphasis)
    }

    /// Phase 867 — `Metric.trendPolarity` for `UpdateProp`. Routed through the
    /// decode function, so an op naming the RESERVED `Neutral` is refused at
    /// apply time exactly as it is at decode: a tree the codec would not accept
    /// must not be reachable by mutating one it did.
    pub fn trend_polarity(v: &JVal) -> C<TrendPolarity> {
        via(v, decode_trend_polarity)
    }

    /// The behavioural `emphasis` BOOL (Fact / LabelValueRow) — cross-vocab
    /// coerced exactly as at decode.
    pub fn emphasis_flag(v: &JVal) -> C<bool> {
        via(v, decode_emphasis_flag)
    }

    pub fn heading_variant(v: &JVal) -> C<HeadingVariant> {
        via(v, decode_heading_variant)
    }

    pub fn badge_variant(v: &JVal) -> C<BadgeVariant> {
        via(v, decode_badge_variant)
    }

    /// An icon rides the wire as its raw string name.
    pub fn icon_source(v: &JVal) -> C<String> {
        string(v)
    }

    /// Phase 821 — the `Icon` display kind's size modifier.
    pub fn icon_size(v: &JVal) -> C<IconSize> {
        via(v, decode_icon_size)
    }
}
