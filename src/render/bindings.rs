//! Binding resolution + text / number formatting for the server renderer.
//!
//! The server resolves bindings exactly as a hydrating client's first render
//! would: `Static` to its typed value, `Query` / `Filter` / `Selection` from
//! host-supplied [`BindingSources`] (the decoded accessors are identity
//! projections, so the raw source value flows through), `State` to the source
//! value or its carried default. `Computed` resolves `NotResolved` on this
//! decoded-tree host (it erases on the wire).
//!
//! `Transform` is render-time evaluated (Phase 649) through the host's own
//! certified evaluator (`crate::transform`), but NOT inside [`resolve`]: an
//! evaluated pipeline yields *owned* rows/cells, which cannot ride the
//! borrow-based [`Resolution`]. Instead a single shared seam —
//! [`eval_transform_frame`] — is consumed two ways: [`resolve_rows`] projects
//! the result table to rows for the data-bearing kinds (row contexts), and the
//! scalar-slot path ([`resolve_scalar_number`] / [`try_scalar_string`], the
//! Phase 632 1×1 law) interprets it as one cell for `TextSource.Bound`,
//! Metric value/trend, and LabelValueRow value. The server renderer and the
//! `wasm32` client both render through this one module, so the two consumption
//! legs resolve identically. Phase 629 `Selection.defaultValue` seeding is
//! automatic: a `Transform` param whose `from` is a `Selection` resolves its
//! declared default through [`resolve`] below.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

use crate::canonical::{JVal, format_number as canonical_number};
use crate::transform::{self, Table};
use crate::wire::{
    Accessibility, AggFn, Binding, Cell, CellFormat, DataSource, DateStyle, DurationStyle,
    DurationUnit, Format, LocaleSource, RelativeTimeUnit, SelectOption, StaticValue, TextSource,
    TransformParam, TransformSource, TransformStep,
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
    /// Phase 765 — the host-furnished current instant (an ISO-8601 string),
    /// pinned once per render pass. Absent (or empty) ⇒ `Binding::Now`
    /// resolves not-resolved, so SSR output stays reproducible.
    pub now: Option<String>,
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
        // Phase 765 — host-furnished, resolved once per render pass; never a
        // clock read here, so SSR output is reproducible for a pinned instant.
        Binding::Now => match &sources.now {
            Some(iso) if !iso.is_empty() => Resolution::Resolved(Value::Text(iso.clone())),
            _ => Resolution::NotResolved,
        },
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
            NumberResolution::NotResolved | NumberResolution::Errored(_) => Resolution::NotResolved,
            NumberResolution::I18nUnresolved(key) => Resolution::I18nUnresolved(key),
        },
        // A `Transform` yields *owned* rows/cells, which cannot ride the
        // borrow-based `Resolution` — it is evaluated at the consumption seams
        // instead: `resolve_rows` (row contexts) and the scalar-slot path
        // (`resolve_scalar_number` / `try_scalar_string`). See the module doc.
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
    /// A scalar-slot `Transform` that could not yield a single numeric cell
    /// (Phase 632 — loud ambiguity / a non-numeric or null result cell). Kept
    /// distinct from `NotResolved` so the slot renders the didactic message
    /// rather than silently falling to its loading surface.
    Errored(String),
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

// ─── Render-time Transform resolution (Phase 649) ────────────────────────────
//
// The single shared seam that wires the host's own certified dataframe
// evaluator (`crate::transform`) into render-time binding resolution, mirroring
// the F#/TS reference (Phases 629/632; fuaran-ts ea16811, fuaran 660182a). Two
// consumers, one evaluation:
//   • `resolve_rows` (row contexts — DataGrid / Chart) projects the result
//     table to rows;
//   • the scalar-slot path (`resolve_scalar_number` / `try_scalar_string`)
//     interprets an exactly-1×1 result as one cell (the Phase 632 1×1 law).
// The `wasm32` client renders through the very same module, so both legs
// resolve identically.

