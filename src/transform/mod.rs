//! The columnar dataframe evaluator — a pure fold over the columnar model that
//! runs a serialisable `Binding.Transform` pipeline **as data**, client- or
//! server-side, with no external engine. This is the compute substrate behind
//! the Living Sheet (a spreadsheet whose cells are a transform pipeline) and the
//! Pattern Bank's computed metric — a host evaluates the wire's transform to
//! rows deterministically.
//!
//! A faithful port of the cross-host reference evaluator: the pinned semantics
//! (null/NA propagation, int↔float coercion, group/sort stability,
//! round-half-away, division-by-zero, the canonical float layout) match exactly,
//! so the output table encodes byte-identically under the shared canonical codec
//! (the `§11.1` transform-laws parity contract). Where the reference dispatches
//! verbs and operators on strings, this host dispatches on the native `enum`s —
//! recovering compile-time exhaustiveness (a new verb/op is a build error).
//!
//! Total: every recoverable failure is an [`EvalError`], never a panic.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crate::canonical::format_number;
use crate::wire::{
    Agg, AggFn, BinOp, Cell, ColExpr, ColumnType, DataColumn, DataSource, JoinKind, ScalarFn,
    SchemaEntry, SortDir, SortKey, TransformStep, WindowFn,
};

/// A concrete evaluated table — schema (column order + types) + columnar cells.
/// The public input/output form of the evaluator (an `Embedded` `DataSource`).
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub schema: Vec<SchemaEntry>,
    pub columns: Vec<DataColumn>,
}

/// A recoverable evaluation failure. Mirrors the cross-host `EvalError` variants
/// (and their `eval_error_string` rendering) so diagnostics read identically.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    UnknownColumn {
        name: String,
        available: Vec<String>,
    },
    TypeError {
        detail: String,
    },
    AggError {
        detail: String,
    },
    JoinError {
        detail: String,
    },
    ArityError {
        func: String,
        expected: usize,
        got: usize,
    },
    UnresolvedSource {
        source_ref: String,
    },
    UnboundParam {
        name: String,
        bound: Vec<String>,
    },
}

/// The evaluation environment — binds each `ColExpr::Param` name to a `Cell`. A
/// `BTreeMap` so the `bound` list in an `UnboundParam` is deterministically
/// sorted (matching the reference's `Object.keys(env).sort()`).
pub type EvalEnv = BTreeMap<String, Cell>;

/// Resolve a `Ref` data source to a concrete [`Table`]. The default
/// ([`no_resolve`]) rejects every ref — embedded-only pipelines never call it.
pub type Resolver<'a> = dyn Fn(&str) -> Result<Table, EvalError> + 'a;

/// A `Ref`-rejecting resolver — the default for embedded-only pipelines.
pub fn no_resolve(source_ref: &str) -> Result<Table, EvalError> {
    Err(EvalError::UnresolvedSource {
        source_ref: source_ref.to_string(),
    })
}

// ─── internal row-oriented frame (the evaluator's working form) ──────────────

#[derive(Clone)]
struct Frame {
    cols: Vec<SchemaEntry>,
    rows: Vec<Vec<Cell>>,
}

const NULL: Cell = Cell::Null;

fn is_null(c: &Cell) -> bool {
    matches!(c, Cell::Null)
}

fn col_index(cols: &[SchemaEntry], name: &str) -> Option<usize> {
    cols.iter().position(|e| e.name == name)
}

fn col_type(cols: &[SchemaEntry], name: &str) -> Option<ColumnType> {
    cols.iter().find(|e| e.name == name).map(|e| e.column_type)
}

fn available(cols: &[SchemaEntry]) -> Vec<String> {
    cols.iter().map(|e| e.name.clone()).collect()
}

fn unknown_column(name: &str, cols: &[SchemaEntry]) -> EvalError {
    EvalError::UnknownColumn {
        name: name.to_string(),
        available: available(cols),
    }
}

impl Table {
    /// The number of rows (length of the first column, or 0 for an empty table).
    fn row_count(&self) -> usize {
        self.columns.first().map(|c| c.cells.len()).unwrap_or(0)
    }

    fn to_frame(&self) -> Frame {
        let n = self.row_count();
        let mut rows: Vec<Vec<Cell>> = Vec::with_capacity(n);
        for i in 0..n {
            let row = self
                .schema
                .iter()
                .map(|e| match self.columns.iter().find(|c| c.name == e.name) {
                    Some(c) if i < c.cells.len() => c.cells[i].clone(),
                    _ => NULL,
                })
                .collect();
            rows.push(row);
        }
        Frame {
            cols: self.schema.clone(),
            rows,
        }
    }
}

fn of_frame(f: &Frame) -> Table {
    let columns = f
        .cols
        .iter()
        .enumerate()
        .map(|(ci, e)| DataColumn {
            name: e.name.clone(),
            column_type: e.column_type,
            cells: f.rows.iter().map(|row| row[ci].clone()).collect(),
        })
        .collect();
    Table {
        schema: f.cols.clone(),
        columns,
    }
}

// ─── pinned scalar semantics ─────────────────────────────────────────────────

/// The canonical string of a cell (float via the shared canonical layout).
pub fn cell_string(c: &Cell) -> String {
    match c {
        Cell::Int(v) => v.to_string(),
        Cell::Float(v) => format_number(*v),
        Cell::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Cell::Str(s) | Cell::Date(s) | Cell::Timestamp(s) => s.clone(),
        Cell::Null => String::new(),
    }
}

fn as_num(c: &Cell) -> Option<f64> {
    match c {
        Cell::Int(v) => Some(*v as f64),
        Cell::Float(v) => Some(*v),
        _ => None,
    }
}

