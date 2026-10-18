//! The canonical-JSON encoder: typed tree → the deterministic, byte-stable wire
//! form pinned by `WIRE_FORMAT.md` §2. Certified byte-for-byte against the
//! shared conformance corpus by `tests/conformance.rs`.
//!
//! Phase 2333: the per-owner encoders are GENERATED (`gen_encode` in
//! `generated.rs`) from the IDL; this file keeps the canonical-JSON helpers they
//! write through, the host-modelled types' encoders, the `FormFieldKind`
//! encoder (it takes the enclosing control's auto-bind context), and the hooks
//! `dialect.rs` declares. A hook takes every member its field or owner carries,
//! and every `match` here is exhaustive with no `_ =>` arm, so a field or case
//! the IDL gains is written by the generator or is a build error here, never
//! silently absent from the canonical bytes.

use crate::canonical::{JVal, escape_string, format_number, render_array, render_object};

use std::collections::BTreeMap;

use super::generated::gen_encode::*;
use super::model::*;

pub(super) type Field = (String, String);

pub(super) const CLOSURE: &str = "\"<closure>\"";
pub(super) const OPAQUE: &str = "\"<opaque>\"";

pub(super) fn s(v: &str) -> String {
    escape_string(v)
}

pub(super) fn num(n: f64) -> String {
    format_number(n)
}

pub(super) fn int(n: i64) -> String {
    n.to_string()
}

pub(super) fn boolean(b: bool) -> String {
    if b { "true" } else { "false" }.to_string()
}

pub(super) fn obj(fields: Vec<Field>) -> String {
    let mut fields = fields;
    render_object(&mut fields)
}

pub(super) fn arr(items: Vec<String>) -> String {
    render_array(&items)
}

pub(super) fn field(key: &str, value: String) -> Field {
    (key.to_string(), value)
}

/// A `$type`-discriminated DU object (§2 rule 8).
pub(super) fn case_obj(discriminator: &str, mut fields: Vec<Field>) -> String {
    fields.push(field("$type", s(discriminator)));
    obj(fields)
}

/// A structured `JVal` payload (rule 12) — rendered faithfully at any depth via
/// the canonical renderer.
pub(super) fn json_value(v: &JVal) -> String {
    crate::canonical::render_canonical(v)
}

/// Best-effort `obj`-typed payload (§2 rule 11): recognised JSON primitives
/// encode natively; arrays / objects collapse to the `"<opaque>"` sentinel —
/// the residual-opaque boundary (§5).
pub(super) fn obj_value(v: &JVal) -> String {
    match v {
        JVal::Null => "null".to_string(),
        JVal::Str(x) => s(x),
        JVal::Bool(b) => boolean(*b),
        JVal::Num(n) => num(*n),
        JVal::Arr(_) | JVal::Obj(_) => OPAQUE.to_string(),
    }
}

/// fuaran#665 — one row object: Ordinal-sorted keys (via `obj`) over best-effort
/// scalar cells. A `null` cell is OMITTED rather than emitted as `"k":null` —
/// rule 4, absence is structural — and a nested array/object cell rides
/// `obj_value`'s `"<opaque>"` sentinel, the residual boundary narrowed to the
/// cell seam.
pub(super) fn row(r: &Row) -> String {
    obj(r
        .iter()
        .filter(|(_, v)| !matches!(v, JVal::Null))
        .map(|(k, v)| field(k, obj_value(v)))
        .collect())
}

/// Phase 677 — absence is STRUCTURAL, never a value. These payloads carry no
/// value, so their key is omitted rather than emitting JSON `null`, for which
/// the wire model has no case. A typed empty (`Options(vec![])`) is NOT absent:
/// "no selection" and "selected nothing" stay distinguishable.
///
/// Phase 1656 — this is the `Static.value` rule ONLY. `State.defaultValue`
/// reads its own wire fact (`Binding::State`'s `default_declared`), so this
/// function is no longer consulted there; it used to stand in for that fact and
/// could not tell an undeclared default at a numeric or bool slot from a
/// declared zero.
pub(super) fn static_is_absent(v: &StaticValue) -> bool {
    matches!(
        v,
        StaticValue::StringOpt(None) | StaticValue::Ast(JVal::Null)
    )
}

