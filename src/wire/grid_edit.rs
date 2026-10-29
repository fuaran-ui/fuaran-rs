//! The `GridEdit` wire codec (WIRE_FORMAT §3.2, "Edits as column ops —
//! GridEdit"; Phase 2500) and the published face of the grid-edit algebra.
//!
//! A grid edit is carried as the columnar op it is, beside the whole-rows write.
//! The algebra — the op's application and inverse, the row delta under a
//! declared `rowKeyField`, the edit history — is the Core twin
//! (`crate::core::grid_edit`), re-exported here whole. What lives here is the
//! codec: the op's cells in the columnar op codec's `{"$type":…,"v":…}` form, the
//! pipeline and table through this crate's dataframe codec, and the strict
//! `GridEdit` envelope.

use crate::canonical::{JVal, parse};
pub use crate::core::grid_edit::*;
use crate::core::transform::Table;

use super::decode::{decode_data_source, decode_pipeline};
use super::encode::{arr, boolean, case_obj, data_source, field, int, num, obj, s, transform_step};
use super::model::{Cell, ColumnType, DataColumn, DataSource};

// ─── encode ──────────────────────────────────────────────────────────────────

fn op_cell(c: &Cell) -> String {
    match c {
        Cell::Null => case_obj("Null", vec![]),
        Cell::Int(v) => case_obj("Int", vec![field("v", int(*v))]),
        Cell::Float(v) => case_obj("Float", vec![field("v", num(*v))]),
        Cell::Bool(v) => case_obj("Bool", vec![field("v", boolean(*v))]),
        Cell::Str(v) => case_obj("Str", vec![field("v", s(v))]),
        Cell::Date(v) => case_obj("Date", vec![field("v", s(v))]),
        Cell::Timestamp(v) => case_obj("Timestamp", vec![field("v", s(v))]),
        Cell::Decimal(v) => case_obj("Decimal", vec![field("v", s(v))]),
    }
}

fn op_column(c: &DataColumn) -> String {
    obj(vec![
        field("name", s(&c.name)),
        field("type", s(c.column_type.as_str())),
        field("cells", arr(c.cells.iter().map(op_cell).collect())),
    ])
}

/// The canonical wire string for a columnar op.
pub fn encode_column_op(op: &ColumnOp) -> String {
    match op {
        ColumnOp::SetCell { col, row, value } => case_obj(
            "setCell",
            vec![
                field("col", s(col)),
                field("row", int(*row)),
                field("value", op_cell(value)),
            ],
        ),
        ColumnOp::SetColumn { column } => {
            case_obj("setColumn", vec![field("column", op_column(column))])
        }
        ColumnOp::InsertColumn { index, column } => case_obj(
            "insertColumn",
            vec![
                field("index", int(*index)),
                field("column", op_column(column)),
            ],
        ),
        ColumnOp::RemoveColumn { name } => case_obj("removeColumn", vec![field("name", s(name))]),
        ColumnOp::AppendRows { rows } => case_obj(
            "appendRows",
            vec![field(
                "rows",
                arr(rows
                    .iter()
                    .map(|row| {
                        arr(row
                            .iter()
                            .map(|(n, v)| obj(vec![field("col", s(n)), field("value", op_cell(v))]))
                            .collect())
                    })
                    .collect()),
            )],
        ),
        ColumnOp::ApplyTransform { pipeline } => case_obj(
            "applyTransform",
            vec![field(
                "pipeline",
                arr(pipeline.iter().map(transform_step).collect()),
            )],
        ),
    }
}

/// The canonical wire string for a grid edit; an undeclared `rowKeyField` is
/// omitted (absence means no identity is declared).
pub fn encode_grid_edit(edit: &GridEdit) -> String {
    let mut fields = vec![
        field("key", s(&edit.key)),
        field("op", encode_column_op(&edit.op)),
    ];
    if let Some(rk) = &edit.row_key_field {
        fields.push(field("rowKeyField", s(rk)));
    }
    case_obj("GridEdit", fields)
}