/// Total comparison of two present cells; `None` ⇒ incomparable families.
fn compare_cells(a: &Cell, b: &Cell) -> Option<Ordering> {
    if let (Some(an), Some(bn)) = (as_num(a), as_num(b)) {
        return an.partial_cmp(&bn);
    }
    match (a, b) {
        (Cell::Bool(x), Cell::Bool(y)) => Some(x.cmp(y)),
        (Cell::Str(x), Cell::Str(y)) => Some(x.cmp(y)),
        (Cell::Date(x), Cell::Date(y)) => Some(x.cmp(y)),
        (Cell::Timestamp(x), Cell::Timestamp(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn cell_eq(a: &Cell, b: &Cell) -> bool {
    if is_null(a) || is_null(b) {
        return false;
    }
    compare_cells(a, b) == Some(Ordering::Equal)
}

fn both_int(a: &Cell, b: &Cell) -> bool {
    matches!(a, Cell::Int(_)) && matches!(b, Cell::Int(_))
}

fn arith(op: BinOp, a: &Cell, b: &Cell) -> Result<Cell, EvalError> {
    if is_null(a) || is_null(b) {
        return Ok(NULL);
    }
    let (x, y) = match (as_num(a), as_num(b)) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            return Err(EvalError::TypeError {
                detail: "arithmetic on a non-numeric operand".to_string(),
            });
        }
    };
    let bi = both_int(a, b);
    match op {
        BinOp::Add => Ok(if bi {
            Cell::Int(x.trunc() as i64 + y.trunc() as i64)
        } else {
            Cell::Float(x + y)
        }),
        BinOp::Sub => Ok(if bi {
            Cell::Int(x.trunc() as i64 - y.trunc() as i64)
        } else {
            Cell::Float(x - y)
        }),
        BinOp::Mul => Ok(if bi {
            Cell::Int(x.trunc() as i64 * y.trunc() as i64)
        } else {
            Cell::Float(x * y)
        }),
        BinOp::Div => Ok(if y == 0.0 { NULL } else { Cell::Float(x / y) }),
        BinOp::Mod => {
            if !bi {
                return Err(EvalError::TypeError {
                    detail: "mod requires integer operands".to_string(),
                });
            }
            let yi = y.trunc() as i64;
            if yi == 0 {
                Ok(NULL)
            } else {
                Ok(Cell::Int(x.trunc() as i64 % yi))
            }
        }
        _ => Err(EvalError::TypeError {
            detail: "not an arithmetic operator".to_string(),
        }),
    }
}

fn comparison(op: BinOp, a: &Cell, b: &Cell) -> Result<Cell, EvalError> {
    if is_null(a) || is_null(b) {
        return Ok(NULL);
    }
    let c = compare_cells(a, b).ok_or(EvalError::TypeError {
        detail: "comparison between incompatible types".to_string(),
    })?;
    let r = match op {
        BinOp::Eq => c == Ordering::Equal,
        BinOp::Ne => c != Ordering::Equal,
        BinOp::Lt => c == Ordering::Less,
        BinOp::Le => c != Ordering::Greater,
        BinOp::Gt => c == Ordering::Greater,
        BinOp::Ge => c != Ordering::Less,
        _ => false,
    };
    Ok(Cell::Bool(r))
}

/// Kleene three-valued AND/OR over Bool/Null operands (`None` ⇒ unknown/Null).
fn logical(op: BinOp, a: &Cell, b: &Cell) -> Result<Cell, EvalError> {
    fn as_bool(c: &Cell) -> Result<Option<bool>, EvalError> {
        match c {
            Cell::Bool(v) => Ok(Some(*v)),
            Cell::Null => Ok(None),
            _ => Err(EvalError::TypeError {
                detail: "logical operator on a non-bool operand".to_string(),
            }),
        }
    }
    let xv = as_bool(a)?;
    let yv = as_bool(b)?;
    match op {
        BinOp::And => {
            if xv == Some(false) || yv == Some(false) {
                Ok(Cell::Bool(false))
            } else if xv == Some(true) && yv == Some(true) {
                Ok(Cell::Bool(true))
            } else {
                Ok(NULL)
            }
        }
        BinOp::Or => {
            if xv == Some(true) || yv == Some(true) {
                Ok(Cell::Bool(true))
            } else if xv == Some(false) && yv == Some(false) {
                Ok(Cell::Bool(false))
            } else {
                Ok(NULL)
            }
        }
        _ => Err(EvalError::TypeError {
            detail: "not a logical operator".to_string(),
        }),
    }
}

// .NET Double.TryParse(NumberStyles.Float, InvariantCulture) approximation —
// sign + decimal + exponent over a trimmed, gated string (no hex / Infinity).
fn try_parse_float(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // Gate: optional sign, digits with optional '.', optional exponent.
    let mut chars = t.chars().peekable();
    if matches!(chars.peek(), Some('+') | Some('-')) {
        chars.next();
    }
    let mut saw_digit = false;
    let mut saw_dot = false;
    let mut saw_exp = false;
    let rest: Vec<char> = chars.collect();
    for (i, ch) in rest.iter().enumerate() {
        match ch {
            '0'..='9' => saw_digit = true,
            '.' if !saw_dot && !saw_exp => saw_dot = true,
            'e' | 'E' if saw_digit && !saw_exp => {
                saw_exp = true;
                if matches!(rest.get(i + 1), Some('+') | Some('-')) {
                    // sign after exponent handled by skipping in the parse below
                } else if !matches!(rest.get(i + 1), Some('0'..='9')) {
                    return None;
                }
            }
            '+' | '-'
                if saw_exp && matches!(rest.get(i.wrapping_sub(1)), Some('e') | Some('E')) => {}
            _ => return None,
        }
    }
    if !saw_digit {
        return None;
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn try_parse_int(s: &str) -> Option<i64> {
    let t = s.trim();
    t.parse::<i64>().ok()
}

fn cast_cell(ty: ColumnType, c: &Cell) -> Result<Cell, EvalError> {
    if is_null(c) {
        return Ok(NULL);
    }
    match ty {
        ColumnType::Str => Ok(Cell::Str(cell_string(c))),
        ColumnType::Float => {
            if let Some(n) = as_num(c) {
                Ok(Cell::Float(n))
            } else if let Cell::Str(s) = c {
                try_parse_float(s)
                    .map(Cell::Float)
                    .ok_or(EvalError::TypeError {
                        detail: format!("cannot cast '{s}' to float"),
                    })
            } else {
                Err(EvalError::TypeError {
                    detail: "cannot cast to float".to_string(),
                })
            }
        }
        ColumnType::Int => match c {
            Cell::Int(_) => Ok(c.clone()),
            Cell::Float(v) => Ok(Cell::Int(v.trunc() as i64)),
            Cell::Bool(b) => Ok(Cell::Int(if *b { 1 } else { 0 })),
            Cell::Str(s) => try_parse_int(s).map(Cell::Int).ok_or(EvalError::TypeError {
                detail: format!("cannot cast '{s}' to int"),
            }),
            _ => Err(EvalError::TypeError {
                detail: "cannot cast to int".to_string(),
            }),
        },
        ColumnType::Bool => match c {
            Cell::Bool(_) => Ok(c.clone()),
            Cell::Int(v) => Ok(Cell::Bool(*v != 0)),
            _ => Err(EvalError::TypeError {
                detail: "cannot cast to bool".to_string(),
            }),
        },
        ColumnType::Date => match c {
            Cell::Date(_) => Ok(c.clone()),
            Cell::Str(s) => Ok(Cell::Date(s.clone())),
            _ => Err(EvalError::TypeError {
                detail: "cannot cast to date".to_string(),
            }),
        },
        ColumnType::Timestamp => match c {
            Cell::Timestamp(_) => Ok(c.clone()),
            Cell::Str(s) => Ok(Cell::Timestamp(s.clone())),
            _ => Err(EvalError::TypeError {
                detail: "cannot cast to timestamp".to_string(),
            }),
        },
    }
}

/// Round half away from zero — host-independent (not banker's rounding).
fn round_half_away(x: f64) -> f64 {
    if x >= 0.0 {
        (x + 0.5).floor()
    } else {
        (x - 0.5).ceil()
    }
}

fn apply_scalar(func: ScalarFn, args: &[Cell]) -> Result<Cell, EvalError> {
    let name = |f: ScalarFn| f.as_str().to_string();
    let arity = |n: usize| -> Result<(), EvalError> {
        if args.len() == n {
            Ok(())
        } else {
            Err(EvalError::ArityError {
                func: name(func),
                expected: n,
                got: args.len(),
            })
        }
    };
    // A unary numeric/string helper that short-circuits Null.
    let unary = |g: &dyn Fn(&Cell) -> Result<Cell, EvalError>| -> Result<Cell, EvalError> {
        arity(1)?;
        let c = &args[0];
        if is_null(c) { Ok(NULL) } else { g(c) }
    };
    match func {
        ScalarFn::Abs => unary(&|c| match c {
            Cell::Int(v) => Ok(Cell::Int(v.abs())),
            Cell::Float(v) => Ok(Cell::Float(v.abs())),
            _ => Err(EvalError::TypeError {
                detail: "abs of a non-numeric".to_string(),
            }),
        }),
        ScalarFn::Round => unary(&|c| {
            as_num(c)
                .map(|n| Cell::Float(round_half_away(n)))
                .ok_or(EvalError::TypeError {
                    detail: "round of a non-numeric".to_string(),
                })
        }),
        ScalarFn::Floor => unary(&|c| {
            as_num(c)
                .map(|n| Cell::Float(n.floor()))
                .ok_or(EvalError::TypeError {
                    detail: "floor of a non-numeric".to_string(),
                })
        }),
        ScalarFn::Ceil => unary(&|c| {
            as_num(c)
                .map(|n| Cell::Float(n.ceil()))
                .ok_or(EvalError::TypeError {
                    detail: "ceil of a non-numeric".to_string(),
                })
        }),
        ScalarFn::Length => unary(&|c| match c {
            Cell::Str(s) => Ok(Cell::Int(s.chars().count() as i64)),
            _ => Err(EvalError::TypeError {
                detail: "length of a non-string".to_string(),
            }),
        }),
        ScalarFn::Lower => unary(&|c| match c {
            Cell::Str(s) => Ok(Cell::Str(s.to_lowercase())),
            _ => Err(EvalError::TypeError {
                detail: "lower of a non-string".to_string(),
            }),
        }),
        ScalarFn::Upper => unary(&|c| match c {
            Cell::Str(s) => Ok(Cell::Str(s.to_uppercase())),
            _ => Err(EvalError::TypeError {
                detail: "upper of a non-string".to_string(),
            }),
        }),
        ScalarFn::Substr => {
            arity(3)?;
            let (s, start, len) = (&args[0], &args[1], &args[2]);
            if is_null(s) {
                return Ok(NULL);
            }
            match (s, start, len) {
                (Cell::Str(sv), Cell::Int(st0), Cell::Int(ln0)) => {
                    let chars: Vec<char> = sv.chars().collect();
                    let n = chars.len() as i64;
                    let st = (*st0).max(0).min(n);
                    let ln = (*ln0).max(0).min(n - st);
                    let slice: String = chars[st as usize..(st + ln) as usize].iter().collect();
                    Ok(Cell::Str(slice))
                }
                _ => Err(EvalError::TypeError {
                    detail: "substr expects (string, int, int)".to_string(),
                }),
            }
        }
        ScalarFn::DatePart => {
            arity(2)?;
            let (part, date) = (&args[0], &args[1]);
            if is_null(date) {
                return Ok(NULL);
            }
            let date_str = match date {
                Cell::Date(s) | Cell::Timestamp(s) | Cell::Str(s) => s,
                _ => {
                    return Err(EvalError::TypeError {
                        detail: "datePart expects (string part, date/timestamp/string)".to_string(),
                    });
                }
            };
            let part_str = match part {
                Cell::Str(s) => s,
                _ => {
                    return Err(EvalError::TypeError {
                        detail: "datePart expects (string part, date/timestamp/string)".to_string(),
                    });
                }
            };
            let slice = |lo: usize, len: usize| -> Result<Cell, EvalError> {
                if date_str.len() >= lo + len {
                    try_parse_int(&date_str[lo..lo + len]).map(Cell::Int).ok_or(
                        EvalError::TypeError {
                            detail: format!("datePart: unparseable component in '{date_str}'"),
                        },
                    )
                } else {
                    Err(EvalError::TypeError {
                        detail: format!("datePart: '{date_str}' too short for {part_str}"),
                    })
                }
            };
            match part_str.as_str() {
                "year" => slice(0, 4),
                "month" => slice(5, 2),
                "day" => slice(8, 2),
                other => Err(EvalError::TypeError {
                    detail: format!("datePart: unknown part '{other}'"),
                }),
            }
        }
    }
}

fn is_arith(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod
    )
}

fn is_comparison(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
    )
}

/// Evaluate a `ColExpr` against one row, resolving params from `env`.
fn eval_expr(
    cols: &[SchemaEntry],
    row: &[Cell],
    e: &ColExpr,
    env: &EvalEnv,
) -> Result<Cell, EvalError> {
    match e {
        ColExpr::Col { name } => col_index(cols, name)
            .map(|i| row[i].clone())
            .ok_or_else(|| unknown_column(name, cols)),
        ColExpr::Lit { cell } => Ok(cell.clone()),
        ColExpr::Param { name } => env
            .get(name)
            .cloned()
            .ok_or_else(|| EvalError::UnboundParam {
                name: name.clone(),
                bound: env.keys().cloned().collect(),
            }),
        ColExpr::Binary { op, left, right } => {
            let a = eval_expr(cols, row, left, env)?;
            let b = eval_expr(cols, row, right, env)?;
            if is_arith(*op) {
                arith(*op, &a, &b)
            } else if is_comparison(*op) {
                comparison(*op, &a, &b)
            } else {
                logical(*op, &a, &b)
            }
        }
        ColExpr::Not { expr } => match eval_expr(cols, row, expr, env)? {
            Cell::Bool(v) => Ok(Cell::Bool(!v)),
            Cell::Null => Ok(NULL),
            _ => Err(EvalError::TypeError {
                detail: "not of a non-bool".to_string(),
            }),
        },
        ColExpr::Coalesce { exprs } => {
            for x in exprs {
                let v = eval_expr(cols, row, x, env)?;
                if !is_null(&v) {
                    return Ok(v);
                }
            }
            Ok(NULL)
        }
        ColExpr::Case { cases, else_expr } => {
            for br in cases {
                let w = eval_expr(cols, row, &br.when, env)?;
                if matches!(w, Cell::Bool(true)) {
                    return eval_expr(cols, row, &br.then, env);
                }
            }
            eval_expr(cols, row, else_expr, env)
        }
        ColExpr::Cast { column_type, expr } => {
            let inner = eval_expr(cols, row, expr, env)?;
            cast_cell(*column_type, &inner)
        }
        ColExpr::Apply { func, args } => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval_expr(cols, row, a, env)?);
            }
            apply_scalar(*func, &vals)
        }
    }
}