pub(super) fn static_value(v: &StaticValue) -> String {
    match v {
        StaticValue::Ast(j) => obj_value(j),
        StaticValue::Options(options) => arr(options.iter().map(select_option).collect()),
        // Phase 677: unreachable — an absent payload omits its key upstream.
        StaticValue::StringOpt(None) => String::new(),
        StaticValue::StringOpt(Some(x)) => s(x),
        StaticValue::StringList(items) => arr(items.iter().map(|x| s(x)).collect()),
        StaticValue::FloatSeq(items) => arr(items.iter().map(|&x| num(x)).collect()),
        StaticValue::Markers(items) => arr(items.iter().map(map_marker).collect()),
        StaticValue::Rows(rows) => arr(rows.iter().map(row).collect()),
        // The Range pair rides as the bare `{max, min}` object (Phase 423
        // shape, kept at 0.2.0) — the `Static` envelope is stripped at the
        // Range value position itself (see `form_field_kind`).
        StaticValue::FloatPair(min, max) => {
            obj(vec![field("max", num(*max)), field("min", num(*min))])
        }
        // Likewise the DateRange pair rides as the bare `{from, to}` object
        // (0.7.0) — envelope stripped at the value position (`form_field_kind`).
        StaticValue::StringPair(from, to) => obj(vec![field("from", s(from)), field("to", s(to))]),
    }
}

// ─── Compute layer ───────────────────────────────────────────────────────────

pub(super) fn cell_column_wire(ty: ColumnType, c: &Cell) -> String {
    match c {
        Cell::Null => match ty {
            ColumnType::Bool => boolean(false),
            ColumnType::Int | ColumnType::Float => "0".to_string(),
            ColumnType::Str | ColumnType::Date | ColumnType::Timestamp => s(""),
        },
        Cell::Int(v) => {
            if ty == ColumnType::Float {
                num(*v as f64)
            } else {
                int(*v)
            }
        }
        Cell::Float(v) => num(*v),
        Cell::Bool(v) => boolean(*v),
        Cell::Str(v) | Cell::Date(v) | Cell::Timestamp(v) => s(v),
    }
}

pub(super) fn data_column(c: &DataColumn) -> String {
    obj(vec![
        field(
            "values",
            arr(c
                .cells
                .iter()
                .map(|cell| cell_column_wire(c.column_type, cell))
                .collect()),
        ),
        field(
            "validity",
            arr(c
                .cells
                .iter()
                .map(|cell| boolean(!matches!(cell, Cell::Null)))
                .collect()),
        ),
    ])
}

pub(super) fn schema_json(schema: &[SchemaEntry]) -> String {
    arr(schema
        .iter()
        .map(|e| {
            obj(vec![
                field("name", s(&e.name)),
                field("type", s(e.column_type.as_str())),
            ])
        })
        .collect())
}

pub(super) fn data_source(src: &DataSource) -> String {
    match src {
        DataSource::Ref { name } => obj(vec![field("schema", arr(vec![])), field("ref", s(name))]),
        DataSource::Embedded { schema, columns } => {
            let empty = |e: &SchemaEntry| DataColumn {
                name: e.name.clone(),
                column_type: ColumnType::Str,
                cells: vec![],
            };
            let cols: Vec<Field> = schema
                .iter()
                .map(|e| {
                    let col = columns.iter().find(|c| c.name == e.name);
                    (
                        e.name.clone(),
                        data_column(&col.cloned().unwrap_or_else(|| empty(e))),
                    )
                })
                .collect();
            obj(vec![
                field("schema", schema_json(schema)),
                field("columns", obj(cols)),
            ])
        }
    }
}

pub(super) fn cell_lit(c: &Cell) -> String {
    match c {
        Cell::Null => case_obj("Null", vec![]),
        Cell::Int(v) => case_obj("Int", vec![field("value", int(*v))]),
        Cell::Float(v) => case_obj("Float", vec![field("value", num(*v))]),
        Cell::Bool(v) => case_obj("Bool", vec![field("value", boolean(*v))]),
        Cell::Str(v) => case_obj("Str", vec![field("value", s(v))]),
        Cell::Date(v) => case_obj("Date", vec![field("value", s(v))]),
        Cell::Timestamp(v) => case_obj("Timestamp", vec![field("value", s(v))]),
    }
}