/// The canonical wire string for a row delta.
pub fn encode_table_delta(d: &TableDelta) -> String {
    match d {
        TableDelta::FullRefresh => case_obj("fullRefresh", vec![]),
        TableDelta::RowSet { scheme, rows } => case_obj(
            "rowSet",
            vec![
                field("scheme", s(scheme)),
                field(
                    "rows",
                    arr(rows
                        .iter()
                        .map(|(k, c)| case_obj(c.as_str(), vec![field("key", s(k))]))
                        .collect()),
                ),
                field("columns", arr(vec![])),
            ],
        ),
    }
}

/// The canonical wire string for an embedded table (the column codec's form).
pub fn encode_table(t: &Table) -> String {
    data_source(&DataSource::Embedded {
        schema: t.schema.clone(),
        columns: t.columns.clone(),
    })
}

// ─── decode ──────────────────────────────────────────────────────────────────

fn member<'a>(j: &'a JVal, key: &str) -> Option<&'a JVal> {
    j.field(key)
}

fn str_member(j: &JVal, key: &str) -> Option<String> {
    match member(j, key) {
        Some(JVal::Str(v)) => Some(v.clone()),
        _ => None,
    }
}

fn int_member(j: &JVal, key: &str) -> Option<i64> {
    match member(j, key) {
        Some(JVal::Num(n)) if n.fract() == 0.0 && n.abs() <= 2f64.powi(31) => Some(*n as i64),
        _ => None,
    }
}

fn decode_op_cell(j: &JVal) -> Result<Cell, String> {
    let kind = str_member(j, "$type").ok_or("cell: expected a $type-tagged object")?;
    if kind == "Null" {
        return Ok(Cell::Null);
    }
    let v = member(j, "v").ok_or("cell: missing field: v")?;
    match (kind.as_str(), v) {
        ("Int", JVal::Num(n))
            if n.fract() == 0.0 && (-2_147_483_648.0..=2_147_483_647.0).contains(n) =>
        {
            Ok(Cell::Int(*n as i64))
        }
        ("Float", JVal::Num(n)) => Ok(Cell::Float(*n)),
        ("Bool", JVal::Bool(b)) => Ok(Cell::Bool(*b)),
        ("Str", JVal::Str(t)) => Ok(Cell::Str(t.clone())),
        ("Date", JVal::Str(t)) => Ok(Cell::Date(t.clone())),
        ("Timestamp", JVal::Str(t)) => Ok(Cell::Timestamp(t.clone())),
        ("Decimal", JVal::Str(t)) => crate::core::decimal::try_canonical(t)
            .map(Cell::Decimal)
            .ok_or_else(|| format!("cell: not decimal text: {t}")),
        ("Decimal", JVal::Num(n)) if n.fract() == 0.0 => {
            Ok(Cell::Decimal(format!("{}", *n as i64)))
        }
        ("Int" | "Float" | "Bool" | "Str" | "Date" | "Timestamp" | "Decimal", _) => {
            Err(format!("cell: a {kind} cell carries the wrong value type"))
        }
        _ => Err(format!("unknown cell kind: {kind}")),
    }
}

fn decode_op_column(j: &JVal) -> Result<DataColumn, String> {
    let (Some(name), Some(ty), Some(JVal::Arr(cells))) = (
        str_member(j, "name"),
        str_member(j, "type"),
        member(j, "cells"),
    ) else {
        return Err("column: expected {name, type, cells}".to_string());
    };
    let column_type =
        ColumnType::from_wire(&ty).ok_or_else(|| format!("unknown column type: {ty}"))?;
    let cells = cells
        .iter()
        .map(decode_op_cell)
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(row) = cells.iter().position(|c| !cell_fits(column_type, c)) {
        return Err(format!(
            "column '{name}': a cell of the wrong type at row {row}"
        ));
    }
    Ok(DataColumn {
        name,
        column_type,
        cells,
    })
}