// ─── type inference for derived/melted columns ───────────────────────────────

fn cell_type_of(c: &Cell) -> Option<ColumnType> {
    match c {
        Cell::Int(_) => Some(ColumnType::Int),
        Cell::Float(_) => Some(ColumnType::Float),
        Cell::Bool(_) => Some(ColumnType::Bool),
        Cell::Str(_) => Some(ColumnType::Str),
        Cell::Date(_) => Some(ColumnType::Date),
        Cell::Timestamp(_) => Some(ColumnType::Timestamp),
        Cell::Null => None,
    }
}

fn infer_type(cells: &[Cell]) -> ColumnType {
    cells
        .iter()
        .find_map(cell_type_of)
        .unwrap_or(ColumnType::Str)
}

// ─── aggregates (pinned) ─────────────────────────────────────────────────────

fn present_nums(cells: &[Cell]) -> Vec<f64> {
    cells.iter().filter_map(as_num).collect()
}

fn agg_cells(func: AggFn, src_type: ColumnType, cells: &[Cell]) -> Cell {
    let present: Vec<&Cell> = cells.iter().filter(|c| !is_null(c)).collect();
    match func {
        AggFn::Count => Cell::Int(present.len() as i64),
        AggFn::First => cells.first().cloned().unwrap_or(NULL),
        AggFn::Last => cells.last().cloned().unwrap_or(NULL),
        AggFn::Sum => {
            let nums = present_nums(cells);
            if nums.is_empty() {
                NULL
            } else if src_type == ColumnType::Int {
                Cell::Int(nums.iter().map(|x| x.trunc() as i64).sum())
            } else {
                Cell::Float(nums.iter().sum())
            }
        }
        AggFn::Mean => {
            let nums = present_nums(cells);
            if nums.is_empty() {
                NULL
            } else {
                Cell::Float(nums.iter().sum::<f64>() / nums.len() as f64)
            }
        }
        AggFn::Stddev => {
            let nums = present_nums(cells);
            if nums.is_empty() {
                NULL
            } else {
                let n = nums.len() as f64;
                let mean = nums.iter().sum::<f64>() / n;
                let variance = nums.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n;
                Cell::Float(variance.sqrt())
            }
        }
        AggFn::Median => {
            let mut nums = present_nums(cells);
            if nums.is_empty() {
                return NULL;
            }
            nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
            let n = nums.len();
            let mid = n / 2;
            if n % 2 == 1 {
                Cell::Float(nums[mid])
            } else {
                Cell::Float((nums[mid - 1] + nums[mid]) / 2.0)
            }
        }
        AggFn::Min | AggFn::Max => {
            if present.is_empty() {
                return NULL;
            }
            let mut acc = present[0].clone();
            for b in &present[1..] {
                if let Some(ord) = compare_cells(&acc, b) {
                    let c_le = ord != Ordering::Greater;
                    let keep_acc = (func == AggFn::Min) == c_le;
                    if !keep_acc {
                        acc = (*b).clone();
                    }
                }
                // incomparable ⇒ keep acc (matches the reference `None -> a`)
            }
            acc
        }
    }
}

