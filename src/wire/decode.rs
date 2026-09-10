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

/// An unrecognised case at a `$type`-DISCRIMINATED position: the document carries a
/// literal `"$type"` member and its value is not a known case. WIRE_FORMAT.md §6 —
/// "`$type` appears literally in the path when the discriminator is at fault" — so the
/// reported path names that member.
fn unknown_du_case(path: &str, got: &str, expected: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::UnknownDuCase,
        format!("{path}.$type"),
        format!("unknown discriminator '{got}'"),
        Some(expected.to_string()),
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
fn unknown_enum_case(path: &str, got: &str, expected: &str) -> DecodeError {
    make_error(
        DecodeErrorCode::UnknownDuCase,
        path,
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

/// The width of every typed integer slot this format declares (§7.1). A
/// different bound from §2 rule 5's ±(2⁵³−1), answering a different question:
/// that one is where integer IDENTITY stops in an untyped payload position,
/// this one is the width of the SLOT, and a value the slot cannot hold has
/// nowhere to land.
const INT_SLOT_MIN: f64 = -2_147_483_648.0;
const INT_SLOT_MAX: f64 = 2_147_483_647.0;

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
fn as_int(path: &str, j: &JVal) -> DResult<i64> {
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

/// Phase 867 — `Metric.trendPolarity`, omitted-when-`HigherIsBetter` (§3.6.1
/// clause 4: an absent declaration means the ordinary reading, up is good).
fn opt_trend_polarity_default(path: &str, fields: &Fields, key: &str) -> DResult<TrendPolarity> {
    match get(fields, key) {
        None => Ok(TrendPolarity::HigherIsBetter),
        Some(v) => decode_trend_polarity(&format!("{path}.{key}"), v),
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
            $ty::from_wire(s).ok_or_else(|| unknown_enum_case(path, s, &$ty::WIRE_NAMES.join(" | ")))
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
                _ => Err(unknown_enum_case(path, s, &$ty::WIRE_NAMES.join(" | "))),
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
// Phase 867 — `TrendPolarity` is deliberately NOT aliased, and the omission is
// load-bearing rather than an oversight. The obvious candidates are exactly the
// spellings that must NOT be accepted: `"Neutral"` is the RESERVED case (see
// `TrendPolarity`), so aliasing it onto either canonical case would silently
// decide the question the reservation holds open and make a later admission a
// breaking re-meaning of already-emitted bytes rather than an addition; and
// `"Inverted"` / `"Descending"` would resurrect the boolean spelling §3.6.1
// refuses. An unknown spelling therefore fails `UNKNOWN_DU_CASE` naming only the
// two the format accepts.
decode_bare_enum!(decode_trend_polarity, TrendPolarity, "TrendPolarity");
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
decode_bare_enum!(
    decode_chart_legend_position,
    ChartLegendPosition,
    "ChartLegendPosition"
);
decode_bare_enum!(decode_chart_data_labels, ChartDataLabels, "ChartDataLabels");
decode_bare_enum!(decode_chart_x_scale, ChartXScale, "ChartXScale");
decode_bare_enum!(decode_image_variant, ImageVariant, "ImageVariant");
// Phase 1077 — the three `Image` presentation vocabularies (§3.6.2). BARE
// enums, so `reject/reject-unknown-image-aspect` reports at `$.kind.aspectRatio`
// with no `.$type` suffix (the Phase 1073 ruling) — the CSS ratio spelling
// `"16/9"` is refused rather than parsed, because admitting a numeric pair
// would reintroduce the free-form escape the tokens exist to close.
decode_bare_enum!(decode_image_fit, ImageFit, "ImageFit");
decode_bare_enum!(decode_image_aspect, ImageAspect, "ImageAspect");
decode_bare_enum!(decode_image_loading, ImageLoading, "ImageLoading");
// Phase 1110 - the timed-text track vocabulary. BARE, so an unrecognised
// spelling reports at `$.kind.tracks[i].kind` with no `.$type` suffix.
decode_bare_enum!(decode_track_kind, TrackKind, "TrackKind");
// Phase 1111 - the sandbox-relaxation vocabulary. BARE and reported at the
// ELEMENT's own path (`$.kind.permissions[0]`), which is what makes the HTML
// token an author reaches for from memory (`"allow-top-navigation"`) an
// UNKNOWN_DU_CASE there rather than a silent drop.
decode_bare_enum!(decode_embed_permission, EmbedPermission, "EmbedPermission");
// WIRE_FORMAT.md 3.6.11 / Phase 1472 - the modality and direction tokens.
decode_bare_enum!(decode_modality_kind, ModalityKind, "ModalityKind");
decode_bare_enum!(decode_text_direction, TextDirection, "TextDirection");
decode_bare_enum!(decode_link_protection, LinkProtection, "LinkProtection");
decode_bare_enum!(decode_math_display, MathDisplay, "MathDisplay");
decode_bare_enum!(decode_date_variant, DateVariant, "DateVariant");
// Phase 1536 — `Action::Navigate`'s destination window. BARE, so `"_blank"`
// reports at `$.kind.onClick.target` with no `.$type` suffix, and is refused
// rather than aliased: the HTML vocabulary it comes from also contains
// `_parent` and `_top`, which are frame-busting gestures a hosted tree must not
// be able to ask for.
decode_bare_enum!(decode_navigate_target, NavigateTarget, "NavigateTarget");
// Phase 1116 — the recording device. BARE, so `"Screen"` reports at
// `$.kind.capture` with no `.$type` suffix, and a host MUST NOT fall back to
// either device on an unrecognised value.
decode_bare_enum!(decode_capture_source, CaptureSource, "CaptureSource");
decode_bare_enum!(decode_text_format, TextFormat, "TextFormat");
decode_bare_enum!(decode_compare_op, CompareOp, "CompareOp");
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
// Phase 1533 — the `Binding::Now` grain: a strict SUBSET of RelativeTimeUnit.
// `Week` / `Month` / `Year` are refused rather than quietly accepted, because a
// calendar instant has no truncation to those that every host agrees on.
decode_bare_enum!(decode_time_grain, TimeGrain, "TimeGrain");
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

/// The coercion-bridge integer reader, held to the same §7.1 accept set as
/// `as_int`. A second reader with a looser rule would be an undeclared divergent
/// entry point under §20.1 — the coercion bridge is reached from
/// `TreeOp::UpdateProp`, which is wire input like any other.
fn c_int(j: &JVal) -> CResult<i64> {
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

/// Every direct sub-expression of `e`, so the three walks below share one
/// definition of the shape and cannot disagree about which arms recurse.
/// The `params` slot shared by `Binding::Transform` and `Binding::Expr` —
/// ONE decoder, because §3.3.2 makes it the SAME slot following the same rules,
/// and two copies would drift on the lenient form below.
///
/// Lenient AI-ingest (§3.6): a `{name: <Binding>}` MAP is accepted alongside
/// the canonical `[{from, name}]` array — normalised to the array form sorted
/// by name (the reference host's map iteration order). Omitted when empty.
fn decode_binding_params(path: &str, fields: &Fields) -> DResult<Option<Vec<TransformParam>>> {
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
                let pf = as_obj(&p, item)?;
                let name = req_string(&p, pf, "name", "param name string")?;
                let from_j = req(&p, pf, "from", "param source Binding")?;
                let from = decode_binding(&format!("{p}.from"), from_j)?;
                out.push(TransformParam { name, from });
            }
            Ok(Some(out))
        }
    }
}

fn expr_children(e: &ColExpr) -> Vec<&ColExpr> {
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
    }
}

/// The `ColExpr` node count of one expression — the subject of §21.8's
/// `MaxExprNodes`, which is counted per EXPRESSION rather than per document.
fn count_expr_nodes(e: &ColExpr) -> usize {
    1 + expr_children(e)
        .into_iter()
        .map(count_expr_nodes)
        .sum::<usize>()
}

/// The first `col` reference reachable in `e`, if any (§3.3.2 refusal 1).
fn first_col_reference(e: &ColExpr) -> Option<&str> {
    if let ColExpr::Col { name } = e {
        return Some(name);
    }
    expr_children(e).into_iter().find_map(first_col_reference)
}

/// The first param name `e` references that `bound` does not carry, if any
/// (§3.3.2 refusal 2). `InParam`'s name is a param too — it is the LIST
/// spelling of the same reference, so leaving it out would admit an unbound
/// membership test through the one arm that reads a param without being one.
fn first_unbound_param<'a>(e: &'a ColExpr, bound: &[&str]) -> Option<&'a str> {
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
        "Since" => {
            // Phase 1533 — `unit` is OPTIONAL and its absence is the
            // auto-selection request, not a default. Present-but-unreadable is
            // still a refusal.
            let unit = match get(fields, "unit") {
                None => None,
                Some(v) => Some(decode_relative_time_unit(&format!("{path}.unit"), v)?),
            };
            Ok(Format::Since { unit })
        }
        other => Err(unknown_du_case(
            path,
            other,
            "Number | Currency | Percent | Date | RelativeTime | Duration | Since",
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
        "Computed" => Ok(Binding::Computed),
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
        "Now" => {
            let grain = match get(fields, "grain") {
                None => None,
                Some(v) => Some(decode_time_grain(&format!("{path}.grain"), v)?),
            };
            Ok(Binding::Now { grain })
        }
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
        "Expr" => {
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

fn opt_binding_slot(
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
            //
            // Phase 1536 — the route is a `TextSource`, so a tree can name a
            // destination it computes from what the reader is looking at. The
            // bare JSON string IS `Literal`'s canonical form, so every document
            // written before the widening decodes exactly as it did — aliases
            // included, since they resolve before the value is decoded.
            route: req_text_source_aliased(
                path,
                fields,
                "route",
                &["href", "url", "to"],
                "route TextSource",
            )?,
            // Omitted at `Self`, so absence is the pre-1536 behaviour.
            target: match get(fields, "target") {
                None => NavigateTarget::Self_,
                Some(v) => decode_navigate_target(&format!("{path}.target"), v)?,
            },
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
        // Phase 1126 — the payload is a `TextSource`; the bare JSON string IS
        // `Literal`'s canonical form, so the explicit `{"$type":"Literal",…}`
        // envelope normalises down to it here as at every other text slot
        // (§16). Never coerced from a non-text JSON value: a host that read the
        // widening as "this member is now open" would put a JSON literal on the
        // reader's clipboard, which the reader later pastes with authority.
        "WriteToClipboard" => Ok(Action::WriteToClipboard {
            text: req_text_source(path, fields, "text", "clipboard payload TextSource")?,
        }),
        // Phase 1124 — the payload-free print, and the ONE `Action` arm strict
        // about unrecognised members. Everywhere else in this format an unknown
        // member is one the reading host has not learned yet, and dropping it is
        // the forward-compatible answer; here there is nothing to learn — page
        // range, size, margins and copies are the host's page setup and the
        // reader's dialogue — so accepting `{"$type":"Print","pageRange":"1-3"}`
        // would leave the emitter believing it had constrained a printing it had
        // not, with no error anywhere saying otherwise. The refusal names the
        // offending member's own path, taking the FIRST in sorted order so which
        // member is named is deterministic.
        "Print" => {
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
        // Phase 1537 — ask, then act. The DEPTH-ONE REFUSAL is the substance of
        // this arm: a `Confirm` reachable from either continuation is refused,
        // and the check walks the DECODED continuation rather than its immediate
        // `$type`, so a nested confirm inside a `Chain` is caught by the same
        // line that catches a bare one. A dialogue that answers a dialogue is a
        // modal stack the reader cannot escape, and it says nothing one question
        // does not.
        //
        // `WRONG_TYPE` follows the `SetState` value/valueFrom and
        // `Print`-with-payload precedents: a decoder POLICY refusal reuses it
        // rather than minting a code every host in the roster would owe an
        // adoption for.
        "Confirm" => {
            let prompt = req_text_source(path, fields, "prompt", "confirmation prompt TextSource")?;
            let on_confirm_j = req(path, fields, "onConfirm", "Action (onConfirm)")?;
            let on_confirm_path = format!("{path}.onConfirm");
            let on_confirm = decode_action(&on_confirm_path, on_confirm_j)?;
            refuse_nested_confirm(&on_confirm, &on_confirm_path)?;
            let on_cancel = match get(fields, "onCancel") {
                None => None,
                Some(v) => {
                    let on_cancel_path = format!("{path}.onCancel");
                    let action = decode_action(&on_cancel_path, v)?;
                    refuse_nested_confirm(&action, &on_cancel_path)?;
                    Some(Box::new(action))
                }
            };
            Ok(Action::Confirm {
                prompt,
                on_confirm: Box::new(on_confirm),
                on_cancel,
            })
        }
        // Phase 1537 — a bare node id, the `CommitLocal` shape above. It
        // addresses a node in THIS document, so there is nothing for a binding
        // to compute and no `TextSource` here.
        "Focus" => Ok(Action::Focus {
            node_id: req_string(path, fields, "nodeId", "focus target NodeId string")?,
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
            "Dispatch | Call | Notify | Navigate | SetState | AiTool | Chain | CommitLocal | WriteToClipboard | Print | Confirm | Focus | ReadFileBody | Invoke",
        )),
    }
}

/// Phase 1537 — fail when a `Confirm` is reachable from `action`. Confirmation
/// is bounded at ONE question: a dialogue that answers a dialogue is a modal
/// stack the reader cannot escape, and it expresses no intent a single question
/// does not.
///
/// It walks the DECODED action rather than raw JSON, and descends `Chain`,
/// because a chain is otherwise a hiding place — a check written against the
/// continuation's immediate `$type` passes a nested confirm one level down.
fn refuse_nested_confirm(action: &Action, path: &str) -> DResult<()> {
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
    let value = req_binding_slot_aliased(
        path,
        fields,
        "value",
        &["data"],
        "Metric value binding",
        StaticSlot::Float,
    )?;
    // Phase 460 — the stylistic fields are omitted-when-default on the wire.
    let format = opt_cell_format_default(path, fields, "format")?;
    let tone = opt_tone_default(path, fields, "tone")?;
    let weight = opt_weight_default(path, fields, "weight")?;
    let emphasis = opt_emphasis_default(path, fields, "emphasis")?;
    let trend = opt_binding_slot(path, fields, "trend", StaticSlot::Float)?;
    let trend_format = match get(fields, "trendFormat") {
        None => None,
        Some(v) => Some(decode_cell_format(&format!("{path}.trendFormat"), v)?),
    };
    // Phase 867 — the polarity declaration (§3.6.1). Decoded UNCONDITIONALLY,
    // not gated on `trend` being present: the slot is inert without a trend but
    // it is legal, and refusing it would reject a document the format admits.
    let trend_polarity = opt_trend_polarity_default(path, fields, "trendPolarity")?;
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
        trend_polarity,
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
    let value = req_binding_slot_aliased(
        path,
        fields,
        "value",
        &["data"],
        "row Binding<float> value",
        StaticSlot::Float,
    )?;
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
    let href = req_binding_slot(
        path,
        fields,
        "href",
        "link Binding<string> Href",
        StaticSlot::Str,
    )?;
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

/// Phase 1080 — one `srcSet` candidate (§3.6.4). `width` is the intrinsic pixel
/// width of this rendition and MUST be a POSITIVE INTEGER; zero and negative
/// values are a `WRONG_TYPE` at `<path>.width`, which is what the published
/// schema's `minimum: 1` says too, so the two expressions of the contract
/// agree. Zero is refused as firmly as a negative and that is the interesting
/// half: a `0w` descriptor is not a small image, it is a candidate a client can
/// never select, so admitting it would let the wire carry a rendition no host
/// can use.
fn decode_src_set_entry(path: &str, j: &JVal) -> DResult<SrcSetEntry> {
    let fields = as_obj(path, j)?;
    let src = req_binding_slot(
        path,
        fields,
        "src",
        "srcSet entry Binding<string> src",
        StaticSlot::Str,
    )?;
    let width_j = req(path, fields, "width", "positive intrinsic pixel width")?;
    match width_j {
        JVal::Num(n) if *n > 0.0 && n.fract() == 0.0 => Ok(SrcSetEntry {
            src,
            width: *n as i64,
        }),
        _ => Err(wrong_type(
            &format!("{path}.width"),
            "JSON number (positive integer pixel width)",
        )),
    }
}

fn decode_image_spec(path: &str, j: &JVal) -> DResult<ImageSpec> {
    let fields = as_obj(path, j)?;
    let alt = req_text_source(path, fields, "alt", "Image alt TextSource")?;
    let src = req_binding_slot(
        path,
        fields,
        "src",
        "Image Binding<string> Src",
        StaticSlot::Str,
    )?;
    let variant_j = req(path, fields, "variant", "ImageVariant")?;
    let variant = decode_image_variant(&format!("{path}.variant"), variant_j)?;
    // Phase 1077 — omitted-when-default on both boundaries; an absent slot
    // restores today's behaviour, which is what keeps every pre-phase document
    // decoding unchanged and re-encoding to the bytes it already had.
    let fit = match get(fields, "fit") {
        None => ImageFit::Natural,
        Some(v) => decode_image_fit(&format!("{path}.fit"), v)?,
    };
    let aspect_ratio = match get(fields, "aspectRatio") {
        None => ImageAspect::Natural,
        Some(v) => decode_image_aspect(&format!("{path}.aspectRatio"), v)?,
    };
    let loading = match get(fields, "loading") {
        None => ImageLoading::Eager,
        Some(v) => decode_image_loading(&format!("{path}.loading"), v)?,
    };
    // Phase 1078 — `caption` is optional CONTENT, so absent means ABSENT
    // (rule 4), not absent-means-a-default. `opt_text_source` is the same
    // decoder `alt` reaches through, which is what lets the §16 bare-string
    // shorthand and every `TextSource` case reach the slot with no
    // caption-specific rule.
    let caption = opt_text_source(path, fields, "caption")?;
    // Phase 1080 — the MISSING-LIST-FIELD decode class, and the one branch in
    // this decoder most worth reading. An ABSENT `srcSet` IS the empty list:
    // not `None`, not `null`, not an error. A PRESENT `null` is refused
    // (`WRONG_TYPE` at `$.kind.srcSet`) rather than read as absence — absence
    // already has a spelling, and admitting a second lets two conformant hosts
    // emit different canonical bytes for the same document. The authored ORDER
    // is preserved: a JSON array is ordered data, canonicalisation sorts object
    // KEYS only, and a codec that re-sorted here would emit bytes it did not
    // decode.
    let src_set = match get(fields, "srcSet") {
        None => Vec::new(),
        Some(v) => {
            let arr = as_arr(&format!("{path}.srcSet"), v)?;
            let mut entries = Vec::with_capacity(arr.len());
            for (i, entry) in arr.iter().enumerate() {
                entries.push(decode_src_set_entry(&format!("{path}.srcSet[{i}]"), entry)?);
            }
            entries
        }
    };
    // Phase 1079 — an ordinary omit-at-default bool: absent is `false`, and a
    // present non-boolean is a `WRONG_TYPE` rather than a truthiness coercion.
    // `"expandable":"true"` is an emitter guessing, and a decoder that guessed
    // back would have to rule on `"false"` and `""` as well — at which point
    // two conformant hosts can disagree about whether the document declares an
    // affordance at all.
    let expandable = opt_bool(path, fields, "expandable")?.unwrap_or(false);
    Ok(ImageSpec {
        alt,
        src,
        variant,
        fit,
        aspect_ratio,
        loading,
        caption,
        src_set,
        expandable,
    })
}

/// Phase 1076 — which media surface this is (§3.6.6). A `$type`-DISCRIMINATED
/// union, so an unknown case reports at `<path>.$type` (the `Binding` /
/// `TextSource` position) rather than at the bare slot, and the case set is
/// CLOSED at `Video | Audio` — admitting a third surface later is an addition,
/// never a re-meaning of shipped bytes.
///
/// `Audio` reads no field and refuses none either. What matters is that there
/// is no autoplay slot to read: `{"$type":"Audio","autoplay":true}` decodes to
/// an audio surface that does not autoplay, because the value has nowhere to
/// land.
fn decode_media_kind(path: &str, j: &JVal) -> DResult<MediaKind> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Video" => {
            // Omit-at-default bool, refused rather than coerced when present
            // and non-boolean — the `Image.expandable` ruling, on the slot
            // where getting it wrong starts playing a video the document says
            // not to.
            let autoplay = opt_bool(path, fields, "autoplay")?.unwrap_or(false);
            let poster = opt_binding_slot(path, fields, "poster", StaticSlot::Str)?;
            Ok(MediaKind::Video { autoplay, poster })
        }
        "Audio" => Ok(MediaKind::Audio),
        other => Err(unknown_du_case(path, other, "Video | Audio")),
    }
}

/// Phase 1076 — the media spec (§3.6.6). `label` is REQUIRED, which is the a11y
/// floor expressed where a decoder can enforce it: a transport has no
/// decorative case, and there is no value to default to that would not be a
/// fabricated name for someone else's recording. `controls` is the second
/// omit-at-TRUE slot in the vocabulary, so an absent key is the ACCESSIBLE
/// value and a document only spends a key to take the transport away.
fn decode_media_spec(path: &str, j: &JVal) -> DResult<MediaSpec> {
    let fields = as_obj(path, j)?;
    let src = req_binding_slot(
        path,
        fields,
        "src",
        "Media Binding<string> Src",
        StaticSlot::Str,
    )?;
    let label = req_text_source(path, fields, "label", "Media accessible label TextSource")?;
    let kind_j = req(path, fields, "kind", "MediaKind (Video | Audio)")?;
    let kind = decode_media_kind(&format!("{path}.kind"), kind_j)?;
    let controls = opt_bool(path, fields, "controls")?.unwrap_or(true);
    let r#loop = opt_bool(path, fields, "loop")?.unwrap_or(false);
    // Phase 1110 - the MISSING-LIST-FIELD class again (`srcSet`'s rule, one kind
    // over): an ABSENT `tracks` IS the empty list, never `None` and never null.
    // A PRESENT `null` goes through `as_arr` and is refused, because absence
    // already has a spelling and admitting a second lets two conformant hosts
    // emit different canonical bytes for the same document. The AUTHORED order
    // is preserved here and at render - a reader picks a track from a menu the
    // user agent builds in document order, so sorting it would be rewriting
    // somebody else's menu.
    let tracks = match get(fields, "tracks") {
        None => Vec::new(),
        Some(v) => {
            let arr = as_arr(&format!("{path}.tracks"), v)?;
            let mut out = Vec::with_capacity(arr.len());
            for (i, entry) in arr.iter().enumerate() {
                out.push(decode_track_entry(&format!("{path}.tracks[{i}]"), entry)?);
            }
            out
        }
    };
    // An ordinary optional: absent means the document offers no transcript,
    // which is a different statement from offering an empty one.
    let transcript = opt_text_source(path, fields, "transcript")?;
    Ok(MediaSpec {
        src,
        label,
        controls,
        r#loop,
        kind,
        tracks,
        transcript,
    })
}

/// Phase 1110 - one `TrackEntry` (WIRE_FORMAT.md 3.6.6), the strictest record on
/// the wire: four of five members REQUIRED.
///
/// `srcLang` is required on EVERY kind, where HTML makes it mandatory only on a
/// subtitles track - there is no value to default to that would not be an
/// invented claim about someone else's recording. The path carries the array
/// index the caller supplied, so a document with four tracks names the one at
/// fault.
///
/// `default` is the one omitted-at-`false` slot and is NOT truthiness-coerced: a
/// stringified boolean is `WRONG_TYPE`, at the position a host decoding array
/// elements with a looser walker than its records would get wrong.
fn decode_track_entry(path: &str, j: &JVal) -> DResult<TrackEntry> {
    let fields = as_obj(path, j)?;
    let kind_j = req(path, fields, "kind", "TrackKind")?;
    let kind = decode_track_kind(&format!("{path}.kind"), kind_j)?;
    let src = req_binding_slot(
        path,
        fields,
        "src",
        "TrackEntry Binding<string> Src",
        StaticSlot::Str,
    )?;
    let src_lang = req_string(path, fields, "srcLang", "TrackEntry srcLang string")?;
    let label = req_text_source(path, fields, "label", "TrackEntry label TextSource")?;
    let default = opt_bool(path, fields, "default")?.unwrap_or(false);
    Ok(TrackEntry {
        kind,
        src,
        src_lang,
        label,
        default,
    })
}

/// Phase 1111 - the sandboxed third-party embed (WIRE_FORMAT.md 3.6.8).
///
/// `title` is REQUIRED and refused when absent rather than defaulted: an
/// invented title is a claim about somebody else's document. `permissions` is
/// the empty list when absent, and empty means TOTAL DENIAL - so the
/// wire-cheapest document is also the most locked-down one.
fn decode_embed_spec(path: &str, j: &JVal) -> DResult<EmbedSpec> {
    let fields = as_obj(path, j)?;
    let src = req_binding_slot(
        path,
        fields,
        "src",
        "Embed Binding<string> Src",
        StaticSlot::Str,
    )?;
    let title = req_text_source(path, fields, "title", "Embed accessible title TextSource")?;
    let aspect_ratio = match get(fields, "aspectRatio") {
        None => ImageAspect::Natural,
        Some(v) => decode_image_aspect(&format!("{path}.aspectRatio"), v)?,
    };
    // An unrecognised permission is REFUSED, never dropped: dropping would turn
    // a document asking for something this vocabulary has no name for into a
    // document asking for LESS, which reads as success.
    let permissions = match get(fields, "permissions") {
        None => Vec::new(),
        Some(v) => {
            let arr = as_arr(&format!("{path}.permissions"), v)?;
            let mut out = Vec::with_capacity(arr.len());
            for (i, entry) in arr.iter().enumerate() {
                out.push(decode_embed_permission(
                    &format!("{path}.permissions[{i}]"),
                    entry,
                )?);
            }
            out
        }
    };
    Ok(EmbedSpec {
        src,
        title,
        aspect_ratio,
        permissions,
    })
}

/// Phase 1120 - `Tree` (WIRE_FORMAT.md 3.6.12).
fn decode_tree_spec(path: &str, j: &JVal) -> DResult<TreeSpec> {
    let fields = as_obj(path, j)?;
    let items_j = req(path, fields, "items", "Tree item list")?;
    let arr = as_arr(&format!("{path}.items"), items_j)?;
    let mut items = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        items.push(decode_tree_item(&format!("{path}.items[{i}]"), item)?);
    }
    Ok(TreeSpec {
        items,
        expanded_state_key: opt_string(path, fields, "expandedStateKey")?,
        selection_state_key: opt_string(path, fields, "selectionStateKey")?,
        on_select: opt_closure(fields, "onSelect"),
    })
}

/// Phase 1120 - one `TreeItem`, the format's first SELF-REFERENTIAL shape.
///
/// Item nesting is bounded on its OWN axis (WIRE_FORMAT.md 21.5): a whole
/// hierarchy lives inside one node, so it consumes no node depth at all, and the
/// `TreeOp::Batch` precedent is what fixes the ceiling at `MAX_NODE_DEPTH`
/// counted separately. The guard refuses on the way DOWN and pops in `Drop`,
/// which is what keeps the counter correct on this decoder's many `?` returns.
///
/// The child walk is THIS function, not a looser inline one: a host whose child
/// walker is looser than its root walker passes
/// `reject-tree-item-missing-{id,label}` and fails
/// `reject-tree-nested-item-missing-id` one level down.
fn decode_tree_item(path: &str, j: &JVal) -> DResult<TreeItem> {
    let _guard = crate::limits::TreeItemGuard::enter().map_err(|b| limit_error(path, b))?;

    let fields = as_obj(path, j)?;
    let id = req_string(path, fields, "id", "TreeItem id string")?;
    let label = req_text_source(path, fields, "label", "TreeItem label TextSource")?;
    let children = match get(fields, "children") {
        None => Vec::new(),
        Some(v) => {
            let arr = as_arr(&format!("{path}.children"), v)?;
            let mut out = Vec::with_capacity(arr.len());
            for (i, child) in arr.iter().enumerate() {
                out.push(decode_tree_item(&format!("{path}.children[{i}]"), child)?);
            }
            out
        }
    };
    let icon = opt_string(path, fields, "icon")?;
    Ok(TreeItem {
        id,
        label,
        children,
        icon,
    })
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
    let open = req_binding_slot(path, fields, "open", "Toast open binding", StaticSlot::Bool)?;
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

// Phase 1666 — `Skeleton.rows` is bounded by §21.9.
//
// `req_int` decides FIRST, so §7.1's slot rule is untouched: a fractional,
// non-finite or out-of-32-bit value is still a `WrongType` and never a limit
// breach. The bound then refuses a value the slot CAN hold but the format will
// not carry the work of — a renderer emits one placeholder row per count, so
// `{"rows":100000000}` names 10^8 rendered rows in a handful of bytes. The two
// codes answer different questions and the ORDER is what keeps them apart.
//
// Upper bound only, deliberately: a negative count is an authoring defect
// (`FUARAN150` in the pre-emit family), not a resource breach.
fn decode_skeleton_spec(path: &str, j: &JVal) -> DResult<SkeletonSpec> {
    let fields = as_obj(path, j)?;
    let rows = req_int(path, fields, "rows", "skeleton row count integer")?;
    if rows > crate::limits::MAX_SKELETON_ROWS {
        return Err(make_error(
            DecodeErrorCode::LimitExceeded,
            &format!("{path}.rows"),
            format!(
                "skeleton rows {rows} exceeds the maximum of {} (WIRE_FORMAT 21.9)",
                crate::limits::MAX_SKELETON_ROWS
            ),
            Some(format!(
                "at most {} rows on one Skeleton",
                crate::limits::MAX_SKELETON_ROWS
            )),
        ));
    }
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
    let fraction = req_binding_slot(
        path,
        fields,
        "fraction",
        "Progress fraction binding",
        StaticSlot::Float,
    )?;
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
            value: value_or(StaticSlot::Str, control_value_defaults::text())?,
            on_change,
        }),
        "Number" => Ok(FormFieldKind::Number {
            value: value_or(StaticSlot::Float, control_value_defaults::number())?,
            on_change,
        }),
        "Checkbox" => Ok(FormFieldKind::Checkbox {
            value: value_or(StaticSlot::Bool, control_value_defaults::checkbox())?,
            on_toggle,
        }),
        // Phase 766 — the switch affordance: Checkbox's mechanics under a
        // distinct tag.
        "Toggle" => Ok(FormFieldKind::Toggle {
            value: value_or(StaticSlot::Bool, control_value_defaults::checkbox())?,
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
        // Phase 1113 - the searchable form of `Choice`, sharing its value contract
        // exactly: a document migrating between the two changes its `$type` and
        // nothing else, so the auto-bind placeholder is `choice()`'s too.
        // `allowFreeText` is NOT truthiness-coerced - a lenient read would widen
        // the field on `"no"` and `"false"` alike.
        "Combobox" => Ok(FormFieldKind::Combobox {
            options: req_binding_slot(
                path,
                fields,
                "options",
                "Combobox options binding",
                StaticSlot::Options,
            )?,
            value: value_or(StaticSlot::StringOpt, control_value_defaults::choice())?,
            allow_free_text: opt_bool(path, fields, "allowFreeText")?.unwrap_or(false),
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
            value: value_or(StaticSlot::Float, control_value_defaults::number())?,
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
            value: value_or(StaticSlot::Str, control_value_defaults::text())?,
            on_change,
        }),
        "Date" => {
            let value = value_or(StaticSlot::Str, control_value_defaults::date())?;
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
        // Phase 1121 — the multi-token input. Every member is OPTIONAL, and
        // `allowFreeText` omits at TRUE (the opposite polarity to `Combobox`,
        // whose option source is required). The one decode refusal is the
        // control that cannot exist: free text denied AND no suggestion source,
        // so no gesture could ever put a token in. It is refused at
        // `allowFreeText` rather than at `suggestions`, because the member that
        // was written is the one that names the impossible state — an absent
        // `suggestions` is the ordinary open token box.
        "Tokens" => {
            let suggestions = opt_binding_slot(path, fields, "suggestions", StaticSlot::Options)?;
            let allow_free_text = opt_bool(path, fields, "allowFreeText")?.unwrap_or(true);
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
        "Rating" => {
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
                allow_half: opt_bool(path, fields, "allowHalf")?.unwrap_or(false),
                on_change,
            })
        }
        // Phase 1130 — the swatch. Only the STATIC case is judged here, and the
        // split is recorded rather than hidden: a State / Query / Selection
        // binding carries its text from outside the document, where a decoder
        // cannot see it. The same rule is owed by the pre-emit validator and by
        // the server-side submission floor.
        "Color" => {
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
        other => Err(unknown_du_case(
            path,
            other,
            &CANONICAL_FORM_FIELD_KINDS.join(" | "),
        )),
    }
}

/// The rule slot's rejected spellings. Small and enumerated for the same reason
/// the grid's set is: tolerance of unknown keys is right for a field a future
/// profile may add and wrong for a near miss of one that exists, because the tree
/// then decodes and renders while constraining nothing.
const FORM_FIELD_NEAR_MISSES: &[(&str, &str)] = &[
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
const FORM_FIELD_NEAR_MISS_CONSEQUENCE: &str = ", and the field would accept anything";

fn decode_compare_rule(path: &str, j: &JVal) -> DResult<CompareRule> {
    let fields = as_obj(path, j)?;
    let op = decode_compare_op(&format!("{path}.op"), req(path, fields, "op", "CompareOp")?)?;
    let against = req_binding(path, fields, "against", "against Binding")?;
    Ok(CompareRule { op, against })
}

/// A field's declared constraint. Every slot is optional structurally, and two
/// shapes are refused here as POLICY:
///
/// - a rule with every slot absent. A rule that constrains nothing is a defect,
///   not a no-op: it decodes, validates and renders while declaring nothing,
///   which is the fake-affordance shape the near-miss table also forecloses,
///   arriving through an empty object instead of a wrong key. `message` alone does
///   not rescue it — the message is the prose shown when some OTHER slot is unmet,
///   so a message-only rule is the help-text failure wearing the new vocabulary's
///   clothes.
/// - `minLength` above `maxLength`. The ordered-pair rule applied to a length
///   pair: an inverted bound admits no value at all, so the field can never be
///   submitted and the form is dead on arrival.
///
/// Neither is a shape — both are relations BETWEEN slots — which is why they live
/// here rather than in the structural layer.
fn decode_field_rule(path: &str, j: &JVal) -> DResult<FieldRule> {
    let fields = as_obj(path, j)?;
    let format = match get(fields, "format") {
        Some(v) => Some(decode_text_format(&format!("{path}.format"), v)?),
        None => None,
    };
    let pattern = opt_string(path, fields, "pattern")?;
    let min_length = opt_int(path, fields, "minLength")?;
    let max_length = opt_int(path, fields, "maxLength")?;
    let compare = match get(fields, "compare") {
        Some(v) => Some(decode_compare_rule(&format!("{path}.compare"), v)?),
        None => None,
    };
    let message = opt_text_source(path, fields, "message")?;

    if format.is_none()
        && pattern.is_none()
        && min_length.is_none()
        && max_length.is_none()
        && compare.is_none()
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

    if let (Some(lo), Some(hi)) = (min_length, max_length)
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

    Ok(FieldRule {
        format,
        pattern,
        min_length,
        max_length,
        compare,
        message,
    })
}

fn decode_form_field(path: &str, j: &JVal) -> DResult<FormField> {
    let fields = as_obj(path, j)?;
    // The near-miss check runs BEFORE the rule decode, so a field carrying both
    // `validation` and a well-formed `rule` still names the ignored key.
    //
    // The vocabulary LABEL is `form`, not `form field` — Phase 1659. The reference
    // hosts say "is not part of the form vocabulary" and this host said "the form
    // field vocabulary": terser, not wrong, and invisible to every gate, because an
    // op-side reject fixture pins the code and the path and never the prose. A
    // didactic message that reads differently on two hosts sends two authors to two
    // documents for one defect, which is the whole failure the corpus's
    // message-parity contract exists to prevent one level up.
    check_near_misses_in(
        path,
        fields,
        FORM_FIELD_NEAR_MISSES,
        "form",
        FORM_FIELD_NEAR_MISS_CONSEQUENCE,
    )?;
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
    let rule = match get(fields, "rule") {
        Some(v) => Some(decode_field_rule(&format!("{path}.rule"), v)?),
        None => None,
    };
    Ok(FormField {
        id,
        kind,
        label,
        required,
        help,
        rule,
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
    let disabled = opt_binding_slot(path, obj, "disabled", StaticSlot::Bool)?;
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
    let disabled = opt_binding_slot(path, fields, "disabled", StaticSlot::Bool)?;
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
    let disabled = opt_binding_slot(path, fields, "disabled", StaticSlot::Bool)?;
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
    let disabled = opt_binding_slot(path, fields, "disabled", StaticSlot::Bool)?;
    // Phase 1115 - two ADDITIONAL ingress routes. Absent reads as `false`; a
    // present member of any other type is `WRONG_TYPE` and MUST NOT be coerced,
    // because the slot decides whether a whole ingress route exists and a
    // lenient truthiness read would open a drop target on `"no"` and `"false"`
    // alike.
    let drop_target = opt_bool(path, fields, "dropTarget")?.unwrap_or(false);
    let accept_paste = opt_bool(path, fields, "acceptPaste")?.unwrap_or(false);
    // Phase 1116 — OPTIONAL, not omit-at-default: an absent member asks for the
    // ordinary picker, which is not one of the two devices wearing a default.
    let capture = match get(fields, "capture") {
        None => None,
        Some(v) => Some(decode_capture_source(&format!("{path}.capture"), v)?),
    };
    // Phase 1117 — the empty string is a name no host registers, so a document
    // carrying it describes an upload that can never stream. Refused rather
    // than read as absence: that coercion silently turns an upload the author
    // meant to stream into a client-only one, while every visible thing about
    // the control still works.
    let destination = match opt_string(path, fields, "destination")? {
        Some(name) if name.is_empty() => {
            return Err(wrong_type(
                &format!("{path}.destination"),
                "a registered destination name — an absent member is already the spelling for \
                 an upload that streams nowhere",
            ));
        }
        other => other,
    };
    // Phase 1548 — the two declared ceilings (§3.6.23). Absent declares no
    // ceiling, which is the pre-1548 control and the wire identity. Both go
    // through `opt_int` FIRST, so §7.1's slot rule decides the shape — a
    // fractional value is not truncated, and a value beyond the signed 32-bit
    // slot is refused naming the slot's range rather than wrapped into one the
    // author never wrote — and only then does the positive floor decide the
    // sign. Zero is refused as firmly as a negative: a ceiling of zero is not a
    // small ceiling but a control that can accept nothing, and the author who
    // means "no ceiling" omits the member.
    let ceiling = |key: &str, n: Option<i64>| -> DResult<Option<i64>> {
        match n {
            Some(v) if v <= 0 => Err(wrong_type(
                &format!("{path}.{key}"),
                "a positive integer ceiling — an absent member is already the spelling for \
                 an upload that declares no ceiling",
            )),
            other => Ok(other),
        }
    };
    let max_bytes = ceiling("maxBytes", opt_int(path, fields, "maxBytes")?)?;
    let max_files = ceiling("maxFiles", opt_int(path, fields, "maxFiles")?)?;
    Ok(FileUploadSpec {
        accept,
        label,
        multiple,
        disabled,
        drop_target,
        accept_paste,
        capture,
        destination,
        max_bytes,
        max_files,
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
fn retired_positional_field(path: &str, fields: &Fields, name: &str, op_kind: &str) -> DResult<()> {
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

fn check_near_misses(path: &str, fields: &Fields, candidates: &[(&str, &str)]) -> DResult<()> {
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
fn check_near_misses_in(
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
const A11Y_NEAR_MISSES: &[(&str, &str)] = &[
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
const A11Y_NEAR_MISS_CONSEQUENCE: &str =
    ", and the intent would reach assistive technology as nothing at all";

/// Named by the census row itself. Deliberately NOT aliased to `editable:
/// false`: an inverting alias that guesses wrong makes a read-only column
/// editable.
const COLUMN_NEAR_MISSES: &[(&str, &str)] = &[(
    "readOnly",
    "editable: false — the column flag NARROWS the grid's editable capability",
)];

/// The grid-level rejected spellings, walked in declaration order so which
/// defect surfaces first is deterministic across hosts.
const GRID_NEAR_MISSES: &[(&str, &str)] = &[
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

fn decode_column_erased(path: &str, j: &JVal) -> DResult<ColumnErased> {
    let fields = as_obj(path, j)?;
    check_near_misses(path, fields, COLUMN_NEAR_MISSES)?;
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
    // Phase 861 / 863 — per-column sort and editability NARROWING; absent
    // inherits the grid-level flag, so an explicit `false` is carried rather
    // than omitted-when-default (omitting it would erase the narrowing).
    let sortable = opt_bool(path, fields, "sortable")?;
    let editable = opt_bool(path, fields, "editable")?;
    Ok(ColumnErased {
        format,
        kind,
        label,
        width,
        value: opt_closure(fields, "value"),
        field,
        sortable,
        editable,
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
    // Phase 862 — declarative pagination. `pageSize` is how many rows a page
    // holds; a page of zero or fewer rows names no page at all, so it is
    // WRONG_TYPE — which is also what schema.json's `minimum: 1` says. The pager
    // that writes `pageStateKey` is renderer-owned, so nothing here decodes a
    // control.
    let page_size = match opt_int(path, fields, "pageSize")? {
        None => None,
        Some(n) if n >= 1 => Some(n),
        Some(_) => {
            return Err(wrong_type(
                &format!("{path}.pageSize"),
                "JSON number (integer page size of 1 or more)",
            ));
        }
    };
    // Phase 861 — the bound path's declared initial order, decoded by the SAME
    // function the staticRows spelling uses: same record, same bound, same
    // message at a different path.
    let default_sort = match get(fields, "defaultSort") {
        None => None,
        Some(v) => Some(decode_default_sort(&format!("{path}.defaultSort"), v)?),
    };
    check_near_misses(path, fields, GRID_NEAR_MISSES)?;
    // Phase 863 — the declared edit destination.
    let edit_state_key = opt_string(path, fields, "editStateKey")?;
    let page_state_key = opt_string(path, fields, "pageStateKey")?;
    // Phase 934 — omitted-when-default (false), the same convention as
    // `editable`.
    let reorderable = opt_bool(path, fields, "reorderable")?.unwrap_or(false);
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
        default_sort,
        page_size,
        page_state_key,
        edit_state_key,
        reorderable,
        static_rows,
        // Phase 1473 — the DataGrid arm of the same refusal.
        keep_rows_together: opt_bool(path, fields, "keepRowsTogether")?.unwrap_or(false),
        repeat_header: opt_bool(path, fields, "repeatHeader")?.unwrap_or(false),
        // Phase 1123 — a bool, omitted at `false`, never truthiness-coerced:
        // the slot decides whether a whole affordance exists.
        exportable: opt_bool(path, fields, "exportable")?.unwrap_or(false),
        // Phase 1125 — separate decoder arms, so a wrong type on either is
        // reported at its own path.
        transfer_in_key: opt_string(path, fields, "transferInKey")?,
        transfer_out_key: opt_string(path, fields, "transferOutKey")?,
    })
}

/// `true` when `text` is a canonical ISO-8601 date the temporal axis can place —
/// `YYYY-MM-DD`, optionally followed by `T…` whose time-of-day is discarded.
///
/// STRICT by shape AND by calendar: four digits, two, two, both hyphens, a month
/// in 1–12 and a day the month actually has. A locale spelling (`15/01/2026`) and
/// a bare year are both refused — admitting either would be the string-sniffing
/// the temporal axis exists to avoid.
fn is_canonical_iso_day(text: &str) -> bool {
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

/// An annotation's X ADDRESS (Phase 1491, §4l "The three addressing forms").
///
/// THE DATE MUST BE A DATE, and this refusal is the twin of `ReferenceLine`'s
/// finite-value narrowing rather than a new posture. The lowering's calendar is
/// deliberately TOTAL — an unparseable x CELL reads as 1970-01-01, because
/// FUARAN097 makes a non-date COLUMN loud upstream and refusing per-cell would be
/// worse. An annotation has no column to be loud about: the string is authored
/// directly, so nothing upstream can catch it. And because §4l rule 3 has a
/// temporal address ENTER the axis extent before the ticks are chosen, a typo does
/// not misplace one marker — it drags the domain back to the epoch and rescales
/// the whole picture.
fn decode_chart_annotation_x(path: &str, j: &JVal) -> DResult<ChartAnnotationX> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Category" => Ok(ChartAnnotationX::Category(req_string(
            path,
            fields,
            "key",
            "category key (the band's own label)",
        )?)),
        "Date" => {
            let iso = req_string(path, fields, "iso", "ISO-8601 date (YYYY-MM-DD)")?;
            if !is_canonical_iso_day(&iso) {
                return Err(wrong_type(
                    &format!("{path}.iso"),
                    "a canonical ISO-8601 date (YYYY-MM-DD, optionally followed by a time) naming a real calendar day — an event marker's date is the address it is drawn at, and an unreadable one would place the marker at 1970-01-01 and drag the axis back with it",
                ));
            }
            Ok(ChartAnnotationX::Date(iso))
        }
        other => Err(unknown_du_case(path, other, "Category, Date")),
    }
}

/// A range band's PAIR (Phase 1492, §4l). The case carries the AXIS as well as
/// the pair, so a value axis addressed by category keys is not a document this
/// decoder has to refuse — it is one no encoder can write.
///
/// TWO REFUSALS, and they are the pair rules the WIRE can decide by itself. A
/// non-finite endpoint is `ReferenceLine`'s narrowing at two slots instead of one,
/// for its reason exactly: §4l rule 3 has both ends enter the value domain, so a
/// NaN takes the nice-domain, every gridline and every mark with it. An UNORDERED
/// pair is refused at the pair's own slot — the defect is the pair's, not either
/// end's — rather than silently swapped.
///
/// A CATEGORY pair's order is NOT decided here: the order of two band keys is the
/// ROWS' order, a cross-reference rather than a local property of the address.
fn decode_chart_annotation_range(path: &str, j: &JVal) -> DResult<ChartAnnotationRange> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "ValueRange" => {
            let from = req_float(
                path,
                fields,
                "from",
                "range-band lower value (a finite JSON number)",
            )?;
            let to = req_float(
                path,
                fields,
                "to",
                "range-band upper value (a finite JSON number)",
            )?;
            for (slot, v) in [("from", from), ("to", to)] {
                if !v.is_finite() {
                    return Err(wrong_type(
                        &format!("{path}.{slot}"),
                        "a FINITE JSON number — a range band's end names a place on the value axis, and NaN / Infinity names none; give the value in the axis's own units, or drop the annotation",
                    ));
                }
            }
            if from > to {
                return Err(wrong_type(
                    path,
                    "an ORDERED pair — a range band runs from its lower value to its upper one, and this pair runs backwards; swapping the ends silently would draw a band the author did not describe",
                ));
            }
            Ok(ChartAnnotationRange::ValueRange { from, to })
        }
        "XRange" => {
            let from_j = req(
                path,
                fields,
                "from",
                "range-band lower x address (a ChartAnnotationX)",
            )?;
            let from = decode_chart_annotation_x(&format!("{path}.from"), from_j)?;
            let to_j = req(
                path,
                fields,
                "to",
                "range-band upper x address (a ChartAnnotationX)",
            )?;
            let to = decode_chart_annotation_x(&format!("{path}.to"), to_j)?;
            // Both dates are already known canonical and calendar-valid (the
            // address decoder refused anything else), and a canonical
            // `YYYY-MM-DD` sorts lexicographically exactly as it sorts
            // chronologically — so no calendar arithmetic is needed here.
            if let (ChartAnnotationX::Date(a), ChartAnnotationX::Date(b)) = (&from, &to) {
                if a > b {
                    return Err(wrong_type(
                        path,
                        "an ORDERED pair — a range band runs from its earlier date to its later one, and this pair runs backwards; swapping the ends silently would draw a band the author did not describe",
                    ));
                }
            }
            Ok(ChartAnnotationRange::XRange { from, to })
        }
        other => Err(unknown_du_case(path, other, "ValueRange, XRange")),
    }
}

/// A chart's data-addressed annotation (Phase 1490, §4l).
///
/// THE REFERENCE LINE'S VALUE MUST BE FINITE, and that is a slot-specific
/// NARROWING of §7 rather than a disagreement with it. §7 admits the quoted
/// `"NaN"` / `"Infinity"` / `"-Infinity"` sentinels at every float slot and
/// `as_float` reads them — the widening is deliberate and stays. But a reference
/// line addresses a place on the VALUE AXIS, and a non-finite value names no such
/// place: it would enter the domain computation and put every gridline, tick and
/// mark at a NaN coordinate. The picture is not merely wrong at the annotation, it
/// is wrong everywhere, and nothing downstream can recover it.
fn decode_chart_annotation(path: &str, j: &JVal) -> DResult<ChartAnnotation> {
    let fields = as_obj(path, j)?;
    let label = opt_text_source(path, fields, "label")?;
    match disc(path, fields)? {
        "ReferenceLine" => {
            let value = req_float(
                path,
                fields,
                "value",
                "reference-line value (a finite JSON number)",
            )?;
            if !value.is_finite() {
                return Err(wrong_type(
                    &format!("{path}.value"),
                    "a FINITE JSON number — a reference line names a place on the value axis, and NaN / Infinity names none; give the value in the axis's own units, or drop the annotation",
                ));
            }
            Ok(ChartAnnotation::ReferenceLine { value, label })
        }
        "EventMarker" => {
            let at_j = req(
                path,
                fields,
                "at",
                "event-marker x address (a ChartAnnotationX)",
            )?;
            let at = decode_chart_annotation_x(&format!("{path}.at"), at_j)?;
            Ok(ChartAnnotation::EventMarker { at, label })
        }
        "RangeBand" => {
            let range_j = req(
                path,
                fields,
                "range",
                "range-band pair (a ChartAnnotationRange)",
            )?;
            let range = decode_chart_annotation_range(&format!("{path}.range"), range_j)?;
            Ok(ChartAnnotation::RangeBand { range, label })
        }
        other => Err(unknown_du_case(
            path,
            other,
            "ReferenceLine, EventMarker, RangeBand",
        )),
    }
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
    // `stacked` round-trips (carried since the fixture corpus pinned it).
    // Absent restores `false` BY CONTRACT since Phase 1585 made the member
    // omit-at-default — not, as this note read until then, as tolerance of the
    // legacy wire that predated the field.
    let stacked = opt_bool(path, fields, "stacked")?.unwrap_or(false);
    // Phase 876 — `valueFormat`: the value axis's number format, reusing the
    // existing `Format` vocabulary. Absent is the ordinary shape.
    let value_format = match get(fields, "valueFormat") {
        None => None,
        Some(j) => Some(decode_format(&format!("{path}.valueFormat"), j)?),
    };
    // Phase 878 — the axis names + the subtitle. All optional; absent is the
    // ordinary shape, and the lowering supplies the field-name fallback.
    let x_title = opt_text_source(path, fields, "xTitle")?;
    let y_title = opt_text_source(path, fields, "yTitle")?;
    let subtitle = opt_text_source(path, fields, "subtitle")?;
    // Phase 880 — the legend's placement. Absent is the ordinary shape and means
    // the host's default (`Right`), never "no legend": suppression is the explicit
    // `None` case, which is why this stays an `Option` rather than defaulting here.
    let legend_position = match get(fields, "legendPosition") {
        None => None,
        Some(j) => Some(decode_chart_legend_position(
            &format!("{path}.legendPosition"),
            j,
        )?),
    };
    // Phase 881 — whether the values are written onto the picture. Absent is the
    // ordinary shape and means `Off`, which is ALSO the default, so a pre-881 tree
    // lowers to the pre-881 picture byte-for-byte.
    let data_labels = match get(fields, "dataLabels") {
        None => None,
        Some(j) => Some(decode_chart_data_labels(&format!("{path}.dataLabels"), j)?),
    };
    // Phase 882 — what the x column MEANS: discrete `Category` bands or `Temporal`
    // dates on a continuous day-scale. Absent means `Category`, which is ALSO the
    // default, so the ordinary wire shape omits the key and lowers to the pre-882
    // picture byte-for-byte. A `Temporal` declaration is GROUNDED pre-emit
    // (FUARAN097) rather than second-guessed here: the decoder's job is to carry
    // the author's claim faithfully, not to check it against the rows.
    let x_scale = match get(fields, "xScale") {
        None => None,
        Some(j) => Some(decode_chart_x_scale(&format!("{path}.xScale"), j)?),
    };
    // Phase 1490 — `annotations` (§4l): the data-addressed attachments — reference
    // lines, event markers and range bands — as one closed union, so a further
    // member is a case rather than a further widening of this record. Absent OMITS
    // on the wire, so every pre-1490 document decodes and lowers byte-for-byte as
    // it did. An EMPTY list is a different document from an absent field and is
    // carried as such: it round-trips to `"annotations":[]`, which is what an
    // author who declared a list and then removed its last member wrote.
    let annotations = match get(fields, "annotations") {
        None => None,
        Some(j) => {
            let items = as_arr(&format!("{path}.annotations"), j)?;
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                out.push(decode_chart_annotation(
                    &format!("{path}.annotations[{i}]"),
                    item,
                )?);
            }
            Some(out)
        }
    };
    Ok(ChartSpec {
        kind,
        source,
        stacked,
        x_field,
        y_fields,
        title,
        value_format,
        x_title,
        y_title,
        subtitle,
        legend_position,
        data_labels,
        x_scale,
        annotations,
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
        fill: opt_binding_slot(path, fields, "fill", StaticSlot::Str)?,
        stroke: opt_binding_slot(path, fields, "stroke", StaticSlot::Str)?,
        stroke_width: opt_binding_slot(path, fields, "strokeWidth", StaticSlot::Float)?,
        opacity: opt_binding_slot(path, fields, "opacity", StaticSlot::Float)?,
        text_anchor,
        font_size: opt_float(path, fields, "fontSize")?,
        emphasis,
        font_family: opt_string(path, fields, "fontFamily")?,
        // Phase 642 — keyed mark identity; omitted-when-None.
        mark_id: opt_string(path, fields, "markId")?,
        // Phase 877 — Label text rotation in degrees; optional, no default.
        rotation: opt_float(path, fields, "rotation")?,
        // Phase 883 — the per-mark hover readout, a full TextSource (so a
        // `Bound` envelope decodes here as well as the canonical bare-string
        // `Literal`); optional, absent = untipped.
        tip: opt_text_source(path, fields, "tip")?,
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
        "Masonry" => {
            // WIRE_FORMAT §3.6.7 — column-fill. `cols` is REQUIRED and
            // POSITIVE, on the §3.6.4 srcSet width-floor pattern:
            // `column-count: 0` is invalid CSS, so a container declaring it
            // would fall back to whatever the host stylesheet last said and the
            // wire would be carrying a host-defined layout.
            //
            // No auto-column leniency here, unlike `Grid` above, and the
            // asymmetry is deliberate rather than an omission: `Grid`
            // canonicalises a column-less spec to `Auto` because the language
            // already owns that concept, whereas `Auto` is a ROW-fill mode —
            // rewriting a masonry into it would discard the author's intent
            // rather than recover it. Mirror of F# / fuaran-ts.
            let cols_j = get_aliased(fields, "cols", &["columns"])
                .ok_or_else(|| missing_field(path, "cols", "positive integer column count"))?;
            let cols = as_int(&format!("{path}.cols"), cols_j)?;
            if cols <= 0 {
                return Err(wrong_type(
                    &format!("{path}.cols"),
                    "JSON number (positive integer column count)",
                ));
            }
            Ok(BoxLayout::Masonry {
                cols,
                gap: opt_int(path, fields, "gap")?,
            })
        }
        "Auto" => Ok(BoxLayout::Auto),
        other => Err(unknown_du_case(path, other, "Flex | Grid | Masonry | Auto")),
    }
}

fn decode_box_role(path: &str, j: &JVal) -> DResult<BoxRole> {
    let s = as_str(path, j)?;
    BoxRole::from_wire(s)
        .ok_or_else(|| unknown_enum_case(path, s, "Group | Card | Dashboard | Separator"))
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
        // Phase 1473 — refused, never coerced: a document that meant `true` and
        // wrote `"true"` would otherwise render with its declaration silently
        // dropped, which is exactly the split block the member exists to
        // prevent. Pinned on BOTH decoder arms the vocabulary reaches, because
        // they are separate branches and a vector on one proves nothing about
        // the other.
        keep_together: opt_bool(path, fields, "keepTogether")?.unwrap_or(false),
        break_before: opt_bool(path, fields, "breakBefore")?.unwrap_or(false),
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
    let disabled = opt_binding_slot(path, fields, "disabled", StaticSlot::Bool)?;
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
    let active_tag = opt_binding_slot(path, fields, "activeTag", StaticSlot::Str)?;
    // `activeIndex` round-trips. Absent restores `Static 0` BY CONTRACT since
    // Phase 1585 made the member omit-at-default — not, as this note read until
    // then, as tolerance of the legacy wire that predated the field.
    let active_index = match get(fields, "activeIndex") {
        None => Binding::Static {
            value: StaticValue::Ast(JVal::Num(0.0)),
        },
        Some(v) => decode_binding_slot(&format!("{path}.activeIndex"), v, StaticSlot::Int)?,
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
    let active_step = req_binding_slot(
        path,
        fields,
        "activeStep",
        "activeStep binding",
        StaticSlot::Int,
    )?;
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
    let open = req_binding_slot(path, fields, "open", "open binding", StaticSlot::Bool)?;
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
    let open = req_binding_slot(path, fields, "open", "open binding", StaticSlot::Bool)?;
    // Field alias: title → heading.
    let heading = opt_text_source_aliased(path, fields, "heading", &["title"])?;
    // WIRE_FORMAT.md 3.6.11 - omitted at `Modal`. A non-string is `WRONG_TYPE`
    // and an unrecognised spelling `UNKNOWN_DU_CASE`, both at `$.kind.modality`
    // with no `.$type` suffix: this is a BARE enum.
    let modality = match get(fields, "modality") {
        None => ModalityKind::Modal,
        Some(v) => decode_modality_kind(&format!("{path}.modality"), v)?,
    };
    Ok(ModalSpec {
        children,
        dismissable,
        open,
        on_dismiss,
        heading,
        modality,
        anchor: opt_string(path, fields, "anchor")?,
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
        unknown_enum_case(
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
        unknown_enum_case(
            &format!("{path}.hostEffect"),
            &host,
            "Pure | ReadsHost | WritesHost",
        )
    })?;
    let det = req_string(path, fields, "determinism", "EffectClass determinism")?;
    let determinism = DeterminismSource::from_wire(&det).ok_or_else(|| {
        unknown_enum_case(
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

/// Decode a `NodeKind` from its `$type`-discriminated object.
///
/// # Why this is a chain of small functions rather than one match
///
/// It WAS one match over the whole kind vocabulary, and that shape cost this
/// host its conformance. In a debug build a function's frame reserves space for
/// every local across all its branches, so a match carrying one spec temporary
/// per arm held the entire vocabulary's worth of stack at EVERY level of the
/// recursion. Measured: ~128 KB per node level, which overflows the default
/// 1 MB main-thread stack at 8 levels — below `MAX_NODE_DEPTH` of 24. So the
/// unoptimised build ABORTED on documents `WIRE_FORMAT.md` §21.2 rule 1
/// requires every conformant host to ACCEPT.
///
/// A Rust stack overflow is not catchable, so no depth guard could have rescued
/// that: the guard at 24 was never reached, because the process was already gone
/// at 9. Shrinking the frame was the prerequisite for the guard, not a tidy-up
/// beside it.
///
/// The groups below are called in SEQUENCE, not nested, so only one group's
/// slots are live at a time. Each returns `None` for a tag it does not own and
/// the next is tried. The split is load-bearing: keep the groups small when
/// adding a kind, and do not merge them back into one match.
fn decode_node_kind(path: &str, j: &JVal) -> DResult<NodeKind> {
    let fields = as_obj(path, j)?;
    let tag = disc(path, fields)?;
    if let Some(r) = decode_node_kind_g0(tag, path, j, fields) {
        return r;
    }
    if let Some(r) = decode_node_kind_g1(tag, path, j, fields) {
        return r;
    }
    if let Some(r) = decode_node_kind_g2(tag, path, j, fields) {
        return r;
    }
    if let Some(r) = decode_node_kind_g3(tag, path, j, fields) {
        return r;
    }
    if let Some(r) = decode_node_kind_g4(tag, path, j, fields) {
        return r;
    }
    let other = tag;
    Err(make_error(
        DecodeErrorCode::WrongNodeKind,
        format!("{path}.$type"),
        format!("unknown NodeKind discriminator '{other}'"),
        Some(WRONG_NODE_KIND_HINT.to_string()),
    ))
}

/// One group of the `NodeKind` dispatch — see `decode_node_kind` for why the
/// dispatch is split. Returns `None` for a tag this group does not own.
fn decode_node_kind_g0(
    tag: &str,
    path: &str,
    j: &JVal,
    fields: &Fields,
) -> Option<DResult<NodeKind>> {
    let _ = fields;
    Some(match tag {
        // Layout.
        "Box" => (|| -> DResult<NodeKind> { Ok(NodeKind::Box(decode_box_spec(path, j)?)) })(),
        "SplitPanel" => (|| -> DResult<NodeKind> {
            Ok(NodeKind::SplitPanel(decode_split_panel_spec(path, j)?))
        })(),
        "Tabs" => (|| -> DResult<NodeKind> { Ok(NodeKind::Tabs(decode_tabs_spec(path, j)?)) })(),
        "Stepper" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Stepper(decode_stepper_spec(path, j)?)) })()
        }
        "SummaryList" => (|| -> DResult<NodeKind> {
            Ok(NodeKind::SummaryList(decode_summary_list_spec(path, j)?))
        })(),
        "Disclosure" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Disclosure(decode_disclosure_spec(path, j)?)) })(
            )
        }
        "Modal" => (|| -> DResult<NodeKind> { Ok(NodeKind::Modal(decode_modal_spec(path, j)?)) })(),
        _ => return None,
    })
}

/// One group of the `NodeKind` dispatch — see `decode_node_kind` for why the
/// dispatch is split. Returns `None` for a tag this group does not own.
fn decode_node_kind_g1(
    tag: &str,
    path: &str,
    j: &JVal,
    fields: &Fields,
) -> Option<DResult<NodeKind>> {
    let _ = fields;
    Some(match tag {
        // Display.
        "ScrollArea" => (|| -> DResult<NodeKind> {
            Ok(NodeKind::ScrollArea(decode_scroll_area_spec(path, j)?))
        })(),
        "Heading" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Heading(decode_heading_spec(path, j)?)) })()
        }
        "Markdown" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Markdown(decode_markdown_spec(path, j)?)) })()
        }
        "Metric" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Metric(decode_metric_spec(path, j)?)) })()
        }
        "Badge" => (|| -> DResult<NodeKind> { Ok(NodeKind::Badge(decode_badge_spec(path, j)?)) })(),
        "Sparkline" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Sparkline(decode_sparkline_spec(path, j)?)) })()
        }
        "Callout" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Callout(decode_callout_spec(path, j)?)) })()
        }
        "Progress" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Progress(decode_progress_spec(path, j)?)) })()
        }
        "Skeleton" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Skeleton(decode_skeleton_spec(path, j)?)) })()
        }
        "Icon" => (|| -> DResult<NodeKind> { Ok(NodeKind::Icon(decode_icon_spec(path, j)?)) })(),
        _ => return None,
    })
}