/// Decode a columnar op from a parsed JSON value.
pub fn decode_column_op_json(j: &JVal) -> Result<ColumnOp, String> {
    let kind = str_member(j, "$type").ok_or("op: expected a $type-tagged object")?;
    match kind.as_str() {
        "setCell" => {
            let (Some(col), Some(row), Some(value)) = (
                str_member(j, "col"),
                int_member(j, "row"),
                member(j, "value"),
            ) else {
                return Err("setCell: expected {col, row, value}".to_string());
            };
            Ok(ColumnOp::SetCell {
                col,
                row,
                value: decode_op_cell(value)?,
            })
        }
        "setColumn" => Ok(ColumnOp::SetColumn {
            column: decode_op_column(
                member(j, "column").ok_or("setColumn: missing field: column")?,
            )?,
        }),
        "insertColumn" => {
            let index = int_member(j, "index").ok_or("insertColumn: expected an int index")?;
            let column = decode_op_column(
                member(j, "column").ok_or("insertColumn: missing field: column")?,
            )?;
            Ok(ColumnOp::InsertColumn { index, column })
        }
        "removeColumn" => Ok(ColumnOp::RemoveColumn {
            name: str_member(j, "name").ok_or("removeColumn: expected a name")?,
        }),
        "appendRows" => {
            let Some(JVal::Arr(rows)) = member(j, "rows") else {
                return Err("appendRows: expected a rows array".to_string());
            };
            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let JVal::Arr(cells) = row else {
                    return Err("appendRows: a row is an array".to_string());
                };
                let mut decoded = Vec::with_capacity(cells.len());
                for cell in cells {
                    let (Some(col), Some(value)) = (str_member(cell, "col"), member(cell, "value"))
                    else {
                        return Err("appendRows: expected {col, value}".to_string());
                    };
                    decoded.push((col, decode_op_cell(value)?));
                }
                out.push(decoded);
            }
            Ok(ColumnOp::AppendRows { rows: out })
        }
        "applyTransform" => Ok(ColumnOp::ApplyTransform {
            pipeline: decode_pipeline(
                member(j, "pipeline").ok_or("applyTransform: missing field: pipeline")?,
            )?,
        }),
        other => Err(format!("unknown columnar op: {other}")),
    }
}

/// Decode a `GridEdit` from a parsed JSON value. STRICT: a member outside the
/// four, an empty `key` or `rowKeyField`, or an op the codec does not know is
/// refused, because an edit applied differently from the one sent is worse than
/// one refused.
pub fn decode_grid_edit_json(j: &JVal) -> Result<GridEdit, String> {
    let JVal::Obj(fields) = j else {
        return Err("GridEdit: expected an object".to_string());
    };
    if let Some((k, _)) = fields
        .iter()
        .find(|(k, _)| !matches!(k.as_str(), "$type" | "key" | "op" | "rowKeyField"))
    {
        return Err(format!("GridEdit: unknown member '{k}'"));
    }
    if str_member(j, "$type").as_deref() != Some("GridEdit") {
        return Err("GridEdit: $type must be \"GridEdit\"".to_string());
    }
    let key = str_member(j, "key")
        .filter(|k| !k.is_empty())
        .ok_or("GridEdit: key must be a non-empty string")?;
    let op = member(j, "op").ok_or("GridEdit: missing op")?;
    let row_key_field = match member(j, "rowKeyField") {
        None => None,
        Some(JVal::Str(rk)) if !rk.is_empty() => Some(rk.clone()),
        Some(_) => return Err("GridEdit: rowKeyField must be a non-empty string".to_string()),
    };
    let op = decode_column_op_json(op).map_err(|e| format!("GridEdit: op: {e}"))?;
    Ok(GridEdit {
        key,
        op,
        row_key_field,
    })
}

/// Decode a `GridEdit` from its wire text.
pub fn decode_grid_edit(text: &str) -> Result<GridEdit, String> {
    let j = parse(text).map_err(|e| format!("GridEdit: not json: {}", e.message))?;
    decode_grid_edit_json(&j)
}

/// Decode an embedded table (the column codec's `{schema, columns}` form).
pub fn decode_table_json(j: &JVal) -> Result<Table, String> {
    match decode_data_source(j)? {
        DataSource::Embedded { schema, columns } => Ok(Table { schema, columns }),
        DataSource::Ref { .. } => Err("a ref source carries no rows".to_string()),
    }
}