pub(super) fn col_expr(e: &ColExpr) -> String {
    match e {
        ColExpr::Col { name } => case_obj("col", vec![field("name", s(name))]),
        ColExpr::Param { name } => case_obj("param", vec![field("name", s(name))]),
        ColExpr::Lit { cell } => case_obj("lit", vec![field("cell", cell_lit(cell))]),
        ColExpr::Binary { op, left, right } => case_obj(
            "binary",
            vec![
                field("op", s(op.as_str())),
                field("left", col_expr(left)),
                field("right", col_expr(right)),
            ],
        ),
        ColExpr::Not { expr } => case_obj("not", vec![field("expr", col_expr(expr))]),
        ColExpr::Coalesce { exprs } => case_obj(
            "coalesce",
            vec![field("exprs", arr(exprs.iter().map(col_expr).collect()))],
        ),
        ColExpr::Case { cases, else_expr } => case_obj(
            "case",
            vec![
                field(
                    "cases",
                    arr(cases
                        .iter()
                        .map(|c| {
                            obj(vec![
                                field("when", col_expr(&c.when)),
                                field("then", col_expr(&c.then)),
                            ])
                        })
                        .collect()),
                ),
                field("else", col_expr(else_expr)),
            ],
        ),
        ColExpr::Cast { column_type, expr } => case_obj(
            "cast",
            vec![
                field("type", s(column_type.as_str())),
                field("expr", col_expr(expr)),
            ],
        ),
        ColExpr::Apply { func, args } => case_obj(
            "apply",
            vec![
                field("fn", s(func.as_str())),
                field("args", arr(args.iter().map(col_expr).collect())),
            ],
        ),
        ColExpr::InList { subject, items } => case_obj(
            "in",
            vec![
                field("expr", col_expr(subject)),
                field("items", arr(items.iter().map(col_expr).collect())),
            ],
        ),
        ColExpr::InParam { subject, name } => case_obj(
            "in",
            vec![field("expr", col_expr(subject)), field("param", s(name))],
        ),
        ColExpr::IsNull { expr } => case_obj("isNull", vec![field("expr", col_expr(expr))]),
    }
}

pub(super) fn col_pair(p: &ColPair) -> String {
    obj(vec![field("a", s(&p.a)), field("b", s(&p.b))])
}

pub(super) fn agg_json(a: &Agg) -> String {
    obj(vec![
        field("name", s(&a.name)),
        field("fn", s(a.func.as_str())),
        field("of", s(&a.of)),
    ])
}

// 0.28.0 — the member is `column`, not `col`: a wire member whose only honest name is
// "the column" is spelled out in full. `col` remains a decode alias and is never emitted.
// The model field keeps its own name; this is a wire rename, not an API one.
pub(super) fn order_json(k: &SortKey) -> String {
    obj(vec![
        field("column", s(&k.col)),
        field("dir", s(k.dir.as_str())),
    ])
}

pub(super) fn transform_step(t: &TransformStep) -> String {
    match t {
        TransformStep::Filter { pred } => case_obj("filter", vec![field("pred", col_expr(pred))]),
        TransformStep::Project { cols } => case_obj(
            "project",
            vec![field("columns", arr(cols.iter().map(col_pair).collect()))],
        ),
        TransformStep::Derive { name, expr } => case_obj(
            "derive",
            vec![field("name", s(name)), field("expr", col_expr(expr))],
        ),
        TransformStep::GroupBy { keys, aggs } => case_obj(
            "groupBy",
            vec![
                field("keys", arr(keys.iter().map(|k| s(k)).collect())),
                field("aggs", arr(aggs.iter().map(agg_json).collect())),
            ],
        ),
        TransformStep::Join { source, on, how } => case_obj(
            "join",
            vec![
                field("source", data_source(source)),
                field("on", arr(on.iter().map(col_pair).collect())),
                field("how", s(how.as_str())),
            ],
        ),
        TransformStep::Window {
            partition_by,
            order_by,
            func,
            of,
            alias,
        } => case_obj(
            "window",
            vec![
                field(
                    "partitionBy",
                    arr(partition_by.iter().map(|k| s(k)).collect()),
                ),
                field("orderBy", arr(order_by.iter().map(order_json).collect())),
                field("fn", s(func.as_str())),
                field("of", s(of)),
                field("as", s(alias)),
            ],
        ),
        TransformStep::Pivot {
            index,
            on,
            values,
            agg,
        } => case_obj(
            "pivot",
            vec![
                field("index", arr(index.iter().map(|k| s(k)).collect())),
                field("on", s(on)),
                field("values", s(values)),
                field("agg", s(agg.as_str())),
            ],
        ),
        TransformStep::Unpivot {
            id_vars,
            value_vars,
        } => case_obj(
            "unpivot",
            vec![
                field("idVars", arr(id_vars.iter().map(|k| s(k)).collect())),
                field("valueVars", arr(value_vars.iter().map(|k| s(k)).collect())),
            ],
        ),
        TransformStep::Sort { by } => case_obj(
            "sort",
            vec![field("by", arr(by.iter().map(order_json).collect()))],
        ),
        TransformStep::Distinct => case_obj("distinct", vec![]),
        TransformStep::Limit { n, offset } => case_obj(
            "limit",
            vec![field("n", int(*n)), field("offset", int(*offset))],
        ),
        TransformStep::Union { source } => {
            case_obj("union", vec![field("source", data_source(source))])
        }
    }
}