/// Coerce a resolved param value to an evaluator [`Cell`] (mirrors the TS
/// `objToCell`): numbers → `Float` (int/float indistinguishable at the wire),
/// strings → `Str`, bools → `Bool`, null → `Null`; a structured/opaque value
/// has no scalar form (`None` ⇒ a param error).
fn value_to_cell(value: &Value<'_>) -> Option<Cell> {
    match value {
        Value::Text(s) => Some(Cell::Str(s.clone())),
        Value::Json(j) => jval_to_cell(j),
        Value::Static(StaticValue::Ast(j)) => jval_to_cell(j),
        Value::Static(StaticValue::StringOpt(Some(s))) => Some(Cell::Str(s.clone())),
        Value::Static(StaticValue::StringOpt(None)) => Some(Cell::Null),
        Value::Static(_) => None,
    }
}

fn jval_to_cell(j: &JVal) -> Option<Cell> {
    match j {
        JVal::Null => Some(Cell::Null),
        JVal::Str(s) => Some(Cell::Str(s.clone())),
        JVal::Num(n) => Some(Cell::Float(*n)),
        JVal::Bool(b) => Some(Cell::Bool(*b)),
        JVal::Arr(_) | JVal::Obj(_) => None,
    }
}

/// Evaluate a `Binding::Transform` to a concrete [`Table`] — the shared frame
/// seam (mirrors the TS `evalTransformFrame`). Each param's scalar `from`
/// binding resolves through [`resolve`] (so a `Selection.defaultValue` seeds
/// the env — Phase 629); a param whose source is `NotResolved` (an unset
/// choice filter) is *unbound*, and every `filter` step referencing an unbound
/// param is PRUNED — the one lenient "unset filter ⇒ no constraint" rule (the
/// evaluator itself stays strict, so a *non-filter* step over an unbound param
/// still surfaces `UnboundParam` loudly). A non-scalar / i18n-unresolved param
/// source, a missing `Ref`, and an evaluator error are all `Err`.
pub fn eval_transform_frame(
    sources: &BindingSources,
    params: &Option<Vec<TransformParam>>,
    pipeline: &[TransformStep],
    source: &TransformSource,
) -> Result<Table, String> {
    // Phase 818 — a LIVE source resolves its preserved binding against the
    // stores and evaluates over the CURRENT data (row-major rows transpose
    // through the same 815 normalisation the decode-time snapshot used); an
    // unwritten store falls back to the decode-time `initial`, which is what
    // keeps SSR byte-identical to the snapshot era. Non-tabular live values
    // error loudly, never silently.
    let live_input: Option<std::borrow::Cow<'_, DataSource>> = match source {
        TransformSource::Data(_) => None,
        TransformSource::Live { binding, initial } => match resolve(sources, binding) {
            Resolution::Resolved(v) => {
                let j: JVal = match v {
                    Value::Json(j) => j.clone(),
                    Value::Static(StaticValue::Rows(rows)) => {
                        JVal::Arr(rows.iter().map(|r| JVal::Obj(r.clone())).collect())
                    }
                    Value::Static(StaticValue::Ast(j)) => j.clone(),
                    _ => {
                        return Err(
                            "Transform live source resolved to a value that cannot be read as data — expected rows (an array of row objects) or canonical columnar data"
                                .to_string(),
                        );
                    }
                };
                match crate::wire::live_data_source(&j) {
                    Ok(ds) => Some(std::borrow::Cow::Owned(ds)),
                    Err(e) => return Err(format!("Transform live source: {e}")),
                }
            }
            Resolution::NotResolved => Some(std::borrow::Cow::Borrowed(initial)),
            Resolution::I18nUnresolved(key) => {
                return Err(format!(
                    "Transform live source is an unresolved i18n key '{key}'"
                ));
            }
        },
    };
    let data_source: &DataSource = match (&live_input, source) {
        (Some(ds), _) => ds,
        (None, TransformSource::Data(ds)) => ds,
        (None, TransformSource::Live { initial, .. }) => initial,
    };
    let params = params.as_deref().unwrap_or(&[]);
    let mut env = transform::EvalEnv::new();
    let mut unbound: BTreeSet<String> = BTreeSet::new();
    for p in params {
        match resolve(sources, &p.from) {
            Resolution::Resolved(value) => match value_to_cell(&value) {
                Some(cell) => {
                    env.insert(p.name.clone(), cell);
                }
                None => {
                    return Err(format!(
                        "Transform param '{}' resolved to a non-scalar value",
                        p.name
                    ));
                }
            },
            Resolution::NotResolved => {
                unbound.insert(p.name.clone());
            }
            Resolution::I18nUnresolved(key) => {
                return Err(format!(
                    "Transform param '{}' source is an unresolved i18n key '{key}'",
                    p.name
                ));
            }
        }
    }
    // Prune filter steps that reference an unbound param (only `Filter` /
    // `Derive` carry params; only `Filter` is prunable).
    let mut pruned: Vec<TransformStep> = Vec::with_capacity(pipeline.len());
    for step in pipeline {
        let keep = match step {
            TransformStep::Filter { .. } => transform::step_params(step)
                .iter()
                .all(|pn| !unbound.contains(pn)),
            _ => true,
        };
        if keep {
            pruned.push(step.clone());
        }
    }
    transform::eval_transform(&transform::no_resolve, &env, data_source, &pruned).map_err(|e| {
        format!(
            "Transform evaluation failed: {}",
            transform::eval_error_string(&e)
        )
    })
}

