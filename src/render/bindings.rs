//! Binding resolution + text / number formatting for the server renderer.
//!
//! The server resolves bindings exactly as a hydrating client's first render
//! would: `Static` to its typed value, `Query` / `Filter` / `Selection` from
//! host-supplied [`BindingSources`] (the decoded accessors are identity
//! projections, so the raw source value flows through), `State` to the source
//! value or its carried default. Host-only escapes resolve `NotResolved` on
//! this decoded-tree host: `Computed` erases on the wire, and `Transform`
//! awaits the dataframe-evaluation tier.

use std::collections::HashMap;

use crate::canonical::{JVal, format_number as canonical_number};
use crate::wire::{
    Accessibility, Binding, CellFormat, DateStyle, Format, LocaleSource, SelectOption, StaticValue,
    TextSource,
};

/// The em-dash placeholder an unresolved value renders as.
pub const EM_DASH: &str = "—";

/// Data sources the renderer consults when it encounters a binding. Values are
/// wire-shaped [`JVal`]s — the same JSON the host would feed a hydrating
/// client.
#[derive(Debug, Clone, Default)]
pub struct BindingSources {
    pub query_results: HashMap<String, JVal>,
    pub state: HashMap<String, JVal>,
    pub filters: HashMap<String, JVal>,
    pub selections: HashMap<String, JVal>,
    /// i18n key → template (`{arg}` placeholders).
    pub i18n: HashMap<String, String>,
}