/// One group of the `NodeKind` dispatch — see `decode_node_kind` for why the
/// dispatch is split. Returns `None` for a tag this group does not own.
fn decode_node_kind_g2(
    tag: &str,
    path: &str,
    j: &JVal,
    fields: &Fields,
) -> Option<DResult<NodeKind>> {
    let _ = fields;
    Some(match tag {
        "Fact" => (|| -> DResult<NodeKind> { Ok(NodeKind::Fact(decode_fact_spec(path, j)?)) })(),
        "LabelValueRow" => (|| -> DResult<NodeKind> {
            Ok(NodeKind::LabelValueRow(decode_label_value_row_spec(
                path, j,
            )?))
        })(),
        "Link" => (|| -> DResult<NodeKind> { Ok(NodeKind::Link(decode_link_spec(path, j)?)) })(),
        "Image" => (|| -> DResult<NodeKind> { Ok(NodeKind::Image(decode_image_spec(path, j)?)) })(),
        "Media" => (|| -> DResult<NodeKind> { Ok(NodeKind::Media(decode_media_spec(path, j)?)) })(),
        "Embed" => (|| -> DResult<NodeKind> { Ok(NodeKind::Embed(decode_embed_spec(path, j)?)) })(),
        "Tree" => (|| -> DResult<NodeKind> { Ok(NodeKind::Tree(decode_tree_spec(path, j)?)) })(),
        "List" => (|| -> DResult<NodeKind> { Ok(NodeKind::List(decode_list_spec(path, j)?)) })(),
        "Toast" => (|| -> DResult<NodeKind> { Ok(NodeKind::Toast(decode_toast_spec(path, j)?)) })(),
        "CodeBlock" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::CodeBlock(decode_code_block_spec(path, j)?)) })(
            )
        }
        "Math" => (|| -> DResult<NodeKind> { Ok(NodeKind::Math(decode_math_spec(path, j)?)) })(),
        _ => return None,
    })
}