/// Project an evaluated [`Table`] into wire-shaped row objects (one
/// `JVal::Obj` per row, `{ columnName: jval }`) — the shape a data-bearing
/// node's source slot consumes. Mirrors the TS `tableToRows`; a `Null` cell
/// becomes `JVal::Null`.
fn table_to_rows(table: &Table) -> Vec<JVal> {
    let n = table.columns.first().map(|c| c.cells.len()).unwrap_or(0);
    (0..n)
        .map(|i| {
            JVal::Obj(
                table
                    .columns
                    .iter()
                    .map(|col| {
                        (
                            col.name.clone(),
                            cell_to_jval(col.cells.get(i).unwrap_or(&Cell::Null)),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

fn cell_to_jval(c: &Cell) -> JVal {
    match c {
        Cell::Null => JVal::Null,
        Cell::Int(v) => JVal::Num(*v as f64),
        Cell::Float(v) => JVal::Num(*v),
        Cell::Bool(b) => JVal::Bool(*b),
        Cell::Str(s) | Cell::Date(s) | Cell::Timestamp(s) => JVal::Str(s.clone()),
    }
}

/// The outcome of interpreting a scalar-slot `Transform`'s 1×1 result.
enum ScalarOutcome<T> {
    Resolved(T),
    NotResolved,
    Errored(String),
}

/// Coerce a result cell to a text-slot string (via the certified
/// `transform::cell_string` canonical layout); a `Null` cell is the slot's
/// empty state.
fn cell_to_text(c: &Cell) -> Result<String, String> {
    if matches!(c, Cell::Null) {
        return Err("Transform yielded a null cell in a text slot".to_string());
    }
    Ok(transform::cell_string(c))
}

/// Coerce a result cell to a numeric-slot number (a non-numeric / null cell is
/// a loud didactic).
fn cell_to_float(c: &Cell) -> Result<f64, String> {
    match c {
        Cell::Int(v) => Ok(*v as f64),
        Cell::Float(v) => Ok(*v),
        Cell::Str(s) => Err(format!(
            "Transform yielded a text cell ('{s}') in a numeric slot — project a numeric column, or aggregate with count / sum / mean"
        )),
        Cell::Bool(_) => Err("Transform yielded a bool cell in a numeric slot".to_string()),
        Cell::Date(s) | Cell::Timestamp(s) => Err(format!(
            "Transform yielded a date cell ('{s}') in a numeric slot — project a numeric column, or aggregate with count / sum / mean"
        )),
        Cell::Null => Err("Transform yielded a null cell in a numeric slot".to_string()),
    }
}

/// Resolve a scalar-slot `Transform` to one cell under the Phase 632 1×1 law:
/// exactly one row × one column resolves (a non-null cell coerced), ambiguity
/// (>1 row/col) is a loud didactic (never a silent first cell), and an empty
/// result renders absence — except a trailing global single-`count` groupBy
/// over an empty frame, which resolves 0 (the count of nothing is 0, the SQL
/// global-aggregate semantic the strict fold leaves empty).
fn resolve_scalar_transform<T>(
    coerce: impl Fn(&Cell) -> Result<T, String>,
    sources: &BindingSources,
    params: &Option<Vec<TransformParam>>,
    pipeline: &[TransformStep],
    source: &TransformSource,
) -> ScalarOutcome<T> {
    let table = match eval_transform_frame(sources, params, pipeline, source) {
        Ok(t) => t,
        Err(e) => return ScalarOutcome::Errored(e),
    };
    let col_count = table.columns.len();
    let row_count = table.columns.first().map(|c| c.cells.len()).unwrap_or(0);
    let coerce_into = |c: &Cell| match coerce(c) {
        Ok(v) => ScalarOutcome::Resolved(v),
        Err(e) => ScalarOutcome::Errored(e),
    };
    if row_count == 1 && col_count == 1 {
        let cell = &table.columns[0].cells[0];
        if matches!(cell, Cell::Null) {
            return ScalarOutcome::NotResolved;
        }
        return coerce_into(cell);
    }
    if row_count == 0 {
        // A trailing global single-`count` aggregate completes to 0 over an
        // empty frame (checked against the ORIGINAL pipeline, matching the
        // reference — pruning never removes a groupBy).
        if let Some(TransformStep::GroupBy { keys, aggs }) = pipeline.last()
            && keys.is_empty()
            && aggs.len() == 1
            && matches!(aggs[0].func, AggFn::Count)
        {
            return coerce_into(&Cell::Int(0));
        }
        return ScalarOutcome::NotResolved;
    }
    ScalarOutcome::Errored(format!(
        "Transform in a scalar slot must yield exactly one row × one column (got {row_count}×{col_count}) — end the pipeline with `project` to one column + `limit` 1 (a row-field lookup), or aggregate with `groupBy` keys [] + one agg (count / sum / mean / first)"
    ))
}

/// Resolve a binding in a numeric scalar slot (Metric / LabelValueRow value,
/// Metric trend): a `Binding::Transform` evaluates through the shared frame and
/// interprets its 1×1 result; every other binding resolves exactly as
/// [`resolve_number`], so non-Transform slots are unaffected.
pub fn resolve_scalar_number(sources: &BindingSources, binding: &Binding) -> NumberResolution {
    match binding {
        Binding::Transform {
            params,
            pipeline,
            source,
        } => match resolve_scalar_transform(cell_to_float, sources, params, pipeline, source) {
            ScalarOutcome::Resolved(n) => NumberResolution::Resolved(n),
            ScalarOutcome::NotResolved => NumberResolution::NotResolved,
            ScalarOutcome::Errored(msg) => NumberResolution::Errored(msg),
        },
        _ => resolve_number(sources, binding),
    }
}

/// Best-effort numeric scalar resolution — the `try_number` twin (used by the
/// Metric trend slot).
pub fn try_scalar_number(sources: &BindingSources, binding: &Binding) -> Option<f64> {
    match resolve_scalar_number(sources, binding) {
        NumberResolution::Resolved(n) => Some(n),
        _ => None,
    }
}

/// Best-effort text scalar resolution: a `Binding::Transform` yields its 1×1
/// result cell as text (never the rows list), matching the client's
/// `renderText`; every other binding resolves exactly as [`try_string`]. An
/// ambiguous / errored / not-resolved Transform yields `None` (an empty text
/// slot), the reference `?? ''` behaviour.
pub fn try_scalar_string(sources: &BindingSources, binding: &Binding) -> Option<String> {
    match binding {
        Binding::Transform {
            params,
            pipeline,
            source,
        } => match resolve_scalar_transform(cell_to_text, sources, params, pipeline, source) {
            ScalarOutcome::Resolved(s) => Some(s),
            ScalarOutcome::NotResolved | ScalarOutcome::Errored(_) => None,
        },
        _ => try_string(sources, binding),
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
/// The display-string form of a bare `Static` payload — the Phase 768 Switch
/// selector's `State.defaultValue` seeding reads it without a store round-trip.
pub fn static_display_string(value: &StaticValue) -> Option<String> {
    value_display_string(&Value::Static(value))
}

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

/// Rows for a data-bound grid / chart / map: a `Binding::Transform` evaluates
/// its pipeline (Phase 649 — the row-context leg of the shared seam) and
/// projects the result table to rows; otherwise a source-supplied JSON array;
/// anything else is empty. A Transform that errors renders the loading surface
/// (`NotResolved`) rather than a wrong table.
pub fn resolve_rows<'a>(sources: &'a BindingSources, binding: &'a Binding) -> ResolvedRows<'a> {
    if let Binding::Transform {
        params,
        pipeline,
        source,
    } = binding
    {
        return match eval_transform_frame(sources, params, pipeline, source) {
            Ok(table) => ResolvedRows::Rows(Cow::Owned(table_to_rows(&table))),
            Err(_) => ResolvedRows::NotResolved,
        };
    }
    match resolve(sources, binding) {
        Resolution::Resolved(Value::Json(JVal::Arr(items))) => {
            ResolvedRows::Rows(Cow::Borrowed(items))
        }
        // fuaran#665 — an authored rows feed now survives the wire as a typed
        // `Static`/`State` payload rather than the `"<opaque>"` sentinel, so it
        // reaches the renderer as data. Same shape a host-supplied array takes.
        Resolution::Resolved(Value::Static(StaticValue::Rows(rows))) => ResolvedRows::Rows(
            Cow::Owned(rows.iter().map(|r| JVal::Obj(r.clone())).collect()),
        ),
        Resolution::Resolved(_) => ResolvedRows::Rows(Cow::Borrowed(&[])),
        Resolution::NotResolved => ResolvedRows::NotResolved,
        Resolution::I18nUnresolved(_) => ResolvedRows::Rows(Cow::Borrowed(&[])),
    }
}

pub enum ResolvedRows<'a> {
    Rows(Cow<'a, [JVal]>),
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

/// The `(from, to)` pair of a `DateRange` control's value binding: the typed
/// `Static` pair, a source-supplied `{from, to}` object, or a two-element array.
/// The [`resolve_float_pair`] twin — a resolved pair is deliberately NOT
/// re-checked for ordering: the spec's ordered-pair rule binds a LITERAL on the
/// wire, and a resolved value's ordering is a runtime concern.
pub fn resolve_string_pair(
    sources: &BindingSources,
    binding: &Binding,
) -> Option<(String, String)> {
    match resolve(sources, binding) {
        Resolution::Resolved(Value::Static(StaticValue::StringPair(a, b))) => {
            Some((a.clone(), b.clone()))
        }
        Resolution::Resolved(Value::Json(j)) => match j {
            JVal::Arr(items) if items.len() == 2 => match (&items[0], &items[1]) {
                (JVal::Str(a), JVal::Str(b)) => Some((a.clone(), b.clone())),
                _ => None,
            },
            JVal::Obj(_) => match (j.field("from"), j.field("to")) {
                (Some(JVal::Str(a)), Some(JVal::Str(b))) => Some((a.clone(), b.clone())),
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
        // Phase 632/649 — text slots resolve through the scalar path, so a
        // `Binding.Transform` yields its 1×1 result cell (never the rows list);
        // every non-Transform binding routes to `try_string` unchanged.
        TextSource::Bound(binding) => try_scalar_string(sources, binding).unwrap_or_default(),
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

// ─── Duration / relative-time rendering (Phase 819) ─────────────────────────
//
// Mirrors the reference hosts' shared `formatDuration` / `formatRelativeEnglish`
// exactly (ONE hand-rolled implementation per host — a duration is deliberately
// LOCALE-INDEPENDENT: "1h 20m" / "1:20:00" / "1 hour 20 minutes" are unit
// glyphs and English words, not CLDR-driven forms — and the cell vocabulary has
// no locale dimension, so the English relative form IS the canonical cell
// rendering). The numeric source counts `unit`s (Seconds = 1, Minutes = 60,
// Hours = 3600); negatives render with a leading "-" once the rounded magnitude
// is nonzero. Rounding is HALF-TO-EVEN, matching .NET `round` (Rust's
// `f64::round` is half-away-from-zero, so the tie is handled explicitly).

fn round_half_even(v: f64) -> f64 {
    let f = v.floor();
    let diff = v - f;
    // On the exact .5 tie, round to the even neighbour; otherwise nearest.
    let up = diff > 0.5 || (diff == 0.5 && (f as i64) % 2 != 0);
    if up { f + 1.0 } else { f }
}

fn duration_unit_seconds(u: DurationUnit) -> f64 {
    match u {
        DurationUnit::Seconds => 1.0,
        DurationUnit::Minutes => 60.0,
        DurationUnit::Hours => 3600.0,
    }
}

/// Render `value` (a signed count of `unit`s) per the bounded `DurationStyle`.
pub fn format_duration(unit: DurationUnit, style: DurationStyle, value: f64) -> String {
    let total_seconds = value * duration_unit_seconds(unit);
    let total = round_half_even(total_seconds.abs()) as i64;
    let sign = if total_seconds < 0.0 && total > 0 {
        "-"
    } else {
        ""
    };
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    let body = match style {
        DurationStyle::Compact => {
            // Largest two grains, zero tails omitted: "1h 20m" / "2h" /
            // "5m 30s" / "42s"; zero → "0s".
            if hours >= 1 {
                if minutes > 0 {
                    format!("{hours}h {minutes}m")
                } else {
                    format!("{hours}h")
                }
            } else if minutes >= 1 {
                if seconds > 0 {
                    format!("{minutes}m {seconds}s")
                } else {
                    format!("{minutes}m")
                }
            } else {
                format!("{seconds}s")
            }
        }
        DurationStyle::Clock => {
            // "h:mm:ss" from one hour up, "m:ss" below it.
            if hours >= 1 {
                format!("{hours}:{minutes:02}:{seconds:02}")
            } else {
                format!("{minutes}:{seconds:02}")
            }
        }
        DurationStyle::Long => {
            // English words, singular/plural, zero components omitted;
            // zero → "0 minutes".
            let part = |n: i64, word: &str| -> Option<String> {
                if n == 0 {
                    None
                } else if n == 1 {
                    Some(format!("1 {word}"))
                } else {
                    Some(format!("{n} {word}s"))
                }
            };
            let parts: Vec<String> = [
                part(hours, "hour"),
                part(minutes, "minute"),
                part(seconds, "second"),
            ]
            .into_iter()
            .flatten()
            .collect();
            if parts.is_empty() {
                "0 minutes".to_string()
            } else {
                parts.join(" ")
            }
        }
    };
    format!("{sign}{body}")
}

/// English relative-time rendering over a signed count of `unit` — "in 2
/// hours" / "3 minutes ago" / "this minute" (Phase 819). The cell vocabulary
/// has no locale dimension, so the English form IS the canonical cell
/// rendering.
pub fn format_relative_english(unit: RelativeTimeUnit, value: f64) -> String {
    let n = round_half_even(value) as i64;
    let unit_word = unit.as_str().to_lowercase();
    if n == 0 {
        format!("this {unit_word}")
    } else {
        let magnitude = n.abs();
        let plural = if magnitude == 1 {
            unit_word
        } else {
            format!("{unit_word}s")
        };
        if n < 0 {
            format!("{magnitude} {plural} ago")
        } else {
            format!("in {magnitude} {plural}")
        }
    }
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
        CellFormat::Duration { style, unit } => format_duration(*unit, *style, value),
        CellFormat::RelativeTime { unit } => format_relative_english(*unit, value),
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
        Format::Duration { style, unit } => {
            // Phase 819 — locale-independent by design (see `format_duration`
            // above): the one Format case with exact cross-host parity, so it
            // does not go through the invariant-fallback caveat.
            format_duration(*unit, *style, value)
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
