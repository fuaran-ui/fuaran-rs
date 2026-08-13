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

use std::collections::BTreeMap;

use crate::canonical::{JVal, parse};

use super::model::*;
use super::result::{DecodeError, DecodeErrorCode};

type DResult<T> = Result<T, DecodeError>;

const OPAQUE: &str = "<opaque>";

// ─── Error constructors ──────────────────────────────────────────────────────

fn make_error(
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

fn missing_field(path: &str, key: &str, expected: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::MissingField,
        format!("{path}.{key}"),
        format!("missing required field '{key}'"),
        Some(expected.to_string()),
    )
}

fn wrong_type(path: &str, expected: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::WrongType,
        path,
        format!("expected {expected}"),
        Some(expected.to_string()),
    )
}

fn unknown_du_case(path: &str, got: &str, expected: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::UnknownDuCase,
        format!("{path}.$type"),
        format!("unknown discriminator '{got}'"),
        Some(expected.to_string()),
    )
}

fn null_not_representable(path: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::WrongType,
        path,
        "null is not representable in the Fuaran wire model — omit the field instead",
        None,
    )
}

// ─── AST require-helpers ─────────────────────────────────────────────────────

type Fields = [(String, JVal)];

/// Lenient AI-ingest (§3.6, generalised): a `Static` envelope wrapped around a
/// PLAIN scalar unwraps before the scalar readers — the inverse of the
/// bare-scalar-in-Binding-slot confusion, applied at every plain-scalar
/// position in one place. Unambiguous: at a plain-scalar position the envelope
/// has exactly one reading. Objects that are NOT a well-formed Static envelope
/// pass through untouched and fail with the normal error.
fn unwrap_static_envelope(j: &JVal) -> &JVal {
    if let JVal::Obj(fields) = j
        && let Some(JVal::Str(t)) = get(fields, "$type")
        && t == "Static"
        && let Some(inner) = get(fields, "value")
    {
        return inner;
    }
    j
}

fn as_obj<'a>(path: &str, j: &'a JVal) -> DResult<&'a Fields> {
    match j {
        JVal::Obj(fields) => Ok(fields),
        _ => Err(wrong_type(path, "JSON object")),
    }
}

fn as_str<'a>(path: &str, j: &'a JVal) -> DResult<&'a str> {
    match unwrap_static_envelope(j) {
        JVal::Str(s) => Ok(s),
        _ => Err(wrong_type(path, "JSON string")),
    }
}

fn as_bool(path: &str, j: &JVal) -> DResult<bool> {
    match unwrap_static_envelope(j) {
        JVal::Bool(b) => Ok(*b),
        _ => Err(wrong_type(path, "JSON boolean")),
    }
}

fn as_float(path: &str, j: &JVal) -> DResult<f64> {
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

fn as_int(path: &str, j: &JVal) -> DResult<i64> {
    match unwrap_static_envelope(j) {
        JVal::Num(n) => Ok(n.trunc() as i64),
        _ => Err(wrong_type(path, "JSON number (integer)")),
    }
}

fn as_arr<'a>(path: &str, j: &'a JVal) -> DResult<&'a [JVal]> {
    match j {
        JVal::Arr(items) => Ok(items),
        _ => Err(wrong_type(path, "JSON array")),
    }
}

fn get<'a>(fields: &'a Fields, key: &str) -> Option<&'a JVal> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn req<'a>(path: &str, fields: &'a Fields, key: &str, expected: &str) -> DResult<&'a JVal> {
    get(fields, key).ok_or_else(|| missing_field(path, key, expected))
}

fn disc<'a>(path: &str, fields: &'a Fields) -> DResult<&'a str> {
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

fn req_string(path: &str, fields: &Fields, key: &str, expected: &str) -> DResult<String> {
    let v = req(path, fields, key, expected)?;
    Ok(as_str(&format!("{path}.{key}"), v)?.to_string())
}

fn req_bool(path: &str, fields: &Fields, key: &str, expected: &str) -> DResult<bool> {
    let v = req(path, fields, key, expected)?;
    as_bool(&format!("{path}.{key}"), v)
}

fn req_float(path: &str, fields: &Fields, key: &str, expected: &str) -> DResult<f64> {
    let v = req(path, fields, key, expected)?;
    as_float(&format!("{path}.{key}"), v)
}

fn req_int(path: &str, fields: &Fields, key: &str, expected: &str) -> DResult<i64> {
    let v = req(path, fields, key, expected)?;
    as_int(&format!("{path}.{key}"), v)
}

fn opt_string(path: &str, fields: &Fields, key: &str) -> DResult<Option<String>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(as_str(&format!("{path}.{key}"), v)?.to_string())),
    }
}

fn opt_int(path: &str, fields: &Fields, key: &str) -> DResult<Option<i64>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(as_int(&format!("{path}.{key}"), v)?)),
    }
}

fn opt_float(path: &str, fields: &Fields, key: &str) -> DResult<Option<f64>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(as_float(&format!("{path}.{key}"), v)?)),
    }
}

fn opt_bool(path: &str, fields: &Fields, key: &str) -> DResult<Option<bool>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(as_bool(&format!("{path}.{key}"), v)?)),
    }
}

/// A closure sentinel slot: presence maps to `Some(Closure)`, absence to `None`
/// (Phases 423/426 — an omitted handler arms the renderer's write-back default).
fn opt_closure(fields: &Fields, key: &str) -> Option<Closure> {
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
fn get_aliased<'a>(fields: &'a Fields, canonical: &str, aliases: &[&str]) -> Option<&'a JVal> {
    get(fields, canonical).or_else(|| aliases.iter().find_map(|a| get(fields, a)))
}

/// Require a canonical field or one of its aliases; the nested path always uses
/// the canonical name (matching the F# reference).
fn req_aliased<'a>(
    path: &str,
    fields: &'a Fields,
    canonical: &str,
    aliases: &[&str],
    expected: &str,
) -> DResult<&'a JVal> {
    get_aliased(fields, canonical, aliases).ok_or_else(|| missing_field(path, canonical, expected))
}

fn req_string_aliased(
    path: &str,
    fields: &Fields,
    canonical: &str,
    aliases: &[&str],
    expected: &str,
) -> DResult<String> {
    let v = req_aliased(path, fields, canonical, aliases, expected)?;
    Ok(as_str(&format!("{path}.{canonical}"), v)?.to_string())
}

fn req_binding_aliased(
    path: &str,
    fields: &Fields,
    canonical: &str,
    aliases: &[&str],
    expected: &str,
) -> DResult<Binding> {
    let v = req_aliased(path, fields, canonical, aliases, expected)?;
    decode_binding(&format!("{path}.{canonical}"), v)
}

fn req_binding_slot_aliased(
    path: &str,
    fields: &Fields,
    canonical: &str,
    aliases: &[&str],
    expected: &str,
    slot: StaticSlot,
) -> DResult<Binding> {
    let v = req_aliased(path, fields, canonical, aliases, expected)?;
    decode_binding_slot(&format!("{path}.{canonical}"), v, slot)
}

fn req_text_source_aliased(
    path: &str,
    fields: &Fields,
    canonical: &str,
    aliases: &[&str],
    expected: &str,
) -> DResult<TextSource> {
    let v = req_aliased(path, fields, canonical, aliases, expected)?;
    decode_text_source(&format!("{path}.{canonical}"), v)
}

fn opt_text_source_aliased(
    path: &str,
    fields: &Fields,
    canonical: &str,
    aliases: &[&str],
) -> DResult<Option<TextSource>> {
    match get_aliased(fields, canonical, aliases) {
        None => Ok(None),
        Some(v) => Ok(Some(decode_text_source(&format!("{path}.{canonical}"), v)?)),
    }
}

// ─── Phase 460 — stylistic fields omitted-when-default on decode (§3.6) ───────
//
// These stylistic slots restore an identity default when ABSENT; a present
// explicit-default value keeps decoding (read-compat). The canonical encoder
// still emits them (fixtures byte-unchanged), so a re-encode is always full.

fn opt_cell_format_default(path: &str, fields: &Fields, key: &str) -> DResult<CellFormat> {
    match get(fields, key) {
        None => Ok(CellFormat::None),
        Some(v) => decode_cell_format(&format!("{path}.{key}"), v),
    }
}

fn opt_tone_default(path: &str, fields: &Fields, key: &str) -> DResult<ToneVariant> {
    match get(fields, key) {
        None => Ok(ToneVariant::Default),
        Some(v) => decode_tone(&format!("{path}.{key}"), v),
    }
}

fn opt_weight_default(path: &str, fields: &Fields, key: &str) -> DResult<StyleWeight> {
    match get(fields, key) {
        None => Ok(StyleWeight::Standard),
        Some(v) => decode_weight(&format!("{path}.{key}"), v),
    }
}

fn opt_emphasis_default(path: &str, fields: &Fields, key: &str) -> DResult<Emphasis> {
    match get(fields, key) {
        None => Ok(Emphasis::Normal),
        Some(v) => decode_emphasis(&format!("{path}.{key}"), v),
    }
}

fn opt_column_width_default(path: &str, fields: &Fields, key: &str) -> DResult<ColumnWidth> {
    match get(fields, key) {
        None => Ok(ColumnWidth::Auto),
        Some(v) => decode_column_width(&format!("{path}.{key}"), v),
    }
}

// ─── Bare-string enum decode ─────────────────────────────────────────────────

macro_rules! decode_bare_enum {
    ($fn_name:ident, $ty:ident, $label:literal) => {
        fn $fn_name(path: &str, j: &JVal) -> DResult<$ty> {
            let s = match j {
                JVal::Str(s) => s,
                _ => return Err(wrong_type(path, concat!("JSON string (", $label, ")"))),
            };
            $ty::from_wire(s).ok_or_else(|| unknown_du_case(path, s, &$ty::WIRE_NAMES.join(" | ")))
        }
    };
    // Lenient-ingest alias arm (decode-only; WIRE_FORMAT.md §3.6). Canonical wire
    // spellings decode via `from_wire`; a curated set of same-concept synonyms
    // maps to a canonical case. A re-encode always normalises to the canonical
    // name (aliases never appear in the schema or the conformance corpus). Any
    // other unknown case still fails `UNKNOWN_DU_CASE` with the canonical list.
    ($fn_name:ident, $ty:ident, $label:literal, aliases: { $($alias:literal => $case:ident),+ $(,)? }) => {
        fn $fn_name(path: &str, j: &JVal) -> DResult<$ty> {
            let s = match j {
                JVal::Str(s) => s,
                _ => return Err(wrong_type(path, concat!("JSON string (", $label, ")"))),
            };
            if let Some(v) = $ty::from_wire(s) {
                return Ok(v);
            }
            match s.as_str() {
                $($alias => Ok($ty::$case),)+
                _ => Err(unknown_du_case(path, s, &$ty::WIRE_NAMES.join(" | "))),
            }
        }
    };
}

// The CSS flex-direction prior: a row lays out horizontally, a column vertically.
decode_bare_enum!(decode_orientation, Orientation, "Orientation", aliases: {
    "Row" => Horizontal, "row" => Horizontal,
    "Column" => Vertical, "column" => Vertical,
});
decode_bare_enum!(
    decode_scroll_orientation,
    ScrollOrientation,
    "ScrollOrientation"
);
// `Default` is the universal no-special-variant prior (BadgeVariant's identity
// case is Neutral); `Danger` is the Bootstrap prior for Critical.
decode_bare_enum!(decode_badge_variant, BadgeVariant, "BadgeVariant", aliases: {
    "Default" => Neutral, "Danger" => Critical,
});
// Bootstrap's `Danger` names the same concept as Destructive (the red button).
decode_bare_enum!(decode_button_variant, ButtonVariant, "ButtonVariant", aliases: {
    "Danger" => Destructive,
});
// `Default` → the identity case. Other guesses (Title/Page/Section) stay rejects.
decode_bare_enum!(decode_heading_variant, HeadingVariant, "HeadingVariant", aliases: {
    "Default" => Standard,
});
// Faithful semantic mappings only: Positive→Success, Danger/Negative→Critical,
// Neutral→Default.
decode_bare_enum!(decode_tone, ToneVariant, "ToneVariant", aliases: {
    "Positive" => Success, "Danger" => Critical, "Negative" => Critical, "Neutral" => Default,
});
// `StyleWeight` is deliberately NOT aliased — Bold/Heavy is font-weight intent,
// but the language means layout density (Compact|Standard|Spacious).
decode_bare_enum!(decode_weight, StyleWeight, "StyleWeight");
// Prominence intent survives: Strong/Bold→Loud, Subtle/Muted→Quiet. The
// `emphasis` name is a same-name cross-vocabulary collision (style ENUM here
// vs behavioural BOOL on Fact/LabelValueRow) and models cross it in both
// directions: a bool in the enum slot projects one-to-one (true ⇒ Loud,
// false ⇒ Normal). The bool sites' direction lives in `decode_emphasis_flag`.
fn decode_emphasis(path: &str, j: &JVal) -> DResult<Emphasis> {
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
        _ => Err(unknown_du_case(path, s, &Emphasis::WIRE_NAMES.join(" | "))),
    }
}