/// One group of the `NodeKind` dispatch — see `decode_node_kind` for why the
/// dispatch is split. Returns `None` for a tag this group does not own.
fn decode_node_kind_g3(
    tag: &str,
    path: &str,
    j: &JVal,
    fields: &Fields,
) -> Option<DResult<NodeKind>> {
    let _ = fields;
    Some(match tag {
        // Input.
        "Drawing" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Drawing(decode_drawing_spec(path, j)?)) })()
        }
        "Form" => (|| -> DResult<NodeKind> { Ok(NodeKind::Form(decode_form_spec(path, j)?)) })(),
        "Filters" => (|| -> DResult<NodeKind> {
            {
                let items_j = req(path, fields, "items", "Filters item list")?;
                let arr = as_arr(&format!("{path}.items"), items_j)?;
                let mut specs = Vec::with_capacity(arr.len());
                for (i, item) in arr.iter().enumerate() {
                    specs.push(decode_filter_spec(&format!("{path}.items[{i}]"), item)?);
                }
                Ok(NodeKind::Filters(specs))
            }
        })(),
        "Button" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Button(decode_button_spec(path, j)?)) })()
        }
        "FileUpload" => (|| -> DResult<NodeKind> {
            Ok(NodeKind::FileUpload(decode_file_upload_spec(path, j)?))
        })(),
        _ => return None,
    })
}