fn agg_type(func: AggFn, src_type: ColumnType) -> ColumnType {
    match func {
        AggFn::Count => ColumnType::Int,
        AggFn::Mean | AggFn::Median | AggFn::Stddev => ColumnType::Float,
        AggFn::Sum | AggFn::Min | AggFn::Max | AggFn::First | AggFn::Last => src_type,
    }
}

// ─── sort (pinned: stable; nulls last regardless of direction) ───────────────

fn row_key_compare(cols: &[SchemaEntry], by: &[SortKey], r1: &[Cell], r2: &[Cell]) -> Ordering {
    for sk in by {
        let Some(i) = col_index(cols, &sk.col) else {
            continue;
        };
        let (a, b) = (&r1[i], &r2[i]);
        let (an, bn) = (is_null(a), is_null(b));
        let c = if an && bn {
            Ordering::Equal
        } else if an {
            Ordering::Greater // null sorts last
        } else if bn {
            Ordering::Less
        } else {
            match compare_cells(a, b) {
                None => Ordering::Equal,
                Some(cc) => {
                    if sk.dir == SortDir::Asc {
                        cc
                    } else {
                        cc.reverse()
                    }
                }
            }
        };
        if c != Ordering::Equal {
            return c;
        }
    }
    Ordering::Equal
}

