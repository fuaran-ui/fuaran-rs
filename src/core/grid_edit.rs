//! Grid edits as columnar ops (WIRE_FORMAT §3.2, "Edits as column ops —
//! GridEdit"; this host since Phase 2500). The Core twin of the reference's
//! columnar op algebra (`Fuaran.Compute.ColumnOps`) and its server-driven edit
//! log: the op's application to a table (a typed refusal, never a panic), its
//! inverse read from the table it applies to, the row delta it induces under the
//! grid's declared `rowKeyField`, and an immutable edit history (apply, undo,
//! redo). The `GridEdit` wire codec stays in `crate::wire`, which re-exports
//! everything here.

use super::dataframe::{
    Cell, ColumnType, DataColumn, SchemaEntry, TransformStep, instant_fraction_digits,
};
use super::transform::{Table, cell_token, eval_error_string, eval_pipeline};

/// A columnar edit op over a [`Table`].
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnOp {
    /// Set the cell at `row` of column `col` (null, or a cell that widens into the column's type).
    SetCell { col: String, row: i64, value: Cell },
    /// Replace an existing column by name (its length must equal the row count).
    SetColumn { column: DataColumn },
    /// Insert a NEW column at `index` (clamped into range).
    InsertColumn { index: i64, column: DataColumn },
    /// Remove the named column.
    RemoveColumn { name: String },
    /// Append rows, each naming a subset of the columns; an unnamed column gets null.
    AppendRows { rows: Vec<Vec<(String, Cell)>> },
    /// Apply a whole pipeline; the result replaces the table.
    ApplyTransform { pipeline: Vec<TransformStep> },
}

/// A grid edit: the State key it commits to, the op, and the grid's declared row
/// identity (`None` — the member absent — means none is declared).
#[derive(Debug, Clone, PartialEq)]
pub struct GridEdit {
    pub key: String,
    pub op: ColumnOp,
    pub row_key_field: Option<String>,
}

/// Why an op, or a history gesture, was refused: the corpus's token and a reason.
#[derive(Debug, Clone, PartialEq)]
pub struct EditRefusal {
    pub token: &'static str,
    pub detail: String,
}

fn refuse<T>(token: &'static str, detail: String) -> Result<T, EditRefusal> {
    Err(EditRefusal { token, detail })
}

// ─── table helpers ───────────────────────────────────────────────────────────

fn row_count(t: &Table) -> usize {
    t.columns.first().map_or(0, |c| c.cells.len())
}

fn names(t: &Table) -> String {
    t.columns
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn find<'a>(t: &'a Table, name: &str) -> Option<(usize, &'a DataColumn)> {
    t.columns.iter().enumerate().find(|(_, c)| c.name == name)
}

/// A cell fits a column when it is null or of a type that widens into the
/// column's: the type itself, an int into a float or a decimal column, a coarser
/// instant into a finer timestamp unit.
pub fn cell_fits(ty: ColumnType, c: &Cell) -> bool {
    match c {
        Cell::Null => true,
        Cell::Int(_) => matches!(
            ty,
            ColumnType::Int | ColumnType::Float | ColumnType::Decimal
        ),
        Cell::Float(_) => ty == ColumnType::Float,
        Cell::Bool(_) => ty == ColumnType::Bool,
        Cell::Str(_) => ty == ColumnType::Str,
        Cell::Date(_) => ty == ColumnType::Date,
        Cell::Decimal(_) => ty == ColumnType::Decimal,
        Cell::Timestamp(s) => {
            let digits = instant_fraction_digits(s).unwrap_or(0);
            let cell = ColumnType::timestamp_of_digits(digits)
                .timestamp_digits()
                .unwrap_or(0);
            ty.timestamp_digits().is_some_and(|d| cell <= d)
        }
    }
}

/// `t` with the column of `col.name` replaced; a retyped column gets a fresh schema entry.
fn with_column(t: &Table, col: DataColumn) -> Table {
    let schema = t
        .schema
        .iter()
        .map(|e| {
            if e.name == col.name && e.column_type != col.column_type {
                SchemaEntry::new(col.name.clone(), col.column_type)
            } else {
                e.clone()
            }
        })
        .collect();
    let columns = t
        .columns
        .iter()
        .map(|c| {
            if c.name == col.name {
                col.clone()
            } else {
                c.clone()
            }
        })
        .collect();
    Table { schema, columns }
}