/// One `TextSource.I18n` argument (§5, Phase 1661). A LITERAL argument emits the
/// BARE value through the faithful rule-12 renderer — that is the whole reason
/// the widening moves no shipped byte — and a bound argument emits its own
/// `$type` object, a valueless `Static` included, since absence is structural
/// and has no bare spelling.
pub(super) fn i18n_arg(a: &I18nArg) -> String {
    match a {
        I18nArg::Literal(v) => json_value(v),
        I18nArg::Bound(b) => binding(b),
    }
}

// ─── Controls (FormFieldKind, encoded in its auto-bind context) ───────────────

pub(super) fn handler_field(name: &str, present: &Option<Closure>) -> Vec<Field> {
    if present.is_some() {
        vec![field(name, CLOSURE.to_string())]
    } else {
        vec![]
    }
}

/// Phase 596 — the auto-bind context for a control's `value` slot, mirroring
/// the decoder's synthesis exactly. A `value` that is the context's exact
/// auto-binding is OMITTED — `Filter(name, None)` on a chip (0.2.0),
/// `State(field id, typed placeholder)` on a form field (0.2.1) — so the
/// canonical minimal control carries no `value` key at all.
#[derive(Clone, Copy)]
pub(super) enum ControlAutoBind<'a> {
    FilterChip(&'a str),
    FormFieldId(&'a str),
}

/// The typed auto-bind placeholders — must stay byte-identical to the decode
/// side's `control_value_defaults`.
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
    pub fn tokens() -> StaticValue {
        StaticValue::StringList(Vec::new())
    }
    pub fn rating() -> StaticValue {
        StaticValue::Ast(JVal::Num(0.0))
    }
    pub fn color() -> StaticValue {
        StaticValue::Ast(JVal::Str("#000000".to_string()))
    }
}

/// The `value` field for a control slot: omitted when it is exactly the
/// context's auto-binding; any other binding emits.
pub(super) fn control_value_field(
    auto_bind: ControlAutoBind<'_>,
    placeholder: StaticValue,
    value: &Binding,
) -> Vec<Field> {
    let is_auto = match (auto_bind, value) {
        (
            ControlAutoBind::FilterChip(n),
            Binding::Filter {
                name,
                default_value: None,
            },
        ) => name == n,
        (
            ControlAutoBind::FormFieldId(id),
            // The declaration is deliberately NOT consulted here. This
            // asks "is this binding exactly the auto-binding", and a
            // document that spells the auto-binding out in full still
            // IS it — which is precisely what
            // `lenient/lenient-596-form-explicit-auto-state` pins.
            Binding::State {
                key, default_value, ..
            },
        ) => key == id && *default_value == placeholder,
        _ => false,
    };
    if is_auto {
        vec![]
    } else {
        vec![field("value", binding(value))]
    }
}