// ─── grouping keys (structural, type-aware) ──────────────────────────────────

fn cell_key(c: &Cell) -> String {
    match c {
        Cell::Null => "N".to_string(),
        Cell::Int(v) => format!("I{v}"),
        Cell::Float(v) => format!("F{}", format_number(*v)),
        Cell::Bool(b) => format!("B{}", if *b { "1" } else { "0" }),
        Cell::Str(s) => format!("S{s}"),
        Cell::Date(s) => format!("D{s}"),
        Cell::Timestamp(s) => format!("T{s}"),
    }
}

fn row_key(cells: &[Cell]) -> String {
    cells.iter().map(cell_key).collect()
}

// ─── per-verb evaluation ─────────────────────────────────────────────────────

fn eval_filter(f: &Frame, pred: &ColExpr, env: &EvalEnv) -> Result<Frame, EvalError> {
    let mut rows = Vec::new();
    for row in &f.rows {
        if matches!(eval_expr(&f.cols, row, pred, env)?, Cell::Bool(true)) {
            rows.push(row.clone());
        }
    }
    Ok(Frame {
        cols: f.cols.clone(),
        rows,
    })
}

fn eval_project(f: &Frame, pairs: &[(String, String)]) -> Result<Frame, EvalError> {
    let mut resolved: Vec<(String, ColumnType, usize)> = Vec::new();
    for (a, b) in pairs {
        let i = col_index(&f.cols, a).ok_or_else(|| unknown_column(a, &f.cols))?;
        resolved.push((b.clone(), f.cols[i].column_type, i));
    }
    Ok(Frame {
        cols: resolved
            .iter()
            .map(|(out, ty, _)| SchemaEntry {
                name: out.clone(),
                column_type: *ty,
            })
            .collect(),
        rows: f
            .rows
            .iter()
            .map(|row| resolved.iter().map(|(_, _, i)| row[*i].clone()).collect())
            .collect(),
    })
}

fn eval_derive(f: &Frame, name: &str, expr: &ColExpr, env: &EvalEnv) -> Result<Frame, EvalError> {
    let mut new_cells = Vec::with_capacity(f.rows.len());
    for row in &f.rows {
        new_cells.push(eval_expr(&f.cols, row, expr, env)?);
    }
    let ty = infer_type(&new_cells);
    if let Some(i) = col_index(&f.cols, name) {
        let cols = f
            .cols
            .iter()
            .enumerate()
            .map(|(j, e)| {
                if j == i {
                    SchemaEntry {
                        name: e.name.clone(),
                        column_type: ty,
                    }
                } else {
                    e.clone()
                }
            })
            .collect();
        let rows = f
            .rows
            .iter()
            .enumerate()
            .map(|(ri, row)| {
                row.iter()
                    .enumerate()
                    .map(|(j, cell)| {
                        if j == i {
                            new_cells[ri].clone()
                        } else {
                            cell.clone()
                        }
                    })
                    .collect()
            })
            .collect();
        Ok(Frame { cols, rows })
    } else {
        let mut cols = f.cols.clone();
        cols.push(SchemaEntry {
            name: name.to_string(),
            column_type: ty,
        });
        let rows = f
            .rows
            .iter()
            .enumerate()
            .map(|(ri, row)| {
                let mut r = row.clone();
                r.push(new_cells[ri].clone());
                r
            })
            .collect();
        Ok(Frame { cols, rows })
    }
}

fn eval_group_by(f: &Frame, keys: &[String], aggs: &[Agg]) -> Result<Frame, EvalError> {
    for k in keys {
        if col_index(&f.cols, k).is_none() {
            return Err(unknown_column(k, &f.cols));
        }
    }
    let idxs: Vec<usize> = keys.iter().filter_map(|k| col_index(&f.cols, k)).collect();

    // First-appearance group order.
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, (Vec<Cell>, Vec<Vec<Cell>>)> = BTreeMap::new();
    for row in &f.rows {
        let key: Vec<Cell> = idxs.iter().map(|i| row[*i].clone()).collect();
        let ks = row_key(&key);
        match groups.get_mut(&ks) {
            Some(g) => g.1.push(row.clone()),
            None => {
                order.push(ks.clone());
                groups.insert(ks, (key, vec![row.clone()]));
            }
        }
    }

    // Resolve aggregate source columns.
    let mut resolved: Vec<(&Agg, ColumnType, usize)> = Vec::new();
    for a in aggs {
        let ty = col_type(&f.cols, &a.of).ok_or_else(|| unknown_column(&a.of, &f.cols))?;
        resolved.push((a, ty, col_index(&f.cols, &a.of).unwrap()));
    }

    let mut out_cols: Vec<SchemaEntry> = keys
        .iter()
        .map(|k| SchemaEntry {
            name: k.clone(),
            column_type: col_type(&f.cols, k).unwrap(),
        })
        .collect();
    out_cols.extend(resolved.iter().map(|(a, ty, _)| SchemaEntry {
        name: a.name.clone(),
        column_type: agg_type(a.func, *ty),
    }));

    let rows = order
        .iter()
        .map(|ks| {
            let (key, grp_rows) = &groups[ks];
            let mut r = key.clone();
            for (a, ty, idx) in &resolved {
                let col: Vec<Cell> = grp_rows.iter().map(|row| row[*idx].clone()).collect();
                r.push(agg_cells(a.func, *ty, &col));
            }
            r
        })
        .collect();

    Ok(Frame {
        cols: out_cols,
        rows,
    })
}