fn mismatch(name: &str, ty: ColumnType) -> String {
    format!("column '{name}': expected {} cell", ty.as_str())
}

// ─── apply / invert ──────────────────────────────────────────────────────────

/// Apply a columnar op to a table — total: a typed refusal, never a panic.
pub fn apply_column_op(op: &ColumnOp, t: &Table) -> Result<Table, EditRefusal> {
    match op {
        ColumnOp::SetCell { col, row, value } => {
            let Some((_, c)) = find(t, col) else {
                return refuse(
                    "noSuchColumn",
                    format!("no such column '{col}'; available: {}", names(t)),
                );
            };
            let rc = row_count(t);
            if *row < 0 || *row as usize >= rc {
                return refuse(
                    "rowOutOfRange",
                    format!("row {row} out of range (row count {rc})"),
                );
            }
            if !cell_fits(c.column_type, value) {
                return refuse("cellTypeMismatch", mismatch(col, c.column_type));
            }
            let mut cells = c.cells.clone();
            cells[*row as usize] = value.clone();
            Ok(with_column(
                t,
                DataColumn {
                    name: c.name.clone(),
                    column_type: c.column_type,
                    cells,
                },
            ))
        }
        ColumnOp::SetColumn { column } => {
            if find(t, &column.name).is_none() {
                return refuse(
                    "noSuchColumn",
                    format!("no such column '{}'; available: {}", column.name, names(t)),
                );
            }
            let (rc, n) = (row_count(t), column.cells.len());
            if n != rc {
                return refuse(
                    "columnLengthMismatch",
                    format!("column '{}': length {n} != row count {rc}", column.name),
                );
            }
            Ok(with_column(t, column.clone()))
        }
        ColumnOp::InsertColumn { index, column } => {
            if find(t, &column.name).is_some() {
                return refuse(
                    "duplicateColumn",
                    format!("duplicate column: {}", column.name),
                );
            }
            let (rc, n) = (row_count(t), column.cells.len());
            if !t.columns.is_empty() && n != rc {
                return refuse(
                    "columnLengthMismatch",
                    format!("column '{}': length {n} != row count {rc}", column.name),
                );
            }
            let i = (*index).clamp(0, t.columns.len() as i64) as usize;
            let mut out = t.clone();
            out.schema
                .insert(i, SchemaEntry::new(column.name.clone(), column.column_type));
            out.columns.insert(i, column.clone());
            Ok(out)
        }
        ColumnOp::RemoveColumn { name } => {
            if find(t, name).is_none() {
                return refuse(
                    "noSuchColumn",
                    format!("no such column '{name}'; available: {}", names(t)),
                );
            }
            Ok(Table {
                schema: t
                    .schema
                    .iter()
                    .filter(|e| &e.name != name)
                    .cloned()
                    .collect(),
                columns: t
                    .columns
                    .iter()
                    .filter(|c| &c.name != name)
                    .cloned()
                    .collect(),
            })
        }
        ColumnOp::AppendRows { rows } => {
            for (name, value) in rows.iter().flatten() {
                let Some((_, c)) = find(t, name) else {
                    return refuse(
                        "rowShapeUnknownColumn",
                        format!(
                            "row references unknown column '{name}'; available: {}",
                            names(t)
                        ),
                    );
                };
                if !cell_fits(c.column_type, value) {
                    return refuse("cellTypeMismatch", mismatch(name, c.column_type));
                }
            }
            let columns = t
                .columns
                .iter()
                .map(|c| {
                    let mut cells = c.cells.clone();
                    for row in rows {
                        let v = row
                            .iter()
                            .find(|(n, _)| n == &c.name)
                            .map_or(Cell::Null, |(_, v)| v.clone());
                        cells.push(v);
                    }
                    DataColumn {
                        name: c.name.clone(),
                        column_type: c.column_type,
                        cells,
                    }
                })
                .collect();
            Ok(Table {
                schema: t.schema.clone(),
                columns,
            })
        }
        ColumnOp::ApplyTransform { pipeline } => eval_pipeline(pipeline, t)
            .or_else(|e| refuse("transformRejected", eval_error_string(&e))),
    }
}