/// One group of the `NodeKind` dispatch — see `decode_node_kind` for why the
/// dispatch is split. Returns `None` for a tag this group does not own.
fn decode_node_kind_g4(
    tag: &str,
    path: &str,
    j: &JVal,
    fields: &Fields,
) -> Option<DResult<NodeKind>> {
    let _ = fields;
    Some(match tag {
        // Visualisation.
        "Select" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::Select(decode_select_spec(path, j)?)) })()
        }
        "DataGrid" => {
            (|| -> DResult<NodeKind> { Ok(NodeKind::DataGrid(decode_grid_spec(path, j)?)) })()
        }
        "Chart" => (|| -> DResult<NodeKind> { Ok(NodeKind::Chart(decode_chart_spec(path, j)?)) })(),
        // Structural.
        "Map" => (|| -> DResult<NodeKind> { Ok(NodeKind::Map(decode_map_spec(path, j)?)) })(),
        "Custom" => (|| -> DResult<NodeKind> {
            {
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
                            ids.push(
                                as_str(&format!("{path}.exposedNodeIds[{i}]"), item)?.to_string(),
                            );
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
        })(),
        "ErrorBoundary" => (|| -> DResult<NodeKind> {
            {
                let child_j = req(path, fields, "child", "ErrorBoundary child Node")?;
                let child = decode_node_ast(&format!("{path}.child"), child_j)?;
                let fallback_j = req(path, fields, "fallback", "ErrorBoundary fallback Node")?;
                let fallback = decode_node_ast(&format!("{path}.fallback"), fallback_j)?;
                Ok(NodeKind::ErrorBoundary(ErrorBoundarySpec {
                    child: Box::new(child),
                    fallback: Box::new(fallback),
                }))
            }
        })(),
        "Switch" => (|| -> DResult<NodeKind> {
            {
                // Phase 768 — the selector is any Binding: `on` carries the
                // canonical Binding wire form; the no-default `State` form keeps
                // the compact `stateKey` spelling. Both absent keeps the stateKey
                // MISSING_FIELD, so the reject fixture's error is unchanged.
                let on = match get(fields, "on") {
                    Some(v) => decode_binding(&format!("{path}.on"), v)?,
                    None => Binding::State {
                        key: req_string(path, fields, "stateKey", "Switch stateKey string")?,
                        default_value: StaticValue::Ast(JVal::Null),
                        // The compact `stateKey` spelling carries no default by
                        // construction, which is exactly what lets the encoder
                        // collapse back to it.
                        default_declared: false,
                    },
                };
                let cases_j = req(path, fields, "cases", "Switch cases array")?;
                let arr = as_arr(&format!("{path}.cases"), cases_j)?;
                let mut cases = Vec::with_capacity(arr.len());
                for (i, item) in arr.iter().enumerate() {
                    let cp = format!("{path}.cases[{i}]");
                    let cf = as_obj(&cp, item)?;
                    // Phase 1535 — EXACTLY ONE of `match` and `when`. "Both" is
                    // refused rather than resolved by precedence (a precedence
                    // rule would have to be specified, agreed on every host and
                    // remembered by every author, for a document nobody meant
                    // to write); "neither" keeps the pre-1535 MISSING_FIELD at
                    // `.match`, which is what the corpus pins — a case naming
                    // no condition is not one that never matches, and skipping
                    // it silently is the class of silence the predicate form
                    // was added to remove.
                    let condition = match (get(cf, "match"), get(cf, "when")) {
                        (Some(_), Some(_)) => {
                            return Err(wrong_type(
                                &format!("{cp}.when"),
                                "exactly one of 'match' and 'when' — a precedence rule between                                  them would have to be agreed on every host for a document                                  nobody meant to write",
                            ));
                        }
                        (None, Some(when_j)) => SwitchCondition::When(decode_binding_slot(
                            &format!("{cp}.when"),
                            when_j,
                            StaticSlot::Bool,
                        )?),
                        _ => SwitchCondition::Match(req_string(
                            &cp,
                            cf,
                            "match",
                            "Switch case match string",
                        )?),
                    };
                    let child_j = req(&cp, cf, "child", "Switch case child Node")?;
                    let child = decode_node_ast(&format!("{cp}.child"), child_j)?;
                    cases.push(SwitchCase { condition, child });
                }
                let default_j = req(path, fields, "default", "Switch default Node")?;
                let default = decode_node_ast(&format!("{path}.default"), default_j)?;
                // Phase 1122 — a POSITIVE INTEGER count of milliseconds.
                // Non-positive is refused rather than canonicalised: `0` is
                // what an emitter reaches for to mean "off" and absence is
                // already that spelling, so rewriting it would make two
                // document shapes mean one thing and tell the emitter nothing
                // about its misreading. Fractional is refused separately — the
                // slot is an integer count, and a decoder truncating where
                // another rounded would leave two hosts disagreeing about a
                // document neither refused.
                let auto_advance_ms = match get(fields, "autoAdvanceMs") {
                    None => None,
                    Some(v) => {
                        let ms_path = format!("{path}.autoAdvanceMs");
                        let ms = opt_int(path, fields, "autoAdvanceMs")?.ok_or_else(|| {
                            wrong_type(&ms_path, "an integer millisecond interval")
                        })?;
                        if !matches!(v, JVal::Num(n) if n.fract() == 0.0) {
                            return Err(wrong_type(
                                &ms_path,
                                "a whole millisecond interval — the slot is an integer count, and                                  a decoder truncating where another rounded would leave two hosts                                  disagreeing",
                            ));
                        }
                        if ms < 1 {
                            return Err(wrong_type(
                                &ms_path,
                                "a positive millisecond interval — an absent key is already the                                  spelling for off",
                            ));
                        }
                        Some(ms)
                    }
                };
                Ok(NodeKind::Switch(SwitchSpec {
                    on,
                    cases,
                    default: Box::new(default),
                    auto_advance_ms,
                }))
            }
        })(),
        "FragmentDecl" => (|| -> DResult<NodeKind> {
            {
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
        })(),
        "FragmentRef" => (|| -> DResult<NodeKind> {
            {
                let name = req_string(path, fields, "name", "FragmentRef name string")?;
                let args = match get(fields, "args") {
                    None => vec![],
                    Some(v) => decode_fragment_args(&format!("{path}.args"), v)?,
                };
                Ok(NodeKind::FragmentRef(FragmentRefSpec { name, args }))
            }
        })(),
        "Mount" => (|| -> DResult<NodeKind> {
            {
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
                    capabilities
                        .push(as_str(&format!("{path}.capabilities[{i}]"), item)?.to_string());
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
        })(),
        _ => return None,
    })
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
    // Phase 1472 - omitted at `Auto`. Lower-case tokens, so the upper-case
    // spelling an author reaches for (`"LTR"`) is UNKNOWN_DU_CASE rather than
    // being case-folded into acceptance.
    let direction = match get(fields, "direction") {
        None => TextDirection::Auto,
        Some(v) => decode_text_direction(&format!("{path}.direction"), v)?,
    };
    Ok(SemanticStyle {
        emphasis,
        tone,
        weight,
        role,
        voice,
        direction,
    })
}