fn eval_sort(f: &Frame, by: &[SortKey]) -> Frame {
    let mut rows = f.rows.clone();
    rows.sort_by(|a, b| row_key_compare(&f.cols, by, a, b));
    Frame {
        cols: f.cols.clone(),
        rows,
    }
}

fn eval_distinct(f: &Frame) -> Frame {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut rows = Vec::new();
    for row in &f.rows {
        let k = row_key(row);
        if seen.insert(k) {
            rows.push(row.clone());
        }
    }
    Frame {
        cols: f.cols.clone(),
        rows,
    }
}

fn eval_limit(f: &Frame, n: i64, offset: i64) -> Frame {
    let len = f.rows.len();
    let start = (offset.max(0) as usize).min(len);
    let take = n.max(0) as usize;
    let end = start.saturating_add(take).min(len);
    Frame {
        cols: f.cols.clone(),
        rows: f.rows[start..end].to_vec(),
    }
}

fn eval_join(
    f: &Frame,
    right: &Frame,
    on: &[(String, String)],
    how: JoinKind,
) -> Result<Frame, EvalError> {
    let mut li = Vec::new();
    let mut ri = Vec::new();
    for (a, b) in on {
        let il = col_index(&f.cols, a).ok_or_else(|| EvalError::UnknownColumn {
            name: a.clone(),
            available: [available(&f.cols), available(&right.cols)].concat(),
        })?;
        let ir = col_index(&right.cols, b).ok_or_else(|| EvalError::UnknownColumn {
            name: b.clone(),
            available: [available(&f.cols), available(&right.cols)].concat(),
        })?;
        li.push(il);
        ri.push(ir);
    }
    let key_match =
        |lr: &[Cell], rr: &[Cell]| li.iter().zip(&ri).all(|(l, r)| cell_eq(&lr[*l], &rr[*r]));

    let left_names: BTreeSet<String> = available(&f.cols).into_iter().collect();
    let right_cols: Vec<SchemaEntry> = right
        .cols
        .iter()
        .map(|e| SchemaEntry {
            name: if left_names.contains(&e.name) {
                format!("{}_right", e.name)
            } else {
                e.name.clone()
            },
            column_type: e.column_type,
        })
        .collect();
    let out_cols = [f.cols.clone(), right_cols].concat();
    let left_nulls: Vec<Cell> = f.cols.iter().map(|_| NULL).collect();
    let right_nulls: Vec<Cell> = right.cols.iter().map(|_| NULL).collect();

    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for lr in &f.rows {
        let matches: Vec<&Vec<Cell>> = right.rows.iter().filter(|rr| key_match(lr, rr)).collect();
        if matches.is_empty() {
            if how == JoinKind::Left || how == JoinKind::Outer {
                rows.push([lr.clone(), right_nulls.clone()].concat());
            }
        } else {
            for rr in matches {
                rows.push([lr.clone(), rr.clone()].concat());
            }
        }
    }
    if how == JoinKind::Right || how == JoinKind::Outer {
        for rr in &right.rows {
            if !f.rows.iter().any(|lr| key_match(lr, rr)) {
                rows.push([left_nulls.clone(), rr.clone()].concat());
            }
        }
    }

    Ok(Frame {
        cols: out_cols,
        rows,
    })
}

fn eval_union(f: &Frame, other: &Frame) -> Result<Frame, EvalError> {
    let an = available(&f.cols);
    let bn = available(&other.cols);
    if an != bn {
        return Err(EvalError::JoinError {
            detail: "union requires matching column names".to_string(),
        });
    }
    Ok(Frame {
        cols: f.cols.clone(),
        rows: [f.rows.clone(), other.rows.clone()].concat(),
    })
}

#[allow(clippy::too_many_arguments)]
fn eval_window(
    f: &Frame,
    partition_by: &[String],
    order_by: &[SortKey],
    func: WindowFn,
    of: &str,
    alias: &str,
) -> Result<Frame, EvalError> {
    let of_idx = col_index(&f.cols, of);
    if of_idx.is_none() && func != WindowFn::RowNumber && func != WindowFn::Rank {
        return Err(unknown_column(of, &f.cols));
    }
    let part_idx: Vec<usize> = partition_by
        .iter()
        .filter_map(|p| col_index(&f.cols, p))
        .collect();
    let part_key = |row: &[Cell]| -> String {
        row_key(&part_idx.iter().map(|i| row[*i].clone()).collect::<Vec<_>>())
    };

    // Partition in first-appearance order, tagging each row with its index.
    let mut order: Vec<String> = Vec::new();
    let mut partitions: BTreeMap<String, Vec<(usize, Vec<Cell>)>> = BTreeMap::new();
    for (i, row) in f.rows.iter().enumerate() {
        let k = part_key(row);
        match partitions.get_mut(&k) {
            Some(p) => p.push((i, row.clone())),
            None => {
                order.push(k.clone());
                partitions.insert(k, vec![(i, row.clone())]);
            }
        }
    }

    let value_at = |row: &[Cell]| -> Cell { of_idx.map(|i| row[i].clone()).unwrap_or(NULL) };

    let mut computed: Vec<(usize, Cell)> = Vec::new();
    for k in &order {
        let members = &partitions[k];
        let mut ordered = members.clone();
        ordered.sort_by(|a, b| row_key_compare(&f.cols, order_by, &a.1, &b.1));
        let vals: Vec<Cell> = ordered.iter().map(|m| value_at(&m.1)).collect();
        let outs: Vec<Cell> = match func {
            WindowFn::RowNumber => (0..ordered.len())
                .map(|i| Cell::Int(i as i64 + 1))
                .collect(),
            WindowFn::Rank => {
                let mut acc: i64 = 0;
                ordered
                    .iter()
                    .enumerate()
                    .map(|(i, m)| {
                        let step = if i == 0 {
                            1
                        } else if row_key_compare(&f.cols, order_by, &ordered[i - 1].1, &m.1)
                            == Ordering::Equal
                        {
                            0
                        } else {
                            1
                        };
                        acc += step;
                        Cell::Int(acc)
                    })
                    .collect()
            }
            WindowFn::Lag => {
                let mut o = vec![NULL];
                let take = vals.len().saturating_sub(1);
                o.extend(vals[..take].iter().cloned());
                o
            }
            WindowFn::Lead => {
                let skip = 1.min(vals.len());
                let mut o: Vec<Cell> = vals[skip..].to_vec();
                o.push(NULL);
                o
            }
            WindowFn::CumSum => {
                let mut acc = 0.0;
                vals.iter()
                    .map(|v| {
                        if let Some(n) = as_num(v) {
                            acc += n;
                        }
                        Cell::Float(acc)
                    })
                    .collect()
            }
            WindowFn::RollingMean => (0..vals.len())
                .map(|i| {
                    let lo = i.saturating_sub(2);
                    let window = &vals[lo..=i];
                    let nums = present_nums(window);
                    if nums.is_empty() {
                        NULL
                    } else {
                        Cell::Float(nums.iter().sum::<f64>() / nums.len() as f64)
                    }
                })
                .collect(),
        };
        for (m, out) in ordered.iter().zip(outs) {
            computed.push((m.0, out));
        }
    }

    computed.sort_by_key(|c| c.0);
    let out_cells: Vec<Cell> = computed.into_iter().map(|c| c.1).collect();

    let ty = match func {
        WindowFn::RowNumber | WindowFn::Rank => ColumnType::Int,
        WindowFn::CumSum | WindowFn::RollingMean => ColumnType::Float,
        WindowFn::Lag | WindowFn::Lead => col_type(&f.cols, of).unwrap_or(ColumnType::Str),
    };

    let mut cols = f.cols.clone();
    cols.push(SchemaEntry {
        name: alias.to_string(),
        column_type: ty,
    });
    let rows = f
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.push(out_cells[i].clone());
            r
        })
        .collect();
    Ok(Frame { cols, rows })
}

