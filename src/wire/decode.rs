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

fn as_obj<'a>(path: &str, j: &'a JVal) -> DResult<&'a Fields> {
    match j {
        JVal::Obj(fields) => Ok(fields),
        _ => Err(wrong_type(path, "JSON object")),
    }
}

fn as_str<'a>(path: &str, j: &'a JVal) -> DResult<&'a str> {
    match j {
        JVal::Str(s) => Ok(s),
        _ => Err(wrong_type(path, "JSON string")),
    }
}

fn as_bool(path: &str, j: &JVal) -> DResult<bool> {
    match j {
        JVal::Bool(b) => Ok(*b),
        _ => Err(wrong_type(path, "JSON boolean")),
    }
}

fn as_float(path: &str, j: &JVal) -> DResult<f64> {
    match j {
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
    match j {
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
}

decode_bare_enum!(decode_orientation, Orientation, "Orientation");
decode_bare_enum!(
    decode_scroll_orientation,
    ScrollOrientation,
    "ScrollOrientation"
);
decode_bare_enum!(decode_badge_variant, BadgeVariant, "BadgeVariant");
decode_bare_enum!(decode_button_variant, ButtonVariant, "ButtonVariant");
decode_bare_enum!(decode_heading_variant, HeadingVariant, "HeadingVariant");
decode_bare_enum!(decode_tone, ToneVariant, "ToneVariant");
decode_bare_enum!(decode_weight, StyleWeight, "StyleWeight");
decode_bare_enum!(decode_emphasis, Emphasis, "Emphasis");
decode_bare_enum!(decode_text_anchor, TextAnchor, "TextAnchor");
decode_bare_enum!(decode_style_role, StyleRole, "StyleRole");
decode_bare_enum!(decode_font_voice, FontVoice, "FontVoice");
decode_bare_enum!(decode_chart_kind, ChartKind, "ChartKind");
decode_bare_enum!(decode_image_variant, ImageVariant, "ImageVariant");
decode_bare_enum!(decode_math_display, MathDisplay, "MathDisplay");
decode_bare_enum!(decode_date_variant, DateVariant, "DateVariant");
decode_bare_enum!(
    decode_file_read_encoding,
    FileReadEncoding,
    "FileReadEncoding"
);
decode_bare_enum!(decode_live_region, LiveRegionKind, "LiveRegionKind");
decode_bare_enum!(decode_date_style, DateStyle, "DateStyle");
decode_bare_enum!(
    decode_relative_time_unit,
    RelativeTimeUnit,
    "RelativeTimeUnit"
);

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
        _ => Err(mismatch()),
    }
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

fn decode_data_column(columns: &Fields, name: &str, ty: ColumnType) -> CResult<DataColumn> {
    let col = get(columns, name).ok_or_else(|| format!("missing field: columns.{name}"))?;
    let fields = c_obj(col)?;
    let values = c_arr(c_field(fields, "values")?)?;
    let validity = c_arr(c_field(fields, "validity")?)?;
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

fn decode_data_source(j: &JVal) -> CResult<DataSource> {
    let fields = c_obj(j)?;
    let schema = decode_col_schema(c_field(fields, "schema")?)?;
    if let Some(r) = get(fields, "ref") {
        return Ok(DataSource::Ref { name: c_str(r)? });
    }
    let cols_obj = c_obj(c_field(fields, "columns")?)?;
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
        "apply" => {
            let f = c_str_field(fields, "fn")?;
            let func = c_enum(&f, ScalarFn::from_wire, ScalarFn::WIRE_NAMES)?;
            let args = c_arr(c_field(fields, "args")?)?
                .iter()
                .map(decode_col_expr)
                .collect::<CResult<Vec<_>>>()?;
            Ok(ColExpr::Apply { func, args })
        }
        other => Err(format!(
            "unknown column type '{other}'; expected one of: col, lit, param, binary, not, coalesce, case, cast, apply"
        )),
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
    let name = c_str_field(fields, "name")?;
    let f = c_str_field(fields, "fn")?;
    let func = c_enum(&f, AggFn::from_wire, AggFn::WIRE_NAMES)?;
    let of = c_str_field(fields, "of")?;
    Ok(Agg { name, func, of })
}

fn decode_order(j: &JVal) -> CResult<SortKey> {
    let fields = c_obj(j)?;
    let col = c_str_field(fields, "col")?;
    let dir = c_str_field(fields, "dir")?;
    // Only "desc" sorts descending; anything else reads ascending.
    let dir = if dir == "desc" {
        SortDir::Desc
    } else {
        SortDir::Asc
    };
    Ok(SortKey { col, dir })
}

fn decode_transform_step(j: &JVal) -> CResult<TransformStep> {
    let fields = c_obj(j)?;
    let tag = c_str_field(fields, "$type")?;
    match tag.as_str() {
        "filter" => Ok(TransformStep::Filter {
            pred: decode_col_expr(c_field(fields, "pred")?)?,
        }),
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
            let keys = c_str_list(c_field(fields, "keys")?)?;
            let aggs = c_arr(c_field(fields, "aggs")?)?
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
            let func = c_enum(&f, WindowFn::from_wire, WindowFn::WIRE_NAMES)?;
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
            let by = c_arr(c_field(fields, "by")?)?
                .iter()
                .map(decode_order)
                .collect::<CResult<Vec<_>>>()?;
            Ok(TransformStep::Sort { by })
        }
        "distinct" => Ok(TransformStep::Distinct),
        "limit" => Ok(TransformStep::Limit {
            n: c_int(c_field(fields, "n")?)?,
            offset: c_int(c_field(fields, "offset")?)?,
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
        }
    }
}

fn decode_select_option(path: &str, j: &JVal) -> DResult<SelectOption> {
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
        other => Err(unknown_du_case(
            path,
            other,
            "Number | Currency | Percent | Date | RelativeTime",
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
        "Custom" => Ok(CellFormat::Custom),
        other => Err(unknown_du_case(
            path,
            other,
            "None | Number | Currency | Percent | SignificantDigits | Date | Custom",
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

fn decode_binding(path: &str, j: &JVal) -> DResult<Binding> {
    decode_binding_slot(path, j, StaticSlot::Untyped)
}

fn decode_binding_slot(path: &str, j: &JVal, slot: StaticSlot) -> DResult<Binding> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        "Static" => {
            let v = req(
                path,
                fields,
                "value",
                "Binding.Static value of the slot's expected type",
            )?;
            let value = slot.parse(&format!("{path}.value"), v)?;
            Ok(Binding::Static { value })
        }
        "Query" => {
            let name = req_string(path, fields, "name", "query name string")?;
            let depends_on = match get(fields, "dependsOn") {
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
        "Filter" => Ok(Binding::Filter {
            name: req_string(path, fields, "name", "filter name string")?,
        }),
        "Selection" => Ok(Binding::Selection {
            node_id: req_string(path, fields, "nodeId", "selection NodeId string")?,
        }),
        "State" => {
            let key = req_string(path, fields, "key", "state key string")?;
            let default_value = match get(fields, "defaultValue") {
                None => slot.placeholder(),
                Some(v) => slot
                    .parse(&format!("{path}.defaultValue"), v)
                    .unwrap_or_else(|_| slot.placeholder()),
            };
            Ok(Binding::State { key, default_value })
        }
        "Computed" => Ok(Binding::Computed),
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
            let source = decode_data_source(src_j).map_err(|e| {
                make_error(
                    DecodeErrorCode::WrongType,
                    format!("{path}.source"),
                    e,
                    None,
                )
            })?;
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
            let endpoint = req_string(path, fields, "endpoint", "ApiEndpoint string")?;
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
            route: req_string(path, fields, "route", "route string")?,
        }),
        "SetState" => {
            let key = req_string(path, fields, "key", "state key string")?;
            let value_j = req(path, fields, "value", "JsonValue value")?;
            let value = decode_jval(&format!("{path}.value"), value_j)?;
            Ok(Action::SetState { key, value })
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
    let source = req_binding(path, fields, "source", "Metric Source binding")?;
    let format_j = req(path, fields, "format", "CellFormat")?;
    let format = decode_cell_format(&format!("{path}.format"), format_j)?;
    let tone_j = req(path, fields, "tone", "ToneVariant")?;
    let tone = decode_tone(&format!("{path}.tone"), tone_j)?;
    let weight_j = req(path, fields, "weight", "StyleWeight")?;
    let weight = decode_weight(&format!("{path}.weight"), weight_j)?;
    let emphasis_j = req(path, fields, "emphasis", "Emphasis")?;
    let emphasis = decode_emphasis(&format!("{path}.emphasis"), emphasis_j)?;
    let trend = opt_binding(path, fields, "trend")?;
    let trend_format = match get(fields, "trendFormat") {
        None => None,
        Some(v) => Some(decode_cell_format(&format!("{path}.trendFormat"), v)?),
    };
    let icon = opt_string(path, fields, "icon")?;
    let subtext = opt_text_source(path, fields, "subtext")?;
    Ok(MetricSpec {
        label,
        source,
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
    let source = req_binding(path, fields, "source", "row Binding<float> Source")?;
    let format_j = req(path, fields, "format", "CellFormat")?;
    let format = decode_cell_format(&format!("{path}.format"), format_j)?;
    let emphasis = req_bool(path, fields, "emphasis", "emphasis bool")?;
    let help = opt_text_source(path, fields, "help")?;
    Ok(LabelValueRowSpec {
        label,
        source,
        format,
        emphasis,
        help,
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
    Ok(LinkSpec {
        href,
        label,
        download,
        rel,
        target,
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
    let tone_j = req(path, fields, "tone", "ToneVariant")?;
    let tone = decode_tone(&format!("{path}.tone"), tone_j)?;
    let open = req_binding(path, fields, "open", "Toast open binding")?;
    let dismissable = req_bool(path, fields, "dismissable", "dismissable bool")?;
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
    let source = req_binding_slot(
        path,
        fields,
        "source",
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

fn decode_callout_spec(path: &str, j: &JVal) -> DResult<CalloutSpec> {
    let fields = as_obj(path, j)?;
    let body = req_text_source(path, fields, "body", "Callout body TextSource")?;
    let dismissable = req_bool(path, fields, "dismissable", "dismissable bool")?;
    let tone_j = req(path, fields, "tone", "ToneVariant")?;
    let tone = decode_tone(&format!("{path}.tone"), tone_j)?;
    let heading = opt_text_source(path, fields, "heading")?;
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
    let indeterminate = req_bool(path, fields, "indeterminate", "indeterminate bool")?;
    let tone_j = req(path, fields, "tone", "ToneVariant")?;
    let tone = decode_tone(&format!("{path}.tone"), tone_j)?;
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

fn decode_form_field_kind(path: &str, j: &JVal) -> DResult<FormFieldKind> {
    let fields = as_obj(path, j)?;
    let on_change = opt_closure(fields, "onChange");
    let on_toggle = opt_closure(fields, "onToggle");
    match disc(path, fields)? {
        "Text" => Ok(FormFieldKind::Text {
            value: req_binding(path, fields, "value", "Text value binding")?,
            on_change,
        }),
        "Number" => Ok(FormFieldKind::Number {
            value: req_binding(path, fields, "value", "Number value binding")?,
            on_change,
        }),
        "Checkbox" => Ok(FormFieldKind::Checkbox {
            value: req_binding(path, fields, "value", "Checkbox value binding")?,
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
            value: req_binding_slot(
                path,
                fields,
                "value",
                "Choice value binding",
                StaticSlot::StringOpt,
            )?,
            on_change,
        }),
        "RangedNumber" => Ok(FormFieldKind::RangedNumber {
            value: req_binding(path, fields, "value", "RangedNumber value binding")?,
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
            let orientation_j = req(path, fields, "orientation", "Orientation")?;
            let orientation = decode_orientation(&format!("{path}.orientation"), orientation_j)?;
            let value = req_binding_slot(
                path,
                fields,
                "value",
                "SegmentedChoice value binding",
                StaticSlot::StringOpt,
            )?;
            Ok(FormFieldKind::SegmentedChoice {
                options,
                orientation,
                value,
                on_change,
            })
        }
        "TextArea" => Ok(FormFieldKind::TextArea {
            rows: req_int(path, fields, "rows", "textarea row count integer")?,
            value: req_binding(path, fields, "value", "TextArea value binding")?,
            on_change,
        }),
        "Date" => {
            let value = req_binding(path, fields, "value", "Date value binding")?;
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
        other => Err(unknown_du_case(
            path,
            other,
            "Text | Number | Checkbox | Choice | RangedNumber | SegmentedChoice | TextArea | Date",
        )),
    }
}

fn decode_form_field(path: &str, j: &JVal) -> DResult<FormField> {
    let fields = as_obj(path, j)?;
    let id = req_string(path, fields, "id", "form field id string")?;
    let kind_j = req(path, fields, "kind", "FormFieldKind")?;
    let kind = decode_form_field_kind(&format!("{path}.kind"), kind_j)?;
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

fn decode_filter_kind(path: &str, j: &JVal) -> DResult<FilterKind> {
    let fields = as_obj(path, j)?;
    let on_change = opt_closure(fields, "onChange");
    match disc(path, fields)? {
        "TextFilter" => Ok(FilterKind::TextFilter {
            value: req_binding(path, fields, "value", "TextFilter value binding")?,
            on_change,
        }),
        "ChoiceFilter" => Ok(FilterKind::ChoiceFilter {
            options: req_binding_slot(
                path,
                fields,
                "options",
                "ChoiceFilter options binding",
                StaticSlot::Options,
            )?,
            value: req_binding_slot(
                path,
                fields,
                "value",
                "ChoiceFilter value binding",
                StaticSlot::StringOpt,
            )?,
            on_change,
        }),
        "RangeFilter" => {
            // Typed `{min,max}` bounds (Phase 423); the legacy `"<opaque>"`
            // sentinel / an absent or partial object reads as the (0,0)
            // placeholder, whose re-encode is the typed form.
            let mut bounds = (0.0, 0.0);
            if let Some(JVal::Obj(vf)) = get(fields, "value")
                && let (Some(min_j), Some(max_j)) = (get(vf, "min"), get(vf, "max"))
            {
                let min = as_float(&format!("{path}.value.min"), min_j)?;
                let max = as_float(&format!("{path}.value.max"), max_j)?;
                bounds = (min, max);
            }
            Ok(FilterKind::RangeFilter {
                min: bounds.0,
                max: bounds.1,
                on_change,
            })
        }
        "SegmentedFilter" => {
            let options = req_binding_slot(
                path,
                fields,
                "options",
                "SegmentedFilter options binding",
                StaticSlot::Options,
            )?;
            let orientation_j = req(path, fields, "orientation", "Orientation")?;
            let orientation = decode_orientation(&format!("{path}.orientation"), orientation_j)?;
            let value = req_binding_slot(
                path,
                fields,
                "value",
                "SegmentedFilter value binding",
                StaticSlot::StringOpt,
            )?;
            Ok(FilterKind::SegmentedFilter {
                options,
                orientation,
                value,
                on_change,
            })
        }
        other => Err(unknown_du_case(
            path,
            other,
            "TextFilter | ChoiceFilter | RangeFilter | SegmentedFilter",
        )),
    }
}

fn decode_filter_spec(path: &str, j: &JVal) -> DResult<FilterSpec> {
    let fields = as_obj(path, j)?;
    let kind_j = req(path, fields, "kind", "FilterKind")?;
    let kind = decode_filter_kind(&format!("{path}.kind"), kind_j)?;
    let label = req_text_source(path, fields, "label", "filter label TextSource")?;
    let name = req_string(path, fields, "name", "filter name string")?;
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
    let source = req_binding_slot(
        path,
        fields,
        "source",
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

fn decode_cell_kind_erased(path: &str, j: &JVal) -> DResult<CellKindErased> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
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
            "Text | Numeric | Date | Editable | Checkbox | Button | ButtonGroup | Link | Pill | Progress | Custom",
        )),
    }
}

fn decode_column_erased(path: &str, j: &JVal) -> DResult<ColumnErased> {
    let fields = as_obj(path, j)?;
    let format_j = req(path, fields, "format", "CellFormat")?;
    let format = decode_cell_format(&format!("{path}.format"), format_j)?;
    let kind_j = req(path, fields, "kind", "CellKindErased")?;
    let kind = decode_cell_kind_erased(&format!("{path}.kind"), kind_j)?;
    let label = req_string(path, fields, "label", "column label string")?;
    let width_j = req(path, fields, "width", "ColumnWidth")?;
    let width = decode_column_width(&format!("{path}.width"), width_j)?;
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
    Ok(StaticRows { headers, rows })
}

fn decode_grid_spec(path: &str, j: &JVal) -> DResult<GridSpec> {
    let fields = as_obj(path, j)?;
    let columns_j = req(path, fields, "columns", "columns list")?;
    let arr = as_arr(&format!("{path}.columns"), columns_j)?;
    let mut columns = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        columns.push(decode_column_erased(&format!("{path}.columns[{i}]"), item)?);
    }
    let editable = req_bool(path, fields, "editable", "editable bool")?;
    let source = req_binding(path, fields, "source", "Grid source binding")?;
    let row_key_field = opt_string(path, fields, "rowKeyField")?;
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
        static_rows,
    })
}

fn decode_chart_spec(path: &str, j: &JVal) -> DResult<ChartSpec> {
    let fields = as_obj(path, j)?;
    let kind_j = req(path, fields, "kind", "ChartKind")?;
    let kind = decode_chart_kind(&format!("{path}.kind"), kind_j)?;
    let source = req_binding(path, fields, "source", "Chart source binding")?;
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
    let source = req_binding_slot(
        path,
        fields,
        "source",
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
        "Grid" => Ok(BoxLayout::Grid {
            cols: req_int(path, fields, "cols", "cols integer")?,
            gap: opt_int(path, fields, "gap")?,
            template_columns: opt_string(path, fields, "templateColumns")?,
        }),
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
    let heading = opt_text_source(path, fields, "heading")?;
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

// Legacy decode-upgrades: the four retired container tags fold into the
// equivalent `Box` on read (permalink / op-stream compatibility) and never
// re-encode to their old form.

fn decode_legacy_dashboard(path: &str, j: &JVal) -> DResult<BoxSpec> {
    let fields = as_obj(path, j)?;
    Ok(BoxSpec {
        children: decode_children(path, fields)?,
        heading: None,
        layout: BoxLayout::Auto,
        role: BoxRole::Dashboard,
    })
}

fn decode_legacy_stack(path: &str, j: &JVal) -> DResult<BoxSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    let orientation_j = req(path, fields, "orientation", "Orientation")?;
    let direction = decode_orientation(&format!("{path}.orientation"), orientation_j)?;
    let wrap = req_bool(path, fields, "wrap", "wrap bool")?;
    Ok(BoxSpec {
        children,
        heading: None,
        layout: BoxLayout::Flex {
            direction,
            gap: None,
            wrap,
        },
        role: BoxRole::Group,
    })
}

fn decode_legacy_grid_layout(path: &str, j: &JVal) -> DResult<BoxSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    let cols = req_int(path, fields, "cols", "cols integer")?;
    let template_columns = opt_string(path, fields, "templateColumns")?;
    Ok(BoxSpec {
        children,
        heading: None,
        layout: BoxLayout::Grid {
            cols,
            gap: None,
            template_columns,
        },
        role: BoxRole::Group,
    })
}

fn decode_legacy_card(path: &str, j: &JVal) -> DResult<BoxSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    let heading = opt_text_source(path, fields, "heading")?;
    Ok(BoxSpec {
        children,
        heading,
        layout: BoxLayout::Flex {
            direction: Orientation::Vertical,
            gap: None,
            wrap: false,
        },
        role: BoxRole::Card,
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
    let orientation_j = req(path, fields, "orientation", "Orientation")?;
    let orientation = decode_orientation(&format!("{path}.orientation"), orientation_j)?;
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
    let heading = opt_text_source(path, fields, "heading")?;
    Ok(SummaryListSpec { children, heading })
}

fn decode_disclosure_spec(path: &str, j: &JVal) -> DResult<DisclosureSpec> {
    let fields = as_obj(path, j)?;
    let children = decode_children(path, fields)?;
    let default_open = req_bool(path, fields, "defaultOpen", "defaultOpen bool")?;
    let heading = req_text_source(path, fields, "heading", "Disclosure heading TextSource")?;
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
    let heading = opt_text_source(path, fields, "heading")?;
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

const WRONG_NODE_KIND_HINT: &str = "a Layout primitive (Box | Dashboard | Stack | GridLayout | SplitPanel | Tabs | Card | Stepper | SummaryList | Disclosure | Modal | ScrollArea), a Display primitive (Heading | Markdown | Metric | Badge | Sparkline | Callout | Progress | Skeleton | LabelValueRow | Link | Image | List | Toast | CodeBlock | Math | Drawing), an Input primitive (Form | Filters | Button | FileUpload | Select), a Visualisation primitive (DataGrid | Chart | Table | Map), or Custom | ErrorBoundary | Switch | FragmentDecl | FragmentRef | Mount";

fn decode_node_kind(path: &str, j: &JVal) -> DResult<NodeKind> {
    let fields = as_obj(path, j)?;
    match disc(path, fields)? {
        // Layout (incl. the four legacy decode-upgrade tags).
        "Box" => Ok(NodeKind::Box(decode_box_spec(path, j)?)),
        "Dashboard" => Ok(NodeKind::Box(decode_legacy_dashboard(path, j)?)),
        "Stack" => Ok(NodeKind::Box(decode_legacy_stack(path, j)?)),
        "GridLayout" => Ok(NodeKind::Box(decode_legacy_grid_layout(path, j)?)),
        "Card" => Ok(NodeKind::Box(decode_legacy_card(path, j)?)),
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
        // Legacy decode-upgrade: a `Table` tag folds into a read-only grid
        // (the staticRows mode) and never re-encodes as `Table`.
        "Table" => {
            let rows = decode_static_rows(path, j)?;
            Ok(NodeKind::DataGrid(GridSpec {
                columns: vec![],
                editable: false,
                source: Binding::Static {
                    value: StaticValue::Ast(JVal::Str(OPAQUE.to_string())),
                },
                on_row_click: None,
                row_key: None,
                row_key_field: None,
                static_rows: Some(rows),
            }))
        }
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
            let state_key = req_string(path, fields, "stateKey", "Switch stateKey string")?;
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
                state_key,
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
    let tone_j = req(path, fields, "tone", "ToneVariant")?;
    let tone = decode_tone(&format!("{path}.tone"), tone_j)?;
    let weight_j = req(path, fields, "weight", "StyleWeight")?;
    let weight = decode_weight(&format!("{path}.weight"), weight_j)?;
    let emphasis_j = req(path, fields, "emphasis", "Emphasis")?;
    let emphasis = decode_emphasis(&format!("{path}.emphasis"), emphasis_j)?;
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
            let position = req_int(path, fields, "position", "position integer")?;
            let child_j = req(path, fields, "child", "child Node object")?;
            let child = decode_node_ast(&format!("{path}.child"), child_j)?;
            Ok(TreeOp::InsertChild {
                parent_id,
                position,
                child,
            })
        }
        "RemoveNode" => Ok(TreeOp::RemoveNode {
            target: req_string(path, fields, "target", "target NodeId")?,
        }),
        "MoveNode" => {
            let target = req_string(path, fields, "target", "target NodeId")?;
            let new_parent_id = req_string(path, fields, "newParentId", "new parent NodeId")?;
            let new_position = req_int(path, fields, "newPosition", "new position integer")?;
            Ok(TreeOp::MoveNode {
                target,
                new_parent_id,
                new_position,
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
}