/// The behavioural `emphasis` BOOL (Fact / LabelValueRow) — the other half of
/// the same-name collision with the `Emphasis` style enum. Booleans pass
/// through; the enum AND its aliases project one-to-one (Loud/Strong/Bold ⇒
/// true, Normal/Quiet/Subtle/Muted ⇒ false); any other string is the didactic
/// reject naming both vocabularies.
fn decode_emphasis_flag(path: &str, j: &JVal) -> DResult<bool> {
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
decode_bare_enum!(decode_text_anchor, TextAnchor, "TextAnchor");
decode_bare_enum!(decode_style_role, StyleRole, "StyleRole");
decode_bare_enum!(decode_font_voice, FontVoice, "FontVoice");
decode_bare_enum!(decode_chart_kind, ChartKind, "ChartKind");
decode_bare_enum!(decode_image_variant, ImageVariant, "ImageVariant");
decode_bare_enum!(decode_link_protection, LinkProtection, "LinkProtection");
decode_bare_enum!(decode_math_display, MathDisplay, "MathDisplay");
decode_bare_enum!(decode_date_variant, DateVariant, "DateVariant");
decode_bare_enum!(
    decode_file_read_encoding,
    FileReadEncoding,
    "FileReadEncoding"
);
decode_bare_enum!(decode_live_region, LiveRegionKind, "LiveRegionKind");
decode_bare_enum!(decode_sort_direction, SortDirection, "SortDirection");
decode_bare_enum!(decode_date_style, DateStyle, "DateStyle");
decode_bare_enum!(
    decode_relative_time_unit,
    RelativeTimeUnit,
    "RelativeTimeUnit"
);
// Phase 819 — the Duration format enums (`decode_cell_format` /
// `decode_format` share them).
decode_bare_enum!(decode_duration_unit, DurationUnit, "DurationUnit");
decode_bare_enum!(decode_duration_style, DurationStyle, "DurationStyle");
// Phase 821 — the standalone icon-only display kind's size modifier.
decode_bare_enum!(decode_icon_size, IconSize, "IconSize");

// ─── Strict JVal decode (rule 12 — structured JSON positions) ────────────────

fn decode_jval(path: &str, j: &JVal) -> DResult<JVal> {
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

fn decode_jval_map(path: &str, j: &JVal) -> DResult<Vec<(String, JVal)>> {
    let fields = as_obj(path, j)?;
    let mut out = Vec::with_capacity(fields.len());
    for (k, v) in fields {
        out.push((k.clone(), decode_jval(&format!("{path}.{k}"), v)?));
    }
    Ok(out)
}

// ─── Compute layer (Core-style string errors) ────────────────────────────────

type CResult<T> = Result<T, String>;

fn ast_kind(j: &JVal) -> &'static str {
    match j {
        JVal::Str(_) => "string",
        JVal::Num(_) => "number",
        JVal::Bool(_) => "bool",
        JVal::Arr(_) => "array",
        JVal::Obj(_) => "object",
        JVal::Null => "null",
    }
}

fn c_obj(j: &JVal) -> CResult<&Fields> {
    match j {
        JVal::Obj(fields) => Ok(fields),
        _ => Err(format!("malformed: expected object, got {}", ast_kind(j))),
    }
}

fn c_field<'a>(fields: &'a Fields, key: &str) -> CResult<&'a JVal> {
    get(fields, key).ok_or_else(|| format!("missing field: {key}"))
}

fn c_str(j: &JVal) -> CResult<String> {
    match j {
        JVal::Str(s) => Ok(s.clone()),
        _ => Err(format!("malformed: expected string, got {}", ast_kind(j))),
    }
}

fn c_arr(j: &JVal) -> CResult<&[JVal]> {
    match j {
        JVal::Arr(items) => Ok(items),
        _ => Err(format!("malformed: expected array, got {}", ast_kind(j))),
    }
}

fn c_int(j: &JVal) -> CResult<i64> {
    match j {
        JVal::Num(n) => Ok(n.trunc() as i64),
        _ => Err(format!("malformed: expected int, got {}", ast_kind(j))),
    }
}

fn c_str_field(fields: &Fields, key: &str) -> CResult<String> {
    c_str(c_field(fields, key)?)
}

fn c_str_list(j: &JVal) -> CResult<Vec<String>> {
    c_arr(j)?.iter().map(c_str).collect()
}

fn c_enum<T: Copy>(value: &str, parse_wire: fn(&str) -> Option<T>, names: &[&str]) -> CResult<T> {
    parse_wire(value).ok_or_else(|| {
        format!(
            "unknown column type '{value}'; expected one of: {}",
            names.join(", ")
        )
    })
}

fn decode_cell_lit(j: &JVal) -> CResult<Cell> {
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
        _ => Err(mismatch()),
    }
}

fn decode_column_cell(col_name: &str, ty: ColumnType, v: &JVal) -> CResult<Cell> {
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
        (ColumnType::Timestamp, JVal::Str(s)) => Ok(Cell::Timestamp(s.clone())),
        // Lenient-ingest (Core Phase 94): a declared-timestamp column accepts
        // an epoch NUMBER — unit by magnitude (>= 1e11 => milliseconds; epoch
        // seconds stay below 1e11 until year 5138) — normalised to the
        // canonical ISO string on decode.
        (ColumnType::Timestamp, JVal::Num(n)) if *n == n.trunc() && n.abs() < 9e15 => {
            let i = *n as i64;
            let secs = if i.abs() >= 100_000_000_000 {
                i / 1000
            } else {
                i
            };
            Ok(Cell::Timestamp(iso_of_epoch_seconds(secs)))
        }
        _ => Err(mismatch()),
    }
}

/// Render an epoch-seconds instant as the canonical ISO-8601 UTC timestamp
/// string. Pure integer civil-from-days arithmetic — deterministic and
/// clock-free; negative epochs (pre-1970) are handled.
fn iso_of_epoch_seconds(secs: i64) -> String {
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

fn decode_col_schema(j: &JVal) -> CResult<Vec<SchemaEntry>> {
    c_arr(j)?
        .iter()
        .map(|e| {
            let fields = c_obj(e)?;
            let name = c_str_field(fields, "name")?;
            let ty = c_str_field(fields, "type")?;
            let column_type = c_enum(&ty, ColumnType::from_wire, ColumnType::WIRE_NAMES)?;
            Ok(SchemaEntry { name, column_type })
        })
        .collect()
}

/// Lenient-ingest (Core Phases 88 + 94) — the `(values, validity)` parts of a
/// column element: a BARE JSON array is the "just the data" shorthand (an
/// all-present mask is synthesised — the wire has no JSON null, so a bare
/// array can only mean every cell present); a wrapped object carrying `values`
/// but no `validity` is the same all-present statement. The full wrapped form
/// stays canonical.
fn column_parts<'a>(
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

fn decode_data_column(columns: &Fields, name: &str, ty: ColumnType) -> CResult<DataColumn> {
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
fn infer_column_type(name: &str, values: &[JVal]) -> CResult<ColumnType> {
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

fn decode_data_source(j: &JVal) -> CResult<DataSource> {
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
                out.push(SchemaEntry {
                    name: name.clone(),
                    column_type: infer_column_type(name, values)?,
                });
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

fn decode_col_expr(j: &JVal) -> CResult<ColExpr> {
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
                "unknown column type '{other}'; expected one of: col, lit, param, binary, not, coalesce, case, cast, apply, in, isNull"
            )),
        },
    }
}

/// One of the canonical field or its observed alias — Core Phase 92's
/// `fieldAliased` (both present is not distinguished here; canonical wins).
fn c_field_aliased<'a>(fields: &'a Fields, canonical: &str, alias: &str) -> CResult<&'a JVal> {
    get(fields, canonical)
        .or_else(|| get(fields, alias))
        .ok_or_else(|| format!("missing field: {canonical}"))
}

/// The `args` of an apply-shaped node: the canonical array, or a lone `expr`
/// field standing in for a one-element list (Core Phase 94).
fn expr_args(fields: &Fields) -> CResult<Vec<ColExpr>> {
    match get(fields, "args") {
        Some(args_j) => c_arr(args_j)?
            .iter()
            .map(decode_col_expr)
            .collect::<CResult<Vec<_>>>(),
        None => Ok(vec![decode_col_expr(c_field(fields, "expr")?)?]),
    }
}

fn decode_col_pair(j: &JVal) -> CResult<ColPair> {
    let fields = c_obj(j)?;
    Ok(ColPair {
        a: c_str_field(fields, "a")?,
        b: c_str_field(fields, "b")?,
    })
}

fn decode_agg(j: &JVal) -> CResult<Agg> {
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

fn decode_order(j: &JVal) -> CResult<SortKey> {
    let fields = c_obj(j)?;
    // Lenient-ingest (Core Phases 92/93) — `column` aliases `col`; direction
    // is one of `dir` (canonical asc|desc), boolean `descending`, or
    // `direction`; a directionless entry is the SQL default (asc). Only
    // "desc" sorts descending; anything else reads ascending.
    let col = c_str(c_field_aliased(fields, "col", "column")?)?;
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

fn decode_transform_step(j: &JVal) -> CResult<TransformStep> {
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
            let cols = c_arr(c_field(fields, "cols")?)?
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
        other => Err(format!(
            "unknown column type '{other}'; expected one of: filter, project, derive, groupBy, join, window, pivot, unpivot, sort, distinct, limit, union"
        )),
    }
}

fn decode_pipeline(j: &JVal) -> CResult<Vec<TransformStep>> {
    match j {
        JVal::Arr(items) => items.iter().map(decode_transform_step).collect(),
        _ => Err("malformed: pipeline: expected a JSON array of transform steps".to_string()),
    }
}

fn decode_invoke_args(path: &str, j: &JVal) -> DResult<Vec<InvokeArg>> {
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
enum StaticSlot {
    Untyped,
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
                                "date-range start '{from}' is after end '{to}' — a DateRange pair \
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

fn decode_select_option(path: &str, j: &JVal) -> DResult<SelectOption> {
    // Lenient AI-ingest shorthand (§5): a bare JSON string `"A"` reads as
    // `{label: "A", value: "A"}` (the HTML `<select>` prior); the canonical
    // re-encode is the object form.
    if let JVal::Str(s) = j {
        return Ok(SelectOption {
            value: s.clone(),
            label: TextSource::Literal(s.clone()),
        });
    }
    let fields = as_obj(path, j)?;
    let value = req_string(path, fields, "value", "option value string")?;
    let label_j = req(path, fields, "label", "option label TextSource")?;
    let label = decode_text_source(&format!("{path}.label"), label_j)?;
    Ok(SelectOption { value, label })
}

fn decode_map_marker(path: &str, j: &JVal) -> DResult<MapMarker> {
    let fields = as_obj(path, j)?;
    let label_j = req(path, fields, "label", "marker label TextSource")?;
    let label = decode_text_source(&format!("{path}.label"), label_j)?;
    let latitude = req_float(path, fields, "latitude", "marker latitude float")?;
    let longitude = req_float(path, fields, "longitude", "marker longitude float")?;
    Ok(MapMarker {
        label,
        latitude,
        longitude,
    })
}

// ─── LocalFlushTrigger / Format / LocaleSource / CellFormat / ColumnWidth ────

fn decode_local_flush_trigger(path: &str, j: &JVal) -> DResult<LocalFlushTrigger> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "OnBlur" => Ok(LocalFlushTrigger::OnBlur),
        "OnSubmit" => Ok(LocalFlushTrigger::OnSubmit),
        "OnCommitAction" => Ok(LocalFlushTrigger::OnCommitAction),
        "OnDebounce" => Ok(LocalFlushTrigger::OnDebounce {
            milliseconds: req_int(
                path,
                fields,
                "milliseconds",
                "debounce milliseconds integer",
            )?,
        }),
        other => Err(unknown_du_case(
            path,
            other,
            "OnBlur | OnSubmit | OnDebounce | OnCommitAction",
        )),
    }
}

fn decode_format(path: &str, j: &JVal) -> DResult<Format> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Number" => Ok(Format::Number {
            decimals: opt_int(path, fields, "decimals")?,
        }),
        "Currency" => Ok(Format::Currency {
            iso_code: req_string(path, fields, "isoCode", "ISO-4217 currency code string")?,
        }),
        "Percent" => Ok(Format::Percent {
            decimals: opt_int(path, fields, "decimals")?,
        }),
        "Date" => {
            let v = req(path, fields, "dateStyle", "DateStyle string")?;
            Ok(Format::Date {
                date_style: decode_date_style(&format!("{path}.dateStyle"), v)?,
            })
        }
        "RelativeTime" => {
            let v = req(path, fields, "unit", "RelativeTimeUnit string")?;
            Ok(Format::RelativeTime {
                unit: decode_relative_time_unit(&format!("{path}.unit"), v)?,
            })
        }
        "Duration" => {
            // Phase 819 — locale-independent duration formatting.
            let unit_j = req(path, fields, "unit", "DurationUnit string")?;
            let unit = decode_duration_unit(&format!("{path}.unit"), unit_j)?;
            let style_j = req(path, fields, "style", "DurationStyle string")?;
            let style = decode_duration_style(&format!("{path}.style"), style_j)?;
            Ok(Format::Duration { style, unit })
        }
        other => Err(unknown_du_case(
            path,
            other,
            "Number | Currency | Percent | Date | RelativeTime | Duration",
        )),
    }
}

fn decode_locale_source(path: &str, j: &JVal) -> DResult<LocaleSource> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Ambient" => Ok(LocaleSource::Ambient),
        "Explicit" => Ok(LocaleSource::Explicit {
            tag: req_string(path, fields, "tag", "BCP-47 locale tag string")?,
        }),
        other => Err(unknown_du_case(path, other, "Ambient | Explicit")),
    }
}

fn decode_cell_format(path: &str, j: &JVal) -> DResult<CellFormat> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "None" => Ok(CellFormat::None),
        "Number" => Ok(CellFormat::Number {
            decimals: opt_int(path, fields, "decimals")?,
        }),
        "Currency" => Ok(CellFormat::Currency {
            code: req_string(path, fields, "code", "ISO currency code string")?,
        }),
        "Percent" => Ok(CellFormat::Percent {
            decimals: opt_int(path, fields, "decimals")?,
        }),
        "SignificantDigits" => Ok(CellFormat::SignificantDigits {
            digits: req_int(path, fields, "digits", "integer digit count")?,
        }),
        "Date" => Ok(CellFormat::Date {
            format: req_string(path, fields, "format", "format string")?,
        }),
        "Duration" => {
            // Phase 819 — trendable duration cells: raw float counts `unit`s,
            // rendered per `style`.
            let unit_j = req(path, fields, "unit", "DurationUnit string")?;
            let unit = decode_duration_unit(&format!("{path}.unit"), unit_j)?;
            let style_j = req(path, fields, "style", "DurationStyle string")?;
            let style = decode_duration_style(&format!("{path}.style"), style_j)?;
            Ok(CellFormat::Duration { style, unit })
        }
        "RelativeTime" => {
            // Phase 819 — cell-vocabulary parity with `Format::RelativeTime`.
            let v = req(path, fields, "unit", "RelativeTimeUnit string")?;
            Ok(CellFormat::RelativeTime {
                unit: decode_relative_time_unit(&format!("{path}.unit"), v)?,
            })
        }
        "Custom" => Ok(CellFormat::Custom),
        other => Err(unknown_du_case(
            path,
            other,
            "None | Number | Currency | Percent | SignificantDigits | Date | Duration | RelativeTime | Custom",
        )),
    }
}