fn decode_accessibility(path: &str, j: &JVal) -> DResult<Accessibility> {
    let fields = as_obj(path, j)?;
    // Phase 959 — the near-miss check runs BEFORE the slot reads, matching the
    // `FormField` ordering, so a trait carrying both `ariaLabel` and a
    // well-formed `label` still names the ignored key rather than decoding half
    // the author's intent in silence.
    check_near_misses_in(
        path,
        fields,
        A11Y_NEAR_MISSES,
        "accessibility",
        A11Y_NEAR_MISS_CONSEQUENCE,
    )?;
    let label = opt_binding_slot(path, fields, "label", StaticSlot::Str)?;
    let labelled_by = opt_string(path, fields, "labelledBy")?;
    let described_by = opt_string(path, fields, "describedBy")?;
    // Any string is accepted — named ARIA roles and the custom raw escape both
    // encode as the raw string (§10.2).
    let role = opt_string(path, fields, "role")?;
    let live_region = match get(fields, "liveRegion") {
        None => None,
        Some(v) => Some(decode_live_region(&format!("{path}.liveRegion"), v)?),
    };
    let hidden = opt_binding_slot(path, fields, "hidden", StaticSlot::Bool)?;
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
    // §21 node-depth + total-node bounds, on the way DOWN (rule 4). The guard
    // pops in `Drop`, which is what makes the counter correct on the ERROR
    // paths — and those are most of the paths, since this decoder is a long
    // chain of `?` early returns.
    let _guard = crate::limits::NodeGuard::enter().map_err(|b| limit_error(path, b))?;

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
    // Phase 1112 - the tooltip TRAIT, on the envelope beside `accessibility`
    // rather than in any kind. It takes every `TextSource` arm, so a non-string,
    // non-object value is `WRONG_TYPE` at `$.tooltip` - reported through the
    // shared `TextSource` decoder rather than by a second reading here.
    let tooltip = opt_text_source(path, fields, "tooltip")?;
    // Phase 1535 — the conditional-presence TRAIT, an ordinary `Binding<bool>`
    // slot beside `tooltip`. The §3.6 bare-scalar coercion reaches it like any
    // other binding slot.
    let visible = opt_binding_slot(path, fields, "visible", StaticSlot::Bool)?;
    Ok(Node {
        id: id.to_string(),
        kind,
        state,
        style,
        accessibility,
        tooltip,
        visible,
    })
}