pub(super) fn form_field_kind(auto_bind: ControlAutoBind<'_>, k: &FormFieldKind) -> String {
    match k {
        FormFieldKind::Text { value, on_change } => {
            let mut fields = handler_field("onChange", on_change);
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::text(),
                value,
            ));
            case_obj("Text", fields)
        }
        FormFieldKind::Number { value, on_change } => {
            let mut fields = handler_field("onChange", on_change);
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::number(),
                value,
            ));
            case_obj("Number", fields)
        }
        FormFieldKind::Checkbox { value, on_change } => {
            let mut fields = handler_field("onChange", on_change);
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::checkbox(),
                value,
            ));
            case_obj("Checkbox", fields)
        }
        // Phase 766 — the switch affordance: Checkbox's mechanics under a
        // distinct tag.
        FormFieldKind::Toggle { value, on_change } => {
            let mut fields = handler_field("onChange", on_change);
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::checkbox(),
                value,
            ));
            case_obj("Toggle", fields)
        }
        FormFieldKind::Choice {
            options,
            value,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            fields.push(field("options", binding(options)));
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::choice(),
                value,
            ));
            case_obj("Choice", fields)
        }
        // Phase 1113 - `allowFreeText` omits at `false`: the SHORTEST combobox
        // document is the CONSTRAINED one, so a host that emitted the member
        // unconditionally would diverge on every constrained fixture.
        FormFieldKind::Combobox {
            options,
            value,
            allow_free_text,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            if *allow_free_text != defaults::FORM_FIELD_KIND_COMBOBOX_ALLOW_FREE_TEXT {
                fields.push(field("allowFreeText", boolean(*allow_free_text)));
            }
            fields.push(field("options", binding(options)));
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::choice(),
                value,
            ));
            case_obj("Combobox", fields)
        }
        // 0.2.0 — the dual-thumb range (absorbed FilterKind.RangeFilter). The
        // Static pair rides as the bare `{max, min}` object — no envelope.
        FormFieldKind::Range {
            value,
            min,
            max,
            step,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            let is_auto = match (auto_bind, value) {
                (
                    ControlAutoBind::FilterChip(n),
                    Binding::Filter {
                        name,
                        default_value: None,
                    },
                ) => name == n,
                (
                    ControlAutoBind::FormFieldId(id),
                    // The declaration is deliberately NOT consulted here. This
                    // asks "is this binding exactly the auto-binding", and a
                    // document that spells the auto-binding out in full still
                    // IS it — which is precisely what
                    // `lenient/lenient-596-form-explicit-auto-state` pins.
                    Binding::State {
                        key, default_value, ..
                    },
                ) => key == id && *default_value == control_value_defaults::range(),
                _ => false,
            };
            if !is_auto {
                let rendered = match value {
                    Binding::Static {
                        value: StaticValue::FloatPair(a, b),
                    } => obj(vec![field("max", num(*b)), field("min", num(*a))]),
                    other => binding(other),
                };
                fields.push(field("value", rendered));
            }
            if let Some(min) = min {
                fields.push(field("min", num(*min)));
            }
            if let Some(max) = max {
                fields.push(field("max", num(*max)));
            }
            if let Some(step) = step {
                fields.push(field("step", num(*step)));
            }
            case_obj("Range", fields)
        }
        FormFieldKind::RangedNumber {
            value,
            min,
            max,
            step,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::number(),
                value,
            ));
            if let Some(min) = min {
                fields.push(field("min", num(*min)));
            }
            if let Some(max) = max {
                fields.push(field("max", num(*max)));
            }
            if let Some(step) = step {
                fields.push(field("step", num(*step)));
            }
            case_obj("RangedNumber", fields)
        }
        FormFieldKind::SegmentedChoice {
            options,
            orientation,
            value,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            fields.push(field("options", binding(options)));
            // Phase 2300 — omitted at the IDL's declared default `Horizontal`.
            if *orientation
                != super::generated::defaults::FORM_FIELD_KIND_SEGMENTED_CHOICE_ORIENTATION
            {
                fields.push(field("orientation", s(orientation.as_str())));
            }
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::choice(),
                value,
            ));
            case_obj("SegmentedChoice", fields)
        }
        FormFieldKind::TextArea {
            rows,
            value,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            fields.push(field("rows", num(*rows as f64)));
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::text(),
                value,
            ));
            case_obj("TextArea", fields)
        }
        FormFieldKind::DateTime {
            value,
            variant,
            min,
            max,
            step,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::date_time(),
                value,
            ));
            fields.push(field("variant", s(variant.as_str())));
            if let Some(min) = min {
                fields.push(field("min", s(min)));
            }
            if let Some(max) = max {
                fields.push(field("max", s(max)));
            }
            if let Some(step) = step {
                fields.push(field("step", num(*step)));
            }
            case_obj("DateTime", fields)
        }
        // 0.7.0 — the single-control date range. Modelled on the `Range` arm, NOT
        // on `Date`: `control_value_field` emits `binding(value)`, which wraps a
        // `Static` in the `{"$type":"Static","value":…}` envelope — wrong for this
        // slot, whose canonical form is the bare `{from, to}` object.
        FormFieldKind::DateTimeRange {
            value,
            variant,
            min,
            max,
            step,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            let is_auto = match (auto_bind, value) {
                (
                    ControlAutoBind::FilterChip(n),
                    Binding::Filter {
                        name,
                        default_value: None,
                    },
                ) => name == n,
                (
                    ControlAutoBind::FormFieldId(id),
                    // The declaration is deliberately NOT consulted here. This
                    // asks "is this binding exactly the auto-binding", and a
                    // document that spells the auto-binding out in full still
                    // IS it — which is precisely what
                    // `lenient/lenient-596-form-explicit-auto-state` pins.
                    Binding::State {
                        key, default_value, ..
                    },
                ) => key == id && *default_value == control_value_defaults::date_time_range(),
                _ => false,
            };
            if !is_auto {
                let rendered = match value {
                    Binding::Static {
                        value: StaticValue::StringPair(from, to),
                    } => obj(vec![field("from", s(from)), field("to", s(to))]),
                    other => binding(other),
                };
                fields.push(field("value", rendered));
            }
            fields.push(field("variant", s(variant.as_str())));
            if let Some(min) = min {
                fields.push(field("min", s(min)));
            }
            if let Some(max) = max {
                fields.push(field("max", s(max)));
            }
            if let Some(step) = step {
                fields.push(field("step", num(*step)));
            }
            case_obj("DateTimeRange", fields)
        }
        // Phase 1121 — `allowFreeText` omits at TRUE here (the opposite of
        // `Combobox`), so the shortest document `{"$type":"Tokens"}` is the
        // plain open token box.
        FormFieldKind::Tokens {
            value,
            suggestions,
            allow_free_text,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            if *allow_free_text != defaults::FORM_FIELD_KIND_TOKENS_ALLOW_FREE_TEXT {
                fields.push(field("allowFreeText", boolean(*allow_free_text)));
            }
            if let Some(suggestions) = suggestions {
                fields.push(field("suggestions", binding(suggestions)));
            }
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::tokens(),
                value,
            ));
            case_obj("Tokens", fields)
        }
        // Phase 1130 — `max` is the scale and always emits; `allowHalf` omits
        // at false, the SHORTEST rating document being the whole-star one.
        FormFieldKind::Rating {
            value,
            max,
            allow_half,
            on_change,
        } => {
            let mut fields = handler_field("onChange", on_change);
            if *allow_half != defaults::FORM_FIELD_KIND_RATING_ALLOW_HALF {
                fields.push(field("allowHalf", boolean(*allow_half)));
            }
            fields.push(field("max", int(*max)));
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::rating(),
                value,
            ));
            case_obj("Rating", fields)
        }
        // Phase 1130 — case is PRESERVED, never normalised: `#FFAA00` rides
        // through the ordinary string binding unchanged.
        FormFieldKind::Color { value, on_change } => {
            let mut fields = handler_field("onChange", on_change);
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::color(),
                value,
            ));
            case_obj("Color", fields)
        }
    }
}