fn decode_column_width(path: &str, j: &JVal) -> DResult<ColumnWidth> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Auto" => Ok(ColumnWidth::Auto),
        "Fixed" => Ok(ColumnWidth::Fixed {
            pixels: req_int(path, fields, "pixels", "integer pixel count")?,
        }),
        "Flex" => Ok(ColumnWidth::Flex {
            weight: req_float(path, fields, "weight", "float weight")?,
        }),
        other => Err(unknown_du_case(path, other, "Auto | Fixed | Flex")),
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
fn normalise_transform_source(j: &JVal) -> JVal {
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

fn decode_binding(path: &str, j: &JVal) -> DResult<Binding> {
    decode_binding_slot(path, j, StaticSlot::Untyped)
}

fn decode_binding_slot(path: &str, j: &JVal, slot: StaticSlot) -> DResult<Binding> {
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
    match disc(path, fields)? {
        "Static" => {
            // Phase 677 — absence is structural: a MISSING `value` means the
            // binding carries none, and the legacy `"value": null` spelling
            // normalises to the same thing (§16 shorthand) by routing to the very
            // same per-slot parse, so the two cannot disagree.
            let null = JVal::Null;
            let v = get(fields, "value").unwrap_or(&null);
            let value = slot.parse(&format!("{path}.value"), v)?;
            Ok(Binding::Static { value })
        }
        "Query" => {
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
        "Filter" => {
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
        "Selection" => {
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
        "State" => {
            let key = req_string(path, fields, "key", "state key string")?;
            // Field aliases: initialValue / default → defaultValue.
            // Phase 677 — an ABSENT default decodes exactly as the legacy
            // `"defaultValue": null` did, or the encoder re-emits a placeholder
            // and the round-trip breaks (caught by `form-declarative`).
            let null = JVal::Null;
            let raw =
                get_aliased(fields, "defaultValue", &["initialValue", "default"]).unwrap_or(&null);
            let default_value = slot
                .parse(&format!("{path}.defaultValue"), raw)
                .unwrap_or_else(|_| slot.placeholder());
            Ok(Binding::State { key, default_value })
        }
        "Computed" => Ok(Binding::Computed),
        // Phase 765 — the host-furnished instant: no wire fields, the bare
        // `{"$type":"Now"}` object. The instant is already the wire-shaped
        // string, so a decoded reader receives it as-is (the sibling hosts'
        // identity projection; this closure-free host has nothing to erase).
        "Now" => Ok(Binding::Now),
        "I18n" => {
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
        "Local" => {
            let initial_j = req(path, fields, "initialFrom", "Local InitialFrom Binding")?;
            let initial_from =
                decode_binding_slot(&format!("{path}.initialFrom"), initial_j, slot)?;
            let flush_on = match get(fields, "flushOn") {
                None => LocalFlushTrigger::OnBlur,
                Some(v) => decode_local_flush_trigger(&format!("{path}.flushOn"), v)?,
            };
            Ok(Binding::Local {
                flush_on,
                initial_from: Box::new(initial_from),
            })
        }
        "Format" => {
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
        "Transform" => {
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
            // carried default data. A State wrapper carrying NO data still
            // errors didactically through the columnar codec (the 815
            // posture), and a State wrapper's carried data is snapshot-decoded
            // here so the ragged-rows didactic stays byte-identical.
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
                    let has_carried =
                        matches!(src_j, JVal::Obj(sf) if get(sf, "defaultValue").is_some());
                    if tag == "State" && !has_carried {
                        // No carried data — surface the columnar codec's own
                        // missing-field didactic (byte-identical to pre-818).
                        let src_n = normalise_transform_source(src_j);
                        TransformSource::Data(decode_data_source(&src_n).map_err(|e| {
                            make_error(
                                DecodeErrorCode::WrongType,
                                format!("{path}.source"),
                                e,
                                None,
                            )
                        })?)
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
        "Invoke" => {
            let capability_id = req_string(path, fields, "capabilityId", "capability id string")?;
            let args_j = req(path, fields, "args", "invoke args array")?;
            let args = decode_invoke_args(&format!("{path}.args"), args_j)?;
            Ok(Binding::Invoke {
                capability_id,
                args,
            })
        }
        // Lenient AI-ingest: the `TextSource.Bound` wrapper convention
        // transferred to a bare-Binding slot — `{"$type":"Bound","binding":X}`
        // carries exactly one payload field, so the unwrap is one-to-one.
        // Decode-only; the canonical encoder never wraps bare-Binding slots.
        "Bound" => {
            let inner = req(path, fields, "binding", "the wrapped Binding object")?;
            decode_binding_slot(&format!("{path}.binding"), inner, slot)
        }
        other => Err(unknown_du_case(
            path,
            other,
            "Static | Query | Filter | Selection | State | Computed | I18n | Local | Format | Transform | Invoke",
        )),
    }
}

fn req_binding(path: &str, fields: &Fields, key: &str, expected: &str) -> DResult<Binding> {
    let v = req(path, fields, key, expected)?;
    decode_binding(&format!("{path}.{key}"), v)
}

fn req_binding_slot(
    path: &str,
    fields: &Fields,
    key: &str,
    expected: &str,
    slot: StaticSlot,
) -> DResult<Binding> {
    let v = req(path, fields, key, expected)?;
    decode_binding_slot(&format!("{path}.{key}"), v, slot)
}

fn opt_binding(path: &str, fields: &Fields, key: &str) -> DResult<Option<Binding>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(decode_binding(&format!("{path}.{key}"), v)?)),
    }
}

// ─── TextSource ──────────────────────────────────────────────────────────────

fn decode_text_source(path: &str, j: &JVal) -> DResult<TextSource> {
    // Lenient AI-ingest shorthand (§16, normative for every conformant host): a
    // bare JSON string decodes as `TextSource.Literal` and re-encodes verbose.
    if let JVal::Str(s) = j {
        return Ok(TextSource::Literal(s.clone()));
    }
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Literal" => Ok(TextSource::Literal(req_string(
            path,
            fields,
            "text",
            "literal text string",
        )?)),
        "Bound" => {
            let v = req(path, fields, "binding", "Binding<string> object")?;
            let binding = decode_binding(&format!("{path}.binding"), v)?;
            Ok(TextSource::Bound(Box::new(binding)))
        }
        "I18n" => {
            let key = req_string(path, fields, "key", "i18n key string")?;
            let args = match get(fields, "args") {
                None => vec![],
                Some(v) => decode_jval_map(&format!("{path}.args"), v)?,
            };
            Ok(TextSource::I18n { key, args })
        }
        other => Err(unknown_du_case(path, other, "Literal | Bound | I18n")),
    }
}

fn req_text_source(path: &str, fields: &Fields, key: &str, expected: &str) -> DResult<TextSource> {
    let v = req(path, fields, key, expected)?;
    decode_text_source(&format!("{path}.{key}"), v)
}

fn opt_text_source(path: &str, fields: &Fields, key: &str) -> DResult<Option<TextSource>> {
    match get(fields, key) {
        None => Ok(None),
        Some(v) => Ok(Some(decode_text_source(&format!("{path}.{key}"), v)?)),
    }
}

// ─── Action ──────────────────────────────────────────────────────────────────

fn decode_action(path: &str, j: &JVal) -> DResult<Action> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Dispatch" => Ok(Action::Dispatch),
        "Call" => {
            // Field alias: url → endpoint.
            let endpoint =
                req_string_aliased(path, fields, "endpoint", &["url"], "ApiEndpoint string")?;
            let into = match get(fields, "into") {
                None => None,
                Some(v) => {
                    let into_path = format!("{path}.into");
                    let io = as_obj(&into_path, v)?;
                    match disc(&into_path, io)? {
                        "State" => Some(CallResultTarget::State {
                            key: req_string(&into_path, io, "key", "state key string")?,
                        }),
                        "Query" => Some(CallResultTarget::Query {
                            name: req_string(&into_path, io, "name", "query name string")?,
                        }),
                        other => {
                            return Err(unknown_du_case(&into_path, other, "State | Query"));
                        }
                    }
                }
            };
            Ok(Action::Call {
                endpoint,
                into,
                on_result: opt_closure(fields, "onResult"),
            })
        }
        "Notify" => {
            let channel = req_string(path, fields, "channel", "notification channel string")?;
            let payload_j = req(path, fields, "payload", "JsonValue payload")?;
            let payload = decode_jval(&format!("{path}.payload"), payload_j)?;
            Ok(Action::Notify { channel, payload })
        }
        "Navigate" => Ok(Action::Navigate {
            // Field aliases: href (the dominant web name) / url / to → route.
            route: req_string_aliased(
                path,
                fields,
                "route",
                &["href", "url", "to"],
                "route string",
            )?,
        }),
        "SetState" => {
            // Phase 818 — `value` (a literal JSON value, written verbatim) XOR
            // `valueFrom` (a Binding evaluated at dispatch time inside the
            // existing gate). Exactly one must be present; both / neither
            // error didactically naming both fields.
            let key = req_string(path, fields, "key", "state key string")?;
            let value_j = get(fields, "value");
            let from_j = get(fields, "valueFrom");
            match (value_j, from_j) {
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
                    value_from: Some(Box::new(decode_binding(
                        &format!("{path}.valueFrom"),
                        b,
                    )?)),
                }),
            }
        }
        "AiTool" => {
            let tool_name = req_string(path, fields, "toolName", "AI tool name string")?;
            let args_j = req(path, fields, "args", "JsonValue args")?;
            let args = decode_jval(&format!("{path}.args"), args_j)?;
            Ok(Action::AiTool { tool_name, args })
        }
        "Chain" => {
            let ops_j = req(path, fields, "ops", "Action list (Chain)")?;
            let items = as_arr(&format!("{path}.ops"), ops_j)?;
            let mut actions = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                actions.push(decode_action(&format!("{path}.ops[{i}]"), item)?);
            }
            Ok(Action::Chain(actions))
        }
        "CommitLocal" => Ok(Action::CommitLocal {
            node_id: req_string(path, fields, "nodeId", "Local-bound input NodeId string")?,
        }),
        "WriteToClipboard" => Ok(Action::WriteToClipboard {
            text: req_string(path, fields, "text", "clipboard payload string")?,
        }),
        "ReadFileBody" => {
            let file_ref = req_string(path, fields, "fileRef", "FileRef id string")?;
            let enc_j = req(path, fields, "encoding", "FileReadEncoding")?;
            let encoding = decode_file_read_encoding(&format!("{path}.encoding"), enc_j)?;
            Ok(Action::ReadFileBody { file_ref, encoding })
        }
        "Invoke" => {
            let capability_id = req_string(path, fields, "capabilityId", "capability id string")?;
            let args_j = req(path, fields, "args", "invoke args array")?;
            let args = decode_invoke_args(&format!("{path}.args"), args_j)?;
            Ok(Action::Invoke {
                capability_id,
                args,
            })
        }
        other => Err(unknown_du_case(
            path,
            other,
            "Dispatch | Call | Notify | Navigate | SetState | AiTool | Chain | CommitLocal | WriteToClipboard | ReadFileBody | Invoke",
        )),
    }
}

fn req_action(path: &str, fields: &Fields, key: &str, expected: &str) -> DResult<Action> {
    let v = req(path, fields, key, expected)?;
    decode_action(&format!("{path}.{key}"), v)
}

// ─── Display specs ───────────────────────────────────────────────────────────

fn decode_metric_spec(path: &str, j: &JVal) -> DResult<MetricSpec> {
    let fields = as_obj(path, j)?;
    let label = req_text_source(path, fields, "label", "Metric label TextSource")?;
    // 0.2.0 rename law — scalar displayed value ⇒ `value`; the retired
    // `source` spelling is a hard error (clean break), the web-prior `data`
    // alias remains (§3.6).
    let value = req_binding_aliased(path, fields, "value", &["data"], "Metric value binding")?;
    // Phase 460 — the stylistic fields are omitted-when-default on the wire.
    let format = opt_cell_format_default(path, fields, "format")?;
    let tone = opt_tone_default(path, fields, "tone")?;
    let weight = opt_weight_default(path, fields, "weight")?;
    let emphasis = opt_emphasis_default(path, fields, "emphasis")?;
    let trend = opt_binding(path, fields, "trend")?;
    let trend_format = match get(fields, "trendFormat") {
        None => None,
        Some(v) => Some(decode_cell_format(&format!("{path}.trendFormat"), v)?),
    };
    let icon = opt_string(path, fields, "icon")?;
    let subtext = opt_text_source(path, fields, "subtext")?;
    Ok(MetricSpec {
        label,
        value,
        format,
        tone,
        weight,
        emphasis,
        trend,
        trend_format,
        icon,
        subtext,
    })
}

fn decode_heading_spec(path: &str, j: &JVal) -> DResult<HeadingSpec> {
    let fields = as_obj(path, j)?;
    let level = req_int(path, fields, "level", "heading level integer")?;
    let text = req_text_source(path, fields, "text", "heading TextSource")?;
    let variant_j = req(path, fields, "variant", "HeadingVariant")?;
    let variant = decode_heading_variant(&format!("{path}.variant"), variant_j)?;
    Ok(HeadingSpec {
        level,
        text,
        variant,
    })
}