/// The op that undoes `op` applied to the PRE-state `t`. An op the table would
/// refuse has no inverse (its refusal is returned); `AppendRows` and
/// `ApplyTransform` have none at any table.
pub fn invert_column_op(op: &ColumnOp, t: &Table) -> Result<ColumnOp, EditRefusal> {
    if matches!(
        op,
        ColumnOp::AppendRows { .. } | ColumnOp::ApplyTransform { .. }
    ) {
        return refuse(
            "notInvertible",
            "appendRows and applyTransform are not invertible".to_string(),
        );
    }
    apply_column_op(op, t)?;
    Ok(match op {
        ColumnOp::SetCell { col, row, .. } => {
            let (_, c) = find(t, col).expect("the apply above found the column");
            ColumnOp::SetCell {
                col: col.clone(),
                row: *row,
                value: c.cells[*row as usize].clone(),
            }
        }
        ColumnOp::SetColumn { column } => {
            let (_, old) = find(t, &column.name).expect("the apply above found the column");
            ColumnOp::SetColumn {
                column: old.clone(),
            }
        }
        ColumnOp::InsertColumn { column, .. } => ColumnOp::RemoveColumn {
            name: column.name.clone(),
        },
        ColumnOp::RemoveColumn { name } => {
            let (i, c) = find(t, name).expect("the apply above found the column");
            ColumnOp::InsertColumn {
                index: i as i64,
                column: c.clone(),
            }
        }
        ColumnOp::AppendRows { .. } | ColumnOp::ApplyTransform { .. } => {
            unreachable!("answered above")
        }
    })
}

// ─── the row delta ───────────────────────────────────────────────────────────

/// How a row changed under a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowChange {
    Added,
    Changed,
    Removed,
}

impl RowChange {
    /// The wire tag.
    pub fn as_str(self) -> &'static str {
        match self {
            RowChange::Added => "added",
            RowChange::Changed => "changed",
            RowChange::Removed => "removed",
        }
    }
}

/// A row delta: the top element, or the rows a change touched by key under a
/// scheme — rows in ordinal (UTF-16) key order.
#[derive(Debug, Clone, PartialEq)]
pub enum TableDelta {
    FullRefresh,
    RowSet {
        scheme: String,
        rows: Vec<(String, RowChange)>,
    },
}

fn keys_of(t: &Table, field: &str) -> Vec<Option<String>> {
    match find(t, field) {
        Some((_, c)) => c
            .cells
            .iter()
            .map(|x| (*x != Cell::Null).then(|| cell_token(x)))
            .collect(),
        None => vec![None; row_count(t)],
    }
}

fn row_set(scheme: String, mut rows: Vec<(String, RowChange)>) -> TableDelta {
    rows.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    TableDelta::RowSet { scheme, rows }
}