pub(super) fn fragment_arg(a: &FragmentArg) -> String {
    match a {
        FragmentArg::Value(scalar) => fragment_scalar(scalar),
        FragmentArg::Slot { tree } => case_obj("SlotArg", vec![field("tree", node(tree))]),
    }
}

/// The flat wire hoists a spec's fields directly under the kind's `$type`
/// discriminator (§3.2) — realised here by adding `$type` to the spec's own
/// field set before the canonical sort, which lands it first (`$` sorts before
/// every lower-case field name).
pub(super) fn case_obj_hoisted(discriminator: &str, spec_json: String) -> String {
    // spec_json is "{…}" with Ordinal-sorted fields; splice "$type" in front.
    let head = format!("{{\"$type\":{}", s(discriminator));
    if spec_json.len() > 2 {
        format!("{head},{}", &spec_json[1..])
    } else {
        format!("{head}}}")
    }
}

// ─── The generated codec's helpers ───────────────────────────────────────────

/// A list position: each element through its own encoder, in order.
pub(super) fn enc_list<T>(items: &[T], f: impl Fn(&T) -> String) -> String {
    arr(items.iter().map(f).collect())
}

/// A map position (an ordered list of name-value pairs): `obj` sorts the keys.
pub(super) fn enc_map<T>(entries: &[(String, T)], f: impl Fn(&T) -> String) -> String {
    obj(entries.iter().map(|(k, v)| (k.clone(), f(v))).collect())
}