fn decode_label_value_row_spec(path: &str, j: &JVal) -> DResult<LabelValueRowSpec> {
    let fields = as_obj(path, j)?;
    let label = req_text_source(path, fields, "label", "row TextSource label")?;
    // 0.2.0 rename law — scalar displayed value ⇒ `value` (`data` alias kept).
    let value = req_binding_aliased(path, fields, "value", &["data"], "row Binding<float> value")?;
    // Phase 460 — `format` omitted-when-default. The `emphasis` here is the
    // behavioural bool (not the `Emphasis` style DU): 0.2.2 omitted-when-false,
    // cross-vocab coerced when a model writes the enum spelling.
    let format = opt_cell_format_default(path, fields, "format")?;
    let emphasis = match get(fields, "emphasis") {
        None => false,
        Some(v) => decode_emphasis_flag(&format!("{path}.emphasis"), v)?,
    };
    let help = opt_text_source(path, fields, "help")?;
    Ok(LabelValueRowSpec {
        label,
        value,
        format,
        emphasis,
        help,
    })
}

fn decode_fact_spec(path: &str, j: &JVal) -> DResult<FactSpec> {
    let fields = as_obj(path, j)?;
    let label = req_text_source(path, fields, "label", "Fact label TextSource")?;
    let value = req_text_source(path, fields, "value", "Fact value TextSource")?;
    let emphasis = match get(fields, "emphasis") {
        None => false,
        Some(v) => decode_emphasis_flag(&format!("{path}.emphasis"), v)?,
    };
    let tone = opt_tone_default(path, fields, "tone")?;
    let help = opt_text_source(path, fields, "help")?;
    let icon = opt_string(path, fields, "icon")?;
    Ok(FactSpec {
        label,
        value,
        emphasis,
        tone,
        help,
        icon,
    })
}

fn decode_markdown_spec(path: &str, j: &JVal) -> DResult<MarkdownSpec> {
    let fields = as_obj(path, j)?;
    let text = req_text_source(path, fields, "text", "markdown TextSource")?;
    Ok(MarkdownSpec { text })
}

fn decode_badge_spec(path: &str, j: &JVal) -> DResult<BadgeSpec> {
    let fields = as_obj(path, j)?;
    let label = req_text_source(path, fields, "label", "Badge label TextSource")?;
    let variant_j = req(path, fields, "variant", "BadgeVariant")?;
    let variant = decode_badge_variant(&format!("{path}.variant"), variant_j)?;
    Ok(BadgeSpec { label, variant })
}

fn decode_link_spec(path: &str, j: &JVal) -> DResult<LinkSpec> {
    let fields = as_obj(path, j)?;
    let href = req_binding(path, fields, "href", "link Binding<string> Href")?;
    let label = req_text_source(path, fields, "label", "link label TextSource")?;
    let download = req_bool(path, fields, "download", "download bool")?;
    let rel = opt_string(path, fields, "rel")?;
    let target = opt_string(path, fields, "target")?;
    // Optional closed enumeration — an unknown case is UNKNOWN_DU_CASE at
    // `$.kind.protection`.
    let protection = match get(fields, "protection") {
        None => None,
        Some(v) => Some(decode_link_protection(&format!("{path}.protection"), v)?),
    };
    Ok(LinkSpec {
        href,
        label,
        download,
        rel,
        target,
        protection,
    })
}

fn decode_image_spec(path: &str, j: &JVal) -> DResult<ImageSpec> {
    let fields = as_obj(path, j)?;
    let alt = req_text_source(path, fields, "alt", "Image alt TextSource")?;
    let src = req_binding(path, fields, "src", "Image Binding<string> Src")?;
    let variant_j = req(path, fields, "variant", "ImageVariant")?;
    let variant = decode_image_variant(&format!("{path}.variant"), variant_j)?;
    Ok(ImageSpec { alt, src, variant })
}

fn decode_list_spec(path: &str, j: &JVal) -> DResult<ListSpec> {
    let fields = as_obj(path, j)?;
    let items_j = req(path, fields, "items", "List items TextSource array")?;
    let arr = as_arr(&format!("{path}.items"), items_j)?;
    let mut items = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        items.push(decode_text_source(&format!("{path}.items[{i}]"), item)?);
    }
    let ordered = req_bool(path, fields, "ordered", "ordered bool")?;
    Ok(ListSpec { items, ordered })
}

fn decode_toast_spec(path: &str, j: &JVal) -> DResult<ToastSpec> {
    let fields = as_obj(path, j)?;
    let message = req_text_source(path, fields, "message", "Toast message TextSource")?;
    let tone = opt_tone_default(path, fields, "tone")?;
    let open = req_binding(path, fields, "open", "Toast open binding")?;
    // 0.2.0 — omitted-when-TRUE (a toast is dismissable unless said otherwise;
    // the one inverted default in §3.6's table).
    let dismissable = opt_bool(path, fields, "dismissable")?.unwrap_or(true);
    Ok(ToastSpec {
        message,
        tone,
        open,
        dismissable,
    })
}

fn decode_code_block_spec(path: &str, j: &JVal) -> DResult<CodeBlockSpec> {
    let fields = as_obj(path, j)?;
    let code = req_string(path, fields, "code", "code-block code string")?;
    let language = req_string(path, fields, "language", "code-block language string")?;
    let line_numbers = req_bool(path, fields, "lineNumbers", "lineNumbers bool")?;
    let copyable = req_bool(path, fields, "copyable", "copyable bool")?;
    let lines_j = req(path, fields, "highlightLines", "highlightLines int array")?;
    let arr = as_arr(&format!("{path}.highlightLines"), lines_j)?;
    let mut highlight_lines = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        highlight_lines.push(as_int(&format!("{path}.highlightLines[{i}]"), item)?);
    }
    Ok(CodeBlockSpec {
        code,
        language,
        line_numbers,
        highlight_lines,
        copyable,
    })
}

fn decode_math_spec(path: &str, j: &JVal) -> DResult<MathSpec> {
    let fields = as_obj(path, j)?;
    let source = req_string(path, fields, "source", "math LaTeX source string")?;
    let display_j = req(path, fields, "display", "MathDisplay")?;
    let display = decode_math_display(&format!("{path}.display"), display_j)?;
    Ok(MathSpec { source, display })
}

fn decode_sparkline_spec(path: &str, j: &JVal) -> DResult<SparklineSpec> {
    let fields = as_obj(path, j)?;
    // Field alias: data → source.
    let source = req_binding_slot_aliased(
        path,
        fields,
        "source",
        &["data"],
        "Sparkline Source binding",
        StaticSlot::FloatSeq,
    )?;
    Ok(SparklineSpec { source })
}

fn decode_skeleton_spec(path: &str, j: &JVal) -> DResult<SkeletonSpec> {
    let fields = as_obj(path, j)?;
    let rows = req_int(path, fields, "rows", "skeleton row count integer")?;
    Ok(SkeletonSpec { rows })
}

// Phase 821 — the standalone icon-only display kind.
fn decode_icon_spec(path: &str, j: &JVal) -> DResult<IconSpec> {
    let fields = as_obj(path, j)?;
    let icon = req_string(path, fields, "icon", "icon name string")?;
    // `size` omitted-when-`Medium`; `tone` omitted-when-default (the
    // Phase 460 discipline); `label` omitted-when-decorative.
    let size = match get(fields, "size") {
        None => IconSize::Medium,
        Some(v) => decode_icon_size(&format!("{path}.size"), v)?,
    };
    let tone = opt_tone_default(path, fields, "tone")?;
    let label = match get(fields, "label") {
        None => None,
        Some(v) => Some(as_str(&format!("{path}.label"), v)?.to_string()),
    };
    Ok(IconSpec {
        icon,
        size,
        tone,
        label,
    })
}

fn decode_callout_spec(path: &str, j: &JVal) -> DResult<CalloutSpec> {
    let fields = as_obj(path, j)?;
    let body = req_text_source(path, fields, "body", "Callout body TextSource")?;
    // 0.2.0 — omitted-when-default (false).
    let dismissable = opt_bool(path, fields, "dismissable")?.unwrap_or(false);
    let tone = opt_tone_default(path, fields, "tone")?;
    // Field alias: title → heading (Callout is in the scoped title→heading set).
    let heading = opt_text_source_aliased(path, fields, "heading", &["title"])?;
    let icon = opt_string(path, fields, "icon")?;
    Ok(CalloutSpec {
        body,
        dismissable,
        tone,
        heading,
        icon,
    })
}

fn decode_progress_spec(path: &str, j: &JVal) -> DResult<ProgressSpec> {
    let fields = as_obj(path, j)?;
    let fraction = req_binding(path, fields, "fraction", "Progress fraction binding")?;
    // 0.2.0 — omitted-when-default (false).
    let indeterminate = opt_bool(path, fields, "indeterminate")?.unwrap_or(false);
    let tone = opt_tone_default(path, fields, "tone")?;
    let label = opt_text_source(path, fields, "label")?;
    let caveat = opt_text_source(path, fields, "caveat")?;
    Ok(ProgressSpec {
        fraction,
        indeterminate,
        tone,
        label,
        caveat,
    })
}

// ─── Input specs ─────────────────────────────────────────────────────────────

/// Phase 596 — the auto-bind context for a control's ABSENT `value` slot. One
/// rule across the whole control vocabulary: every control may omit `value`; a
/// filter chip auto-binds `$filters.<name>` (0.2.0) and a form field
/// auto-binds `$state.<field id>` with the slot's typed placeholder as the
/// State default (0.2.1).
#[derive(Clone, Copy)]
enum ControlAutoBind<'a> {
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
mod control_value_defaults {
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
    pub fn date() -> StaticValue {
        StaticValue::Ast(JVal::Str(String::new()))
    }
    pub fn date_range() -> StaticValue {
        StaticValue::StringPair(String::new(), String::new())
    }
}