/// A resolved binding value: either a typed `Static` payload or a raw
/// source-supplied [`JVal`].
#[derive(Debug, Clone)]
pub enum Value<'a> {
    Static(&'a StaticValue),
    Json(&'a JVal),
    /// A `Binding.Format` production (already a display string).
    Text(String),
}

/// Binding resolution outcome (mirrors the sibling renderers' surface).
#[derive(Debug, Clone)]
pub enum Resolution<'a> {
    Resolved(Value<'a>),
    NotResolved,
    I18nUnresolved(String),
}

/// Resolve a binding against the supplied sources.
pub fn resolve<'a>(sources: &'a BindingSources, binding: &'a Binding) -> Resolution<'a> {
    match binding {
        Binding::Static { value } => Resolution::Resolved(Value::Static(value)),
        Binding::Query { name, .. } => match sources.query_results.get(name) {
            // The decoded accessor is an identity projection (Phase 421).
            Some(raw) => Resolution::Resolved(Value::Json(raw)),
            None => Resolution::NotResolved,
        },
        Binding::Filter {
            name,
            default_value,
        } => match sources.filters.get(name) {
            Some(raw) => Resolution::Resolved(Value::Json(raw)),
            // 0.2.0 — the defaultValue is yielded until the filter is first
            // written.
            None => match default_value {
                Some(d) => Resolution::Resolved(Value::Static(d)),
                None => Resolution::NotResolved,
            },
        },
        Binding::Selection {
            node_id,
            default_value,
            field,
        } => match sources.selections.get(node_id) {
            // Identity projection (Phase 427); 0.2.10 — a present `field`
            // projects that field off the stored row.
            Some(raw) => match field {
                Some(f) => match raw.field(f) {
                    Some(projected) => Resolution::Resolved(Value::Json(projected)),
                    None => Resolution::NotResolved,
                },
                None => Resolution::Resolved(Value::Json(raw)),
            },
            // 0.2.9 — yielded until the user first selects a row.
            None => match default_value {
                Some(d) => Resolution::Resolved(Value::Static(d)),
                None => Resolution::NotResolved,
            },
        },
        Binding::State { key, default_value } => match sources.state.get(key) {
            Some(raw) => Resolution::Resolved(Value::Json(raw)),
            None => Resolution::Resolved(Value::Static(default_value)),
        },
        // Host-only on the wire (§5.1): a decoded Computed is an inert
        // placeholder — resolve as not-resolved so the loading surface shows.
        Binding::Computed => Resolution::NotResolved,
        Binding::I18n { key, .. } => match sources.i18n.get(key) {
            Some(template) => Resolution::Resolved(Value::Text(template.clone())),
            None => Resolution::I18nUnresolved(key.clone()),
        },
        Binding::Local { initial_from, .. } => resolve(sources, initial_from),
        Binding::Format {
            format,
            locale,
            source,
        } => match resolve_number(sources, source) {
            NumberResolution::Resolved(v) => {
                Resolution::Resolved(Value::Text(format_locale_value(locale, format, v)))
            }
            NumberResolution::NotResolved => Resolution::NotResolved,
            NumberResolution::I18nUnresolved(key) => Resolution::I18nUnresolved(key),
        },
        // The dataframe-evaluation tier is beyond this floor — render the
        // loading surface rather than a wrong value.
        Binding::Transform { .. } => Resolution::NotResolved,
        // A capability invoker seam is not wired on this host yet — behaves as
        // pending, exactly as an absent invoker does on the sibling hosts.
        Binding::Invoke { .. } => Resolution::NotResolved,
    }
}

// ─── Typed coercions over resolved values ────────────────────────────────────

pub enum NumberResolution {
    Resolved(f64),
    NotResolved,
    I18nUnresolved(String),
}

fn jval_number(v: &JVal) -> Option<f64> {
    match v {
        JVal::Num(n) => Some(*n),
        _ => None,
    }
}

fn static_number(v: &StaticValue) -> Option<f64> {
    match v {
        StaticValue::Ast(j) => jval_number(j),
        _ => None,
    }
}

/// Resolve a binding expected to carry a number.
pub fn resolve_number(sources: &BindingSources, binding: &Binding) -> NumberResolution {
    match resolve(sources, binding) {
        Resolution::Resolved(Value::Static(s)) => match static_number(s) {
            Some(n) => NumberResolution::Resolved(n),
            None => NumberResolution::NotResolved,
        },
        Resolution::Resolved(Value::Json(j)) => match jval_number(j) {
            Some(n) => NumberResolution::Resolved(n),
            None => NumberResolution::NotResolved,
        },
        Resolution::Resolved(Value::Text(_)) | Resolution::NotResolved => {
            NumberResolution::NotResolved
        }
        Resolution::I18nUnresolved(key) => NumberResolution::I18nUnresolved(key),
    }
}

/// Best-effort number — `None` unless cleanly resolved.
pub fn try_number(sources: &BindingSources, binding: &Binding) -> Option<f64> {
    match resolve_number(sources, binding) {
        NumberResolution::Resolved(n) => Some(n),
        _ => None,
    }
}

/// Best-effort bool.
pub fn try_bool(sources: &BindingSources, binding: &Binding) -> Option<bool> {
    match resolve(sources, binding) {
        Resolution::Resolved(Value::Static(StaticValue::Ast(JVal::Bool(b)))) => Some(*b),
        Resolution::Resolved(Value::Json(JVal::Bool(b))) => Some(*b),
        _ => None,
    }
}

/// Best-effort display string (the `renderText` Bound coercion).
pub fn try_string(sources: &BindingSources, binding: &Binding) -> Option<String> {
    match resolve(sources, binding) {
        Resolution::Resolved(value) => value_display_string(&value),
        _ => None,
    }
}

/// A resolved value's display-string form: strings as-is, numbers in the
/// deterministic canonical layout, bools as `true`/`false`; structured values
/// have no display form.
fn value_display_string(value: &Value<'_>) -> Option<String> {
    match value {
        Value::Text(s) => Some(s.clone()),
        Value::Json(JVal::Str(s)) => Some(s.clone()),
        Value::Json(JVal::Num(n)) => Some(display_number(*n)),
        Value::Json(JVal::Bool(b)) => Some(if *b { "true" } else { "false" }.to_string()),
        Value::Json(_) => None,
        Value::Static(StaticValue::Ast(j)) => value_display_string(&Value::Json(j)),
        Value::Static(StaticValue::StringOpt(opt)) => opt.clone(),
        Value::Static(_) => None,
    }
}

/// Options for a select-shaped slot: the typed `Static` payload, or a
/// source-supplied array of `{label, value}` objects; anything else is empty
/// (the defensive `asArray` posture).
pub fn resolve_options(sources: &BindingSources, binding: &Binding) -> Vec<SelectOption> {
    match resolve(sources, binding) {
        Resolution::Resolved(Value::Static(StaticValue::Options(options))) => options.clone(),
        Resolution::Resolved(Value::Json(JVal::Arr(items))) => items
            .iter()
            .filter_map(|item| {
                let value = match item.field("value") {
                    Some(JVal::Str(s)) => s.clone(),
                    _ => return None,
                };
                let label = match item.field("label") {
                    Some(JVal::Str(s)) => TextSource::Literal(s.clone()),
                    _ => TextSource::Literal(value.clone()),
                };
                Some(SelectOption { value, label })
            })
            .collect(),
        _ => vec![],
    }
}

/// Rows for a data-bound grid / chart / map: a source-supplied JSON array;
/// anything else is empty.
pub fn resolve_rows<'a>(sources: &'a BindingSources, binding: &'a Binding) -> ResolvedRows<'a> {
    match resolve(sources, binding) {
        Resolution::Resolved(Value::Json(JVal::Arr(items))) => ResolvedRows::Rows(items),
        Resolution::Resolved(_) => ResolvedRows::Rows(&[]),
        Resolution::NotResolved => ResolvedRows::NotResolved,
        Resolution::I18nUnresolved(_) => ResolvedRows::Rows(&[]),
    }
}

pub enum ResolvedRows<'a> {
    Rows(&'a [JVal]),
    NotResolved,
}

/// The `(min, max)` pair of a `Range` control's value binding: the typed
/// `Static` pair, a source-supplied `{min, max}` object, or a two-element
/// array.
pub fn resolve_float_pair(sources: &BindingSources, binding: &Binding) -> Option<(f64, f64)> {
    match resolve(sources, binding) {
        Resolution::Resolved(Value::Static(StaticValue::FloatPair(a, b))) => Some((*a, *b)),
        Resolution::Resolved(Value::Json(j)) => match j {
            JVal::Arr(items) if items.len() == 2 => {
                match (jval_number(&items[0]), jval_number(&items[1])) {
                    (Some(a), Some(b)) => Some((a, b)),
                    _ => None,
                }
            }
            JVal::Obj(_) => match (
                j.field("min").and_then(jval_number),
                j.field("max").and_then(jval_number),
            ) {
                (Some(a), Some(b)) => Some((a, b)),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// The float series of a sparkline: typed `Static` payload or source array.
pub fn resolve_float_seq(sources: &BindingSources, binding: &Binding) -> Vec<f64> {
    match resolve(sources, binding) {
        Resolution::Resolved(Value::Static(StaticValue::FloatSeq(values))) => values.clone(),
        Resolution::Resolved(Value::Json(JVal::Arr(items))) => {
            items.iter().filter_map(jval_number).collect()
        }
        _ => vec![],
    }
}

// ─── Text-source rendering ───────────────────────────────────────────────────

fn jval_arg_string(v: &JVal) -> String {
    match v {
        JVal::Null => String::new(),
        JVal::Str(s) => s.clone(),
        JVal::Num(n) => display_number(*n),
        JVal::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        other => crate::canonical::render_canonical(other),
    }
}

/// Render a `TextSource` to a plain string against the supplied sources.
pub fn render_text(sources: &BindingSources, text: &TextSource) -> String {
    match text {
        TextSource::Literal(value) => value.clone(),
        TextSource::Bound(binding) => try_string(sources, binding).unwrap_or_default(),
        TextSource::I18n { key, args } => match sources.i18n.get(key) {
            None => format!("[i18n:{key}]"),
            Some(template) => {
                let mut acc = template.clone();
                for (k, v) in args {
                    acc = acc.replace(&format!("{{{k}}}"), &jval_arg_string(v));
                }
                acc
            }
        },
    }
}

// ─── Number / cell formatting ────────────────────────────────────────────────

/// The deterministic display layout for a number (shared shortest-round-trip
/// digits; the canonical layout).
pub fn display_number(value: f64) -> String {
    canonical_number(value)
}

fn is_integer(value: f64) -> bool {
    value.is_finite() && value.trunc() == value
}

/// Round to `digits` significant digits and re-render shortest.
fn to_precision(value: f64, digits: i64) -> String {
    let digits = digits.clamp(1, 17) as usize;
    let rounded: f64 = format!("{value:.*e}", digits - 1).parse().unwrap_or(value);
    display_number(rounded)
}

/// Format a numeric value per a `CellFormat`. A decoded `Custom` formatter is a
/// closure placeholder — it renders the `"<closure>"` sentinel, as on every
/// decoded-tree path.
pub fn format_number(format: &CellFormat, value: f64) -> String {
    match format {
        CellFormat::None => display_number(value),
        CellFormat::Number { decimals } => match decimals {
            Some(d) => format!("{value:.*}", (*d).clamp(0, 17) as usize),
            None => {
                if is_integer(value) {
                    format!("{value:.0}")
                } else {
                    display_number(value)
                }
            }
        },
        CellFormat::Currency { code } => format!("{code} {value:.2}"),
        CellFormat::Percent { decimals } => {
            let d = decimals.unwrap_or(1).clamp(0, 17) as usize;
            format!("{:.*}%", d, value * 100.0)
        }
        CellFormat::SignificantDigits { digits } => to_precision(value, *digits),
        CellFormat::Date { .. } => display_number(value),
        CellFormat::Custom => "<closure>".to_string(),
    }
}

// ─── Locale formatting (invariant fallback) ──────────────────────────────────
//
// The sibling hosts lean on their platform locale engines here; a stdlib-only
// host has none, so `Binding.Format` renders a deterministic invariant layout.
// A host-locale seam can replace this per deployment; the shapes below are the
// documented fallback, not a cross-host byte contract.

fn civil_from_unix_seconds(seconds: f64) -> (i64, u32, u32) {
    // Howard Hinnant's civil_from_days over the Unix epoch.
    let days = (seconds / 86_400.0).floor() as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Format a numeric value per a bounded `Format` intent. Invariant fallback —
/// see the module note above.
pub fn format_locale_value(_locale: &LocaleSource, format: &Format, value: f64) -> String {
    match format {
        Format::Number { decimals } => match decimals {
            Some(d) => format!("{value:.*}", (*d).clamp(0, 17) as usize),
            None => display_number(value),
        },
        Format::Currency { iso_code } => format!("{iso_code} {value:.2}"),
        Format::Percent { decimals } => {
            let d = decimals.unwrap_or(0).clamp(0, 17) as usize;
            format!("{:.*}%", d, value * 100.0)
        }
        Format::Date { date_style } => {
            let (y, m, d) = civil_from_unix_seconds(value);
            let month_name = MONTHS[(m as usize).saturating_sub(1).min(11)];
            match date_style {
                DateStyle::Short => format!("{y:04}-{m:02}-{d:02}"),
                DateStyle::Medium | DateStyle::Long | DateStyle::Full => {
                    format!("{d} {month_name} {y}")
                }
            }
        }
        Format::RelativeTime { unit } => {
            let unit_name = unit.as_str().to_lowercase();
            let magnitude = value.abs();
            let plural = if magnitude == 1.0 { "" } else { "s" };
            if value < 0.0 {
                format!("{} {unit_name}{plural} ago", display_number(magnitude))
            } else {
                format!("in {} {unit_name}{plural}", display_number(magnitude))
            }
        }
    }
}

// ─── Accessibility projection ────────────────────────────────────────────────

/// Project an optional `Accessibility` (resolved against sources) into ordered
/// `(attr-name, attr-value)` pairs for the outer wrapper. Order matches the
/// reference renderers: label, labelledby, describedby, role, live, hidden.
pub fn accessibility_attributes(
    sources: &BindingSources,
    a11y: Option<&Accessibility>,
) -> Vec<(&'static str, String)> {
    let Some(a11y) = a11y else {
        return vec![];
    };
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if let Some(label) = &a11y.label
        && let Some(resolved) = try_string(sources, label)
        && !resolved.is_empty()
    {
        pairs.push(("aria-label", resolved));
    }
    if let Some(labelled_by) = &a11y.labelled_by {
        pairs.push(("aria-labelledby", labelled_by.clone()));
    }
    if let Some(described_by) = &a11y.described_by {
        pairs.push(("aria-describedby", described_by.clone()));
    }
    if let Some(role) = &a11y.role {
        pairs.push(("role", role.clone()));
    }
    if let Some(live) = &a11y.live_region {
        pairs.push(("aria-live", live.as_str().to_string()));
    }
    if let Some(hidden) = &a11y.hidden
        && try_bool(sources, hidden) == Some(true)
    {
        pairs.push(("aria-hidden", "true".to_string()));
    }
    pairs
}