/// Phase 818 — a `Data` source keeps the columnar encoding byte-identical; a
/// `Live` source re-encodes the preserved binding itself (one wire dialect — the
/// State/Selection/Query-shaped source round-trips byte-for-byte; the derived
/// `initial` snapshot is never encoded).
pub(super) fn transform_source(source: &TransformSource) -> String {
    match source {
        TransformSource::Data(ds) => data_source(ds),
        TransformSource::Live { binding: b, .. } => binding(b),
    }
}

// ─── Encode hooks (declared in dialect.rs) ───────────────────────────────────
//
// Each takes a reference to every Rust member its field maps to and returns the
// field's wire value, or `None` to omit the key. An owner hook takes every member
// of its owner and returns the owner's whole JSON.

/// `Custom.exposedNodeIds` — an absent list decodes empty, so an empty one is not
/// written.
pub(super) fn encode_exposed_node_ids(ids: &[NodeId]) -> Option<String> {
    (!ids.is_empty()).then(|| enc_list(ids, |id| s(id)))
}

/// `FragmentDecl.effect` — omitted at the pure deterministic class, the IDL's
/// record default.
pub(super) fn encode_fragment_effect(effect: &EffectClass) -> Option<String> {
    (!effect.is_pure_deterministic()).then(|| effect_class(effect))
}

/// `FragmentRef.args` / `Mount.inputs` — an absent map decodes empty, so an empty
/// one is not written.
pub(super) fn encode_fragment_args(args: &[(String, FragmentArg)]) -> Option<String> {
    (!args.is_empty()).then(|| enc_map(args, fragment_arg))
}

/// An integer-binding slot omitted at its default, the literal binding `0`
/// (the IDL's `omitDefault Static{value=0}`, which the defaults table does not
/// render).
fn omit_literal_zero(active: &Binding) -> Option<String> {
    let is_identity = matches!(
        active,
        Binding::Static {
            value: StaticValue::Ast(JVal::Num(n)),
        } if *n == 0.0
    );
    (!is_identity).then(|| binding(active))
}

/// `Tabs.activeIndex` — omitted at its default, the literal binding `0`, so a
/// tabs node that starts on its first tab says nothing about it.
pub(super) fn encode_tabs_active_index(active: &Binding) -> Option<String> {
    omit_literal_zero(active)
}

/// `Stepper.activeStep` — the same slot shape as `Tabs.activeIndex`, declared
/// omit-at-default `Static{value=0}` by Phase 2300.
pub(super) fn encode_stepper_active_step(active: &Binding) -> Option<String> {
    omit_literal_zero(active)
}

/// `Accessibility.role` — the hosted ARIA role rides as its token string.
pub(super) fn encode_aria_role(role: &Option<String>) -> Option<String> {
    role.as_deref().map(s)
}

/// `SelectOption.label` / `MapMarker.label` — the host reads a full `TextSource`
/// where the IDL types a string; a literal re-encodes as the bare string.
pub(super) fn encode_text_label(label: &TextSource) -> Option<String> {
    Some(text_source(label))
}

/// `Binding.Static.value` — Phase 677: absence is structural, never a value, so an
/// absent payload omits the key rather than emitting `null`. A typed empty
/// (`Options(vec![])`) is NOT absent.
pub(super) fn encode_static_payload(value: &StaticValue) -> Option<String> {
    (!static_is_absent(value)).then(|| static_value(value))
}

/// `Binding.State.defaultValue` — §5's absent-default posture (Phase 1656): the
/// DECLARATION decides and nothing else. A typed slot's decoded default is the
/// placeholder an unwritten key resolves to, and emitting it would put a
/// `defaultValue` on the wire the document never carried.
pub(super) fn encode_state_default(value: &StaticValue, declared: &bool) -> Option<String> {
    declared.then(|| static_value(value))
}

/// `Binding.Query.dependsOn` — an empty list says nothing, so it is not written.
pub(super) fn encode_depends_on(deps: &Option<Vec<String>>) -> Option<String> {
    deps.as_deref()
        .filter(|d| !d.is_empty())
        .map(|d| enc_list(d, |x| s(x)))
}

/// `Binding.Expr.params` / `Binding.Transform.params` — §3.3 omits the slot when
/// empty: `Some(vec![])` is the same fact as `None` on the wire.
pub(super) fn encode_binding_params(params: &Option<Vec<TransformParam>>) -> Option<String> {
    params
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(|p| enc_list(p, transform_param))
}