fn decode_form_field_kind(
    auto_bind: ControlAutoBind<'_>,
    path: &str,
    j: &JVal,
) -> DResult<FormFieldKind> {
    let fields = as_obj(path, j)?;
    let on_change = opt_closure(fields, "onChange");
    let on_toggle = opt_closure(fields, "onToggle");
    // Value slot: present ⇒ typed decode; absent ⇒ the context's auto-binding
    // — Filter(name) on a chip, State(field id, typed placeholder) on a form
    // field (Phase 596).
    let value_or = |slot: StaticSlot, placeholder: StaticValue| -> DResult<Binding> {
        match get(fields, "value") {
            Some(v) => decode_binding_slot(&format!("{path}.value"), v, slot),
            None => Ok(auto_bind.auto_binding(placeholder)),
        }
    };
    match disc(path, fields)? {
        "Text" => Ok(FormFieldKind::Text {
            value: value_or(StaticSlot::Untyped, control_value_defaults::text())?,
            on_change,
        }),
        "Number" => Ok(FormFieldKind::Number {
            value: value_or(StaticSlot::Untyped, control_value_defaults::number())?,
            on_change,
        }),
        "Checkbox" => Ok(FormFieldKind::Checkbox {
            value: value_or(StaticSlot::Untyped, control_value_defaults::checkbox())?,
            on_toggle,
        }),
        // Phase 766 — the switch affordance: Checkbox's mechanics under a
        // distinct tag.
        "Toggle" => Ok(FormFieldKind::Toggle {
            value: value_or(StaticSlot::Untyped, control_value_defaults::checkbox())?,
            on_toggle,
        }),
        "Choice" => Ok(FormFieldKind::Choice {
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
        // 0.2.0 — the dual-thumb numeric range (absorbed FilterKind.RangeFilter).
        // The canonical Static pair rides as the BARE `{min, max}` object (no
        // `$type`) — accept it before the generic binding dispatch.
        "Range" => Ok(FormFieldKind::Range {
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
        "RangedNumber" => Ok(FormFieldKind::RangedNumber {
            value: value_or(StaticSlot::Untyped, control_value_defaults::number())?,
            min: opt_float(path, fields, "min")?,
            max: opt_float(path, fields, "max")?,
            step: opt_float(path, fields, "step")?,
            on_change,
        }),
        "SegmentedChoice" => {
            let options = req_binding_slot(
                path,
                fields,
                "options",
                "SegmentedChoice options binding",
                StaticSlot::Options,
            )?;
            // Lenient omitted-when-default (§3.6): absent restores the
            // language default `Horizontal`. Decode-only — the encoder still
            // always emits it (unlike Tabs).
            let orientation = match get(fields, "orientation") {
                None => Orientation::Horizontal,
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
        "TextArea" => Ok(FormFieldKind::TextArea {
            rows: req_int(path, fields, "rows", "textarea row count integer")?,
            value: value_or(StaticSlot::Untyped, control_value_defaults::text())?,
            on_change,
        }),
        "Date" => {
            let value = value_or(StaticSlot::Untyped, control_value_defaults::date())?;
            let variant_j = req(path, fields, "variant", "DateVariant")?;
            let variant = decode_date_variant(&format!("{path}.variant"), variant_j)?;
            Ok(FormFieldKind::Date {
                value,
                variant,
                min: opt_string(path, fields, "min")?,
                max: opt_string(path, fields, "max")?,
                step: opt_float(path, fields, "step")?,
                on_change,
            })
        }
        "DateRange" => {
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
                _ => value_or(StaticSlot::StringPair, control_value_defaults::date_range())?,
            };
            let variant_j = req(path, fields, "variant", "DateVariant")?;
            let variant = decode_date_variant(&format!("{path}.variant"), variant_j)?;
            Ok(FormFieldKind::DateRange {
                value,
                variant,
                min: opt_string(path, fields, "min")?,
                max: opt_string(path, fields, "max")?,
                step: opt_float(path, fields, "step")?,
                on_change,
            })
        }
        // The hint is DERIVED from the canonical vocabulary, never hand-typed —
        // the hand-typed form had already drifted (Phase 746).
        other => Err(unknown_du_case(
            path,
            other,
            &CANONICAL_FORM_FIELD_KINDS.join(" | "),
        )),
    }
}

fn decode_form_field(path: &str, j: &JVal) -> DResult<FormField> {
    let fields = as_obj(path, j)?;
    // Field alias: name → id. Id decodes first so the form context's
    // auto-bind can use it (Phase 596).
    let id = req_string_aliased(path, fields, "id", &["name"], "form field id string")?;
    let kind_j = req(path, fields, "kind", "FormFieldKind")?;
    let kind = decode_form_field_kind(
        ControlAutoBind::FormFieldId(&id),
        &format!("{path}.kind"),
        kind_j,
    )?;
    let label = req_text_source(path, fields, "label", "field label TextSource")?;
    let required = req_bool(path, fields, "required", "required bool")?;
    let help = opt_text_source(path, fields, "help")?;
    Ok(FormField {
        id,
        kind,
        label,
        required,
        help,
    })
}

fn decode_form_spec(path: &str, j: &JVal) -> DResult<FormSpec> {
    let obj = as_obj(path, j)?;
    let fields_j = req(path, obj, "fields", "form field list")?;
    let arr = as_arr(&format!("{path}.fields"), fields_j)?;
    let mut form_fields = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        form_fields.push(decode_form_field(&format!("{path}.fields[{i}]"), item)?);
    }
    let on_submit = req_action(path, obj, "onSubmit", "onSubmit Action")?;
    let submit_label = req_text_source(path, obj, "submitLabel", "submitLabel TextSource")?;
    let disabled = opt_binding(path, obj, "disabled")?;
    Ok(FormSpec {
        fields: form_fields,
        on_submit,
        submit_label,
        disabled,
    })
}

fn decode_filter_spec(path: &str, j: &JVal) -> DResult<FilterSpec> {
    let fields = as_obj(path, j)?;
    // 0.2.0 filters-unification: the chip's control is an ordinary
    // FormFieldKind; its absent `value` auto-binds Filter(name). Name decodes
    // first so the synthesis can use it.
    let name = req_string(path, fields, "name", "filter name string")?;
    let kind_j = req(path, fields, "kind", "FormFieldKind control")?;
    let kind = decode_form_field_kind(
        ControlAutoBind::FilterChip(&name),
        &format!("{path}.kind"),
        kind_j,
    )?;
    let label = req_text_source(path, fields, "label", "filter label TextSource")?;
    Ok(FilterSpec { kind, label, name })
}

fn decode_button_spec(path: &str, j: &JVal) -> DResult<ButtonSpec> {
    let fields = as_obj(path, j)?;
    let label = req_text_source(path, fields, "label", "Button label TextSource")?;
    let on_click = req_action(path, fields, "onClick", "onClick Action")?;
    let variant_j = req(path, fields, "variant", "ButtonVariant")?;
    let variant = decode_button_variant(&format!("{path}.variant"), variant_j)?;
    let icon = opt_string(path, fields, "icon")?;
    let disabled = opt_binding(path, fields, "disabled")?;
    Ok(ButtonSpec {
        label,
        on_click,
        variant,
        icon,
        disabled,
    })
}

fn decode_select_spec(path: &str, j: &JVal) -> DResult<SelectSpec> {
    let fields = as_obj(path, j)?;
    let label = req_text_source(path, fields, "label", "Select label TextSource")?;
    // Field aliases: options / data → source.
    let source = req_binding_slot_aliased(
        path,
        fields,
        "source",
        &["options", "data"],
        "Select source binding",
        StaticSlot::Options,
    )?;
    let value = req_binding_slot(
        path,
        fields,
        "value",
        "Select value binding",
        StaticSlot::StringOpt,
    )?;
    let placeholder = opt_text_source(path, fields, "placeholder")?;
    let disabled = opt_binding(path, fields, "disabled")?;
    let multiple = opt_bool(path, fields, "multiple")?;
    let values = match get(fields, "values") {
        None => None,
        Some(v) => Some(decode_binding_slot(
            &format!("{path}.values"),
            v,
            StaticSlot::StringList,
        )?),
    };
    Ok(SelectSpec {
        label,
        source,
        value,
        on_change: opt_closure(fields, "onChange"),
        placeholder,
        disabled,
        multiple: multiple == Some(true),
        values,
        on_change_multi: opt_closure(fields, "onChangeMulti"),
    })
}

fn decode_file_upload_spec(path: &str, j: &JVal) -> DResult<FileUploadSpec> {
    let fields = as_obj(path, j)?;
    let accept_j = req(path, fields, "accept", "accept string list")?;
    let arr = as_arr(&format!("{path}.accept"), accept_j)?;
    let mut accept = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        accept.push(as_str(&format!("{path}.accept[{i}]"), item)?.to_string());
    }
    let label = req_text_source(path, fields, "label", "FileUpload label TextSource")?;
    let multiple = req_bool(path, fields, "multiple", "multiple bool")?;
    let disabled = opt_binding(path, fields, "disabled")?;
    Ok(FileUploadSpec {
        accept,
        label,
        multiple,
        disabled,
    })
}

// ─── Visualisation specs ─────────────────────────────────────────────────────

/// The tone-map field names a `TonedPill` cell accepts, canonical first. `map` is
/// the shortest honest name for a value→tone dictionary and the least descriptive.
const TONE_MAP_KEYS: [&str; 3] = ["map", "toneMap", "tones"];

/// Phase 750 — a `TonedPill`'s `map`: a string-keyed object whose VALUES are
/// `ToneVariant`s. Routed through `decode_tone` per entry, so the §3.6 tone aliases
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
fn decode_tone_map(path: &str, j: &JVal) -> DResult<BTreeMap<String, ToneVariant>> {
    let fields = as_obj(path, j)?;
    let mut map = BTreeMap::new();
    for (key, v) in fields {
        let entry_path = format!("{path}.{key}");
        let tone = decode_tone(&entry_path, v).map_err(|e| {
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

/// The shared body of the canonical `TonedPill` case and the `Pill`-tagged §16
/// shorthand — ONE reader, so the two spellings cannot drift apart in what they
/// accept.
fn decode_toned_pill(path: &str, fields: &Fields) -> DResult<CellKindErased> {
    let field = req_string_aliased(
        path,
        fields,
        "field",
        &[],
        "TonedPill row-field name (drives the label and the map key)",
    )?;
    let map_j = req_aliased(
        path,
        fields,
        TONE_MAP_KEYS[0],
        &TONE_MAP_KEYS[1..],
        "TonedPill value→ToneVariant map",
    )?;
    let map = decode_tone_map(&format!("{path}.map"), map_j)?;
    // `default` is omitted-when-`Default` (Phase 460); an absent key restores the
    // identity, and an aliased `Neutral` normalises to `Default` and then omits —
    // two rules composing, in that order.
    let default_tone = opt_tone_default(path, fields, "default")?;
    Ok(CellKindErased::TonedPill {
        field,
        map,
        default_tone,
    })
}

fn decode_cell_kind_erased(path: &str, j: &JVal) -> DResult<CellKindErased> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        // Lenient-ingest (WIRE_FORMAT.md §16, Phase 750): "pill" is the WORD for the
        // thing, so a declarative tone rule arrives tagged `Pill` more often than
        // tagged `TonedPill`. Before this phase those keys were accepted and
        // DISCARDED — the author's whole intent gone, silently, with no error to
        // notice. Presence of a tone map is the unambiguous tell: a closure `Pill`
        // carries only `labelFn`/`toneFn` and can never carry one.
        "Pill" if TONE_MAP_KEYS.iter().any(|k| get(fields, k).is_some()) => {
            decode_toned_pill(path, fields)
        }
        "TonedPill" => decode_toned_pill(path, fields),
        "Text" => Ok(CellKindErased::Text),
        "Numeric" => Ok(CellKindErased::Numeric),
        "Date" => Ok(CellKindErased::Date),
        "Editable" => Ok(CellKindErased::Editable),
        "Checkbox" => Ok(CellKindErased::Checkbox),
        "Button" => Ok(CellKindErased::Button {
            label: req_text_source(path, fields, "label", "cell button label TextSource")?,
        }),
        "ButtonGroup" => {
            let buttons_j = req(path, fields, "buttons", "button group list")?;
            let arr = as_arr(&format!("{path}.buttons"), buttons_j)?;
            let mut labels = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                let p = format!("{path}.buttons[{i}]");
                let bf = as_obj(&p, item)?;
                labels.push(req_text_source(&p, bf, "label", "button label TextSource")?);
            }
            Ok(CellKindErased::ButtonGroup { labels })
        }
        "Link" => Ok(CellKindErased::Link),
        "Pill" => Ok(CellKindErased::Pill),
        "Progress" => Ok(CellKindErased::Progress),
        "Custom" => Ok(CellKindErased::Custom),
        other => Err(unknown_du_case(
            path,
            other,
            "Text | Numeric | Date | Editable | Checkbox | Button | ButtonGroup | Link | Pill | TonedPill | Progress | Custom",
        )),
    }
}

fn decode_column_erased(path: &str, j: &JVal) -> DResult<ColumnErased> {
    let fields = as_obj(path, j)?;
    // Phase 460 — format/width omitted-when-default. Field aliases: type → kind,
    // header/title → label.
    let format = opt_cell_format_default(path, fields, "format")?;
    let kind_j = req_aliased(path, fields, "kind", &["type"], "CellKindErased")?;
    let kind = decode_cell_kind_erased(&format!("{path}.kind"), kind_j)?;
    let label = req_string_aliased(
        path,
        fields,
        "label",
        &["header", "title"],
        "column label string",
    )?;
    let width = opt_column_width_default(path, fields, "width")?;
    let field = opt_string(path, fields, "field")?;
    Ok(ColumnErased {
        format,
        kind,
        label,
        width,
        value: opt_closure(fields, "value"),
        field,
    })
}

fn decode_static_rows(path: &str, j: &JVal) -> DResult<StaticRows> {
    let fields = as_obj(path, j)?;
    let headers_j = req(path, fields, "headers", "headers TextSource list")?;
    let harr = as_arr(&format!("{path}.headers"), headers_j)?;
    let mut headers = Vec::with_capacity(harr.len());
    for (i, item) in harr.iter().enumerate() {
        headers.push(decode_text_source(&format!("{path}.headers[{i}]"), item)?);
    }
    let rows_j = req(path, fields, "rows", "rows TextSource matrix")?;
    let rarr = as_arr(&format!("{path}.rows"), rows_j)?;
    let mut rows = Vec::with_capacity(rarr.len());
    for (i, row_j) in rarr.iter().enumerate() {
        let row_arr = as_arr(&format!("{path}.rows[{i}]"), row_j)?;
        let mut row = Vec::with_capacity(row_arr.len());
        for (k, cell) in row_arr.iter().enumerate() {
            row.push(decode_text_source(&format!("{path}.rows[{i}][{k}]"), cell)?);
        }
        rows.push(row);
    }
    // Phase 801 — the optional sort-intent slots. Both absent re-encodes
    // byte-identically to the pre-801 wire.
    let sortable = opt_bool(path, fields, "sortable")?;
    let default_sort = match get(fields, "defaultSort") {
        None => None,
        Some(v) => Some(decode_default_sort(&format!("{path}.defaultSort"), v)?),
    };
    Ok(StaticRows {
        headers,
        rows,
        sortable,
        default_sort,
    })
}

/// Phase 801 — the `{column, direction}` initial-order declaration. `column` is
/// a NON-NEGATIVE index into `headers`; a negative value is `WRONG_TYPE`, which
/// is also what `schema.json`'s `minimum: 0` says, so the two expressions of the
/// contract agree.
fn decode_default_sort(path: &str, j: &JVal) -> DResult<DefaultSort> {
    let fields = as_obj(path, j)?;
    let column = req_int(path, fields, "column", "non-negative header index")?;
    if column < 0 {
        return Err(wrong_type(
            &format!("{path}.column"),
            "JSON number (non-negative integer header index)",
        ));
    }
    let direction_j = req(path, fields, "direction", "asc | desc")?;
    let direction = decode_sort_direction(&format!("{path}.direction"), direction_j)?;
    Ok(DefaultSort { column, direction })
}

fn decode_grid_spec(path: &str, j: &JVal) -> DResult<GridSpec> {
    let fields = as_obj(path, j)?;
    let columns_j = req(path, fields, "columns", "columns list")?;
    let arr = as_arr(&format!("{path}.columns"), columns_j)?;
    let mut columns = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        columns.push(decode_column_erased(&format!("{path}.columns[{i}]"), item)?);
    }
    // 0.2.0 — omitted-when-default (false).
    let editable = opt_bool(path, fields, "editable")?.unwrap_or(false);
    // Field aliases: data / rows → source.
    let source = req_binding_slot_aliased(
        path,
        fields,
        "source",
        &["data", "rows"],
        "Grid source binding",
        StaticSlot::Rows,
    )?;
    let row_key_field = opt_string(path, fields, "rowKeyField")?;
    // Phase 818 — the grid-sort header affordance: names the State key
    // carrying the `{column, direction}` sort descriptor. Optional string,
    // encode-omitted when absent.
    let sort_state_key = opt_string(path, fields, "sortStateKey")?;
    let static_rows = match get(fields, "staticRows") {
        None => None,
        Some(v) => Some(decode_static_rows(&format!("{path}.staticRows"), v)?),
    };
    Ok(GridSpec {
        columns,
        editable,
        source,
        on_row_click: opt_closure(fields, "onRowClick"),
        row_key: opt_closure(fields, "rowKey"),
        row_key_field,
        sort_state_key,
        static_rows,
    })
}

fn decode_chart_spec(path: &str, j: &JVal) -> DResult<ChartSpec> {
    let fields = as_obj(path, j)?;
    let kind_j = req(path, fields, "kind", "ChartKind")?;
    let kind = decode_chart_kind(&format!("{path}.kind"), kind_j)?;
    // Field alias: data → source.
    let source = req_binding_slot_aliased(
        path,
        fields,
        "source",
        &["data"],
        "Chart source binding",
        StaticSlot::Rows,
    )?;
    let x_field = req_string(path, fields, "xField", "xField string")?;
    let y_fields_j = req(path, fields, "yFields", "yFields string list")?;
    let arr = as_arr(&format!("{path}.yFields"), y_fields_j)?;
    let mut y_fields = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        y_fields.push(as_str(&format!("{path}.yFields[{i}]"), item)?.to_string());
    }
    let title = opt_text_source(path, fields, "title")?;
    // `stacked` round-trips (carried since the fixture corpus pinned it);
    // absent — the legacy wire — defaults to false.
    let stacked = opt_bool(path, fields, "stacked")?.unwrap_or(false);
    Ok(ChartSpec {
        kind,
        source,
        stacked,
        x_field,
        y_fields,
        title,
        on_point_click: opt_closure(fields, "onPointClick"),
    })
}

fn decode_map_spec(path: &str, j: &JVal) -> DResult<MapSpec> {
    let fields = as_obj(path, j)?;
    let centre_latitude = req_float(path, fields, "centreLatitude", "centre latitude float")?;
    let centre_longitude = req_float(path, fields, "centreLongitude", "centre longitude float")?;
    // Field aliases: data / markers → source.
    let source = req_binding_slot_aliased(
        path,
        fields,
        "source",
        &["data", "markers"],
        "Map source binding",
        StaticSlot::Markers,
    )?;
    let zoom = req_int(path, fields, "zoom", "zoom integer")?;
    Ok(MapSpec {
        centre_latitude,
        centre_longitude,
        source,
        zoom,
        on_marker_click: opt_closure(fields, "onMarkerClick"),
    })
}

// ─── Drawing (Phase 524) — closed Shape / CurveCommand DUs ───────────────────
//
// Geometry is static numbers (a Drawing is a resolved artefact); only DrawStyle
// carries Bindings. An unrecognised Shape / CurveCommand $type is UNKNOWN_DU_CASE
// at $.kind.shapes[i].$type / $.kind.shapes[i].commands[j].$type (the closed-set
// default-deny). Missing style defaults to the all-inherited empty style.

fn decode_view_box(path: &str, j: &JVal) -> DResult<ViewBox> {
    let fields = as_obj(path, j)?;
    Ok(ViewBox {
        height: req_float(path, fields, "height", "height number")?,
        min_x: req_float(path, fields, "minX", "minX number")?,
        min_y: req_float(path, fields, "minY", "minY number")?,
        width: req_float(path, fields, "width", "width number")?,
    })
}

fn decode_draw_point(path: &str, j: &JVal) -> DResult<DrawPoint> {
    let fields = as_obj(path, j)?;
    Ok(DrawPoint {
        x: req_float(path, fields, "x", "x number")?,
        y: req_float(path, fields, "y", "y number")?,
    })
}

fn decode_req_point(path: &str, fields: &Fields, key: &str) -> DResult<DrawPoint> {
    let v = req(path, fields, key, "DrawPoint")?;
    decode_draw_point(&format!("{path}.{key}"), v)
}

fn decode_point_list(path: &str, fields: &Fields) -> DResult<Vec<DrawPoint>> {
    let points_j = req(path, fields, "points", "DrawPoint list")?;
    let arr = as_arr(&format!("{path}.points"), points_j)?;
    let mut points = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        points.push(decode_draw_point(&format!("{path}.points[{i}]"), item)?);
    }
    Ok(points)
}

fn decode_draw_style(path: &str, j: &JVal) -> DResult<DrawStyle> {
    let fields = as_obj(path, j)?;
    let text_anchor = match get(fields, "textAnchor") {
        None => None,
        Some(v) => Some(decode_text_anchor(&format!("{path}.textAnchor"), v)?),
    };
    let emphasis = match get(fields, "emphasis") {
        None => None,
        Some(v) => Some(decode_emphasis(&format!("{path}.emphasis"), v)?),
    };
    Ok(DrawStyle {
        fill: opt_binding(path, fields, "fill")?,
        stroke: opt_binding(path, fields, "stroke")?,
        stroke_width: opt_binding(path, fields, "strokeWidth")?,
        opacity: opt_binding(path, fields, "opacity")?,
        text_anchor,
        font_size: opt_float(path, fields, "fontSize")?,
        emphasis,
        font_family: opt_string(path, fields, "fontFamily")?,
        // Phase 642 — keyed mark identity; omitted-when-None.
        mark_id: opt_string(path, fields, "markId")?,
    })
}

fn decode_style_or_default(path: &str, fields: &Fields) -> DResult<DrawStyle> {
    match get(fields, "style") {
        None => Ok(DrawStyle::default()),
        Some(v) => decode_draw_style(&format!("{path}.style"), v),
    }
}

fn decode_curve_command(path: &str, j: &JVal) -> DResult<CurveCommand> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "MoveTo" => Ok(CurveCommand::MoveTo(decode_req_point(path, fields, "to")?)),
        "LineTo" => Ok(CurveCommand::LineTo(decode_req_point(path, fields, "to")?)),
        "CubicTo" => Ok(CurveCommand::CubicTo {
            control1: decode_req_point(path, fields, "control1")?,
            control2: decode_req_point(path, fields, "control2")?,
            to: decode_req_point(path, fields, "to")?,
        }),
        "QuadraticTo" => Ok(CurveCommand::QuadraticTo {
            control: decode_req_point(path, fields, "control")?,
            to: decode_req_point(path, fields, "to")?,
        }),
        "Close" => Ok(CurveCommand::Close),
        other => Err(unknown_du_case(
            path,
            other,
            "MoveTo | LineTo | CubicTo | QuadraticTo | Close",
        )),
    }
}