// ─── TreeOp ──────────────────────────────────────────────────────────────────

/// Decode a `TreeOp` from its `$type`-discriminated object.
///
/// Split into sequential groups for the same reason as `decode_node_kind` — see
/// that function's note. It matters here despite there being only eleven ops,
/// because `TreeOp` is over a kilobyte and two of its arms carry a whole `Node`
/// as well. Measured before the split: the op axis aborted at 23 levels on a
/// 1 MB main-thread stack, one level short of `MAX_NODE_DEPTH`, so `OpGuard`
/// could never fire on a conformant document.
fn decode_tree_op_ast(path: &str, j: &JVal) -> DResult<TreeOp> {
    // The op axis, counted separately from the node axis and held to the same
    // ceiling — §21.5's note for implementers, since `Batch` makes this
    // self-recursive and the syntactic bound only LOOKS like cover for it.
    let _guard = crate::limits::OpGuard::enter().map_err(|b| limit_error(path, b))?;

    let fields = as_obj(path, j)?;
    let tag = disc(path, fields)?;
    if let Some(r) = decode_tree_op_ast_g0(tag, path, j, fields) {
        return r;
    }
    if let Some(r) = decode_tree_op_ast_g1(tag, path, j, fields) {
        return r;
    }
    if let Some(r) = decode_tree_op_ast_g2(tag, path, j, fields) {
        return r;
    }
    let other = tag;
    Err(unknown_du_case(
        path,
        other,
        "EditNode | UpdateProp | ReplaceBinding | UpdateStyle | UpdateState | InsertChild | RemoveNode | MoveNode | ReorderChildren | ReplaceRoot | Batch",
    ))
}