/// `CellKindErased.ButtonGroup.buttons` — each item is inlined to its label; its
/// click handler is the closure sentinel.
pub(super) fn encode_button_group(labels: &[TextSource]) -> Option<String> {
    Some(enc_list(labels, |label| {
        obj(vec![
            field("label", text_source(label)),
            field("onClick", CLOSURE.to_string()),
        ])
    }))
}

/// `CellKindErased.TonedPill.map` — the lookup table, as an object keyed by the
/// row value (`obj` sorts, so the `BTreeMap`'s order is a convenience).
pub(super) fn encode_tone_map(map: &BTreeMap<String, ToneVariant>) -> Option<String> {
    Some(obj(map
        .iter()
        .map(|(value, tone)| field(value, s(tone.as_str())))
        .collect()))
}

/// `Node.state` — an absent state is the empty behaviour, so only a non-empty one
/// is written.
pub(super) fn encode_node_state(state: &StateBehaviour) -> Option<String> {
    (!state.is_empty()).then(|| state_behaviour(state))
}

/// `Node.style` — an absent style is the default style, so only a styled node
/// writes one (and inside it, each member is omitted at its default).
pub(super) fn encode_node_style(style: &SemanticStyle) -> Option<String> {
    (!style.is_default()).then(|| semantic_style(style))
}

/// The `Filters` kind — its variant holds the `items` list directly.
pub(super) fn encode_filters_kind(items: &[FilterSpec]) -> String {
    case_obj(
        "Filters",
        vec![field("items", enc_list(items, filter_spec))],
    )
}

/// `Switch` — `on` and `stateKey` fold into one binding: an undeclared-default
/// `State` binding is written as the `stateKey` shorthand, anything else as `on`.
pub(super) fn encode_switch_spec(
    auto_advance_ms: &Option<i64>,
    cases: &[SwitchCase],
    default: &Node,
    on: &Binding,
) -> String {
    let mut fields = vec![
        field("cases", enc_list(cases, switch_case)),
        field("default", node(default)),
        match on {
            Binding::State {
                key,
                default_declared: false,
                ..
            } => field("stateKey", s(key)),
            other => field("on", binding(other)),
        },
    ];
    if let Some(ms) = auto_advance_ms {
        fields.push(field("autoAdvanceMs", int(*ms)));
    }
    obj(fields)
}

/// `SwitchCase` — `match` and `when` fold into one `SwitchCondition`.
pub(super) fn encode_switch_case(child: &Node, condition: &SwitchCondition) -> String {
    let condition = match condition {
        SwitchCondition::Match(value) => field("match", s(value)),
        SwitchCondition::When(b) => field("when", binding(b)),
    };
    obj(vec![field("child", node(child)), condition])
}

/// `FormField` — its control's `value` is omitted when it is the auto-binding the
/// field id implies (see `form_field_kind`).
pub(super) fn encode_form_field(
    id: &str,
    kind: &FormFieldKind,
    label: &TextSource,
    required: &bool,
    help: &Option<TextSource>,
    rule: &Option<FieldRule>,
) -> String {
    let mut fields = vec![
        field("id", s(id)),
        field(
            "kind",
            form_field_kind(ControlAutoBind::FormFieldId(id), kind),
        ),
        field("label", text_source(label)),
    ];
    // Phase 2300 — omitted at its declared default `false`.
    if *required {
        fields.push(field("required", boolean(true)));
    }
    if let Some(help) = help {
        fields.push(field("help", text_source(help)));
    }
    if let Some(rule) = rule {
        fields.push(field("rule", field_rule(rule)));
    }
    obj(fields)
}

/// `FilterSpec` — its control's `value` is omitted when it is the auto-binding the
/// chip name implies (see `form_field_kind`).
pub(super) fn encode_filter_spec(kind: &FormFieldKind, label: &TextSource, name: &str) -> String {
    obj(vec![
        field(
            "kind",
            form_field_kind(ControlAutoBind::FilterChip(name), kind),
        ),
        field("label", text_source(label)),
        field("name", s(name)),
    ])
}

// ─── Public surface ──────────────────────────────────────────────────────────

/// Encode a [`Node`] to its canonical wire-JSON string, byte-identical to the
/// shared conformance corpus.
pub fn encode_node(n: &Node) -> String {
    node(n)
}

/// Encode a [`TreeOp`] to its canonical wire-JSON string.
pub fn encode_op(op: &TreeOp) -> String {
    tree_op(op)
}