fn eval_pivot(
    f: &Frame,
    index: &[String],
    on: &str,
    values: &str,
    agg: AggFn,
) -> Result<Frame, EvalError> {
    let mut idx_idx = Vec::new();
    for n in index {
        idx_idx.push(col_index(&f.cols, n).ok_or_else(|| unknown_column(n, &f.cols))?);
    }
    let on_idx = col_index(&f.cols, on).ok_or_else(|| unknown_column(on, &f.cols))?;
    let val_idx = col_index(&f.cols, values).ok_or_else(|| unknown_column(values, &f.cols))?;
    let val_type = f.cols[val_idx].column_type;

    let index_key =
        |row: &[Cell]| -> Vec<Cell> { idx_idx.iter().map(|i| row[*i].clone()).collect() };

    // Distinct on-values, sorted by canonical string for a deterministic order.
    let mut on_seen: BTreeSet<String> = BTreeSet::new();
    let mut on_values: Vec<Cell> = Vec::new();
    for row in &f.rows {
        let c = &row[on_idx];
        if is_null(c) {
            continue;
        }
        if on_seen.insert(cell_key(c)) {
            on_values.push(c.clone());
        }
    }
    on_values.sort_by_key(cell_string);

    // Index groups, first-appearance order.
    let mut order: Vec<Vec<Cell>> = Vec::new();
    let mut order_seen: BTreeSet<String> = BTreeSet::new();
    for row in &f.rows {
        let key = index_key(row);
        if order_seen.insert(row_key(&key)) {
            order.push(key);
        }
    }

    let mut cols: Vec<SchemaEntry> = index
        .iter()
        .map(|n| SchemaEntry {
            name: n.clone(),
            column_type: col_type(&f.cols, n).unwrap(),
        })
        .collect();
    cols.extend(on_values.iter().map(|ov| SchemaEntry {
        name: cell_string(ov),
        column_type: agg_type(agg, val_type),
    }));

    let rows = order
        .iter()
        .map(|key| {
            let ks = row_key(key);
            let mut r = key.clone();
            for ov in &on_values {
                let matching: Vec<Cell> = f
                    .rows
                    .iter()
                    .filter(|row| row_key(&index_key(row)) == ks && cell_eq(&row[on_idx], ov))
                    .map(|row| row[val_idx].clone())
                    .collect();
                r.push(agg_cells(agg, val_type, &matching));
            }
            r
        })
        .collect();

    Ok(Frame { cols, rows })
}

fn eval_unpivot(f: &Frame, id_vars: &[String], value_vars: &[String]) -> Result<Frame, EvalError> {
    let mut id_idx = Vec::new();
    for n in id_vars {
        id_idx.push(col_index(&f.cols, n).ok_or_else(|| unknown_column(n, &f.cols))?);
    }
    let mut val_idx = Vec::new();
    for n in value_vars {
        val_idx.push(col_index(&f.cols, n).ok_or_else(|| unknown_column(n, &f.cols))?);
    }
    let val_type = value_vars
        .iter()
        .find_map(|n| col_type(&f.cols, n))
        .unwrap_or(ColumnType::Str);

    let mut cols: Vec<SchemaEntry> = id_vars
        .iter()
        .map(|n| SchemaEntry {
            name: n.clone(),
            column_type: col_type(&f.cols, n).unwrap(),
        })
        .collect();
    cols.push(SchemaEntry {
        name: "variable".to_string(),
        column_type: ColumnType::Str,
    });
    cols.push(SchemaEntry {
        name: "value".to_string(),
        column_type: val_type,
    });

    let mut rows = Vec::new();
    for row in &f.rows {
        let id_cells: Vec<Cell> = id_idx.iter().map(|i| row[*i].clone()).collect();
        for (k, name) in value_vars.iter().enumerate() {
            let mut r = id_cells.clone();
            r.push(Cell::Str(name.clone()));
            r.push(row[val_idx[k]].clone());
            rows.push(r);
        }
    }
    Ok(Frame { cols, rows })
}

// ─── pipeline driver ─────────────────────────────────────────────────────────

/// Evaluate a `DataSource` to a concrete [`Table`], resolving a `Ref` via `resolve`.
pub fn eval_source(resolve: &Resolver, src: &DataSource) -> Result<Table, EvalError> {
    match src {
        DataSource::Embedded { schema, columns } => Ok(Table {
            schema: schema.clone(),
            columns: columns.clone(),
        }),
        DataSource::Ref { name } => resolve(name),
    }
}