fn decode_shape(path: &str, j: &JVal) -> DResult<Shape> {
    let fields = as_obj(path, j)?;
    let tag = disc(path, fields)?;
    let style = decode_style_or_default(path, fields)?;
    match tag {
        "Group" => {
            let children_j = req(path, fields, "children", "Shape list")?;
            let arr = as_arr(&format!("{path}.children"), children_j)?;
            let mut children = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                children.push(decode_shape(&format!("{path}.children[{i}]"), item)?);
            }
            Ok(Shape::Group { children, style })
        }
        "Rectangle" => Ok(Shape::Rectangle {
            x: req_float(path, fields, "x", "x number")?,
            y: req_float(path, fields, "y", "y number")?,
            width: req_float(path, fields, "width", "width number")?,
            height: req_float(path, fields, "height", "height number")?,
            corner_radius: opt_float(path, fields, "cornerRadius")?,
            style,
        }),
        "Line" => Ok(Shape::Line {
            x1: req_float(path, fields, "x1", "x1 number")?,
            y1: req_float(path, fields, "y1", "y1 number")?,
            x2: req_float(path, fields, "x2", "x2 number")?,
            y2: req_float(path, fields, "y2", "y2 number")?,
            style,
        }),
        "Polyline" => Ok(Shape::Polyline {
            points: decode_point_list(path, fields)?,
            style,
        }),
        "Polygon" => Ok(Shape::Polygon {
            points: decode_point_list(path, fields)?,
            style,
        }),
        "Curve" => {
            let commands_j = req(path, fields, "commands", "CurveCommand list")?;
            let arr = as_arr(&format!("{path}.commands"), commands_j)?;
            let mut commands = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                commands.push(decode_curve_command(
                    &format!("{path}.commands[{i}]"),
                    item,
                )?);
            }
            Ok(Shape::Curve { commands, style })
        }
        "Circle" => Ok(Shape::Circle {
            cx: req_float(path, fields, "cx", "cx number")?,
            cy: req_float(path, fields, "cy", "cy number")?,
            r: req_float(path, fields, "r", "r number")?,
            style,
        }),
        "Ellipse" => Ok(Shape::Ellipse {
            cx: req_float(path, fields, "cx", "cx number")?,
            cy: req_float(path, fields, "cy", "cy number")?,
            rx: req_float(path, fields, "rx", "rx number")?,
            ry: req_float(path, fields, "ry", "ry number")?,
            style,
        }),
        "Label" => Ok(Shape::Label {
            x: req_float(path, fields, "x", "x number")?,
            y: req_float(path, fields, "y", "y number")?,
            text: req_text_source(path, fields, "text", "TextSource")?,
            style,
        }),
        other => Err(unknown_du_case(
            path,
            other,
            "Group | Rectangle | Line | Polyline | Polygon | Curve | Circle | Ellipse | Label",
        )),
    }
}

fn decode_drawing_spec(path: &str, j: &JVal) -> DResult<DrawingSpec> {
    let fields = as_obj(path, j)?;
    let shapes_j = req(path, fields, "shapes", "Shape list")?;
    let arr = as_arr(&format!("{path}.shapes"), shapes_j)?;
    let mut shapes = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        shapes.push(decode_shape(&format!("{path}.shapes[{i}]"), item)?);
    }
    let view_box_j = req(path, fields, "viewBox", "ViewBox")?;
    let view_box = decode_view_box(&format!("{path}.viewBox"), view_box_j)?;
    Ok(DrawingSpec {
        view_box,
        shapes,
        style: decode_style_or_default(path, fields)?,
        title: opt_text_source(path, fields, "title")?,
        description: opt_text_source(path, fields, "description")?,
    })
}

// ─── Layout specs ────────────────────────────────────────────────────────────

fn decode_children(path: &str, fields: &Fields) -> DResult<Vec<Node>> {
    let children_j = req(path, fields, "children", "children Node list")?;
    let arr = as_arr(&format!("{path}.children"), children_j)?;
    let mut children = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        children.push(decode_node_ast(&format!("{path}.children[{i}]"), item)?);
    }
    Ok(children)
}

fn decode_box_layout(path: &str, j: &JVal) -> DResult<BoxLayout> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Flex" => {
            let direction_j = req(path, fields, "direction", "Orientation")?;
            let direction = decode_orientation(&format!("{path}.direction"), direction_j)?;
            let wrap = req_bool(path, fields, "wrap", "wrap bool")?;
            let gap = opt_int(path, fields, "gap")?;
            Ok(BoxLayout::Flex {
                direction,
                gap,
                wrap,
            })
        }
        "Grid" => {
            // Field alias: columns → cols. Lenient (§3.6): absent `cols` with
            // no `templateColumns` reads as `Auto` (the author asked for "a
            // grid" with no shape — the auto packer is the faithful reading);
            // absent `cols` WITH a template reads `cols: 1` (the template
            // carries the real shape; cols is the fallback lane count).
            let cols_j = get_aliased(fields, "cols", &["columns"]);
            let template_columns = opt_string(path, fields, "templateColumns")?;
            match (cols_j, template_columns) {
                (None, None) => Ok(BoxLayout::Auto),
                (cols_j, template_columns) => {
                    let cols = match cols_j {
                        None => 1,
                        Some(v) => as_int(&format!("{path}.cols"), v)?,
                    };
                    Ok(BoxLayout::Grid {
                        cols,
                        gap: opt_int(path, fields, "gap")?,
                        template_columns,
                    })
                }
            }
        }
        "Auto" => Ok(BoxLayout::Auto),
        other => Err(unknown_du_case(path, other, "Flex | Grid | Auto")),
    }
}

fn decode_box_role(path: &str, j: &JVal) -> DResult<BoxRole> {
    let s = as_str(path, j)?;
    BoxRole::from_wire(s)
        .ok_or_else(|| unknown_du_case(path, s, "Group | Card | Dashboard | Separator"))
}

fn decode_box_spec(path: &str, j: &JVal) -> DResult<BoxSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    // Field alias: title → heading (Box is in the scoped title→heading set).
    let heading = opt_text_source_aliased(path, fields, "heading", &["title"])?;
    let layout_j = req(path, fields, "layout", "layout object")?;
    let layout = decode_box_layout(&format!("{path}.layout"), layout_j)?;
    let role_j = req(path, fields, "role", "role string")?;
    let role = decode_box_role(&format!("{path}.role"), role_j)?;
    Ok(BoxSpec {
        children,
        heading,
        layout,
        role,
    })
}

fn decode_split_panel_spec(path: &str, j: &JVal) -> DResult<SplitPanelSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    let weight = req_float(path, fields, "weight", "weight float")?;
    Ok(SplitPanelSpec { children, weight })
}

fn decode_tab_header(path: &str, j: &JVal) -> DResult<TabHeader> {
    let fields = as_obj(path, j)?;
    let label = req_text_source(path, fields, "label", "tab header label TextSource")?;
    let icon = opt_string(path, fields, "icon")?;
    let disabled = opt_binding(path, fields, "disabled")?;
    Ok(TabHeader {
        label,
        icon,
        disabled,
    })
}

fn decode_tabs_spec(path: &str, j: &JVal) -> DResult<TabsSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    // 0.2.0 — omitted-when-default (Horizontal), encoder-symmetric.
    let orientation = match get(fields, "orientation") {
        None => Orientation::Horizontal,
        Some(v) => decode_orientation(&format!("{path}.orientation"), v)?,
    };
    let tab_headers = match get(fields, "tabHeaders") {
        None => None,
        Some(v) => {
            let arr = as_arr(&format!("{path}.tabHeaders"), v)?;
            let mut headers = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                headers.push(decode_tab_header(&format!("{path}.tabHeaders[{i}]"), item)?);
            }
            Some(headers)
        }
    };
    let tab_tags = match get(fields, "tabTags") {
        None => None,
        Some(v) => {
            let arr = as_arr(&format!("{path}.tabTags"), v)?;
            let mut tags = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                tags.push(as_str(&format!("{path}.tabTags[{i}]"), item)?.to_string());
            }
            Some(tags)
        }
    };
    let active_tag = opt_binding(path, fields, "activeTag")?;
    // `activeIndex` round-trips; absent (legacy wire) defaults to Static 0.
    let active_index = match get(fields, "activeIndex") {
        None => Binding::Static {
            value: StaticValue::Ast(JVal::Num(0.0)),
        },
        Some(v) => decode_binding(&format!("{path}.activeIndex"), v)?,
    };
    Ok(TabsSpec {
        children,
        orientation,
        active_index,
        on_select: opt_closure(fields, "onSelect"),
        tab_headers,
        tab_tags,
        active_tag,
        on_select_tag: opt_closure(fields, "onSelectTag"),
    })
}

