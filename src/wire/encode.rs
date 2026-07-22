//! The canonical-JSON encoder: typed tree → the deterministic, byte-stable wire
//! form pinned by `WIRE_FORMAT.md` §2. Certified byte-for-byte against the
//! shared conformance corpus by `tests/conformance.rs`.
//!
//! Every `match` over a wire DU here is compile-time exhaustive with no `_ =>`
//! arm — a new kind cannot ship without its encoder arm building.

use crate::canonical::{JVal, escape_string, format_number, render_array, render_object};

use super::model::*;

type Field = (String, String);

const CLOSURE: &str = "\"<closure>\"";
const OPAQUE: &str = "\"<opaque>\"";

fn s(v: &str) -> String {
    escape_string(v)
}

fn num(n: f64) -> String {
    format_number(n)
}

fn int(n: i64) -> String {
    n.to_string()
}

fn boolean(b: bool) -> String {
    if b { "true" } else { "false" }.to_string()
}

fn obj(fields: Vec<Field>) -> String {
    let mut fields = fields;
    render_object(&mut fields)
}

fn arr(items: Vec<String>) -> String {
    render_array(&items)
}

fn field(key: &str, value: String) -> Field {
    (key.to_string(), value)
}

/// A `$type`-discriminated DU object (§2 rule 8).
fn case_obj(discriminator: &str, mut fields: Vec<Field>) -> String {
    fields.push(field("$type", s(discriminator)));
    obj(fields)
}

/// A structured `JVal` payload (rule 12) — rendered faithfully at any depth via
/// the canonical renderer.
fn json_value(v: &JVal) -> String {
    crate::canonical::render_canonical(v)
}

/// A structured-JSON map position (Custom props, `TextSource.I18n` args).
fn json_map(entries: &[(String, JVal)]) -> String {
    obj(entries
        .iter()
        .map(|(k, v)| (k.clone(), json_value(v)))
        .collect())
}

/// Best-effort `obj`-typed payload (§2 rule 11): recognised JSON primitives
/// encode natively; arrays / objects collapse to the `"<opaque>"` sentinel —
/// the residual-opaque boundary (§5).
fn obj_value(v: &JVal) -> String {
    match v {
        JVal::Null => "null".to_string(),
        JVal::Str(x) => s(x),
        JVal::Bool(b) => boolean(*b),
        JVal::Num(n) => num(*n),
        JVal::Arr(_) | JVal::Obj(_) => OPAQUE.to_string(),
    }
}

// ─── Typed Static payloads (Phase 429) ───────────────────────────────────────

fn select_option(o: &SelectOption) -> String {
    obj(vec![
        field("label", text_source(&o.label)),
        field("value", s(&o.value)),
    ])
}

fn map_marker(m: &MapMarker) -> String {
    obj(vec![
        field("label", text_source(&m.label)),
        field("latitude", num(m.latitude)),
        field("longitude", num(m.longitude)),
    ])
}

fn static_value(v: &StaticValue) -> String {
    match v {
        StaticValue::Ast(j) => obj_value(j),
        StaticValue::Options(options) => arr(options.iter().map(select_option).collect()),
        StaticValue::StringOpt(None) => "null".to_string(),
        StaticValue::StringOpt(Some(x)) => s(x),
        StaticValue::StringList(items) => arr(items.iter().map(|x| s(x)).collect()),
        StaticValue::FloatSeq(items) => arr(items.iter().map(|&x| num(x)).collect()),
        StaticValue::Markers(items) => arr(items.iter().map(map_marker).collect()),
        // The Range pair rides as the bare `{max, min}` object (Phase 423
        // shape, kept at 0.2.0) — the `Static` envelope is stripped at the
        // Range value position itself (see `form_field_kind`).
        StaticValue::FloatPair(min, max) => {
            obj(vec![field("max", num(*max)), field("min", num(*min))])
        }
    }
}

// ─── Compute layer ───────────────────────────────────────────────────────────