fn eval_step(
    resolve: &Resolver,
    f: &Frame,
    step: &TransformStep,
    env: &EvalEnv,
) -> Result<Frame, EvalError> {
    match step {
        TransformStep::Filter { pred } => eval_filter(f, pred, env),
        TransformStep::Project { cols } => {
            let pairs: Vec<(String, String)> =
                cols.iter().map(|p| (p.a.clone(), p.b.clone())).collect();
            eval_project(f, &pairs)
        }
        TransformStep::Derive { name, expr } => eval_derive(f, name, expr, env),
        TransformStep::GroupBy { keys, aggs } => eval_group_by(f, keys, aggs),
        TransformStep::Sort { by } => Ok(eval_sort(f, by)),
        TransformStep::Distinct => Ok(eval_distinct(f)),
        TransformStep::Limit { n, offset } => Ok(eval_limit(f, *n, *offset)),
        TransformStep::Window {
            partition_by,
            order_by,
            func,
            of,
            alias,
        } => eval_window(f, partition_by, order_by, *func, of, alias),
        TransformStep::Pivot {
            index,
            on,
            values,
            agg,
        } => eval_pivot(f, index, on, values, *agg),
        TransformStep::Unpivot {
            id_vars,
            value_vars,
        } => eval_unpivot(f, id_vars, value_vars),
        TransformStep::Join { source, on, how } => {
            let right = eval_source(resolve, source)?;
            let pairs: Vec<(String, String)> =
                on.iter().map(|p| (p.a.clone(), p.b.clone())).collect();
            eval_join(f, &right.to_frame(), &pairs, *how)
        }
        TransformStep::Union { source } => {
            let other = eval_source(resolve, source)?;
            eval_union(f, &other.to_frame())
        }
    }
}

/// The reference evaluator: fold the pipeline over the input table, threading a
/// working frame. `resolve` provides any `Ref` a `Join`/`Union` names; `env`
/// binds `ColExpr::Param` names.
pub fn eval_pipeline_with_in_env(
    resolve: &Resolver,
    env: &EvalEnv,
    pipeline: &[TransformStep],
    input: &Table,
) -> Result<Table, EvalError> {
    let mut f = input.to_frame();
    for step in pipeline {
        f = eval_step(resolve, &f, step, env)?;
    }
    Ok(of_frame(&f))
}

/// Evaluate a pipeline with a source resolver, no params.
pub fn eval_pipeline_with(
    resolve: &Resolver,
    pipeline: &[TransformStep],
    input: &Table,
) -> Result<Table, EvalError> {
    eval_pipeline_with_in_env(resolve, &EvalEnv::new(), pipeline, input)
}

/// The reference evaluator over embedded sources only (`Ref` ⇒ `UnresolvedSource`).
pub fn eval_pipeline(pipeline: &[TransformStep], input: &Table) -> Result<Table, EvalError> {
    eval_pipeline_with(&no_resolve, pipeline, input)
}

/// The env-aware evaluator over embedded sources only.
pub fn eval_pipeline_in_env(
    env: &EvalEnv,
    pipeline: &[TransformStep],
    input: &Table,
) -> Result<Table, EvalError> {
    eval_pipeline_with_in_env(&no_resolve, env, pipeline, input)
}

/// Evaluate a whole `Binding::Transform` — resolve its source, fold its pipeline
/// — the wire-facing entry point the Living Sheet / Pattern Bank exercise.
pub fn eval_transform(
    resolve: &Resolver,
    env: &EvalEnv,
    source: &DataSource,
    pipeline: &[TransformStep],
) -> Result<Table, EvalError> {
    let input = eval_source(resolve, source)?;
    eval_pipeline_with_in_env(resolve, env, pipeline, &input)
}

/// Render an [`EvalError`] as a stable human string (mirrors the reference).
pub fn eval_error_string(e: &EvalError) -> String {
    match e {
        EvalError::UnknownColumn { name, available } => {
            format!(
                "unknown column '{name}'; available: {}",
                available.join(", ")
            )
        }
        EvalError::TypeError { detail } => format!("type error: {detail}"),
        EvalError::AggError { detail } => format!("aggregate error: {detail}"),
        EvalError::JoinError { detail } => format!("join error: {detail}"),
        EvalError::ArityError {
            func,
            expected,
            got,
        } => {
            format!("function '{func}' expects {expected} args, got {got}")
        }
        EvalError::UnresolvedSource { source_ref } => {
            format!("unresolved source ref: {source_ref}")
        }
        EvalError::UnboundParam { name, bound } => {
            format!("unbound param '{name}'; bound: {}", bound.join(", "))
        }
    }
}

// ─── param derivation ────────────────────────────────────────────────────────

fn col_expr_params(e: &ColExpr) -> Vec<String> {
    match e {
        ColExpr::Col { .. } | ColExpr::Lit { .. } => Vec::new(),
        ColExpr::Param { name } => vec![name.clone()],
        ColExpr::Binary { left, right, .. } => {
            [col_expr_params(left), col_expr_params(right)].concat()
        }
        ColExpr::Not { expr } | ColExpr::Cast { expr, .. } => col_expr_params(expr),
        ColExpr::Coalesce { exprs } => exprs.iter().flat_map(col_expr_params).collect(),
        ColExpr::Case { cases, else_expr } => {
            let mut out: Vec<String> = cases
                .iter()
                .flat_map(|br| [col_expr_params(&br.when), col_expr_params(&br.then)].concat())
                .collect();
            out.extend(col_expr_params(else_expr));
            out
        }
        ColExpr::Apply { args, .. } => args.iter().flat_map(col_expr_params).collect(),
    }
}

/// Every `ColExpr::Param` name a single step references (only `Filter`/`Derive`
/// carry a `ColExpr`).
pub fn step_params(t: &TransformStep) -> Vec<String> {
    match t {
        TransformStep::Filter { pred } => col_expr_params(pred),
        TransformStep::Derive { expr, .. } => col_expr_params(expr),
        _ => Vec::new(),
    }
}

/// Every distinct param name a pipeline references (insertion order preserved).
pub fn pipeline_params(pipeline: &[TransformStep]) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for t in pipeline {
        for p in step_params(t) {
            if seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    out
}