fn decode_stepper_spec(path: &str, j: &JVal) -> DResult<StepperSpec> {
    let fields = as_obj(path, j)?;
    let active_step = req_binding(path, fields, "activeStep", "activeStep binding")?;
    let children = decode_children(path, fields)?;
    Ok(StepperSpec {
        active_step,
        children,
    })
}

fn decode_summary_list_spec(path: &str, j: &JVal) -> DResult<SummaryListSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    // Field alias: title → heading.
    let heading = opt_text_source_aliased(path, fields, "heading", &["title"])?;
    Ok(SummaryListSpec { children, heading })
}

fn decode_disclosure_spec(path: &str, j: &JVal) -> DResult<DisclosureSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    let default_open = req_bool(path, fields, "defaultOpen", "defaultOpen bool")?;
    // Field alias: title → heading.
    let heading = req_text_source_aliased(
        path,
        fields,
        "heading",
        &["title"],
        "Disclosure heading TextSource",
    )?;
    let open = req_binding(path, fields, "open", "open binding")?;
    Ok(DisclosureSpec {
        children,
        default_open,
        heading,
        open,
        on_toggle: opt_closure(fields, "onToggle"),
    })
}

fn decode_modal_spec(path: &str, j: &JVal) -> DResult<ModalSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    let dismissable = req_bool(path, fields, "dismissable", "dismissable bool")?;
    let on_dismiss = match get(fields, "onDismiss") {
        None => None,
        Some(v) => Some(decode_action(&format!("{path}.onDismiss"), v)?),
    };
    let open = req_binding(path, fields, "open", "open binding")?;
    // Field alias: title → heading.
    let heading = opt_text_source_aliased(path, fields, "heading", &["title"])?;
    Ok(ModalSpec {
        children,
        dismissable,
        open,
        on_dismiss,
        heading,
    })
}

fn decode_scroll_area_spec(path: &str, j: &JVal) -> DResult<ScrollAreaSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    let orientation_j = req(path, fields, "orientation", "ScrollOrientation")?;
    let orientation = decode_scroll_orientation(&format!("{path}.orientation"), orientation_j)?;
    Ok(ScrollAreaSpec {
        children,
        orientation,
        max_height: opt_int(path, fields, "maxHeight")?,
        max_width: opt_int(path, fields, "maxWidth")?,
    })
}

// ─── Fragments / Mount / structural ──────────────────────────────────────────

fn decode_content_hash(path: &str, j: &JVal) -> DResult<ContentHash> {
    let fields = as_obj(path, j)?;
    let algorithm = req_string(path, fields, "algorithm", "hash algorithm string")?;
    let hash = req_string(path, fields, "hash", "hash string")?;
    let strictness = req_string(
        path,
        fields,
        "strictness",
        "'StrictReplay' | 'AdvisoryWarning' | 'Enforced'",
    )?;
    let strictness = HashStrictness::from_wire(&strictness).ok_or_else(|| {
        unknown_du_case(
            &format!("{path}.strictness"),
            &strictness,
            "StrictReplay | AdvisoryWarning | Enforced",
        )
    })?;
    Ok(ContentHash {
        algorithm,
        hash,
        strictness,
    })
}

fn decode_hole_value_space(path: &str, j: &JVal) -> DResult<HoleValueSpace> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "IntRange" => Ok(HoleValueSpace::IntRange {
            min: req_int(path, fields, "min", "IntRange min")?,
            max: req_int(path, fields, "max", "IntRange max")?,
        }),
        "FloatRange" => Ok(HoleValueSpace::FloatRange {
            min: req_float(path, fields, "min", "FloatRange min")?,
            max: req_float(path, fields, "max", "FloatRange max")?,
        }),
        "StringLen" => Ok(HoleValueSpace::StringLen {
            min_len: req_int(path, fields, "minLen", "StringLen minLen")?,
            max_len: req_int(path, fields, "maxLen", "StringLen maxLen")?,
        }),
        "Enum" => {
            let choices_j = req(path, fields, "choices", "Enum choices")?;
            let arr = as_arr(&format!("{path}.choices"), choices_j)?;
            let mut choices = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                choices.push(as_str(&format!("{path}.choices[{i}]"), item)?.to_string());
            }
            Ok(HoleValueSpace::Enum { choices })
        }
        "AnyString" => Ok(HoleValueSpace::AnyString),
        other => Err(unknown_du_case(
            path,
            other,
            "IntRange | FloatRange | StringLen | Enum | AnyString",
        )),
    }
}

fn decode_fragment_scalar(path: &str, j: &JVal) -> DResult<FragmentScalar> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Int" => Ok(FragmentScalar::Int(req_int(
            path,
            fields,
            "value",
            "Int value",
        )?)),
        "Float" => Ok(FragmentScalar::Float(req_float(
            path,
            fields,
            "value",
            "Float value",
        )?)),
        "Bool" => Ok(FragmentScalar::Bool(req_bool(
            path,
            fields,
            "value",
            "Bool value",
        )?)),
        "Str" => Ok(FragmentScalar::Str(req_string(
            path,
            fields,
            "value",
            "Str value",
        )?)),
        other => Err(unknown_du_case(path, other, "Int | Float | Bool | Str")),
    }
}

fn decode_hole_decl(path: &str, j: &JVal) -> DResult<HoleDecl> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Value" => {
            let name = req_string(path, fields, "name", "Value hole name")?;
            let space_j = req(path, fields, "space", "Value hole space")?;
            let space = decode_hole_value_space(&format!("{path}.space"), space_j)?;
            let default = match get(fields, "default") {
                None => None,
                Some(v) => Some(decode_fragment_scalar(&format!("{path}.default"), v)?),
            };
            Ok(HoleDecl::Value {
                name,
                space,
                default,
            })
        }
        "Slot" => Ok(HoleDecl::Slot {
            name: req_string(path, fields, "name", "Slot hole name")?,
            kind_constraint: opt_string(path, fields, "kindConstraint")?,
        }),
        "Repeat" => {
            let name = req_string(path, fields, "name", "Repeat hole name")?;
            let space_j = req(path, fields, "countSpace", "Repeat hole countSpace")?;
            let count_space = decode_hole_value_space(&format!("{path}.countSpace"), space_j)?;
            Ok(HoleDecl::Repeat { name, count_space })
        }
        other => Err(unknown_du_case(path, other, "Value | Slot | Repeat")),
    }
}

fn decode_effect_class(path: &str, j: &JVal) -> DResult<EffectClass> {
    let fields = as_obj(path, j)?;
    let host = req_string(path, fields, "hostEffect", "EffectClass hostEffect")?;
    let host_effect = HostEffect::from_wire(&host).ok_or_else(|| {
        unknown_du_case(
            &format!("{path}.hostEffect"),
            &host,
            "Pure | ReadsHost | WritesHost",
        )
    })?;
    let det = req_string(path, fields, "determinism", "EffectClass determinism")?;
    let determinism = DeterminismSource::from_wire(&det).ok_or_else(|| {
        unknown_du_case(
            &format!("{path}.determinism"),
            &det,
            "Deterministic | Clock | Random | Network",
        )
    })?;
    Ok(EffectClass {
        host_effect,
        determinism,
    })
}

/// A `FragmentArg` map entry: `SlotArg` carries a subtree; any other
/// discriminator reads as a value scalar (Int | Float | Bool | Str).
fn decode_fragment_arg(path: &str, j: &JVal) -> DResult<FragmentArg> {
    let fields = as_obj(path, j)?;
    if disc(path, fields)? == "SlotArg" {
        let tree_j = req(path, fields, "tree", "SlotArg tree Node")?;
        let tree = decode_node_ast(&format!("{path}.tree"), tree_j)?;
        Ok(FragmentArg::Slot {
            tree: Box::new(tree),
        })
    } else {
        Ok(FragmentArg::Value(decode_fragment_scalar(path, j)?))
    }
}

fn decode_fragment_args(path: &str, j: &JVal) -> DResult<Vec<(String, FragmentArg)>> {
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

// ─── NodeKind ────────────────────────────────────────────────────────────────

const WRONG_NODE_KIND_HINT: &str = "a Layout primitive (Box | SplitPanel | Tabs | Stepper | SummaryList | Disclosure | Modal | ScrollArea), a Display primitive (Heading | Markdown | Metric | Badge | Sparkline | Callout | Progress | Skeleton | Icon | LabelValueRow | Fact | Link | Image | List | Toast | CodeBlock | Math | Drawing), an Input primitive (Form | Filters | Button | FileUpload | Select), a Visualisation primitive (DataGrid | Chart | Map), or Custom | ErrorBoundary | Switch | FragmentDecl | FragmentRef | Mount";

fn decode_node_kind(path: &str, j: &JVal) -> DResult<NodeKind> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        // Layout.
        "Box" => Ok(NodeKind::Box(decode_box_spec(path, j)?)),
        "SplitPanel" => Ok(NodeKind::SplitPanel(decode_split_panel_spec(path, j)?)),
        "Tabs" => Ok(NodeKind::Tabs(decode_tabs_spec(path, j)?)),
        "Stepper" => Ok(NodeKind::Stepper(decode_stepper_spec(path, j)?)),
        "SummaryList" => Ok(NodeKind::SummaryList(decode_summary_list_spec(path, j)?)),
        "Disclosure" => Ok(NodeKind::Disclosure(decode_disclosure_spec(path, j)?)),
        "Modal" => Ok(NodeKind::Modal(decode_modal_spec(path, j)?)),
        "ScrollArea" => Ok(NodeKind::ScrollArea(decode_scroll_area_spec(path, j)?)),
        // Display.
        "Heading" => Ok(NodeKind::Heading(decode_heading_spec(path, j)?)),
        "Markdown" => Ok(NodeKind::Markdown(decode_markdown_spec(path, j)?)),
        "Metric" => Ok(NodeKind::Metric(decode_metric_spec(path, j)?)),
        "Badge" => Ok(NodeKind::Badge(decode_badge_spec(path, j)?)),
        "Sparkline" => Ok(NodeKind::Sparkline(decode_sparkline_spec(path, j)?)),
        "Callout" => Ok(NodeKind::Callout(decode_callout_spec(path, j)?)),
        "Progress" => Ok(NodeKind::Progress(decode_progress_spec(path, j)?)),
        "Skeleton" => Ok(NodeKind::Skeleton(decode_skeleton_spec(path, j)?)),
        "Icon" => Ok(NodeKind::Icon(decode_icon_spec(path, j)?)),
        "Fact" => Ok(NodeKind::Fact(decode_fact_spec(path, j)?)),
        "LabelValueRow" => Ok(NodeKind::LabelValueRow(decode_label_value_row_spec(
            path, j,
        )?)),
        "Link" => Ok(NodeKind::Link(decode_link_spec(path, j)?)),
        "Image" => Ok(NodeKind::Image(decode_image_spec(path, j)?)),
        "List" => Ok(NodeKind::List(decode_list_spec(path, j)?)),
        "Toast" => Ok(NodeKind::Toast(decode_toast_spec(path, j)?)),
        "CodeBlock" => Ok(NodeKind::CodeBlock(decode_code_block_spec(path, j)?)),
        "Math" => Ok(NodeKind::Math(decode_math_spec(path, j)?)),
        "Drawing" => Ok(NodeKind::Drawing(decode_drawing_spec(path, j)?)),
        // Input.
        "Form" => Ok(NodeKind::Form(decode_form_spec(path, j)?)),
        "Filters" => {
            let items_j = req(path, fields, "items", "Filters item list")?;
            let arr = as_arr(&format!("{path}.items"), items_j)?;
            let mut specs = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                specs.push(decode_filter_spec(&format!("{path}.items[{i}]"), item)?);
            }
            Ok(NodeKind::Filters(specs))
        }
        "Button" => Ok(NodeKind::Button(decode_button_spec(path, j)?)),
        "FileUpload" => Ok(NodeKind::FileUpload(decode_file_upload_spec(path, j)?)),
        "Select" => Ok(NodeKind::Select(decode_select_spec(path, j)?)),
        // Visualisation.
        "DataGrid" => Ok(NodeKind::DataGrid(decode_grid_spec(path, j)?)),
        "Chart" => Ok(NodeKind::Chart(decode_chart_spec(path, j)?)),
        "Map" => Ok(NodeKind::Map(decode_map_spec(path, j)?)),
        // Structural.
        "Custom" => {
            let module_id = req_string(path, fields, "moduleId", "Custom moduleId string")?;
            let component_id =
                req_string(path, fields, "componentId", "Custom componentId string")?;
            let props_j = req(path, fields, "props", "Custom props map")?;
            let props = decode_jval_map(&format!("{path}.props"), props_j)?;
            let content_hash = match get(fields, "contentHash") {
                None => None,
                Some(v) => Some(decode_content_hash(&format!("{path}.contentHash"), v)?),
            };
            let exposed_node_ids = match get(fields, "exposedNodeIds") {
                None => vec![],
                Some(v) => {
                    let arr = as_arr(&format!("{path}.exposedNodeIds"), v)?;
                    let mut ids = Vec::with_capacity(arr.len());
                    for (i, item) in arr.iter().enumerate() {
                        ids.push(as_str(&format!("{path}.exposedNodeIds[{i}]"), item)?.to_string());
                    }
                    ids
                }
            };
            Ok(NodeKind::Custom(CustomSpec {
                module_id,
                component_id,
                props,
                content_hash,
                exposed_node_ids,
            }))
        }
        "ErrorBoundary" => {
            let child_j = req(path, fields, "child", "ErrorBoundary child Node")?;
            let child = decode_node_ast(&format!("{path}.child"), child_j)?;
            let fallback_j = req(path, fields, "fallback", "ErrorBoundary fallback Node")?;
            let fallback = decode_node_ast(&format!("{path}.fallback"), fallback_j)?;
            Ok(NodeKind::ErrorBoundary(ErrorBoundarySpec {
                child: Box::new(child),
                fallback: Box::new(fallback),
            }))
        }
        "Switch" => {
            // Phase 768 — the selector is any Binding: `on` carries the
            // canonical Binding wire form; the no-default `State` form keeps
            // the compact `stateKey` spelling. Both absent keeps the stateKey
            // MISSING_FIELD, so the reject fixture's error is unchanged.
            let on = match get(fields, "on") {
                Some(v) => decode_binding(&format!("{path}.on"), v)?,
                None => Binding::State {
                    key: req_string(path, fields, "stateKey", "Switch stateKey string")?,
                    default_value: StaticValue::Ast(JVal::Null),
                },
            };
            let cases_j = req(path, fields, "cases", "Switch cases array")?;
            let arr = as_arr(&format!("{path}.cases"), cases_j)?;
            let mut cases = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                let cp = format!("{path}.cases[{i}]");
                let cf = as_obj(&cp, item)?;
                let match_value = req_string(&cp, cf, "match", "Switch case match string")?;
                let child_j = req(&cp, cf, "child", "Switch case child Node")?;
                let child = decode_node_ast(&format!("{cp}.child"), child_j)?;
                cases.push(SwitchCase { match_value, child });
            }
            let default_j = req(path, fields, "default", "Switch default Node")?;
            let default = decode_node_ast(&format!("{path}.default"), default_j)?;
            Ok(NodeKind::Switch(SwitchSpec {
                on,
                cases,
                default: Box::new(default),
            }))
        }
        "FragmentDecl" => {
            let name = req_string(path, fields, "name", "FragmentDecl name string")?;
            let body_j = req(path, fields, "body", "FragmentDecl body Node")?;
            let body = decode_node_ast(&format!("{path}.body"), body_j)?;
            let holes = match get(fields, "holes") {
                None => vec![],
                Some(v) => {
                    let arr = as_arr(&format!("{path}.holes"), v)?;
                    let mut holes = Vec::with_capacity(arr.len());
                    for (i, item) in arr.iter().enumerate() {
                        holes.push(decode_hole_decl(&format!("{path}.holes[{i}]"), item)?);
                    }
                    holes
                }
            };
            let effect = match get(fields, "effect") {
                None => EffectClass::PURE_DETERMINISTIC,
                Some(v) => decode_effect_class(&format!("{path}.effect"), v)?,
            };
            Ok(NodeKind::FragmentDecl(FragmentDeclSpec {
                name,
                body: Box::new(body),
                holes,
                effect,
            }))
        }
        "FragmentRef" => {
            let name = req_string(path, fields, "name", "FragmentRef name string")?;
            let args = match get(fields, "args") {
                None => vec![],
                Some(v) => decode_fragment_args(&format!("{path}.args"), v)?,
            };
            Ok(NodeKind::FragmentRef(FragmentRefSpec { name, args }))
        }
        "Mount" => {
            let scope_id = req_string(path, fields, "scopeId", "Mount scopeId string")?;
            let channel_j = req(path, fields, "channel", "Mount channel object")?;
            let channel_fields = as_obj(&format!("{path}.channel"), channel_j)?;
            let channel_path = format!("{path}.channel");
            let direction = req_string(
                &channel_path,
                channel_fields,
                "direction",
                "channel direction string",
            )?;
            let direction = ChannelDirection::from_wire(&direction).ok_or_else(|| {
                make_error(
                    DecodeErrorCode::UnknownDuCase,
                    format!("{channel_path}.direction"),
                    format!("unknown ChannelDirection '{direction}'"),
                    Some("OutOnly | TwoWay".to_string()),
                )
            })?;
            let message_shape = opt_string(&channel_path, channel_fields, "messageShape")?;
            let caps_j = req(path, fields, "capabilities", "Mount capabilities array")?;
            let caps_arr = as_arr(&format!("{path}.capabilities"), caps_j)?;
            let mut capabilities = Vec::with_capacity(caps_arr.len());
            for (i, item) in caps_arr.iter().enumerate() {
                capabilities.push(as_str(&format!("{path}.capabilities[{i}]"), item)?.to_string());
            }
            let inputs = match get(fields, "inputs") {
                None => vec![],
                Some(v) => decode_fragment_args(&format!("{path}.inputs"), v)?,
            };
            Ok(NodeKind::Mount(MountSpec {
                scope_id,
                inputs,
                channel: MountChannel {
                    direction,
                    message_shape,
                },
                capabilities,
            }))
        }
        other => Err(make_error(
            DecodeErrorCode::WrongNodeKind,
            format!("{path}.$type"),
            format!("unknown NodeKind discriminator '{other}'"),
            Some(WRONG_NODE_KIND_HINT.to_string()),
        )),
    }
}