fn cell_column_wire(ty: ColumnType, c: &Cell) -> String {
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

fn data_column(c: &DataColumn) -> String {
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

fn schema_json(schema: &[SchemaEntry]) -> String {
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

fn data_source(src: &DataSource) -> String {
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

fn cell_lit(c: &Cell) -> String {
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

fn col_expr(e: &ColExpr) -> String {
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

fn col_pair(p: &ColPair) -> String {
    obj(vec![field("a", s(&p.a)), field("b", s(&p.b))])
}

fn agg_json(a: &Agg) -> String {
    obj(vec![
        field("name", s(&a.name)),
        field("fn", s(a.func.as_str())),
        field("of", s(&a.of)),
    ])
}

fn order_json(k: &SortKey) -> String {
    obj(vec![
        field("col", s(&k.col)),
        field("dir", s(k.dir.as_str())),
    ])
}

fn transform_step(t: &TransformStep) -> String {
    match t {
        TransformStep::Filter { pred } => case_obj("filter", vec![field("pred", col_expr(pred))]),
        TransformStep::Project { cols } => case_obj(
            "project",
            vec![field("cols", arr(cols.iter().map(col_pair).collect()))],
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

fn invoke_args(args: &[InvokeArg]) -> String {
    arr(args
        .iter()
        .map(|a| obj(vec![field("addr", s(&a.addr)), field("value", s(&a.value))]))
        .collect())
}

// ─── Bindings / actions / text ───────────────────────────────────────────────

fn flush_trigger(t: &LocalFlushTrigger) -> String {
    match t {
        LocalFlushTrigger::OnBlur => case_obj("OnBlur", vec![]),
        LocalFlushTrigger::OnSubmit => case_obj("OnSubmit", vec![]),
        LocalFlushTrigger::OnCommitAction => case_obj("OnCommitAction", vec![]),
        LocalFlushTrigger::OnDebounce { milliseconds } => case_obj(
            "OnDebounce",
            vec![field("milliseconds", num(*milliseconds as f64))],
        ),
    }
}

fn binding(b: &Binding) -> String {
    match b {
        Binding::Static { value } => case_obj("Static", vec![field("value", static_value(value))]),
        // 0.2.0 — the no-information `accessor` closure sentinel is OFF the
        // wire (no decoder ever read it); a decoded Query synthesises the
        // identity projection.
        Binding::Query { name, depends_on } => {
            let mut fields = vec![field("name", s(name))];
            if let Some(deps) = depends_on
                && !deps.is_empty()
            {
                fields.push(field("dependsOn", arr(deps.iter().map(|d| s(d)).collect())));
            }
            case_obj("Query", fields)
        }
        Binding::Filter {
            name,
            default_value,
        } => {
            // 0.2.0 — `defaultValue` rides the wire when present (typed via
            // the slot's static encoding); omitted when None so the minimal
            // form stays byte-identical.
            let mut fields = vec![];
            if let Some(d) = default_value {
                fields.push(field("defaultValue", static_value(d)));
            }
            fields.push(field("name", s(name)));
            case_obj("Filter", fields)
        }
        Binding::Selection {
            node_id,
            default_value,
            field: sel_field,
        } => {
            // 0.2.9 — `defaultValue` (the Filter.defaultValue convention);
            // 0.2.10 — `field` (the declarative row-field projection). The
            // accessor closure is off the wire (0.2.0 strand 5).
            let mut fields = vec![];
            if let Some(d) = default_value {
                fields.push(field("defaultValue", static_value(d)));
            }
            if let Some(f) = sel_field {
                fields.push(field("field", s(f)));
            }
            fields.push(field("nodeId", s(node_id)));
            case_obj("Selection", fields)
        }
        Binding::State { key, default_value } => case_obj(
            "State",
            vec![
                field("defaultValue", static_value(default_value)),
                field("key", s(key)),
            ],
        ),
        Binding::Computed => case_obj("Computed", vec![field("fn", CLOSURE.to_string())]),
        Binding::I18n { key, args } => {
            let mut fields = vec![];
            if let Some(args) = args {
                fields.push(field(
                    "args",
                    obj(args.iter().map(|(k, v)| (k.clone(), binding(v))).collect()),
                ));
            }
            fields.push(field("key", s(key)));
            case_obj("I18n", fields)
        }
        Binding::Local {
            flush_on,
            initial_from,
        } => case_obj(
            "Local",
            vec![
                field("flushOn", flush_trigger(flush_on)),
                field("format", CLOSURE.to_string()),
                field("initialFrom", binding(initial_from)),
                field("onCommit", CLOSURE.to_string()),
                field("parse", CLOSURE.to_string()),
            ],
        ),
        Binding::Format {
            format,
            locale,
            source,
        } => case_obj(
            "Format",
            vec![
                field("format", format_intent(format)),
                field("locale", locale_source(locale)),
                field("source", binding(source)),
            ],
        ),
        Binding::Transform {
            params,
            pipeline,
            source,
        } => {
            let mut fields = vec![];
            if let Some(params) = params
                && !params.is_empty()
            {
                fields.push(field(
                    "params",
                    arr(params
                        .iter()
                        .map(|p| {
                            obj(vec![
                                field("from", binding(&p.from)),
                                field("name", s(&p.name)),
                            ])
                        })
                        .collect()),
                ));
            }
            fields.push(field(
                "pipeline",
                arr(pipeline.iter().map(transform_step).collect()),
            ));
            fields.push(field("source", data_source(source)));
            case_obj("Transform", fields)
        }
        Binding::Invoke {
            capability_id,
            args,
        } => case_obj(
            "Invoke",
            vec![
                field("args", invoke_args(args)),
                field("capabilityId", s(capability_id)),
            ],
        ),
    }
}

fn action(a: &Action) -> String {
    match a {
        // 0.2.0 — the `msg` sentinel is OFF the wire (pure token weight).
        Action::Dispatch => case_obj("Dispatch", vec![]),
        Action::Call {
            endpoint,
            into,
            on_result,
        } => {
            let mut fields = vec![field("endpoint", s(endpoint))];
            if let Some(target) = into {
                let rendered = match target {
                    CallResultTarget::State { key } => {
                        case_obj("State", vec![field("key", s(key))])
                    }
                    CallResultTarget::Query { name } => {
                        case_obj("Query", vec![field("name", s(name))])
                    }
                };
                fields.push(field("into", rendered));
            }
            if on_result.is_some() {
                fields.push(field("onResult", CLOSURE.to_string()));
            }
            case_obj("Call", fields)
        }
        Action::Notify { channel, payload } => case_obj(
            "Notify",
            vec![
                field("channel", s(channel)),
                field("payload", json_value(payload)),
            ],
        ),
        Action::Navigate { route } => case_obj("Navigate", vec![field("route", s(route))]),
        Action::SetState { key, value } => case_obj(
            "SetState",
            vec![field("key", s(key)), field("value", json_value(value))],
        ),
        Action::AiTool { tool_name, args } => case_obj(
            "AiTool",
            vec![
                field("args", json_value(args)),
                field("toolName", s(tool_name)),
            ],
        ),
        Action::Chain(actions) => case_obj(
            "Chain",
            vec![field("ops", arr(actions.iter().map(action).collect()))],
        ),
        Action::CommitLocal { node_id } => {
            case_obj("CommitLocal", vec![field("nodeId", s(node_id))])
        }
        Action::WriteToClipboard { text } => {
            case_obj("WriteToClipboard", vec![field("text", s(text))])
        }
        Action::ReadFileBody { file_ref, encoding } => case_obj(
            "ReadFileBody",
            vec![
                field("encoding", s(encoding.as_str())),
                field("fileRef", s(file_ref)),
                field("onRead", CLOSURE.to_string()),
            ],
        ),
        Action::Invoke {
            capability_id,
            args,
        } => case_obj(
            "Invoke",
            vec![
                field("args", invoke_args(args)),
                field("capabilityId", s(capability_id)),
            ],
        ),
    }
}

fn text_source(t: &TextSource) -> String {
    match t {
        // 0.2.0 direction-flip: the bare JSON string IS the canonical Literal
        // form; the `{"$type":"Literal","text":…}` envelope is decode-accepted
        // and normalises down (§16).
        TextSource::Literal(text) => s(text),
        TextSource::Bound(b) => case_obj("Bound", vec![field("binding", binding(b))]),
        TextSource::I18n { key, args } => case_obj(
            "I18n",
            vec![field("args", json_map(args)), field("key", s(key))],
        ),
    }
}

fn cell_format(f: &CellFormat) -> String {
    match f {
        CellFormat::None => case_obj("None", vec![]),
        CellFormat::Number { decimals } => case_obj(
            "Number",
            decimals
                .map(|d| vec![field("decimals", num(d as f64))])
                .unwrap_or_default(),
        ),
        CellFormat::Currency { code } => case_obj("Currency", vec![field("code", s(code))]),
        CellFormat::Percent { decimals } => case_obj(
            "Percent",
            decimals
                .map(|d| vec![field("decimals", num(d as f64))])
                .unwrap_or_default(),
        ),
        CellFormat::SignificantDigits { digits } => case_obj(
            "SignificantDigits",
            vec![field("digits", num(*digits as f64))],
        ),
        CellFormat::Date { format } => case_obj("Date", vec![field("format", s(format))]),
        CellFormat::Custom => case_obj("Custom", vec![field("fn", CLOSURE.to_string())]),
    }
}

fn format_intent(f: &Format) -> String {
    match f {
        Format::Number { decimals } => case_obj(
            "Number",
            decimals
                .map(|d| vec![field("decimals", num(d as f64))])
                .unwrap_or_default(),
        ),
        Format::Currency { iso_code } => case_obj("Currency", vec![field("isoCode", s(iso_code))]),
        Format::Percent { decimals } => case_obj(
            "Percent",
            decimals
                .map(|d| vec![field("decimals", num(d as f64))])
                .unwrap_or_default(),
        ),
        Format::Date { date_style } => {
            case_obj("Date", vec![field("dateStyle", s(date_style.as_str()))])
        }
        Format::RelativeTime { unit } => {
            case_obj("RelativeTime", vec![field("unit", s(unit.as_str()))])
        }
    }
}

fn locale_source(l: &LocaleSource) -> String {
    match l {
        LocaleSource::Ambient => case_obj("Ambient", vec![]),
        LocaleSource::Explicit { tag } => case_obj("Explicit", vec![field("tag", s(tag))]),
    }
}

fn column_width(w: &ColumnWidth) -> String {
    match w {
        ColumnWidth::Auto => case_obj("Auto", vec![]),
        ColumnWidth::Fixed { pixels } => {
            case_obj("Fixed", vec![field("pixels", num(*pixels as f64))])
        }
        ColumnWidth::Flex { weight } => case_obj("Flex", vec![field("weight", num(*weight))]),
    }
}

// ─── Display specs ───────────────────────────────────────────────────────────

fn metric_spec(spec: &MetricSpec) -> String {
    // Phase 460 — the stylistic fields are omitted-when-default on the wire
    // (WIRE_FORMAT.md §3.6); a default-styled Metric round-trips minimal.
    let mut fields = vec![
        field("label", text_source(&spec.label)),
        field("value", binding(&spec.value)),
    ];
    if spec.format != CellFormat::None {
        fields.push(field("format", cell_format(&spec.format)));
    }
    if spec.tone != ToneVariant::Default {
        fields.push(field("tone", s(spec.tone.as_str())));
    }
    if spec.weight != StyleWeight::Standard {
        fields.push(field("weight", s(spec.weight.as_str())));
    }
    if spec.emphasis != Emphasis::Normal {
        fields.push(field("emphasis", s(spec.emphasis.as_str())));
    }
    if let Some(trend) = &spec.trend {
        fields.push(field("trend", binding(trend)));
    }
    if let Some(trend_format) = &spec.trend_format {
        fields.push(field("trendFormat", cell_format(trend_format)));
    }
    if let Some(icon) = &spec.icon {
        fields.push(field("icon", s(icon)));
    }
    if let Some(subtext) = &spec.subtext {
        fields.push(field("subtext", text_source(subtext)));
    }
    obj(fields)
}

fn heading_spec(spec: &HeadingSpec) -> String {
    obj(vec![
        field("level", num(spec.level as f64)),
        field("text", text_source(&spec.text)),
        field("variant", s(spec.variant.as_str())),
    ])
}

fn label_value_row_spec(spec: &LabelValueRowSpec) -> String {
    // Phase 460 — `format` omitted-when-default; the behavioural `emphasis`
    // bool is omitted-when-false (0.2.2, aligning with Fact's identical flag).
    let mut fields = vec![
        field("label", text_source(&spec.label)),
        field("value", binding(&spec.value)),
    ];
    if spec.emphasis {
        fields.push(field("emphasis", boolean(true)));
    }
    if spec.format != CellFormat::None {
        fields.push(field("format", cell_format(&spec.format)));
    }
    if let Some(help) = &spec.help {
        fields.push(field("help", text_source(help)));
    }
    obj(fields)
}

fn fact_spec(spec: &FactSpec) -> String {
    // The labeled TEXT fact. `emphasis` is the behavioural bool
    // (omit-when-false); `tone` omit-when-default; `help` / `icon` optional.
    let mut fields = vec![
        field("label", text_source(&spec.label)),
        field("value", text_source(&spec.value)),
    ];
    if spec.emphasis {
        fields.push(field("emphasis", boolean(true)));
    }
    if spec.tone != ToneVariant::Default {
        fields.push(field("tone", s(spec.tone.as_str())));
    }
    if let Some(help) = &spec.help {
        fields.push(field("help", text_source(help)));
    }
    if let Some(icon) = &spec.icon {
        fields.push(field("icon", s(icon)));
    }
    obj(fields)
}

fn markdown_spec(spec: &MarkdownSpec) -> String {
    obj(vec![field("text", text_source(&spec.text))])
}

fn badge_spec(spec: &BadgeSpec) -> String {
    obj(vec![
        field("label", text_source(&spec.label)),
        field("variant", s(spec.variant.as_str())),
    ])
}

fn link_spec(spec: &LinkSpec) -> String {
    let mut fields = vec![
        field("href", binding(&spec.href)),
        field("label", text_source(&spec.label)),
        field("download", boolean(spec.download)),
    ];
    if let Some(rel) = &spec.rel {
        fields.push(field("rel", s(rel)));
    }
    if let Some(target) = &spec.target {
        fields.push(field("target", s(target)));
    }
    obj(fields)
}

fn image_spec(spec: &ImageSpec) -> String {
    obj(vec![
        field("alt", text_source(&spec.alt)),
        field("src", binding(&spec.src)),
        field("variant", s(spec.variant.as_str())),
    ])
}

fn list_spec(spec: &ListSpec) -> String {
    obj(vec![
        field("items", arr(spec.items.iter().map(text_source).collect())),
        field("ordered", boolean(spec.ordered)),
    ])
}

fn toast_spec(spec: &ToastSpec) -> String {
    // Phase 460 — `tone` omitted-when-default. 0.2.0 — `dismissable` is
    // omitted-when-TRUE (the one inverted default in §3.6's table).
    let mut fields = vec![
        field("message", text_source(&spec.message)),
        field("open", binding(&spec.open)),
    ];
    if !spec.dismissable {
        fields.push(field("dismissable", boolean(false)));
    }
    if spec.tone != ToneVariant::Default {
        fields.push(field("tone", s(spec.tone.as_str())));
    }
    obj(fields)
}

fn code_block_spec(spec: &CodeBlockSpec) -> String {
    obj(vec![
        field("code", s(&spec.code)),
        field("copyable", boolean(spec.copyable)),
        field(
            "highlightLines",
            arr(spec.highlight_lines.iter().map(|&l| int(l)).collect()),
        ),
        field("language", s(&spec.language)),
        field("lineNumbers", boolean(spec.line_numbers)),
    ])
}

fn math_spec(spec: &MathSpec) -> String {
    obj(vec![
        field("display", s(spec.display.as_str())),
        field("source", s(&spec.source)),
    ])
}

fn sparkline_spec(spec: &SparklineSpec) -> String {
    obj(vec![field("source", binding(&spec.source))])
}

fn skeleton_spec(spec: &SkeletonSpec) -> String {
    obj(vec![field("rows", num(spec.rows as f64))])
}

fn callout_spec(spec: &CalloutSpec) -> String {
    // 0.2.0 — `dismissable` omitted-when-default (false).
    let mut fields = vec![field("body", text_source(&spec.body))];
    if spec.dismissable {
        fields.push(field("dismissable", boolean(true)));
    }
    if spec.tone != ToneVariant::Default {
        fields.push(field("tone", s(spec.tone.as_str())));
    }
    if let Some(heading) = &spec.heading {
        fields.push(field("heading", text_source(heading)));
    }
    if let Some(icon) = &spec.icon {
        fields.push(field("icon", s(icon)));
    }
    obj(fields)
}

fn progress_spec(spec: &ProgressSpec) -> String {
    // 0.2.0 — `indeterminate` omitted-when-default (false).
    let mut fields = vec![field("fraction", binding(&spec.fraction))];
    if spec.indeterminate {
        fields.push(field("indeterminate", boolean(true)));
    }
    if spec.tone != ToneVariant::Default {
        fields.push(field("tone", s(spec.tone.as_str())));
    }
    if let Some(label) = &spec.label {
        fields.push(field("label", text_source(label)));
    }
    if let Some(caveat) = &spec.caveat {
        fields.push(field("caveat", text_source(caveat)));
    }
    obj(fields)
}

// ─── Drawing specs (Phase 524) ───────────────────────────────────────────────

fn view_box(vb: &ViewBox) -> String {
    obj(vec![
        field("height", num(vb.height)),
        field("minX", num(vb.min_x)),
        field("minY", num(vb.min_y)),
        field("width", num(vb.width)),
    ])
}

fn draw_point(p: &DrawPoint) -> String {
    obj(vec![field("x", num(p.x)), field("y", num(p.y))])
}

fn draw_style(style: &DrawStyle) -> String {
    // Every field optional; `obj` sorts keys, so push order is immaterial.
    let mut fields = Vec::new();
    if let Some(fill) = &style.fill {
        fields.push(field("fill", binding(fill)));
    }
    if let Some(stroke) = &style.stroke {
        fields.push(field("stroke", binding(stroke)));
    }
    if let Some(sw) = &style.stroke_width {
        fields.push(field("strokeWidth", binding(sw)));
    }
    if let Some(op) = &style.opacity {
        fields.push(field("opacity", binding(op)));
    }
    if let Some(ta) = &style.text_anchor {
        fields.push(field("textAnchor", s(ta.as_str())));
    }
    if let Some(fs) = &style.font_size {
        fields.push(field("fontSize", num(*fs)));
    }
    if let Some(em) = &style.emphasis {
        fields.push(field("emphasis", s(em.as_str())));
    }
    if let Some(ff) = &style.font_family {
        fields.push(field("fontFamily", s(ff)));
    }
    // Phase 642 — keyed mark identity; omitted-when-None (rule 4).
    if let Some(m) = &style.mark_id {
        fields.push(field("markId", s(m)));
    }
    obj(fields)
}

fn curve_command(c: &CurveCommand) -> String {
    match c {
        CurveCommand::MoveTo(to) => case_obj("MoveTo", vec![field("to", draw_point(to))]),
        CurveCommand::LineTo(to) => case_obj("LineTo", vec![field("to", draw_point(to))]),
        CurveCommand::CubicTo {
            control1,
            control2,
            to,
        } => case_obj(
            "CubicTo",
            vec![
                field("control1", draw_point(control1)),
                field("control2", draw_point(control2)),
                field("to", draw_point(to)),
            ],
        ),
        CurveCommand::QuadraticTo { control, to } => case_obj(
            "QuadraticTo",
            vec![
                field("control", draw_point(control)),
                field("to", draw_point(to)),
            ],
        ),
        CurveCommand::Close => case_obj("Close", vec![]),
    }
}

fn shape(sh: &Shape) -> String {
    match sh {
        Shape::Group { children, style } => case_obj(
            "Group",
            vec![
                field("children", arr(children.iter().map(shape).collect())),
                field("style", draw_style(style)),
            ],
        ),
        Shape::Rectangle {
            x,
            y,
            width,
            height,
            corner_radius,
            style,
        } => {
            let mut fields = vec![
                field("height", num(*height)),
                field("style", draw_style(style)),
                field("width", num(*width)),
                field("x", num(*x)),
                field("y", num(*y)),
            ];
            if let Some(cr) = corner_radius {
                fields.push(field("cornerRadius", num(*cr)));
            }
            case_obj("Rectangle", fields)
        }
        Shape::Line {
            x1,
            y1,
            x2,
            y2,
            style,
        } => case_obj(
            "Line",
            vec![
                field("style", draw_style(style)),
                field("x1", num(*x1)),
                field("x2", num(*x2)),
                field("y1", num(*y1)),
                field("y2", num(*y2)),
            ],
        ),
        Shape::Polyline { points, style } => case_obj(
            "Polyline",
            vec![
                field("points", arr(points.iter().map(draw_point).collect())),
                field("style", draw_style(style)),
            ],
        ),
        Shape::Polygon { points, style } => case_obj(
            "Polygon",
            vec![
                field("points", arr(points.iter().map(draw_point).collect())),
                field("style", draw_style(style)),
            ],
        ),
        Shape::Curve { commands, style } => case_obj(
            "Curve",
            vec![
                field(
                    "commands",
                    arr(commands.iter().map(curve_command).collect()),
                ),
                field("style", draw_style(style)),
            ],
        ),
        Shape::Circle { cx, cy, r, style } => case_obj(
            "Circle",
            vec![
                field("cx", num(*cx)),
                field("cy", num(*cy)),
                field("r", num(*r)),
                field("style", draw_style(style)),
            ],
        ),
        Shape::Ellipse {
            cx,
            cy,
            rx,
            ry,
            style,
        } => case_obj(
            "Ellipse",
            vec![
                field("cx", num(*cx)),
                field("cy", num(*cy)),
                field("rx", num(*rx)),
                field("ry", num(*ry)),
                field("style", draw_style(style)),
            ],
        ),
        Shape::Label { x, y, text, style } => case_obj(
            "Label",
            vec![
                field("style", draw_style(style)),
                field("text", text_source(text)),
                field("x", num(*x)),
                field("y", num(*y)),
            ],
        ),
    }
}

fn drawing_spec(spec: &DrawingSpec) -> String {
    let mut fields = vec![
        field("shapes", arr(spec.shapes.iter().map(shape).collect())),
        field("style", draw_style(&spec.style)),
        field("viewBox", view_box(&spec.view_box)),
    ];
    if let Some(title) = &spec.title {
        fields.push(field("title", text_source(title)));
    }
    if let Some(desc) = &spec.description {
        fields.push(field("description", text_source(desc)));
    }
    obj(fields)
}

// ─── Input specs ─────────────────────────────────────────────────────────────

fn handler_field(name: &str, present: &Option<Closure>) -> Vec<Field> {
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
enum ControlAutoBind<'a> {
    FilterChip(&'a str),
    FormFieldId(&'a str),
}

/// The typed auto-bind placeholders — must stay byte-identical to the decode
/// side's `control_value_defaults`.
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
}

/// The `value` field for a control slot: omitted when it is exactly the
/// context's auto-binding; any other binding emits.
fn control_value_field(
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
        (ControlAutoBind::FormFieldId(id), Binding::State { key, default_value }) => {
            key == id && *default_value == placeholder
        }
        _ => false,
    };
    if is_auto {
        vec![]
    } else {
        vec![field("value", binding(value))]
    }
}

fn form_field_kind(auto_bind: ControlAutoBind<'_>, k: &FormFieldKind) -> String {
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
        FormFieldKind::Checkbox { value, on_toggle } => {
            let mut fields = handler_field("onToggle", on_toggle);
            fields.extend(control_value_field(
                auto_bind,
                control_value_defaults::checkbox(),
                value,
            ));
            case_obj("Checkbox", fields)
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
                (ControlAutoBind::FormFieldId(id), Binding::State { key, default_value }) => {
                    key == id && *default_value == control_value_defaults::range()
                }
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
            fields.push(field("orientation", s(orientation.as_str())));
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
        FormFieldKind::Date {
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
                control_value_defaults::date(),
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
            case_obj("Date", fields)
        }
    }
}

fn form_field(f: &FormField) -> String {
    let mut fields = vec![
        field("id", s(&f.id)),
        field(
            "kind",
            form_field_kind(ControlAutoBind::FormFieldId(&f.id), &f.kind),
        ),
        field("label", text_source(&f.label)),
        field("required", boolean(f.required)),
    ];
    if let Some(help) = &f.help {
        fields.push(field("help", text_source(help)));
    }
    obj(fields)
}

fn form_spec(spec: &FormSpec) -> String {
    let mut fields = vec![
        field("fields", arr(spec.fields.iter().map(form_field).collect())),
        field("onSubmit", action(&spec.on_submit)),
        field("submitLabel", text_source(&spec.submit_label)),
    ];
    if let Some(disabled) = &spec.disabled {
        fields.push(field("disabled", binding(disabled)));
    }
    obj(fields)
}

fn filter_spec(spec: &FilterSpec) -> String {
    // 0.2.0 filters-unification: the chip's control is an ordinary
    // FormFieldKind; a `value` that is exactly the chip's auto binding
    // (`Filter(name)`) is omitted.
    obj(vec![
        field(
            "kind",
            form_field_kind(ControlAutoBind::FilterChip(&spec.name), &spec.kind),
        ),
        field("label", text_source(&spec.label)),
        field("name", s(&spec.name)),
    ])
}

fn button_spec(spec: &ButtonSpec) -> String {
    let mut fields = vec![
        field("label", text_source(&spec.label)),
        field("onClick", action(&spec.on_click)),
        field("variant", s(spec.variant.as_str())),
    ];
    if let Some(icon) = &spec.icon {
        fields.push(field("icon", s(icon)));
    }
    if let Some(disabled) = &spec.disabled {
        fields.push(field("disabled", binding(disabled)));
    }
    obj(fields)
}

fn select_spec(spec: &SelectSpec) -> String {
    let mut fields = vec![
        field("label", text_source(&spec.label)),
        field("source", binding(&spec.source)),
        field("value", binding(&spec.value)),
    ];
    if spec.on_change.is_some() {
        fields.push(field("onChange", CLOSURE.to_string()));
    }
    if let Some(placeholder) = &spec.placeholder {
        fields.push(field("placeholder", text_source(placeholder)));
    }
    if let Some(disabled) = &spec.disabled {
        fields.push(field("disabled", binding(disabled)));
    }
    if spec.multiple {
        fields.push(field("multiple", boolean(true)));
    }
    if let Some(values) = &spec.values {
        fields.push(field("values", binding(values)));
    }
    if spec.on_change_multi.is_some() {
        fields.push(field("onChangeMulti", CLOSURE.to_string()));
    }
    obj(fields)
}

fn file_upload_spec(spec: &FileUploadSpec) -> String {
    let mut fields = vec![
        field("accept", arr(spec.accept.iter().map(|a| s(a)).collect())),
        field("label", text_source(&spec.label)),
        field("multiple", boolean(spec.multiple)),
        field("onSelect", CLOSURE.to_string()),
    ];
    if let Some(disabled) = &spec.disabled {
        fields.push(field("disabled", binding(disabled)));
    }
    obj(fields)
}

// ─── Visualisation specs ─────────────────────────────────────────────────────

fn cell_kind_erased(k: &CellKindErased) -> String {
    match k {
        CellKindErased::Text => case_obj("Text", vec![]),
        CellKindErased::Numeric => case_obj("Numeric", vec![]),
        CellKindErased::Date => case_obj("Date", vec![]),
        CellKindErased::Editable => {
            case_obj("Editable", vec![field("onEdit", CLOSURE.to_string())])
        }
        CellKindErased::Checkbox => case_obj(
            "Checkbox",
            vec![
                field("get", CLOSURE.to_string()),
                field("onToggle", CLOSURE.to_string()),
            ],
        ),
        CellKindErased::Button { label } => case_obj(
            "Button",
            vec![
                field("label", text_source(label)),
                field("onClick", CLOSURE.to_string()),
            ],
        ),
        CellKindErased::ButtonGroup { labels } => case_obj(
            "ButtonGroup",
            vec![field(
                "buttons",
                arr(labels
                    .iter()
                    .map(|label| {
                        obj(vec![
                            field("label", text_source(label)),
                            field("onClick", CLOSURE.to_string()),
                        ])
                    })
                    .collect()),
            )],
        ),
        CellKindErased::Link => case_obj(
            "Link",
            vec![
                field("hrefFn", CLOSURE.to_string()),
                field("labelFn", CLOSURE.to_string()),
            ],
        ),
        CellKindErased::Pill => case_obj(
            "Pill",
            vec![
                field("labelFn", CLOSURE.to_string()),
                field("toneFn", CLOSURE.to_string()),
            ],
        ),
        CellKindErased::Progress => case_obj(
            "Progress",
            vec![
                field("fractionFn", CLOSURE.to_string()),
                field("labelFn", CLOSURE.to_string()),
            ],
        ),
        CellKindErased::Custom => case_obj("Custom", vec![field("fn", CLOSURE.to_string())]),
    }
}

fn column_erased(c: &ColumnErased) -> String {
    // Phase 460 — `format` / `width` omitted-when-default.
    let mut fields = vec![
        field("kind", cell_kind_erased(&c.kind)),
        field("label", s(&c.label)),
    ];
    if c.format != CellFormat::None {
        fields.push(field("format", cell_format(&c.format)));
    }
    if c.width != ColumnWidth::Auto {
        fields.push(field("width", column_width(&c.width)));
    }
    if c.value.is_some() {
        fields.push(field("value", CLOSURE.to_string()));
    }
    if let Some(f) = &c.field {
        fields.push(field("field", s(f)));
    }
    obj(fields)
}

fn static_rows(rows: &StaticRows) -> String {
    obj(vec![
        field(
            "headers",
            arr(rows.headers.iter().map(text_source).collect()),
        ),
        field(
            "rows",
            arr(rows
                .rows
                .iter()
                .map(|row| arr(row.iter().map(text_source).collect()))
                .collect()),
        ),
    ])
}

fn grid_spec(spec: &GridSpec) -> String {
    // 0.2.0 — `editable` omitted-when-default (false).
    let mut fields = vec![
        field(
            "columns",
            arr(spec.columns.iter().map(column_erased).collect()),
        ),
        field("source", binding(&spec.source)),
    ];
    if spec.editable {
        fields.push(field("editable", boolean(true)));
    }
    if spec.on_row_click.is_some() {
        fields.push(field("onRowClick", CLOSURE.to_string()));
    }
    if spec.row_key.is_some() {
        fields.push(field("rowKey", CLOSURE.to_string()));
    }
    if let Some(row_key_field) = &spec.row_key_field {
        fields.push(field("rowKeyField", s(row_key_field)));
    }
    if let Some(rows) = &spec.static_rows {
        fields.push(field("staticRows", static_rows(rows)));
    }
    obj(fields)
}

fn chart_spec(spec: &ChartSpec) -> String {
    let mut fields = vec![
        field("kind", s(spec.kind.as_str())),
        field("source", binding(&spec.source)),
        field("stacked", boolean(spec.stacked)),
        field("xField", s(&spec.x_field)),
        field("yFields", arr(spec.y_fields.iter().map(|y| s(y)).collect())),
    ];
    if let Some(title) = &spec.title {
        fields.push(field("title", text_source(title)));
    }
    if spec.on_point_click.is_some() {
        fields.push(field("onPointClick", CLOSURE.to_string()));
    }
    obj(fields)
}

fn map_spec(spec: &MapSpec) -> String {
    let mut fields = vec![
        field("centreLatitude", num(spec.centre_latitude)),
        field("centreLongitude", num(spec.centre_longitude)),
        field("source", binding(&spec.source)),
        field("zoom", num(spec.zoom as f64)),
    ];
    if spec.on_marker_click.is_some() {
        fields.push(field("onMarkerClick", CLOSURE.to_string()));
    }
    obj(fields)
}

// ─── Layout specs ────────────────────────────────────────────────────────────

fn box_layout(l: &BoxLayout) -> String {
    match l {
        BoxLayout::Flex {
            direction,
            gap,
            wrap,
        } => {
            let mut fields = vec![field("direction", s(direction.as_str()))];
            if let Some(gap) = gap {
                fields.push(field("gap", int(*gap)));
            }
            fields.push(field("wrap", boolean(*wrap)));
            case_obj("Flex", fields)
        }
        BoxLayout::Grid {
            cols,
            gap,
            template_columns,
        } => {
            let mut fields = vec![field("cols", int(*cols))];
            if let Some(gap) = gap {
                fields.push(field("gap", int(*gap)));
            }
            if let Some(template) = template_columns {
                fields.push(field("templateColumns", s(template)));
            }
            case_obj("Grid", fields)
        }
        BoxLayout::Auto => case_obj("Auto", vec![]),
    }
}

fn box_spec(spec: &BoxSpec) -> String {
    let mut fields = vec![field(
        "children",
        arr(spec.children.iter().map(node).collect()),
    )];
    if let Some(heading) = &spec.heading {
        fields.push(field("heading", text_source(heading)));
    }
    fields.push(field("layout", box_layout(&spec.layout)));
    fields.push(field("role", s(spec.role.as_str())));
    obj(fields)
}

fn split_panel_spec(spec: &SplitPanelSpec) -> String {
    obj(vec![
        field("children", arr(spec.children.iter().map(node).collect())),
        field("weight", num(spec.weight)),
    ])
}

fn tab_header(h: &TabHeader) -> String {
    let mut fields = vec![field("label", text_source(&h.label))];
    if let Some(icon) = &h.icon {
        fields.push(field("icon", s(icon)));
    }
    if let Some(disabled) = &h.disabled {
        fields.push(field("disabled", binding(disabled)));
    }
    obj(fields)
}

fn tabs_spec(spec: &TabsSpec) -> String {
    // 0.2.0 — `orientation` omitted-when-default (Horizontal),
    // encoder-symmetric (unlike SegmentedChoice, which always emits it).
    let mut fields = vec![
        field("children", arr(spec.children.iter().map(node).collect())),
        field("activeIndex", binding(&spec.active_index)),
    ];
    if spec.orientation != Orientation::Horizontal {
        fields.push(field("orientation", s(spec.orientation.as_str())));
    }
    if spec.on_select.is_some() {
        fields.push(field("onSelect", CLOSURE.to_string()));
    }
    if let Some(headers) = &spec.tab_headers {
        fields.push(field(
            "tabHeaders",
            arr(headers.iter().map(tab_header).collect()),
        ));
    }
    if let Some(tags) = &spec.tab_tags {
        fields.push(field("tabTags", arr(tags.iter().map(|t| s(t)).collect())));
    }
    if let Some(active_tag) = &spec.active_tag {
        fields.push(field("activeTag", binding(active_tag)));
    }
    if spec.on_select_tag.is_some() {
        fields.push(field("onSelectTag", CLOSURE.to_string()));
    }
    obj(fields)
}

fn stepper_spec(spec: &StepperSpec) -> String {
    obj(vec![
        field("activeStep", binding(&spec.active_step)),
        field("children", arr(spec.children.iter().map(node).collect())),
        field("onSelect", CLOSURE.to_string()),
    ])
}

fn summary_list_spec(spec: &SummaryListSpec) -> String {
    let mut fields = vec![field(
        "children",
        arr(spec.children.iter().map(node).collect()),
    )];
    if let Some(heading) = &spec.heading {
        fields.push(field("heading", text_source(heading)));
    }
    obj(fields)
}

fn disclosure_spec(spec: &DisclosureSpec) -> String {
    let mut fields = vec![
        field("children", arr(spec.children.iter().map(node).collect())),
        field("defaultOpen", boolean(spec.default_open)),
        field("heading", text_source(&spec.heading)),
        field("open", binding(&spec.open)),
    ];
    if spec.on_toggle.is_some() {
        fields.push(field("onToggle", CLOSURE.to_string()));
    }
    obj(fields)
}

fn modal_spec(spec: &ModalSpec) -> String {
    let mut fields = vec![
        field("children", arr(spec.children.iter().map(node).collect())),
        field("dismissable", boolean(spec.dismissable)),
        field("open", binding(&spec.open)),
    ];
    if let Some(on_dismiss) = &spec.on_dismiss {
        fields.push(field("onDismiss", action(on_dismiss)));
    }
    if let Some(heading) = &spec.heading {
        fields.push(field("heading", text_source(heading)));
    }
    obj(fields)
}

fn scroll_area_spec(spec: &ScrollAreaSpec) -> String {
    let mut fields = vec![
        field("children", arr(spec.children.iter().map(node).collect())),
        field("orientation", s(spec.orientation.as_str())),
    ];
    if let Some(max_height) = spec.max_height {
        fields.push(field("maxHeight", num(max_height as f64)));
    }
    if let Some(max_width) = spec.max_width {
        fields.push(field("maxWidth", num(max_width as f64)));
    }
    obj(fields)
}

// ─── Fragments / Mount ───────────────────────────────────────────────────────

fn fragment_scalar(scalar: &FragmentScalar) -> String {
    match scalar {
        FragmentScalar::Int(v) => case_obj("Int", vec![field("value", int(*v))]),
        FragmentScalar::Float(v) => case_obj("Float", vec![field("value", num(*v))]),
        FragmentScalar::Bool(v) => case_obj("Bool", vec![field("value", boolean(*v))]),
        FragmentScalar::Str(v) => case_obj("Str", vec![field("value", s(v))]),
    }
}

fn hole_value_space(space: &HoleValueSpace) -> String {
    match space {
        HoleValueSpace::IntRange { min, max } => case_obj(
            "IntRange",
            vec![field("max", int(*max)), field("min", int(*min))],
        ),
        HoleValueSpace::FloatRange { min, max } => case_obj(
            "FloatRange",
            vec![field("max", num(*max)), field("min", num(*min))],
        ),
        HoleValueSpace::StringLen { min_len, max_len } => case_obj(
            "StringLen",
            vec![
                field("maxLen", int(*max_len)),
                field("minLen", int(*min_len)),
            ],
        ),
        HoleValueSpace::Enum { choices } => case_obj(
            "Enum",
            vec![field(
                "choices",
                arr(choices.iter().map(|c| s(c)).collect()),
            )],
        ),
        HoleValueSpace::AnyString => case_obj("AnyString", vec![]),
    }
}

fn hole_decl(h: &HoleDecl) -> String {
    match h {
        HoleDecl::Value {
            name,
            space,
            default,
        } => {
            let mut fields = vec![
                field("name", s(name)),
                field("space", hole_value_space(space)),
            ];
            if let Some(default) = default {
                fields.push(field("default", fragment_scalar(default)));
            }
            case_obj("Value", fields)
        }
        HoleDecl::Slot {
            name,
            kind_constraint,
        } => {
            let mut fields = vec![field("name", s(name))];
            if let Some(constraint) = kind_constraint {
                fields.push(field("kindConstraint", s(constraint)));
            }
            case_obj("Slot", fields)
        }
        HoleDecl::Repeat { name, count_space } => case_obj(
            "Repeat",
            vec![
                field("countSpace", hole_value_space(count_space)),
                field("name", s(name)),
            ],
        ),
    }
}

fn effect_class(e: &EffectClass) -> String {
    obj(vec![
        field("determinism", s(e.determinism.as_str())),
        field("hostEffect", s(e.host_effect.as_str())),
    ])
}

fn fragment_arg(a: &FragmentArg) -> String {
    match a {
        FragmentArg::Value(scalar) => fragment_scalar(scalar),
        FragmentArg::Slot { tree } => case_obj("SlotArg", vec![field("tree", node(tree))]),
    }
}

// ─── NodeKind / Node / StateBehaviour / Style / Accessibility ────────────────

fn node_kind(k: &NodeKind) -> String {
    match k {
        // Layout
        NodeKind::Box(spec) => case_obj_hoisted("Box", box_spec(spec)),
        NodeKind::SplitPanel(spec) => case_obj_hoisted("SplitPanel", split_panel_spec(spec)),
        NodeKind::Tabs(spec) => case_obj_hoisted("Tabs", tabs_spec(spec)),
        NodeKind::Stepper(spec) => case_obj_hoisted("Stepper", stepper_spec(spec)),
        NodeKind::SummaryList(spec) => case_obj_hoisted("SummaryList", summary_list_spec(spec)),
        NodeKind::Disclosure(spec) => case_obj_hoisted("Disclosure", disclosure_spec(spec)),
        NodeKind::Modal(spec) => case_obj_hoisted("Modal", modal_spec(spec)),
        NodeKind::ScrollArea(spec) => case_obj_hoisted("ScrollArea", scroll_area_spec(spec)),
        // Display
        NodeKind::Heading(spec) => case_obj_hoisted("Heading", heading_spec(spec)),
        NodeKind::Markdown(spec) => case_obj_hoisted("Markdown", markdown_spec(spec)),
        NodeKind::Metric(spec) => case_obj_hoisted("Metric", metric_spec(spec)),
        NodeKind::Badge(spec) => case_obj_hoisted("Badge", badge_spec(spec)),
        NodeKind::Sparkline(spec) => case_obj_hoisted("Sparkline", sparkline_spec(spec)),
        NodeKind::Callout(spec) => case_obj_hoisted("Callout", callout_spec(spec)),
        NodeKind::Progress(spec) => case_obj_hoisted("Progress", progress_spec(spec)),
        NodeKind::Skeleton(spec) => case_obj_hoisted("Skeleton", skeleton_spec(spec)),
        NodeKind::Fact(spec) => case_obj_hoisted("Fact", fact_spec(spec)),
        NodeKind::LabelValueRow(spec) => {
            case_obj_hoisted("LabelValueRow", label_value_row_spec(spec))
        }
        NodeKind::Link(spec) => case_obj_hoisted("Link", link_spec(spec)),
        NodeKind::Image(spec) => case_obj_hoisted("Image", image_spec(spec)),
        NodeKind::List(spec) => case_obj_hoisted("List", list_spec(spec)),
        NodeKind::Toast(spec) => case_obj_hoisted("Toast", toast_spec(spec)),
        NodeKind::CodeBlock(spec) => case_obj_hoisted("CodeBlock", code_block_spec(spec)),
        NodeKind::Math(spec) => case_obj_hoisted("Math", math_spec(spec)),
        NodeKind::Drawing(spec) => case_obj_hoisted("Drawing", drawing_spec(spec)),
        // Input
        NodeKind::Form(spec) => case_obj_hoisted("Form", form_spec(spec)),
        NodeKind::Filters(specs) => case_obj(
            "Filters",
            vec![field("items", arr(specs.iter().map(filter_spec).collect()))],
        ),
        NodeKind::Button(spec) => case_obj_hoisted("Button", button_spec(spec)),
        NodeKind::FileUpload(spec) => case_obj_hoisted("FileUpload", file_upload_spec(spec)),
        NodeKind::Select(spec) => case_obj_hoisted("Select", select_spec(spec)),
        // Visualisation
        NodeKind::DataGrid(spec) => case_obj_hoisted("DataGrid", grid_spec(spec)),
        NodeKind::Chart(spec) => case_obj_hoisted("Chart", chart_spec(spec)),
        NodeKind::Map(spec) => case_obj_hoisted("Map", map_spec(spec)),
        // Structural
        NodeKind::Custom(spec) => {
            let mut fields = vec![
                field("componentId", s(&spec.component_id)),
                field("moduleId", s(&spec.module_id)),
                field("props", json_map(&spec.props)),
            ];
            if let Some(hash) = &spec.content_hash {
                fields.push(field(
                    "contentHash",
                    obj(vec![
                        field("algorithm", s(&hash.algorithm)),
                        field("hash", s(&hash.hash)),
                        field("strictness", s(hash.strictness.as_str())),
                    ]),
                ));
            }
            if !spec.exposed_node_ids.is_empty() {
                fields.push(field(
                    "exposedNodeIds",
                    arr(spec.exposed_node_ids.iter().map(|id| s(id)).collect()),
                ));
            }
            case_obj("Custom", fields)
        }
        NodeKind::ErrorBoundary(spec) => case_obj(
            "ErrorBoundary",
            vec![
                field("child", node(&spec.child)),
                field("fallback", node(&spec.fallback)),
            ],
        ),
        NodeKind::Switch(spec) => case_obj(
            "Switch",
            vec![
                field(
                    "cases",
                    arr(spec
                        .cases
                        .iter()
                        .map(|c| {
                            obj(vec![
                                field("child", node(&c.child)),
                                field("match", s(&c.match_value)),
                            ])
                        })
                        .collect()),
                ),
                field("default", node(&spec.default)),
                field("stateKey", s(&spec.state_key)),
            ],
        ),
        NodeKind::FragmentDecl(spec) => {
            let mut fields = vec![
                field("body", node(&spec.body)),
                field("name", s(&spec.name)),
            ];
            if !spec.holes.is_empty() {
                fields.push(field(
                    "holes",
                    arr(spec.holes.iter().map(hole_decl).collect()),
                ));
            }
            if !spec.effect.is_pure_deterministic() {
                fields.push(field("effect", effect_class(&spec.effect)));
            }
            case_obj("FragmentDecl", fields)
        }
        NodeKind::FragmentRef(spec) => {
            let mut fields = vec![field("name", s(&spec.name))];
            if !spec.args.is_empty() {
                fields.push(field(
                    "args",
                    obj(spec
                        .args
                        .iter()
                        .map(|(k, v)| (k.clone(), fragment_arg(v)))
                        .collect()),
                ));
            }
            case_obj("FragmentRef", fields)
        }
        NodeKind::Mount(spec) => {
            let mut channel_fields = vec![field("direction", s(spec.channel.direction.as_str()))];
            if let Some(shape) = &spec.channel.message_shape {
                channel_fields.push(field("messageShape", s(shape)));
            }
            let mut fields = vec![
                field(
                    "capabilities",
                    arr(spec.capabilities.iter().map(|c| s(c)).collect()),
                ),
                field("channel", obj(channel_fields)),
                field("onBubble", CLOSURE.to_string()),
                field("scopeId", s(&spec.scope_id)),
            ];
            if !spec.inputs.is_empty() {
                fields.push(field(
                    "inputs",
                    obj(spec
                        .inputs
                        .iter()
                        .map(|(k, v)| (k.clone(), fragment_arg(v)))
                        .collect()),
                ));
            }
            case_obj("Mount", fields)
        }
    }
}

/// The flat wire hoists a spec's fields directly under the kind's `$type`
/// discriminator (§3.2) — realised here by adding `$type` to the spec's own
/// field set before the canonical sort, which lands it first (`$` sorts before
/// every lower-case field name).
fn case_obj_hoisted(discriminator: &str, spec_json: String) -> String {
    // spec_json is "{…}" with Ordinal-sorted fields; splice "$type" in front.
    let head = format!("{{\"$type\":{}", s(discriminator));
    if spec_json.len() > 2 {
        format!("{head},{}", &spec_json[1..])
    } else {
        format!("{head}}}")
    }
}

fn state_behaviour(state: &StateBehaviour) -> String {
    let mut fields = vec![];
    if let Some(on_loading) = &state.on_loading {
        fields.push(field("onLoading", node(on_loading)));
    }
    if let Some(on_empty) = &state.on_empty {
        fields.push(field("onEmpty", node(on_empty)));
    }
    if state.on_error.is_some() {
        fields.push(field("onError", CLOSURE.to_string()));
    }
    obj(fields)
}

fn semantic_style(style: &SemanticStyle) -> String {
    // Phase 460 — tone/weight/emphasis omitted-when-default (as role/voice).
    let mut fields = vec![];
    if style.tone != ToneVariant::Default {
        fields.push(field("tone", s(style.tone.as_str())));
    }
    if style.weight != StyleWeight::Standard {
        fields.push(field("weight", s(style.weight.as_str())));
    }
    if style.emphasis != Emphasis::Normal {
        fields.push(field("emphasis", s(style.emphasis.as_str())));
    }
    if style.role != StyleRole::None {
        fields.push(field("role", s(style.role.as_str())));
    }
    if style.voice != FontVoice::Default {
        fields.push(field("voice", s(style.voice.as_str())));
    }
    obj(fields)
}

fn accessibility(a: &Accessibility) -> String {
    let mut fields = vec![];
    if let Some(label) = &a.label {
        fields.push(field("label", binding(label)));
    }
    if let Some(labelled_by) = &a.labelled_by {
        fields.push(field("labelledBy", s(labelled_by)));
    }
    if let Some(described_by) = &a.described_by {
        fields.push(field("describedBy", s(described_by)));
    }
    if let Some(role) = &a.role {
        fields.push(field("role", s(role)));
    }
    if let Some(live_region) = &a.live_region {
        fields.push(field("liveRegion", s(live_region.as_str())));
    }
    if let Some(hidden) = &a.hidden {
        fields.push(field("hidden", binding(hidden)));
    }
    obj(fields)
}

fn node(n: &Node) -> String {
    let mut fields = vec![field("id", s(&n.id)), field("kind", node_kind(&n.kind))];
    if !n.state.is_empty() {
        fields.push(field("state", state_behaviour(&n.state)));
    }
    if !n.style.is_default() {
        fields.push(field("style", semantic_style(&n.style)));
    }
    if let Some(a) = &n.accessibility {
        fields.push(field("accessibility", accessibility(a)));
    }
    obj(fields)
}

// ─── TreeOp ──────────────────────────────────────────────────────────────────

fn tree_op(op: &TreeOp) -> String {
    match op {
        TreeOp::EditNode { target, new_kind } => case_obj(
            "EditNode",
            vec![
                field("newKind", node_kind(new_kind)),
                field("target", s(target)),
            ],
        ),
        TreeOp::UpdateProp {
            target,
            path,
            value,
        } => case_obj(
            "UpdateProp",
            vec![
                field("path", s(path)),
                field("target", s(target)),
                field("value", json_value(value)),
            ],
        ),
        TreeOp::ReplaceBinding {
            target,
            slot,
            binding: b,
        } => case_obj(
            "ReplaceBinding",
            vec![
                field("binding", binding(b)),
                field("slot", s(slot)),
                field("target", s(target)),
            ],
        ),
        TreeOp::UpdateStyle { target, style } => case_obj(
            "UpdateStyle",
            vec![
                field("style", semantic_style(style)),
                field("target", s(target)),
            ],
        ),
        TreeOp::UpdateState { target, state } => case_obj(
            "UpdateState",
            vec![
                field("state", state_behaviour(state)),
                field("target", s(target)),
            ],
        ),
        TreeOp::InsertChild {
            parent_id,
            position,
            child,
        } => case_obj(
            "InsertChild",
            vec![
                field("child", node(child)),
                field("parentId", s(parent_id)),
                field("position", num(*position as f64)),
            ],
        ),
        TreeOp::RemoveNode { target } => case_obj("RemoveNode", vec![field("target", s(target))]),
        TreeOp::MoveNode {
            target,
            new_parent_id,
            new_position,
        } => case_obj(
            "MoveNode",
            vec![
                field("newParentId", s(new_parent_id)),
                field("newPosition", num(*new_position as f64)),
                field("target", s(target)),
            ],
        ),
        TreeOp::ReorderChildren {
            parent_id,
            new_order,
        } => case_obj(
            "ReorderChildren",
            vec![
                field("newOrder", arr(new_order.iter().map(|id| s(id)).collect())),
                field("parentId", s(parent_id)),
            ],
        ),
        TreeOp::ReplaceRoot { node: n } => case_obj("ReplaceRoot", vec![field("node", node(n))]),
        TreeOp::Batch(ops) => case_obj(
            "Batch",
            vec![field("ops", arr(ops.iter().map(tree_op).collect()))],
        ),
    }
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