/// One group of the `TreeOp` dispatch — see `decode_tree_op_ast`.
fn decode_tree_op_ast_g0(
    tag: &str,
    path: &str,
    j: &JVal,
    fields: &Fields,
) -> Option<DResult<TreeOp>> {
    let _ = (j, fields);
    Some(match tag {
        "EditNode" => (|| -> DResult<TreeOp> {
            {
                let target = req_string(path, fields, "target", "target NodeId")?;
                let kind_j = req(path, fields, "newKind", "NodeKind object")?;
                let new_kind = decode_node_kind(&format!("{path}.newKind"), kind_j)?;
                Ok(TreeOp::EditNode { target, new_kind })
            }
        })(),
        "UpdateProp" => (|| -> DResult<TreeOp> {
            {
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
        })(),
        "ReplaceBinding" => (|| -> DResult<TreeOp> {
            {
                let target = req_string(path, fields, "target", "target NodeId")?;
                let slot = req_string(path, fields, "slot", "slot name string")?;
                let binding = req_binding(path, fields, "binding", "Binding object")?;
                Ok(TreeOp::ReplaceBinding {
                    target,
                    slot,
                    binding,
                })
            }
        })(),
        "UpdateStyle" => (|| -> DResult<TreeOp> {
            {
                let target = req_string(path, fields, "target", "target NodeId")?;
                let style_j = req(path, fields, "style", "SemanticStyle object")?;
                let style = decode_semantic_style(&format!("{path}.style"), style_j)?;
                Ok(TreeOp::UpdateStyle { target, style })
            }
        })(),
        _ => return None,
    })
}

/// One group of the `TreeOp` dispatch — see `decode_tree_op_ast`.
fn decode_tree_op_ast_g1(
    tag: &str,
    path: &str,
    j: &JVal,
    fields: &Fields,
) -> Option<DResult<TreeOp>> {
    let _ = (j, fields);
    Some(match tag {
        "UpdateState" => (|| -> DResult<TreeOp> {
            {
                let target = req_string(path, fields, "target", "target NodeId")?;
                let state_j = req(path, fields, "state", "StateBehaviour object")?;
                let state = decode_state_behaviour(&format!("{path}.state"), state_j)?;
                Ok(TreeOp::UpdateState { target, state })
            }
        })(),
        "InsertChild" => (|| -> DResult<TreeOp> {
            {
                // Phase 687 CLOSED the migration window Phase 681 opened: a
                // legacy `position` is a decode error. Checked BEFORE the
                // required-field reads — see `retired_positional_field`.
                retired_positional_field(path, fields, "position", "InsertChild")?;
                let parent_id = req_string(path, fields, "parentId", "parent NodeId")?;
                let child_j = req(path, fields, "child", "child Node object")?;
                let child = decode_node_ast(&format!("{path}.child"), child_j)?;
                Ok(TreeOp::InsertChild { parent_id, child })
            }
        })(),
        "RemoveNode" => (|| -> DResult<TreeOp> {
            Ok(TreeOp::RemoveNode {
                target: req_string(path, fields, "target", "target NodeId")?,
            })
        })(),
        "MoveNode" => (|| -> DResult<TreeOp> {
            {
                // Legacy `newPosition` is a decode error — see InsertChild above.
                retired_positional_field(path, fields, "newPosition", "MoveNode")?;
                let target = req_string(path, fields, "target", "target NodeId")?;
                let new_parent_id = req_string(path, fields, "newParentId", "new parent NodeId")?;
                Ok(TreeOp::MoveNode {
                    target,
                    new_parent_id,
                })
            }
        })(),
        _ => return None,
    })
}

/// One group of the `TreeOp` dispatch — see `decode_tree_op_ast`.
fn decode_tree_op_ast_g2(
    tag: &str,
    path: &str,
    j: &JVal,
    fields: &Fields,
) -> Option<DResult<TreeOp>> {
    let _ = (j, fields);
    Some(match tag {
        "ReorderChildren" => (|| -> DResult<TreeOp> {
            {
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
        })(),
        "ReplaceRoot" => (|| -> DResult<TreeOp> {
            {
                let node_j = req(path, fields, "node", "root Node object")?;
                let node = decode_node_ast(&format!("{path}.node"), node_j)?;
                Ok(TreeOp::ReplaceRoot { node })
            }
        })(),
        "Batch" => (|| -> DResult<TreeOp> {
            {
                let ops_j = req(path, fields, "ops", "Batch inner-op list")?;
                let arr = as_arr(&format!("{path}.ops"), ops_j)?;
                let mut ops = Vec::with_capacity(arr.len());
                for (i, item) in arr.iter().enumerate() {
                    ops.push(decode_tree_op_ast(&format!("{path}.ops[{i}]"), item)?);
                }
                Ok(TreeOp::Batch(ops))
            }
        })(),
        _ => return None,
    })
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

/// Turn a parse failure into a decode error, honouring §21.2 rule 2: a resource
/// limit breach is `LIMIT_EXCEEDED`, never `INVALID_JSON`. Only the parser knows
/// which of the two happened, so it flags the distinction rather than leaving it
/// to be re-derived from a message string.
fn parse_failure(e: &crate::canonical::ParseError) -> DecodeError {
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
fn limit_error(path: &str, breach: crate::limits::LimitBreach) -> DecodeError {
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
fn document_bytes_error(json: &str) -> Option<DecodeError> {
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
        via(v, decode_tone)
    }

    pub fn weight(v: &JVal) -> C<StyleWeight> {
        via(v, decode_weight)
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