// ─── StateBehaviour / SemanticStyle / Accessibility / Node ───────────────────

fn decode_state_behaviour(path: &str, j: &JVal) -> DResult<StateBehaviour> {
    let fields = as_obj(path, j)?;
    let on_loading = match get(fields, "onLoading") {
        None => None,
        Some(v) => Some(Box::new(decode_node_ast(&format!("{path}.onLoading"), v)?)),
    };
    let on_empty = match get(fields, "onEmpty") {
        None => None,
        Some(v) => Some(Box::new(decode_node_ast(&format!("{path}.onEmpty"), v)?)),
    };
    Ok(StateBehaviour {
        on_loading,
        on_empty,
        on_error: opt_closure(fields, "onError"),
    })
}

fn decode_semantic_style(path: &str, j: &JVal) -> DResult<SemanticStyle> {
    let fields = as_obj(path, j)?;
    // Phase 460 — tone/weight/emphasis omitted-when-default (as role/voice below).
    let tone = opt_tone_default(path, fields, "tone")?;
    let weight = opt_weight_default(path, fields, "weight")?;
    let emphasis = opt_emphasis_default(path, fields, "emphasis")?;
    // `role` / `voice` are optional on the wire — omitted at their defaults.
    let role = match get(fields, "role") {
        None => StyleRole::None,
        Some(v) => decode_style_role(&format!("{path}.role"), v)?,
    };
    let voice = match get(fields, "voice") {
        None => FontVoice::Default,
        Some(v) => decode_font_voice(&format!("{path}.voice"), v)?,
    };
    Ok(SemanticStyle {
        emphasis,
        tone,
        weight,
        role,
        voice,
    })
}

fn decode_accessibility(path: &str, j: &JVal) -> DResult<Accessibility> {
    let fields = as_obj(path, j)?;
    let label = opt_binding(path, fields, "label")?;
    let labelled_by = opt_string(path, fields, "labelledBy")?;
    let described_by = opt_string(path, fields, "describedBy")?;
    // Any string is accepted — named ARIA roles and the custom raw escape both
    // encode as the raw string (§10.2).
    let role = opt_string(path, fields, "role")?;
    let live_region = match get(fields, "liveRegion") {
        None => None,
        Some(v) => Some(decode_live_region(&format!("{path}.liveRegion"), v)?),
    };
    let hidden = opt_binding(path, fields, "hidden")?;
    Ok(Accessibility {
        label,
        labelled_by,
        described_by,
        role,
        live_region,
        hidden,
    })
}

fn decode_node_ast(path: &str, j: &JVal) -> DResult<Node> {
    let fields = as_obj(path, j)?;
    let id_j = req(path, fields, "id", "Node id string")?;
    let id = as_str(&format!("{path}.id"), id_j)?;
    if id.is_empty() {
        return Err(make_error(
            DecodeErrorCode::EmptyNodeId,
            format!("{path}.id"),
            "Node id is empty",
            Some("non-empty string".to_string()),
        ));
    }
    let kind_j = req(path, fields, "kind", "NodeKind discriminator object")?;
    let kind = decode_node_kind(&format!("{path}.kind"), kind_j)?;
    let state = match get(fields, "state") {
        None => StateBehaviour::default(),
        Some(v) => decode_state_behaviour(&format!("{path}.state"), v)?,
    };
    let style = match get(fields, "style") {
        None => SemanticStyle::default(),
        Some(v) => decode_semantic_style(&format!("{path}.style"), v)?,
    };
    let accessibility = match get(fields, "accessibility") {
        None => None,
        Some(v) => Some(decode_accessibility(&format!("{path}.accessibility"), v)?),
    };
    Ok(Node {
        id: id.to_string(),
        kind,
        state,
        style,
        accessibility,
    })
}

// ─── TreeOp ──────────────────────────────────────────────────────────────────

fn decode_tree_op_ast(path: &str, j: &JVal) -> DResult<TreeOp> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "EditNode" => {
            let target = req_string(path, fields, "target", "target NodeId")?;
            let kind_j = req(path, fields, "newKind", "NodeKind object")?;
            let new_kind = decode_node_kind(&format!("{path}.newKind"), kind_j)?;
            Ok(TreeOp::EditNode { target, new_kind })
        }
        "UpdateProp" => {
            let target = req_string(path, fields, "target", "target NodeId")?;
            let prop_path = req_string(path, fields, "path", "dot-separated path string")?;
            let value_j = req(path, fields, "value", "JsonValue payload")?;
            let value = decode_jval(&format!("{path}.value"), value_j)?;
            Ok(TreeOp::UpdateProp {
                target,
                path: prop_path,
                value,
            })
        }
        "ReplaceBinding" => {
            let target = req_string(path, fields, "target", "target NodeId")?;
            let slot = req_string(path, fields, "slot", "slot name string")?;
            let binding = req_binding(path, fields, "binding", "Binding object")?;
            Ok(TreeOp::ReplaceBinding {
                target,
                slot,
                binding,
            })
        }
        "UpdateStyle" => {
            let target = req_string(path, fields, "target", "target NodeId")?;
            let style_j = req(path, fields, "style", "SemanticStyle object")?;
            let style = decode_semantic_style(&format!("{path}.style"), style_j)?;
            Ok(TreeOp::UpdateStyle { target, style })
        }
        "UpdateState" => {
            let target = req_string(path, fields, "target", "target NodeId")?;
            let state_j = req(path, fields, "state", "StateBehaviour object")?;
            let state = decode_state_behaviour(&format!("{path}.state"), state_j)?;
            Ok(TreeOp::UpdateState { target, state })
        }
        "InsertChild" => {
            let parent_id = req_string(path, fields, "parentId", "parent NodeId")?;
            // A legacy `position` is ACCEPTED AND IGNORED for the migration
            // window: a stored v1 emission must still apply, as an append,
            // because order is now ReorderChildren's. This decoder reads named
            // fields and ignores the rest, so not reading it IS the tolerance.
            let child_j = req(path, fields, "child", "child Node object")?;
            let child = decode_node_ast(&format!("{path}.child"), child_j)?;
            Ok(TreeOp::InsertChild { parent_id, child })
        }
        "RemoveNode" => Ok(TreeOp::RemoveNode {
            target: req_string(path, fields, "target", "target NodeId")?,
        }),
        "MoveNode" => {
            // Legacy `newPosition` accepted and ignored — see InsertChild above.
            let target = req_string(path, fields, "target", "target NodeId")?;
            let new_parent_id = req_string(path, fields, "newParentId", "new parent NodeId")?;
            Ok(TreeOp::MoveNode {
                target,
                new_parent_id,
            })
        }
        "ReorderChildren" => {
            let parent_id = req_string(path, fields, "parentId", "parent NodeId")?;
            let order_j = req(path, fields, "newOrder", "NodeId list")?;
            let arr = as_arr(&format!("{path}.newOrder"), order_j)?;
            let mut new_order = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                new_order.push(as_str(&format!("{path}.newOrder[{i}]"), item)?.to_string());
            }
            Ok(TreeOp::ReorderChildren {
                parent_id,
                new_order,
            })
        }
        "ReplaceRoot" => {
            let node_j = req(path, fields, "node", "root Node object")?;
            let node = decode_node_ast(&format!("{path}.node"), node_j)?;
            Ok(TreeOp::ReplaceRoot { node })
        }
        "Batch" => {
            let ops_j = req(path, fields, "ops", "Batch inner-op list")?;
            let arr = as_arr(&format!("{path}.ops"), ops_j)?;
            let mut ops = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                ops.push(decode_tree_op_ast(&format!("{path}.ops[{i}]"), item)?);
            }
            Ok(TreeOp::Batch(ops))
        }
        other => Err(unknown_du_case(
            path,
            other,
            "EditNode | UpdateProp | ReplaceBinding | UpdateStyle | UpdateState | InsertChild | RemoveNode | MoveNode | ReorderChildren | ReplaceRoot | Batch",
        )),
    }
}

// ─── Public surface ──────────────────────────────────────────────────────────

fn invalid_json(parse_message: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::InvalidJson,
        "$",
        format!("input is not valid JSON: {parse_message}"),
        Some("well-formed JSON object per the canonical-JSON shape".to_string()),
    )
}

/// Decode a canonical-JSON `Node` payload into the storage-shape typed tree.
pub fn decode_node(json: &str) -> Result<Node, DecodeError> {
    match parse(json) {
        Ok(ast) => decode_node_ast("$", &ast),
        Err(e) => Err(invalid_json(&e.message)),
    }
}

/// Decode a canonical-JSON `TreeOp` payload into the storage-shape typed op.
pub fn decode_op(json: &str) -> Result<TreeOp, DecodeError> {
    match parse(json) {
        Ok(ast) => decode_tree_op_ast("$", &ast),
        Err(e) => Err(invalid_json(&e.message)),
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
        via(v, decode_tone)
    }

    pub fn weight(v: &JVal) -> C<StyleWeight> {
        via(v, decode_weight)
    }

    pub fn emphasis(v: &JVal) -> C<Emphasis> {
        via(v, decode_emphasis)
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