/// The row delta `op` induces on `before` under identity by the column
/// `key_field` (scheme `column:<field>`), built from the op and the before table,
/// never by diffing two tables. A schema or whole-table op, an op that does not
/// apply, and a row with no key are the top delta. Cell equality is the
/// reference's: a NaN never equals itself, and the zeros agree.
pub fn grid_edit_delta(key_field: &str, before: &Table, op: &ColumnOp) -> TableDelta {
    let scheme = format!("column:{key_field}");
    if matches!(
        op,
        ColumnOp::InsertColumn { .. }
            | ColumnOp::RemoveColumn { .. }
            | ColumnOp::ApplyTransform { .. }
    ) {
        return TableDelta::FullRefresh;
    }
    let Ok(after) = apply_column_op(op, before) else {
        return TableDelta::FullRefresh;
    };
    let (k0, k1) = (keys_of(before, key_field), keys_of(&after, key_field));
    match op {
        ColumnOp::SetCell { col, row, value } => {
            let r = *row as usize;
            let (Some(a), Some(b)) = (k0[r].clone(), k1[r].clone()) else {
                return TableDelta::FullRefresh;
            };
            let (_, old) = find(before, col).expect("the apply above found the column");
            if &old.cells[r] == value {
                row_set(scheme, vec![])
            } else if a == b {
                row_set(scheme, vec![(a, RowChange::Changed)])
            } else {
                row_set(scheme, vec![(a, RowChange::Removed), (b, RowChange::Added)])
            }
        }
        ColumnOp::SetColumn { column } => {
            if k0 != k1 || k0.iter().any(Option::is_none) {
                return TableDelta::FullRefresh;
            }
            let (_, old) = find(before, &column.name).expect("the apply above found the column");
            let rows = k0
                .iter()
                .zip(old.cells.iter().zip(&column.cells))
                .filter(|(_, (x, y))| x != y)
                .map(|(k, _)| (k.clone().expect("checked above"), RowChange::Changed))
                .collect();
            row_set(scheme, rows)
        }
        ColumnOp::AppendRows { .. } => {
            let added = &k1[k0.len()..];
            let mut seen: Vec<&String> = k0.iter().flatten().collect();
            let mut rows = Vec::new();
            for k in added {
                match k {
                    Some(k) if !seen.contains(&k) => {
                        seen.push(k);
                        rows.push((k.clone(), RowChange::Added));
                    }
                    _ => return TableDelta::FullRefresh,
                }
            }
            row_set(scheme, rows)
        }
        _ => TableDelta::FullRefresh,
    }
}

// ─── the edit history ────────────────────────────────────────────────────────

/// An edit history over one table. Immutable: every gesture returns a new
/// history. An edit's inverse is read from the table it is applied to; an edit
/// with none (`AppendRows`, `ApplyTransform`) applies and clears what could be
/// undone; any new edit clears what could be redone. Bounded: past its capacity
/// the oldest edit is no longer undoable.
#[derive(Debug, Clone, PartialEq)]
pub struct GridEditHistory {
    table: Table,
    capacity: usize,
    undo: Vec<(ColumnOp, ColumnOp)>, // most recent first
    redo: Vec<(ColumnOp, ColumnOp)>, // most recently undone first
}

impl GridEditHistory {
    /// A history over `table` keeping at most `capacity` undoable edits.
    pub fn new(table: Table, capacity: usize) -> GridEditHistory {
        GridEditHistory {
            table,
            capacity: capacity.max(1),
            undo: vec![],
            redo: vec![],
        }
    }

    /// The live table.
    pub fn table(&self) -> &Table {
        &self.table
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn pushed(&self, entry: (ColumnOp, ColumnOp)) -> Vec<(ColumnOp, ColumnOp)> {
        let mut out = vec![entry];
        out.extend(self.undo.iter().cloned());
        out.truncate(self.capacity);
        out
    }

    /// Apply an edit.
    pub fn apply(&self, op: &ColumnOp) -> Result<GridEditHistory, EditRefusal> {
        let table = apply_column_op(op, &self.table)?;
        let undo = match invert_column_op(op, &self.table) {
            Ok(inverse) => self.pushed((op.clone(), inverse)),
            Err(_) => vec![],
        };
        Ok(GridEditHistory {
            table,
            capacity: self.capacity,
            undo,
            redo: vec![],
        })
    }

    /// Undo the most recent undoable edit by applying its inverse.
    pub fn undo(&self) -> Result<GridEditHistory, EditRefusal> {
        let Some(top) = self.undo.first() else {
            return refuse("nothingToUndo", "nothing to undo".to_string());
        };
        let table = apply_column_op(&top.1, &self.table)?;
        let mut redo = vec![top.clone()];
        redo.extend(self.redo.iter().cloned());
        Ok(GridEditHistory {
            table,
            capacity: self.capacity,
            undo: self.undo[1..].to_vec(),
            redo,
        })
    }

    /// Redo the most recently undone edit.
    pub fn redo(&self) -> Result<GridEditHistory, EditRefusal> {
        let Some(top) = self.redo.first() else {
            return refuse("nothingToRedo", "nothing to redo".to_string());
        };
        let table = apply_column_op(&top.0, &self.table)?;
        Ok(GridEditHistory {
            table,
            capacity: self.capacity,
            undo: self.pushed(top.clone()),
            redo: self.redo[1..].to_vec(),
        })
    }
}
