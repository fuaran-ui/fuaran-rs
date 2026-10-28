//! The Transform evaluator and list-param substitution — the Core twin of the
//! reference DataFrame evaluator. Published, and documented, as
//! `crate::transform`, which re-exports this module whole.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use super::dataframe::{
    Agg, AggFn, BinOp, CaseArm, Cell, ColExpr, ColumnType, DataColumn, DataSource, JoinKind,
    Rounding, RoundingMode, RoundingScale, ScalarFn, SchemaEntry, SortDir, SortKey, TransformStep,
    WindowFn,
};
use super::number::format_number;

/// The exact decimal's text and arithmetic (Phase 2001), published beside the
/// evaluator that computes with it as `crate::transform::decimal`.
pub use super::decimal;

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

/// The canonical string of a cell (float via the shared canonical layout, a
/// decimal as its canonical text).
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
        Cell::Decimal(s) => decimal::try_canonical(s).unwrap_or_else(|| s.clone()),
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

/// A cell as exact-decimal text — a decimal's, or an int's digits (the lossless
/// promotion) — or `None` for any other cell.
fn decimal_text(c: &Cell) -> Option<String> {
    match c {
        Cell::Decimal(s) => Some(s.clone()),
        Cell::Int(v) => Some(v.to_string()),
        _ => None,
    }
}

/// The float order every numeric comparison uses: IEEE order on the non-NaN
/// values, `-0 = 0`, and NaN one value ABOVE every other — never the host's
/// partial order, which leaves NaN unordered.
fn compare_float(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
    }
}

/// Total comparison of two present cells; `None` ⇒ incomparable families. A
/// decimal compares EXACTLY against a decimal or an int; against a float it is
/// incomparable — nothing converts between the two unless a `Cast` says which way.
fn compare_cells(a: &Cell, b: &Cell) -> Option<Ordering> {
    match (a, b) {
        (Cell::Int(_) | Cell::Float(_), Cell::Int(_) | Cell::Float(_)) => {
            Some(compare_float(as_num(a)?, as_num(b)?))
        }
        (Cell::Decimal(_) | Cell::Int(_), Cell::Decimal(_) | Cell::Int(_)) => {
            decimal::compare(&decimal_text(a)?, &decimal_text(b)?)
        }
        (Cell::Bool(x), Cell::Bool(y)) => Some(x.cmp(y)),
        (Cell::Str(x), Cell::Str(y)) => Some(x.cmp(y)),
        (Cell::Date(x), Cell::Date(y)) => Some(x.cmp(y)),
        (Cell::Timestamp(x), Cell::Timestamp(y)) => Some(instant_key(x).cmp(&instant_key(y))),
        _ => None,
    }
}

/// Canonical instant text keyed so its order is the instants' (WIRE_FORMAT §3.6.25). The text
/// drops an all-zero fraction and every trailing zero, so `…:00Z` and `…:00.5Z` do not order by
/// their bytes (`.` sorts below `Z`); the key pads the fraction to nanoseconds. Text that is not
/// canonical keys as itself.
fn instant_key(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() < 20 || b[b.len() - 1] != b'Z' || !s.is_char_boundary(19) {
        return s.to_string();
    }
    let fraction = if b[19] == b'.' {
        &s[20..s.len() - 1]
    } else if b.len() == 20 {
        ""
    } else {
        return s.to_string();
    };
    if fraction.len() > 9 {
        return s.to_string();
    }
    format!("{}{:0<9}", &s[..19], fraction)
}

#[cfg(test)]
mod instant_order_tests {
    use super::*;

    /// Phase 2206: timestamp cells order by the instant, not by their canonical text.
    #[test]
    fn timestamp_cells_order_by_instant() {
        let ts = |s: &str| Cell::Timestamp(s.to_string());
        let cases = [
            (
                "2025-07-08T18:40:00Z",
                "2025-07-08T18:40:00.5Z",
                Ordering::Less,
            ),
            (
                "2025-07-08T18:40:00.5Z",
                "2025-07-08T18:40:00Z",
                Ordering::Greater,
            ),
            (
                "2025-07-08T18:40:00.12Z",
                "2025-07-08T18:40:00.123456789Z",
                Ordering::Less,
            ),
            (
                "2025-07-08T18:40:00.9Z",
                "2025-07-08T18:40:01Z",
                Ordering::Less,
            ),
            (
                "2025-07-08T18:40:00.5Z",
                "2025-07-08T18:40:00.5Z",
                Ordering::Equal,
            ),
        ];
        for (a, b, want) in cases {
            assert_eq!(compare_cells(&ts(a), &ts(b)), Some(want), "{a} vs {b}");
        }
    }
}

/// The join-key / pivot on-value match: the numeric family as floats (`Int 1`
/// matches `Float 1.0`), a decimal only against a decimal, and a null nothing.
fn cell_eq(a: &Cell, b: &Cell) -> bool {
    match (a, b) {
        (Cell::Null, _) | (_, Cell::Null) => false,
        (Cell::Decimal(_), Cell::Decimal(_)) => compare_cells(a, b) == Some(Ordering::Equal),
        (Cell::Decimal(_), _) | (_, Cell::Decimal(_)) => false,
        _ => compare_cells(a, b) == Some(Ordering::Equal),
    }
}

fn both_int(a: &Cell, b: &Cell) -> bool {
    matches!(a, Cell::Int(_)) && matches!(b, Cell::Int(_))
}

/// The refusal a decimal beside a float meets, naming the `Cast` that resolves it.
fn decimal_float_mismatch(what: &str) -> EvalError {
    EvalError::TypeError {
        detail: format!(
            "{what} between a decimal and a float: cast one to the other's type first — \
             Cast(decimal, …) to keep the digits exact, Cast(float, …) to compute approximately"
        ),
    }
}

/// A decimal answer, or the refusal for an operand that is not decimal text.
fn decimal_result(ctx: &str, r: Option<String>) -> Result<Cell, EvalError> {
    r.map(Cell::Decimal).ok_or_else(|| EvalError::TypeError {
        detail: format!("{ctx}: an operand is not decimal text"),
    })
}

/// `Add` / `Sub` / `Mul` / `Mod` over two exact operands, at least one a decimal:
/// exact, and a decimal. `Div` is refused by name — a quotient of decimals is not
/// a finite decimal in general, so the pipeline names its rounding.
fn decimal_arith(op: BinOp, x: &str, y: &str) -> Result<Cell, EvalError> {
    match op {
        BinOp::Add => decimal_result("add", decimal::add(x, y)),
        BinOp::Sub => decimal_result("sub", decimal::sub(x, y)),
        BinOp::Mul => decimal_result("mul", decimal::mul(x, y)),
        BinOp::Mod => {
            if decimal::is_zero(y) {
                Ok(NULL)
            } else {
                decimal_result("mod", decimal::rem(x, y))
            }
        }
        BinOp::Div => Err(EvalError::TypeError {
            detail: "division of an exact decimal names its rounding: write Quotient(dividend, divisor, { Scale; Mode })"
                .to_string(),
        }),
        _ => Err(EvalError::TypeError {
            detail: "not an arithmetic operator".to_string(),
        }),
    }
}

fn arith(op: BinOp, a: &Cell, b: &Cell) -> Result<Cell, EvalError> {
    match (a, b) {
        (Cell::Null, _) | (_, Cell::Null) => return Ok(NULL),
        (Cell::Decimal(_), Cell::Decimal(_) | Cell::Int(_)) | (Cell::Int(_), Cell::Decimal(_)) => {
            return match (decimal_text(a), decimal_text(b)) {
                (Some(x), Some(y)) => decimal_arith(op, &x, &y),
                _ => Err(EvalError::TypeError {
                    detail: "arithmetic on a non-numeric operand".to_string(),
                }),
            };
        }
        (Cell::Decimal(_), Cell::Float(_)) | (Cell::Float(_), Cell::Decimal(_)) => {
            return Err(decimal_float_mismatch("arithmetic"));
        }
        _ => {}
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
    let c = match compare_cells(a, b) {
        Some(c) => c,
        None => {
            return Err(match (a, b) {
                (Cell::Decimal(_), Cell::Float(_)) | (Cell::Float(_), Cell::Decimal(_)) => {
                    decimal_float_mismatch("comparison")
                }
                _ => EvalError::TypeError {
                    detail: "comparison between incompatible types".to_string(),
                },
            });
        }
    };
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
            } else {
                match c {
                    // The nearest float to the decimal — the cast says the
                    // pipeline asked for the approximation. Past the float range
                    // it is refused, never an infinity.
                    Cell::Decimal(s) => {
                        decimal::try_to_float(s).map(Cell::Float).ok_or_else(|| {
                            EvalError::TypeError {
                                detail: format!(
                                    "cannot cast decimal '{s}' to float: outside the float range"
                                ),
                            }
                        })
                    }
                    Cell::Str(s) => {
                        try_parse_float(s)
                            .map(Cell::Float)
                            .ok_or(EvalError::TypeError {
                                detail: format!("cannot cast '{s}' to float"),
                            })
                    }
                    _ => Err(EvalError::TypeError {
                        detail: "cannot cast to float".to_string(),
                    }),
                }
            }
        }
        ColumnType::Int => match c {
            Cell::Int(_) => Ok(c.clone()),
            Cell::Float(v) => Ok(Cell::Int(v.trunc() as i64)),
            // Truncated toward zero, as a float is; outside int32 a named refusal.
            Cell::Decimal(s) => match decimal::truncate_to_i64(s) {
                Some(v) if v >= i64::from(i32::MIN) && v <= i64::from(i32::MAX) => Ok(Cell::Int(v)),
                _ => Err(EvalError::TypeError {
                    detail: format!("decimal→int cast out of int32 range: {s}"),
                }),
            },
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
        // Phase 2206 — a cast to any unit keeps the instant's one canonical
        // text; the unit is a statement about the column, not the cell.
        ColumnType::Timestamp
        | ColumnType::TimestampMs
        | ColumnType::TimestampUs
        | ColumnType::TimestampNs => match c {
            Cell::Timestamp(_) => Ok(c.clone()),
            Cell::Str(s) => Ok(Cell::Timestamp(s.clone())),
            _ => Err(EvalError::TypeError {
                detail: format!("cannot cast to {}", ty.as_str()),
            }),
        },
        // An int is exactly a decimal; a string is read by the decimal grammar; a
        // FLOAT is the one place an approximation enters a decimal — its shortest
        // round-trip digits, which are the float's.
        ColumnType::Decimal => {
            let refused = |s: &str| EvalError::TypeError {
                detail: format!("cannot cast '{s}' to decimal"),
            };
            match c {
                Cell::Decimal(s) | Cell::Str(s) => decimal::try_canonical(s)
                    .map(Cell::Decimal)
                    .ok_or_else(|| refused(s)),
                Cell::Int(v) => Ok(Cell::Decimal(v.to_string())),
                Cell::Float(f) => {
                    decimal::of_float(*f)
                        .map(Cell::Decimal)
                        .ok_or(EvalError::TypeError {
                            detail: "cannot cast a non-finite float to decimal".to_string(),
                        })
                }
                _ => Err(EvalError::TypeError {
                    detail: "cannot cast to decimal".to_string(),
                }),
            }
        }
    }
}

/// A rounding's scale against the evaluation env: a literal is itself; a param
/// reads the env — unbound is `UnboundParam`, a cell that is not an int a
/// `TypeError` naming the rounding scale. The range is the primitive's check.
fn resolve_scale(env: &EvalEnv, s: &RoundingScale) -> Result<i64, EvalError> {
    match s {
        RoundingScale::Lit(n) => Ok(*n),
        RoundingScale::Param(name) => match env.get(name) {
            Some(Cell::Int(v)) => Ok(*v),
            Some(_) => Err(EvalError::TypeError {
                detail: format!("rounding scale: param '{name}' is not bound to an int"),
            }),
            None => Err(EvalError::UnboundParam {
                name: name.clone(),
                bound: env.keys().cloned().collect(),
            }),
        },
    }
}

fn check_scale(scale: i64) -> Result<(), EvalError> {
    if (0..=decimal::MAX_SCALE).contains(&scale) {
        Ok(())
    } else {
        Err(EvalError::TypeError {
            detail: format!(
                "rounding scale {scale} is outside 0..{}",
                decimal::MAX_SCALE
            ),
        })
    }
}

/// THE rounding primitive over cells: `Rounded`'s, and through it the decimal
/// arm of `Round` / `Floor` / `Ceil`. The scale is checked first, then a null
/// propagates, then an exact operand is brought to the scale; a float names the
/// `Cast`.
fn rounded_cell(mode: RoundingMode, scale: i64, c: &Cell) -> Result<Cell, EvalError> {
    check_scale(scale)?;
    match c {
        Cell::Null => Ok(NULL),
        Cell::Decimal(_) | Cell::Int(_) => decimal_result(
            "rounded",
            decimal_text(c).and_then(|t| decimal::quantize(mode, scale, &t)),
        ),
        Cell::Float(_) => Err(decimal_float_mismatch("rounding")),
        _ => Err(EvalError::TypeError {
            detail: "rounding of a non-numeric".to_string(),
        }),
    }
}

/// `Quotient`'s primitive: the scale first, then a null operand propagates, then
/// two exact operands divide, correctly rounded; a zero divisor answers null.
fn quotient_cell(mode: RoundingMode, scale: i64, a: &Cell, b: &Cell) -> Result<Cell, EvalError> {
    check_scale(scale)?;
    match (a, b) {
        (Cell::Null, _) | (_, Cell::Null) => Ok(NULL),
        (Cell::Decimal(_) | Cell::Int(_), Cell::Decimal(_) | Cell::Int(_)) => {
            let not_text = || EvalError::TypeError {
                detail: "quotient: an operand is not decimal text".to_string(),
            };
            let (x, y) = (
                decimal_text(a).ok_or_else(not_text)?,
                decimal_text(b).ok_or_else(not_text)?,
            );
            match decimal::divide(mode, scale, &x, &y) {
                Some(Some(q)) => Ok(Cell::Decimal(q)),
                Some(None) => Ok(NULL),
                None => Err(not_text()),
            }
        }
        (Cell::Float(_), _) | (_, Cell::Float(_)) => Err(decimal_float_mismatch("quotient")),
        _ => Err(EvalError::TypeError {
            detail: "quotient of a non-numeric".to_string(),
        }),
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

// ── Phase 2400: the power functions (the reference's D43) ───────────────────
//
// Two guarantees, stated per function. An INTEGER exponent is EXACT on every
// host: one pinned product tree over IEEE-754 multiplication, which every host
// rounds the same way, so the tree and not the host decides the bits. A float
// exponent, `exp`, `ln` and `log10` are HOST-ROUNDED: the special-value table
// below answers every input the hosts' libraries disagree on BEFORE the host is
// called, and what remains is this host's `f64` maths — within the bound the
// transform laws compare in (`expected.tolerance`, in ulps).

/// Binary exponentiation, the accumulator empty until the first set bit, so a
/// power takes exactly the multiplies its bits name: `x^1` is `x`, `x^2` is
/// `x*x`, `x^3` is `x*(x*x)`, `x^4` is `(x*x)*(x*x)`. `k > 0` is the exponent's
/// magnitude.
pub(crate) fn pow_go(mut b: f64, mut k: u64) -> f64 {
    let mut acc: Option<f64> = None;
    loop {
        if k % 2 == 1 {
            acc = Some(match acc {
                Some(a) => a * b,
                None => b,
            });
        }
        k /= 2;
        if k == 0 {
            return acc.unwrap_or(1.0);
        }
        b *= b;
    }
}

/// A float base under an integer exponent: `x^0` is 1 with no multiply; a
/// negative exponent is one reciprocal of the positive power, null where that
/// power is zero (a zero base, or a power that underflowed), exactly as a
/// division by zero is null. The caller has answered a NaN base.
fn pow_int_float(x: f64, n: i64) -> Cell {
    match n {
        0 => Cell::Float(1.0),
        n if n > 0 => Cell::Float(pow_go(x, n.unsigned_abs())),
        n => {
            let p = pow_go(x, n.unsigned_abs());
            if p == 0.0 { NULL } else { Cell::Float(1.0 / p) }
        }
    }
}

/// An int base under a non-negative int exponent: the same product tree over
/// `i64`, every product that leaves the int32 band refused by name — a base
/// square that leaves it with exponent bits still to take included, since every
/// value it would still multiply into is at least as large.
fn pow_int_exact(x: i64, n: i64) -> Result<Cell, EvalError> {
    if n == 0 {
        return Ok(Cell::Int(1));
    }
    let in_band = |v: i64| v >= i64::from(i32::MIN) && v <= i64::from(i32::MAX);
    let overflow = |v: i64| EvalError::TypeError {
        detail: format!("pow overflowed int32: {v}"),
    };
    let (mut acc, mut b, mut k): (Option<i64>, i64, i64) = (None, x, n);
    loop {
        if k % 2 == 1 {
            let a = match acc {
                Some(a) => a * b,
                None => b,
            };
            if !in_band(a) {
                return Err(overflow(a));
            }
            acc = Some(a);
        }
        k /= 2;
        if k == 0 {
            return Ok(Cell::Int(acc.unwrap_or(1)));
        }
        let b2 = b * b;
        if !in_band(b2) {
            return Err(overflow(b2));
        }
        b = b2;
    }
}

/// A float exponent: the special-value table, then the host. A NaN operand
/// propagates (`pow(1, NaN)` and `pow(NaN, 0)` included); a zero exponent and a
/// base of one answer 1, and so does a base of minus one under an infinite
/// exponent; a zero base under a negative exponent and a finite negative base
/// under a finite non-integer exponent are outside the reals and answer null.
pub(crate) fn pow_float(x: f64, y: f64) -> Cell {
    if x.is_nan() || y.is_nan() {
        Cell::Float(f64::NAN)
    } else if y == 0.0 || x == 1.0 {
        Cell::Float(1.0)
    } else if x == 0.0 && y < 0.0 {
        NULL
    } else if x == -1.0 && y.is_infinite() {
        Cell::Float(1.0)
    } else if x < 0.0 && !x.is_infinite() && !y.is_infinite() && y != y.floor() {
        NULL
    } else {
        Cell::Float(x.powf(y))
    }
}

/// `exp`'s table: a NaN propagates; above the natural log of the largest double
/// the answer is +Inf and below the natural log of half the smallest subnormal
/// it is 0 — the two edges where a host's last-ulp rounding decides between a
/// finite value and the limit; between them the host answers.
pub(crate) fn exp_host(x: f64) -> f64 {
    if x.is_nan() {
        x
    } else if x > 709.782_712_893_384 {
        f64::INFINITY
    } else if x < -745.133_219_101_941_2 {
        0.0
    } else {
        x.exp()
    }
}

/// `ln`'s and `log10`'s table over a host logarithm: a NaN propagates, zero and
/// below is null, one is exactly 0 and +Inf is +Inf; in between the host
/// answers.
pub(crate) fn log_host(log: fn(f64) -> f64, x: f64) -> Cell {
    if x.is_nan() {
        Cell::Float(x)
    } else if x <= 0.0 {
        NULL
    } else if x == 1.0 {
        Cell::Float(0.0)
    } else if x == f64::INFINITY {
        Cell::Float(x)
    } else {
        Cell::Float(log(x))
    }
}

fn apply_power(func: ScalarFn, args: &[Cell]) -> Result<Cell, EvalError> {
    let arity = |n: usize| -> Result<(), EvalError> {
        if args.len() == n {
            Ok(())
        } else {
            Err(EvalError::ArityError {
                func: func.as_str().to_string(),
                expected: n,
                got: args.len(),
            })
        }
    };
    let type_error = |detail: String| EvalError::TypeError { detail };
    if func == ScalarFn::Pow {
        arity(2)?;
        return match (&args[0], &args[1]) {
            (Cell::Null, _) | (_, Cell::Null) => Ok(NULL),
            (Cell::Decimal(_), _) | (_, Cell::Decimal(_)) => Err(type_error(
                "pow of a decimal is not exact: write Pow(Cast(float, x), n)".to_string(),
            )),
            (Cell::Int(b), Cell::Int(n)) if *n >= 0 => pow_int_exact(*b, *n),
            (Cell::Int(b), Cell::Int(n)) => Ok(pow_int_float(*b as f64, *n)),
            (x, Cell::Int(n)) => match as_num(x) {
                Some(f) if f.is_nan() => Ok(Cell::Float(f)),
                Some(f) => Ok(pow_int_float(f, *n)),
                None => Err(type_error("pow of a non-numeric".to_string())),
            },
            (x, y) => match (as_num(x), as_num(y)) {
                (Some(f), Some(g)) => Ok(pow_float(f, g)),
                _ => Err(type_error("pow of a non-numeric".to_string())),
            },
        };
    }
    arity(1)?;
    let name = func.as_str();
    match &args[0] {
        Cell::Null => Ok(NULL),
        Cell::Decimal(_) => {
            let spelled = match func {
                ScalarFn::Exp => "Exp",
                ScalarFn::Ln => "Ln",
                _ => "Log10",
            };
            Err(type_error(format!(
                "{name} of a decimal is not exact: write {spelled}(Cast(float, x))"
            )))
        }
        c => match (as_num(c), func) {
            (None, _) => Err(type_error(format!("{name} of a non-numeric"))),
            (Some(f), ScalarFn::Exp) => Ok(Cell::Float(exp_host(f))),
            (Some(f), ScalarFn::Ln) => Ok(log_host(f64::ln, f)),
            (Some(f), _) => Ok(log_host(f64::log10, f)),
        },
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
        ScalarFn::Pow | ScalarFn::Exp | ScalarFn::Ln | ScalarFn::Log10 => apply_power(func, args),
        ScalarFn::Abs => unary(&|c| match c {
            Cell::Int(v) => Ok(Cell::Int(v.abs())),
            Cell::Float(v) => Ok(Cell::Float(v.abs())),
            Cell::Decimal(s) => decimal_result("abs", decimal::abs(s)),
            _ => Err(EvalError::TypeError {
                detail: "abs of a non-numeric".to_string(),
            }),
        }),
        // Over a decimal, a scale-0 `Rounded` under its pinned mode — exact,
        // through the one rounding kernel; over the numeric family, unchanged.
        ScalarFn::Round => unary(&|c| match c {
            Cell::Decimal(_) => rounded_cell(RoundingMode::HalfUp, 0, c),
            _ => as_num(c)
                .map(|n| Cell::Float(round_half_away(n)))
                .ok_or(EvalError::TypeError {
                    detail: "round of a non-numeric".to_string(),
                }),
        }),
        // Over a decimal, a scale-0 `Rounded` under its pinned mode — exact,
        // through the one rounding kernel; over the numeric family, unchanged.
        ScalarFn::Floor => unary(&|c| match c {
            Cell::Decimal(_) => rounded_cell(RoundingMode::Floor, 0, c),
            _ => as_num(c)
                .map(|n| Cell::Float(n.floor()))
                .ok_or(EvalError::TypeError {
                    detail: "floor of a non-numeric".to_string(),
                }),
        }),
        // Over a decimal, a scale-0 `Rounded` under its pinned mode — exact,
        // through the one rounding kernel; over the numeric family, unchanged.
        ScalarFn::Ceil => unary(&|c| match c {
            Cell::Decimal(_) => rounded_cell(RoundingMode::Ceiling, 0, c),
            _ => as_num(c)
                .map(|n| Cell::Float(n.ceil()))
                .ok_or(EvalError::TypeError {
                    detail: "ceil of a non-numeric".to_string(),
                }),
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
        ScalarFn::Concat => {
            // Variadic; any null arg propagates (compose Coalesce for
            // treat-as-empty). Non-string args stringify via the same
            // rendering as `Cast string`.
            if args.is_empty() {
                return Err(EvalError::ArityError {
                    func: "Concat".to_string(),
                    expected: 1,
                    got: 0,
                });
            }
            if args.iter().any(is_null) {
                return Ok(NULL);
            }
            Ok(Cell::Str(args.iter().map(cell_string).collect::<String>()))
        }
        ScalarFn::Trim => unary(&|c| match c {
            // Pinned ASCII set — NOT the full Unicode whitespace class, which
            // hosts disagree on.
            Cell::Str(s) => Ok(Cell::Str(
                s.trim_matches([' ', '\t', '\r', '\n']).to_string(),
            )),
            _ => Err(EvalError::TypeError {
                detail: "trim of a non-string".to_string(),
            }),
        }),
        ScalarFn::Replace => {
            arity(3)?;
            match (&args[0], &args[1], &args[2]) {
                (Cell::Null, _, _) | (_, Cell::Null, _) | (_, _, Cell::Null) => Ok(NULL),
                (Cell::Str(subj), Cell::Str(find), Cell::Str(repl)) => {
                    // Pinned: empty `find` returns the subject unchanged.
                    if find.is_empty() {
                        Ok(Cell::Str(subj.clone()))
                    } else {
                        Ok(Cell::Str(subj.replace(find.as_str(), repl)))
                    }
                }
                _ => Err(EvalError::TypeError {
                    detail: "replace expects (string, string, string)".to_string(),
                }),
            }
        }
        ScalarFn::DateDiffDays => {
            arity(2)?;
            match (&args[0], &args[1]) {
                (Cell::Null, _) | (_, Cell::Null) => Ok(NULL),
                (a, b) => {
                    let da = civil_days(a)?;
                    let db = civil_days(b)?;
                    Ok(Cell::Int(db - da))
                }
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

fn is_string_pred(op: BinOp) -> bool {
    matches!(op, BinOp::Contains | BinOp::StartsWith | BinOp::EndsWith)
}

/// Ordinal substring predicates (Core Phase 90). Null propagates (matching
/// `comparison`); a non-string operand is a typed error. Byte-wise Ordinal is
/// the cross-host pin.
fn string_pred(op: BinOp, a: &Cell, b: &Cell) -> Result<Cell, EvalError> {
    match (a, b) {
        (Cell::Null, _) | (_, Cell::Null) => Ok(NULL),
        (Cell::Str(s), Cell::Str(t)) => {
            let r = match op {
                BinOp::Contains => s.contains(t.as_str()),
                BinOp::StartsWith => s.starts_with(t.as_str()),
                BinOp::EndsWith => s.ends_with(t.as_str()),
                _ => false,
            };
            Ok(Cell::Bool(r))
        }
        _ => Err(EvalError::TypeError {
            detail: "string predicate on a non-string operand".to_string(),
        }),
    }
}

/// Days since the civil epoch (1970-01-01 = 0) for the first 10 chars
/// (`YYYY-MM-DD`) of a date-like cell — pure integer math (Core Phase 90).
fn civil_days(c: &Cell) -> Result<i64, EvalError> {
    let s = match c {
        Cell::Date(s) | Cell::Timestamp(s) | Cell::Str(s) => s,
        _ => {
            return Err(EvalError::TypeError {
                detail: "dateDiffDays expects date/timestamp/string operands".to_string(),
            });
        }
    };
    let bad = || EvalError::TypeError {
        detail: format!("dateDiffDays: '{s}' is not YYYY-MM-DD[...]"),
    };
    let bytes = s.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(bad());
    }
    let part = |lo: usize, len: usize| -> Result<i64, EvalError> {
        s[lo..lo + len].parse::<i64>().map_err(|_| bad())
    };
    let (y, m, d) = (part(0, 4)?, part(5, 2)?, part(8, 2)?);
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Ok(era * 146_097 + doe - 719_468)
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
            } else if is_string_pred(*op) {
                string_pred(*op, &a, &b)
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
        ColExpr::InList { subject, items } => {
            let sv = eval_expr(cols, row, subject, env)?;
            if is_null(&sv) {
                return Ok(NULL);
            }
            // SQL three-valued membership: any equal => true; no match after
            // seeing a null => null.
            let mut saw_null = false;
            for it in items {
                let iv = eval_expr(cols, row, it, env)?;
                if is_null(&iv) {
                    saw_null = true;
                    continue;
                }
                match compare_cells(&sv, &iv) {
                    Some(Ordering::Equal) => return Ok(Cell::Bool(true)),
                    Some(_) => {}
                    None => {
                        return Err(EvalError::TypeError {
                            detail: "in: comparison between incompatible types".to_string(),
                        });
                    }
                }
            }
            Ok(if saw_null { NULL } else { Cell::Bool(false) })
        }
        // List params resolve by substitution BEFORE evaluation on the
        // reference host; one that reaches the evaluator is unbound — the
        // scalar-`Param` strictness.
        ColExpr::InParam { name, .. } => Err(EvalError::UnboundParam {
            name: name.clone(),
            bound: env.keys().cloned().collect(),
        }),
        ColExpr::IsNull { expr } => {
            let v = eval_expr(cols, row, expr, env)?;
            Ok(Cell::Bool(is_null(&v)))
        }
        // Evaluation order: the scale, the dividend, the divisor.
        ColExpr::Quotient {
            dividend,
            divisor,
            rounding,
        } => {
            let scale = resolve_scale(env, &rounding.scale)?;
            let a = eval_expr(cols, row, dividend, env)?;
            let b = eval_expr(cols, row, divisor, env)?;
            quotient_cell(rounding.mode, scale, &a, &b)
        }
        ColExpr::Rounded { expr, rounding } => {
            let scale = resolve_scale(env, &rounding.scale)?;
            let v = eval_expr(cols, row, expr, env)?;
            rounded_cell(rounding.mode, scale, &v)
        }
    }
}

// ─── static typing + the derived-column typing rule (fuaran-core#338) ───────
//
// What an expression's PRESENT values can be, decided from a step's schema and
// nothing else — three states, because a node that never produces a present
// value (a null literal) is compatible with every type, and folding it into
// "unknown" would cost the type of every `Case` whose else is a null literal.
//
// THE derived-column rule, read by `Derive` and `Unpivot` alike:
//  - the typer decides the column (`Of ty`) ⇒ `ty` on every frame, empty or
//    all-null included;
//  - no present value can occur (`Absent`) ⇒ `string`;
//  - undecidable from the schema (`Unknown`) ⇒ the substituted expression where
//    the env's params decide it, else the CELLS decide by the widening join
//    (`string` when none is present);
//  - a float beside a decimal ⇒ refused by name, never widened.

/// The static type of a node's present values.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Typing {
    /// No present value is ever produced: every row answers null or an error.
    Absent,
    /// Every present value carries this type.
    Of(ColumnType),
    /// Undecidable from the schema.
    Unknown,
}

impl Typing {
    /// The least typing covering both: `Absent` is the identity, two equal
    /// types agree, anything else is `Unknown`. Deliberately NOT the widening
    /// join — `Of` is an exactness claim.
    fn join(self, other: Typing) -> Typing {
        match (self, other) {
            (Typing::Absent, t) | (t, Typing::Absent) => t,
            (Typing::Of(x), Typing::Of(y)) if x == y => Typing::Of(x),
            _ => Typing::Unknown,
        }
    }

    fn join_all(ts: impl IntoIterator<Item = Typing>) -> Typing {
        ts.into_iter().fold(Typing::Absent, Typing::join)
    }

    fn of_cell(c: &Cell) -> Typing {
        cell_type_of(c).map_or(Typing::Absent, Typing::Of)
    }

    fn numeric(self) -> bool {
        matches!(self, Typing::Of(ColumnType::Int | ColumnType::Float))
    }

    fn bool_like(self) -> bool {
        matches!(self, Typing::Absent | Typing::Of(ColumnType::Bool))
    }

    /// The exact family: a decimal, or an int, which promotes losslessly.
    fn exact(self) -> bool {
        matches!(self, Typing::Of(ColumnType::Int | ColumnType::Decimal))
    }
}

/// Two exact operands at least one of which is a decimal.
fn decimal_pair(a: Typing, b: Typing) -> bool {
    a.exact()
        && b.exact()
        && (a == Typing::Of(ColumnType::Decimal) || b == Typing::Of(ColumnType::Decimal))
}

fn typing_binary(op: BinOp, a: Typing, b: Typing) -> Typing {
    let null_propagating = |decide: &dyn Fn() -> Typing| -> Typing {
        if a == Typing::Absent || b == Typing::Absent {
            Typing::Absent
        } else {
            decide()
        }
    };
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul => null_propagating(&|| match (a, b) {
            (Typing::Of(ColumnType::Int), Typing::Of(ColumnType::Int)) => {
                Typing::Of(ColumnType::Int)
            }
            _ if a.numeric() && b.numeric() => Typing::Of(ColumnType::Float),
            _ if decimal_pair(a, b) => Typing::Of(ColumnType::Decimal),
            _ => Typing::Unknown,
        }),
        // A decimal `Div` is refused on every row, so no present value of it.
        BinOp::Div => null_propagating(&|| {
            if a.numeric() && b.numeric() {
                Typing::Of(ColumnType::Float)
            } else {
                Typing::Unknown
            }
        }),
        BinOp::Mod => null_propagating(&|| match (a, b) {
            (Typing::Of(ColumnType::Int), Typing::Of(ColumnType::Int)) => {
                Typing::Of(ColumnType::Int)
            }
            _ if decimal_pair(a, b) => Typing::Of(ColumnType::Decimal),
            _ => Typing::Unknown,
        }),
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            null_propagating(&|| match (a, b) {
                _ if a.numeric() && b.numeric() => Typing::Of(ColumnType::Bool),
                _ if a.exact() && b.exact() => Typing::Of(ColumnType::Bool),
                (Typing::Of(x), Typing::Of(y))
                    if x == y
                        && matches!(
                            x,
                            ColumnType::Str
                                | ColumnType::Date
                                | ColumnType::Timestamp
                                | ColumnType::TimestampMs
                                | ColumnType::TimestampUs
                                | ColumnType::TimestampNs
                                | ColumnType::Bool
                        ) =>
                {
                    Typing::Of(ColumnType::Bool)
                }
                _ => Typing::Unknown,
            })
        }
        BinOp::And | BinOp::Or => {
            if a.bool_like() && b.bool_like() {
                a.join(b)
            } else {
                Typing::Unknown
            }
        }
        BinOp::Contains | BinOp::StartsWith | BinOp::EndsWith => {
            null_propagating(&|| match (a, b) {
                (Typing::Of(ColumnType::Str), Typing::Of(ColumnType::Str)) => {
                    Typing::Of(ColumnType::Bool)
                }
                _ => Typing::Unknown,
            })
        }
    }
}

/// A scalar function's typing, total over `ScalarFn`. An arity the function
/// refuses is an error on every row, so `Absent`; a null argument propagates
/// exactly where the function propagates it.
fn typing_apply(func: ScalarFn, args: &[Typing]) -> Typing {
    let any_absent = args.contains(&Typing::Absent);
    let unary = |decide: &dyn Fn(Typing) -> Typing| -> Typing {
        match args {
            [Typing::Absent] => Typing::Absent,
            [t] => decide(*t),
            _ => Typing::Absent,
        }
    };
    match func {
        ScalarFn::Abs => unary(&|t| match t {
            Typing::Of(ty @ (ColumnType::Int | ColumnType::Float | ColumnType::Decimal)) => {
                Typing::Of(ty)
            }
            _ => Typing::Unknown,
        }),
        // Over a decimal these are scale-0 `Rounded`s and answer a decimal; over
        // an operand the typer cannot decide they cannot be decided either.
        ScalarFn::Round | ScalarFn::Floor | ScalarFn::Ceil => unary(&|t| match t {
            Typing::Of(ColumnType::Decimal) => Typing::Of(ColumnType::Decimal),
            Typing::Unknown => Typing::Unknown,
            Typing::Absent | Typing::Of(_) => Typing::Of(ColumnType::Float),
        }),
        ScalarFn::Length => unary(&|_| Typing::Of(ColumnType::Int)),
        ScalarFn::Lower | ScalarFn::Upper | ScalarFn::Trim => {
            unary(&|_| Typing::Of(ColumnType::Str))
        }
        ScalarFn::Substr => match args {
            [Typing::Absent, _, _] => Typing::Absent,
            [_, _, _] => Typing::Of(ColumnType::Str),
            _ => Typing::Absent,
        },
        ScalarFn::DatePart => match args {
            [_, Typing::Absent] => Typing::Absent,
            [_, _] => Typing::Of(ColumnType::Int),
            _ => Typing::Absent,
        },
        ScalarFn::Concat => {
            if args.is_empty() || any_absent {
                Typing::Absent
            } else {
                Typing::Of(ColumnType::Str)
            }
        }
        ScalarFn::Replace => {
            if args.len() != 3 || any_absent {
                Typing::Absent
            } else {
                Typing::Of(ColumnType::Str)
            }
        }
        ScalarFn::DateDiffDays => {
            if args.len() != 2 || any_absent {
                Typing::Absent
            } else {
                Typing::Of(ColumnType::Int)
            }
        }
        // Phase 2400 (the reference's D43): the transcendentals answer a float,
        // as a root does. A power over an int base and an int exponent is decided
        // by the exponent's SIGN, which a typing does not carry — `typing_pow_lit`
        // reads a literal exponent, and otherwise the cells decide; an undecided
        // operand may be that pair, so it is undecided too; every other pair is a
        // float.
        ScalarFn::Exp | ScalarFn::Ln | ScalarFn::Log10 => unary(&|_| Typing::Of(ColumnType::Float)),
        ScalarFn::Pow => match args {
            [Typing::Absent, _] | [_, Typing::Absent] => Typing::Absent,
            [Typing::Of(ColumnType::Int), Typing::Of(ColumnType::Int)] => Typing::Unknown,
            [Typing::Unknown, _] | [_, Typing::Unknown] => Typing::Unknown,
            [_, _] => Typing::Of(ColumnType::Float),
            _ => Typing::Absent,
        },
    }
}

/// A power under a LITERAL integer exponent: an int base answers an int at or
/// above zero and a float below; any other typed base a float.
fn typing_pow_lit(n: i64, b: Typing) -> Typing {
    match b {
        Typing::Absent => Typing::Absent,
        Typing::Unknown => Typing::Unknown,
        Typing::Of(ColumnType::Int) if n >= 0 => Typing::Of(ColumnType::Int),
        Typing::Of(_) => Typing::Of(ColumnType::Float),
    }
}

/// The static typing of an expression over a schema, with no env. Total over
/// the closed `ColExpr` enum with no catch-all.
fn typing(cols: &[SchemaEntry], e: &ColExpr) -> Typing {
    let go = |x: &ColExpr| typing(cols, x);
    match e {
        ColExpr::Col { name } => col_type(cols, name).map_or(Typing::Unknown, Typing::Of),
        ColExpr::Lit { cell } => Typing::of_cell(cell),
        ColExpr::Param { .. } => Typing::Unknown,
        ColExpr::Binary { op, left, right } => typing_binary(*op, go(left), go(right)),
        ColExpr::Not { expr } => match go(expr) {
            Typing::Absent => Typing::Absent,
            Typing::Of(ColumnType::Bool) => Typing::Of(ColumnType::Bool),
            Typing::Of(_) | Typing::Unknown => Typing::Unknown,
        },
        ColExpr::Coalesce { exprs } => Typing::join_all(exprs.iter().map(go)),
        ColExpr::Case { cases, else_expr } => Typing::join_all(
            std::iter::once(go(else_expr)).chain(cases.iter().map(|c| go(&c.then))),
        ),
        // A cast's present values are always of the target type.
        ColExpr::Cast { column_type, expr } => match go(expr) {
            Typing::Absent => Typing::Absent,
            Typing::Of(_) | Typing::Unknown => Typing::Of(*column_type),
        },
        ColExpr::Apply {
            func: ScalarFn::Pow,
            args,
        } if matches!(args.as_slice(), [_, ColExpr::Lit { cell: Cell::Int(_) }]) => {
            match args.as_slice() {
                [b, ColExpr::Lit { cell: Cell::Int(n) }] => typing_pow_lit(*n, go(b)),
                _ => unreachable!("guarded by the arm's pattern"),
            }
        }
        ColExpr::Apply { func, args } => {
            typing_apply(*func, &args.iter().map(go).collect::<Vec<_>>())
        }
        // Membership is three-valued: a null subject answers null, else a bool.
        ColExpr::InList { subject, .. } | ColExpr::InParam { subject, .. } => match go(subject) {
            Typing::Absent => Typing::Absent,
            Typing::Of(_) | Typing::Unknown => Typing::Of(ColumnType::Bool),
        },
        ColExpr::IsNull { .. } => Typing::Of(ColumnType::Bool),
        ColExpr::Quotient {
            dividend, divisor, ..
        } => match (go(dividend), go(divisor)) {
            (Typing::Absent, _) | (_, Typing::Absent) => Typing::Absent,
            (a, b) if a.exact() && b.exact() => Typing::Of(ColumnType::Decimal),
            _ => Typing::Unknown,
        },
        ColExpr::Rounded { expr, .. } => match go(expr) {
            Typing::Absent => Typing::Absent,
            t if t.exact() => Typing::Of(ColumnType::Decimal),
            _ => Typing::Unknown,
        },
    }
}

/// How a derived column is typed.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DerivedTyping {
    /// The schema decides the type: the same on every frame.
    Decided(ColumnType),
    /// The cells decide it, by the widening join of the present cells' types.
    ByCells,
    /// A float beside a decimal, visible in the expression's own arms.
    Refused,
}

/// Does an arm the derived value is drawn from — through `Case` and
/// `Coalesce`, whose answer IS one of their operands' — carry type `ty`?
fn arms_have(ty: ColumnType, cols: &[SchemaEntry], e: &ColExpr) -> bool {
    match e {
        ColExpr::Coalesce { exprs } => exprs.iter().any(|x| arms_have(ty, cols, x)),
        ColExpr::Case { cases, else_expr } => {
            arms_have(ty, cols, else_expr) || cases.iter().any(|c| arms_have(ty, cols, &c.then))
        }
        ColExpr::Col { .. }
        | ColExpr::Lit { .. }
        | ColExpr::Param { .. }
        | ColExpr::Binary { .. }
        | ColExpr::Not { .. }
        | ColExpr::Cast { .. }
        | ColExpr::Apply { .. }
        | ColExpr::InList { .. }
        | ColExpr::InParam { .. }
        | ColExpr::IsNull { .. }
        | ColExpr::Quotient { .. }
        | ColExpr::Rounded { .. } => typing(cols, e) == Typing::Of(ty),
    }
}

/// How a `Derive` of `e` over `cols` types its column, statically.
fn derived_typing(cols: &[SchemaEntry], e: &ColExpr) -> DerivedTyping {
    if arms_have(ColumnType::Float, cols, e) && arms_have(ColumnType::Decimal, cols, e) {
        return DerivedTyping::Refused;
    }
    match typing(cols, e) {
        Typing::Of(ty) => DerivedTyping::Decided(ty),
        Typing::Absent => DerivedTyping::Decided(ColumnType::Str),
        Typing::Unknown => DerivedTyping::ByCells,
    }
}

/// `e` with every param bound in `env` replaced by its literal — the expression
/// the evaluator answers exactly as it answers `e`.
fn substitute_params(env: &EvalEnv, e: &ColExpr) -> ColExpr {
    let go = |x: &ColExpr| Box::new(substitute_params(env, x));
    let scale = |r: &Rounding| Rounding {
        scale: match &r.scale {
            RoundingScale::Param(name) => match env.get(name) {
                Some(Cell::Int(v)) => RoundingScale::Lit(*v),
                _ => r.scale.clone(),
            },
            RoundingScale::Lit(_) => r.scale.clone(),
        },
        mode: r.mode,
    };
    match e {
        ColExpr::Param { name } => match env.get(name) {
            Some(cell) => ColExpr::Lit { cell: cell.clone() },
            None => e.clone(),
        },
        ColExpr::Col { .. } | ColExpr::Lit { .. } => e.clone(),
        ColExpr::Binary { op, left, right } => ColExpr::Binary {
            op: *op,
            left: go(left),
            right: go(right),
        },
        ColExpr::Not { expr } => ColExpr::Not { expr: go(expr) },
        ColExpr::Coalesce { exprs } => ColExpr::Coalesce {
            exprs: exprs.iter().map(|x| substitute_params(env, x)).collect(),
        },
        ColExpr::Case { cases, else_expr } => ColExpr::Case {
            cases: cases
                .iter()
                .map(|c| CaseArm {
                    when: substitute_params(env, &c.when),
                    then: substitute_params(env, &c.then),
                })
                .collect(),
            else_expr: go(else_expr),
        },
        ColExpr::Cast { column_type, expr } => ColExpr::Cast {
            column_type: *column_type,
            expr: go(expr),
        },
        ColExpr::Apply { func, args } => ColExpr::Apply {
            func: *func,
            args: args.iter().map(|x| substitute_params(env, x)).collect(),
        },
        ColExpr::InList { subject, items } => ColExpr::InList {
            subject: go(subject),
            items: items.iter().map(|x| substitute_params(env, x)).collect(),
        },
        ColExpr::InParam { subject, name } => ColExpr::InParam {
            subject: go(subject),
            name: name.clone(),
        },
        ColExpr::IsNull { expr } => ColExpr::IsNull { expr: go(expr) },
        ColExpr::Quotient {
            dividend,
            divisor,
            rounding,
        } => ColExpr::Quotient {
            dividend: go(dividend),
            divisor: go(divisor),
            rounding: scale(rounding),
        },
        ColExpr::Rounded { expr, rounding } => ColExpr::Rounded {
            expr: go(expr),
            rounding: scale(rounding),
        },
    }
}

/// How an EVALUATION of a `Derive` types its column: the static rule wherever
/// it decides or refuses, and where it leaves the column to the cells, the type
/// the same rule decides over the expression with the env's params substituted.
fn derived_typing_in(env: &EvalEnv, cols: &[SchemaEntry], e: &ColExpr) -> DerivedTyping {
    match derived_typing(cols, e) {
        DerivedTyping::ByCells => match derived_typing(cols, &substitute_params(env, e)) {
            dt @ DerivedTyping::Decided(_) => dt,
            DerivedTyping::ByCells | DerivedTyping::Refused => DerivedTyping::ByCells,
        },
        dt @ (DerivedTyping::Decided(_) | DerivedTyping::Refused) => dt,
    }
}

/// The least column type two types both widen into — `int` and `float` join at
/// `float`, `int` and `decimal` at `decimal` — or `None` (a float beside a
/// decimal included).
fn join_column_type(a: ColumnType, b: ColumnType) -> Option<ColumnType> {
    match (a, b) {
        _ if a == b => Some(a),
        (ColumnType::Int, ColumnType::Float) | (ColumnType::Float, ColumnType::Int) => {
            Some(ColumnType::Float)
        }
        (ColumnType::Int, ColumnType::Decimal) | (ColumnType::Decimal, ColumnType::Int) => {
            Some(ColumnType::Decimal)
        }
        _ => None,
    }
}

/// How an `Unpivot`'s value column is typed over the value columns' declared
/// types: the same rule, the value columns being its arms.
fn unpivot_typing(types: &[ColumnType]) -> DerivedTyping {
    if types.contains(&ColumnType::Float) && types.contains(&ColumnType::Decimal) {
        return DerivedTyping::Refused;
    }
    let mut acc: Option<Option<ColumnType>> = None;
    for t in types {
        acc = Some(match acc {
            None => Some(*t),
            Some(Some(a)) => join_column_type(a, *t),
            Some(None) => None,
        });
    }
    match acc {
        None => DerivedTyping::Decided(ColumnType::Str),
        Some(Some(ty)) => DerivedTyping::Decided(ty),
        Some(None) => DerivedTyping::ByCells,
    }
}

fn float_beside_decimal(column: &str) -> EvalError {
    EvalError::TypeError {
        detail: format!(
            "derived column '{column}' joins a float and a decimal: cast one to the other's type first - Cast(decimal, ...) to keep the digits exact, Cast(float, ...) to compute approximately"
        ),
    }
}

fn cell_type_of(c: &Cell) -> Option<ColumnType> {
    match c {
        Cell::Int(_) => Some(ColumnType::Int),
        Cell::Float(_) => Some(ColumnType::Float),
        Cell::Bool(_) => Some(ColumnType::Bool),
        Cell::Str(_) => Some(ColumnType::Str),
        Cell::Date(_) => Some(ColumnType::Date),
        Cell::Timestamp(_) => Some(ColumnType::Timestamp),
        Cell::Decimal(_) => Some(ColumnType::Decimal),
        Cell::Null => None,
    }
}

/// The cells' half of the rule: the widening join over the present cells,
/// `string` when none is present, and the named refusals where the cells hold
/// a float beside a decimal, or two types no column type holds together.
fn type_from_cells(column: &str, cells: &[Cell]) -> Result<ColumnType, EvalError> {
    let mut acc: Option<ColumnType> = None;
    let (mut saw_float, mut saw_decimal) = (false, false);
    let mut disjoint: Option<(ColumnType, ColumnType)> = None;
    for t in cells.iter().filter_map(cell_type_of) {
        saw_float |= t == ColumnType::Float;
        saw_decimal |= t == ColumnType::Decimal;
        acc = Some(match acc {
            None => t,
            Some(a) => match join_column_type(a, t) {
                Some(j) => j,
                None => {
                    disjoint.get_or_insert((a, t));
                    a
                }
            },
        });
    }
    if saw_float && saw_decimal {
        return Err(float_beside_decimal(column));
    }
    if let Some((a, b)) = disjoint {
        return Err(EvalError::TypeError {
            detail: format!(
                "derived column '{column}' holds a {} beside a {}, which no column type holds together: cast one to the other's type first",
                a.as_str(),
                b.as_str()
            ),
        });
    }
    Ok(acc.unwrap_or(ColumnType::Str))
}

/// A derived column's type under its typing: the decided type, or the cells'.
fn column_type_by(
    dt: DerivedTyping,
    column: &str,
    cells: &[Cell],
) -> Result<ColumnType, EvalError> {
    match dt {
        DerivedTyping::Decided(ty) => Ok(ty),
        DerivedTyping::ByCells => type_from_cells(column, cells),
        DerivedTyping::Refused => Err(float_beside_decimal(column)),
    }
}

// ─── aggregates (pinned) ─────────────────────────────────────────────────────

fn present_nums(cells: &[Cell]) -> Vec<f64> {
    cells.iter().filter_map(as_num).collect()
}

fn float_mean(nums: &[f64]) -> Cell {
    if nums.is_empty() {
        NULL
    } else {
        Cell::Float(nums.iter().sum::<f64>() / nums.len() as f64)
    }
}

/// The POPULATION standard deviation.
fn float_stddev(nums: &[f64]) -> Cell {
    if nums.is_empty() {
        return NULL;
    }
    let n = nums.len() as f64;
    let mean = nums.iter().sum::<f64>() / n;
    let variance = nums.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n;
    Cell::Float(variance.sqrt())
}

/// The median in the float order (a STABLE sort, NaN counted at the top).
fn float_median(mut nums: Vec<f64>) -> Cell {
    if nums.is_empty() {
        return NULL;
    }
    nums.sort_by(|a, b| compare_float(*a, *b));
    let mid = nums.len() / 2;
    if nums.len() % 2 == 1 {
        Cell::Float(nums[mid])
    } else {
        Cell::Float((nums[mid - 1] + nums[mid]) / 2.0)
    }
}

/// `Min` / `Max`: the least / greatest present value by the cell order — an
/// incomparable pair keeping the incumbent, `Min` keeping the FIRST of equals
/// and `Max` the LAST — and null over no present value.
fn extreme(func: AggFn, present: &[&Cell]) -> Cell {
    let Some((first, rest)) = present.split_first() else {
        return NULL;
    };
    let mut acc = (*first).clone();
    for b in rest {
        if let Some(ord) = compare_cells(&acc, b) {
            let c_le = ord != Ordering::Greater;
            let keep_acc = (func == AggFn::Min) == c_le;
            if !keep_acc {
                acc = (*b).clone();
            }
        }
    }
    acc
}

/// The distinct present values under the cell identity token.
fn count_distinct(present: &[&Cell]) -> Cell {
    let keys: BTreeSet<String> = present.iter().map(|c| cell_token(c)).collect();
    Cell::Int(keys.len() as i64)
}

/// Every aggregate over a decimal column. The column is ADMITTED first: a
/// present cell that is not decimal text (an int reads as its digits) is a
/// named refusal outranking every answer, and every value is read
/// canonicalised. `Sum` is exact and a decimal; `Min` / `Max` compare exactly;
/// `Mean` / `Median` / `StdDev` read each value at its nearest double, and a
/// value past the float range is refused for them.
fn agg_decimal_cells(func: AggFn, cells: &[Cell]) -> Result<Cell, EvalError> {
    let mut admitted: Vec<Cell> = Vec::with_capacity(cells.len());
    for c in cells {
        admitted.push(match c {
            Cell::Null => NULL,
            other => match decimal_text(other).and_then(|t| decimal::try_canonical(&t)) {
                Some(text) => Cell::Decimal(text),
                None => {
                    return Err(EvalError::AggError {
                        detail: format!(
                            "aggregate over a decimal column met a cell outside its type: {}",
                            cell_string(other)
                        ),
                    });
                }
            },
        });
    }
    let present: Vec<&Cell> = admitted.iter().filter(|c| !is_null(c)).collect();
    let texts = || {
        present.iter().filter_map(|c| match c {
            Cell::Decimal(t) => Some(t.as_str()),
            _ => None,
        })
    };
    let floats = || -> Result<Vec<f64>, EvalError> {
        texts()
            .map(|t| {
                decimal::try_to_float(t).ok_or_else(|| EvalError::AggError {
                    detail: format!(
                        "the decimal {t} is past the float range, and {} is a float",
                        func.as_str()
                    ),
                })
            })
            .collect()
    };
    Ok(match func {
        AggFn::Count => Cell::Int(present.len() as i64),
        AggFn::First => admitted.first().cloned().unwrap_or(NULL),
        AggFn::Last => admitted.last().cloned().unwrap_or(NULL),
        AggFn::Min | AggFn::Max => extreme(func, &present),
        AggFn::CountDistinct => count_distinct(&present),
        AggFn::Sum => {
            let mut total: Option<String> = None;
            for t in texts() {
                total = Some(match total {
                    None => t.to_string(),
                    Some(acc) => decimal::add(&acc, t).unwrap_or(acc),
                });
            }
            total.map_or(NULL, Cell::Decimal)
        }
        AggFn::Mean => float_mean(&floats()?),
        AggFn::Median => float_median(floats()?),
        AggFn::Stddev => float_stddev(&floats()?),
    })
}

fn agg_cells(func: AggFn, src_type: ColumnType, cells: &[Cell]) -> Result<Cell, EvalError> {
    if src_type == ColumnType::Decimal {
        return agg_decimal_cells(func, cells);
    }
    let present: Vec<&Cell> = cells.iter().filter(|c| !is_null(c)).collect();
    Ok(match func {
        AggFn::Count => Cell::Int(present.len() as i64),
        AggFn::CountDistinct => count_distinct(&present),
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
        AggFn::Mean => float_mean(&present_nums(cells)),
        AggFn::Stddev => float_stddev(&present_nums(cells)),
        AggFn::Median => float_median(present_nums(cells)),
        AggFn::Min | AggFn::Max => extreme(func, &present),
    })
}

fn agg_type(func: AggFn, src_type: ColumnType) -> ColumnType {
    match func {
        AggFn::Count | AggFn::CountDistinct => ColumnType::Int,
        AggFn::Mean | AggFn::Median | AggFn::Stddev => ColumnType::Float,
        AggFn::Sum | AggFn::Min | AggFn::Max | AggFn::First | AggFn::Last => src_type,
    }
}

/// One aggregate over a column of `src_type` — the public form the decimal law
/// family's `aggregate` case runs.
pub fn aggregate(func: AggFn, src_type: ColumnType, cells: &[Cell]) -> Result<Cell, EvalError> {
    agg_cells(func, src_type, cells)
}

/// The canonical identity token of a cell — `m:<canonical>` for a decimal, as
/// the reference's `Cell.token` spells it.
pub fn cell_token(c: &Cell) -> String {
    match c {
        Cell::Int(v) => format!("i:{v}"),
        Cell::Float(v) => format!(
            "f:{}",
            if v.is_nan() {
                "NaN".to_string()
            } else if *v == f64::INFINITY {
                "Inf".to_string()
            } else if *v == f64::NEG_INFINITY {
                "-Inf".to_string()
            } else {
                format_number(*v)
            }
        ),
        Cell::Bool(b) => format!("b:{}", if *b { "1" } else { "0" }),
        Cell::Str(s) => format!("s:{s}"),
        Cell::Date(s) => format!("d:{s}"),
        Cell::Timestamp(s) => format!("t:{s}"),
        Cell::Decimal(s) => format!(
            "m:{}",
            decimal::try_canonical(s).unwrap_or_else(|| s.clone())
        ),
        Cell::Null => "n:".to_string(),
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

/// A row's identity key: each cell's token, LENGTH-PREFIXED so the joined key
/// is injective (two rows share a key exactly when their tokens agree cell by
/// cell — a bare concatenation let `("aS","b")` and `("a","Sb")` collide).
fn row_key(cells: &[Cell]) -> String {
    cells
        .iter()
        .map(|c| {
            let t = cell_token(c);
            format!("{}:{t}", t.len())
        })
        .collect()
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

/// Each output column's name beside what produced it, in output order: the
/// first name an earlier output already took is refused — a table cannot hold
/// two columns of one name (the reference's D35, Phase 468: project, groupBy,
/// unpivot and pivot ask this one check, after every name the verb resolves and
/// before any row is read).
fn repeated_output(verb: &str, outs: &[(String, String)]) -> Result<(), EvalError> {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, by) in outs {
        if let Some(first) = seen.get(name.as_str()) {
            return Err(EvalError::TypeError {
                detail: format!(
                    "{verb} output '{name}' is named twice, by {first} and by {by}; give each output a name of its own"
                ),
            });
        }
        seen.insert(name, by);
    }
    Ok(())
}

fn eval_project(f: &Frame, pairs: &[(String, String)]) -> Result<Frame, EvalError> {
    let mut resolved: Vec<(String, ColumnType, usize)> = Vec::new();
    for (a, b) in pairs {
        let i = col_index(&f.cols, a).ok_or_else(|| unknown_column(a, &f.cols))?;
        resolved.push((b.clone(), f.cols[i].column_type, i));
    }
    let outs: Vec<(String, String)> = pairs
        .iter()
        .map(|(a, b)| (b.clone(), format!("the projection of '{a}'")))
        .collect();
    repeated_output("project", &outs)?;
    Ok(Frame {
        cols: resolved
            .iter()
            .map(|(out, ty, _)| SchemaEntry::new(out.clone(), *ty))
            .collect(),
        rows: f
            .rows
            .iter()
            .map(|row| resolved.iter().map(|(_, _, i)| row[*i].clone()).collect())
            .collect(),
    })
}

fn eval_derive(f: &Frame, name: &str, expr: &ColExpr, env: &EvalEnv) -> Result<Frame, EvalError> {
    // The column's typing is read off the schema before any row: a derive
    // refused statically is refused on every frame, an empty one included, and
    // a decided one carries its type whatever rows the frame holds.
    let dt = derived_typing_in(env, &f.cols, expr);
    if dt == DerivedTyping::Refused {
        return Err(float_beside_decimal(name));
    }
    let mut new_cells = Vec::with_capacity(f.rows.len());
    for row in &f.rows {
        new_cells.push(eval_expr(&f.cols, row, expr, env)?);
    }
    let ty = column_type_by(dt, name, &new_cells)?;
    if let Some(i) = col_index(&f.cols, name) {
        let cols = f
            .cols
            .iter()
            .enumerate()
            .map(|(j, e)| {
                if j == i {
                    SchemaEntry::new(e.name.clone(), ty)
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
        cols.push(SchemaEntry::new(name.to_string(), ty));
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
    let outs: Vec<(String, String)> = keys
        .iter()
        .map(|k| (k.clone(), format!("the key '{k}'")))
        .chain(
            aggs.iter()
                .map(|a| (a.name.clone(), format!("the aggregate over '{}'", a.of))),
        )
        .collect();
    repeated_output("groupBy", &outs)?;

    let mut out_cols: Vec<SchemaEntry> = keys
        .iter()
        .map(|k| SchemaEntry::new(k.clone(), col_type(&f.cols, k).unwrap()))
        .collect();
    out_cols.extend(
        resolved
            .iter()
            .map(|(a, ty, _)| SchemaEntry::new(a.name.clone(), agg_type(a.func, *ty))),
    );

    let rows = order
        .iter()
        .map(|ks| {
            let (key, grp_rows) = &groups[ks];
            let mut r = key.clone();
            for (a, ty, idx) in &resolved {
                let col: Vec<Cell> = grp_rows.iter().map(|row| row[*idx].clone()).collect();
                r.push(agg_cells(a.func, *ty, &col)?);
            }
            Ok(r)
        })
        .collect::<Result<_, EvalError>>()?;

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
        .map(|e| {
            let name = if left_names.contains(&e.name) {
                format!("{}_right", e.name)
            } else {
                e.name.clone()
            };
            SchemaEntry::new(name, e.column_type)
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
    // A column of the union takes the JOIN of the two sides' types where one
    // exists (an int column over a float one is a float column, its cells packed
    // under it), and is refused by name where none does.
    let mut cols = Vec::with_capacity(f.cols.len());
    for (l, r) in f.cols.iter().zip(&other.cols) {
        let ty = join_column_type(l.column_type, r.column_type).ok_or_else(|| {
            EvalError::TypeError {
                detail: format!(
                    "union column '{}' holds a {} beside a {}, which no column type holds together: cast one to the other's type first",
                    l.name,
                    l.column_type.as_str(),
                    r.column_type.as_str()
                ),
            }
        })?;
        cols.push(SchemaEntry::new(l.name.clone(), ty));
    }
    let pack = |row: &Vec<Cell>| -> Vec<Cell> {
        row.iter()
            .zip(&cols)
            .map(|(c, e)| match (c, e.column_type) {
                (Cell::Int(v), ColumnType::Float) => Cell::Float(*v as f64),
                (Cell::Int(v), ColumnType::Decimal) => Cell::Decimal(v.to_string()),
                _ => c.clone(),
            })
            .collect()
    };
    let rows = f.rows.iter().chain(&other.rows).map(pack).collect();
    Ok(Frame { cols, rows })
}

/// `Intersect` / `Except` — the multiset set-ops, keyed on the SAME row identity
/// `Distinct` dedups on, so a null is a value that matches itself. `keep_present`
/// selects intersect from except; the left's order and multiplicity survive.
fn eval_set_op(
    verb: &str,
    keep_present: bool,
    f: &Frame,
    other: &Frame,
) -> Result<Frame, EvalError> {
    if available(&f.cols) != available(&other.cols) {
        return Err(EvalError::JoinError {
            detail: format!("{verb} requires matching column names"),
        });
    }
    let right: BTreeSet<String> = other.rows.iter().map(|r| row_key(r)).collect();
    Ok(Frame {
        cols: f.cols.clone(),
        rows: f
            .rows
            .iter()
            .filter(|r| right.contains(&row_key(r)) == keep_present)
            .cloned()
            .collect(),
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
    // Over a decimal column the running total is EXACT and a decimal, as `Sum`
    // over one is — never a float in silence.
    let source_is_decimal = col_type(&f.cols, of) == Some(ColumnType::Decimal);

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
            WindowFn::CumulSum if source_is_decimal => {
                let mut acc = "0".to_string();
                vals.iter()
                    .map(|v| {
                        if let Some(x) = decimal_text(v) {
                            acc = decimal::add(&acc, &x).unwrap_or(acc.clone());
                        }
                        Cell::Decimal(acc.clone())
                    })
                    .collect()
            }
            WindowFn::CumulSum => {
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
                    // A decimal is read at its nearest float: a window's MEAN
                    // over a decimal column is a float, as `Mean` over one is.
                    let nums: Vec<f64> = vals[lo..=i]
                        .iter()
                        .filter_map(|v| match v {
                            Cell::Decimal(t) => decimal::try_to_float(t),
                            other => as_num(other),
                        })
                        .collect();
                    float_mean(&nums)
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
        WindowFn::CumulSum if source_is_decimal => ColumnType::Decimal,
        WindowFn::CumulSum | WindowFn::RollingMean => ColumnType::Float,
        WindowFn::Lag | WindowFn::Lead => col_type(&f.cols, of).unwrap_or(ColumnType::Str),
    };

    let mut cols = f.cols.clone();
    cols.push(SchemaEntry::new(alias.to_string(), ty));
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
        if on_seen.insert(cell_token(c)) {
            on_values.push(c.clone());
        }
    }
    on_values.sort_by_key(cell_string);
    let outs: Vec<(String, String)> = index
        .iter()
        .map(|n| (n.clone(), format!("the index column '{n}'")))
        .chain(on_values.iter().map(|ov| {
            let t = cell_string(ov);
            let by = format!("the column for the on value '{t}'");
            (t, by)
        }))
        .collect();
    repeated_output("pivot", &outs)?;

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
        .map(|n| SchemaEntry::new(n.clone(), col_type(&f.cols, n).unwrap()))
        .collect();
    cols.extend(
        on_values
            .iter()
            .map(|ov| SchemaEntry::new(cell_string(ov), agg_type(agg, val_type))),
    );

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
                r.push(agg_cells(agg, val_type, &matching)?);
            }
            Ok(r)
        })
        .collect::<Result<_, EvalError>>()?;

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
    let outs: Vec<(String, String)> = id_vars
        .iter()
        .map(|n| (n.clone(), format!("the id column '{n}'")))
        .chain([
            (
                "variable".to_string(),
                "the unpivot's variable column".to_string(),
            ),
            (
                "value".to_string(),
                "the unpivot's value column".to_string(),
            ),
        ])
        .collect();
    repeated_output("unpivot", &outs)?;
    // The value column is typed by the one derived-column rule, the value
    // columns being its arms: their declared types' join where the schema
    // decides it — over an empty frame too — and the melted cells only where it
    // does not.
    let declared: Vec<ColumnType> = val_idx.iter().map(|i| f.cols[*i].column_type).collect();
    let dt = unpivot_typing(&declared);
    if dt == DerivedTyping::Refused {
        return Err(float_beside_decimal("value"));
    }
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
    let melted: Vec<Cell> = rows.iter().map(|r| r[id_vars.len() + 1].clone()).collect();
    let val_type = column_type_by(dt, "value", &melted)?;

    let mut cols: Vec<SchemaEntry> = id_vars
        .iter()
        .map(|n| SchemaEntry::new(n.clone(), col_type(&f.cols, n).unwrap()))
        .collect();
    cols.push(SchemaEntry::new("variable".to_string(), ColumnType::Str));
    cols.push(SchemaEntry::new("value".to_string(), val_type));
    Ok(Frame { cols, rows })
}

// ─── the guarded table check ─────────────────────────────────────────────────

/// Does every present cell of `t` belong to its column's type? An int widens
/// into a float or a decimal column; a decimal must be CANONICAL text, so a
/// table built by hand cannot put a second spelling of one value on the wire.
/// The guard a caller runs before encoding a table it did not decode
/// (Phase 2001).
pub fn validate_table(t: &Table) -> Result<(), EvalError> {
    for col in &t.columns {
        for c in &col.cells {
            let fits = match (c, col.column_type) {
                (Cell::Null, _) => true,
                (Cell::Decimal(s), ColumnType::Decimal) => decimal::is_canonical(s),
                (Cell::Int(_), ColumnType::Int | ColumnType::Float | ColumnType::Decimal) => true,
                (Cell::Float(_), ColumnType::Float)
                | (Cell::Bool(_), ColumnType::Bool)
                | (Cell::Str(_), ColumnType::Str)
                | (Cell::Date(_), ColumnType::Date) => true,
                // Phase 2206: a timestamp cell is one canonical instant text
                // whichever unit its column declares.
                (Cell::Timestamp(_), ty) if ty.timestamp_digits().is_some() => true,
                _ => false,
            };
            if !fits {
                return Err(EvalError::TypeError {
                    detail: format!(
                        "column '{}' of type {} holds a cell outside its type: {}",
                        col.name,
                        col.column_type.as_str(),
                        cell_token(c)
                    ),
                });
            }
        }
    }
    Ok(())
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
        TransformStep::Intersect { source } => {
            let other = eval_source(resolve, source)?;
            eval_set_op("intersect", true, f, &other.to_frame())
        }
        TransformStep::Except { source } => {
            let other = eval_source(resolve, source)?;
            eval_set_op("except", false, f, &other.to_frame())
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
        ColExpr::InList { subject, items } => {
            let mut out = col_expr_params(subject);
            out.extend(items.iter().flat_map(col_expr_params));
            out
        }
        ColExpr::InParam { subject, name } => {
            let mut out = col_expr_params(subject);
            out.push(name.clone());
            out
        }
        ColExpr::IsNull { expr } => col_expr_params(expr),
        ColExpr::Quotient {
            dividend,
            divisor,
            rounding,
        } => [
            col_expr_params(dividend),
            col_expr_params(divisor),
            scale_params(rounding),
        ]
        .concat(),
        ColExpr::Rounded { expr, rounding } => {
            [col_expr_params(expr), scale_params(rounding)].concat()
        }
    }
}

/// A rounding's scale param, when it names one.
fn scale_params(r: &Rounding) -> Vec<String> {
    match &r.scale {
        RoundingScale::Param(name) => vec![name.clone()],
        RoundingScale::Lit(_) => Vec::new(),
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

// ─── list-param substitution (fuaran-core#91 / Phase 610) ────────────────────
//
// A LIST param resolves by SUBSTITUTION, not through the evaluation env: every
// `InParam` bound in a [`ListEnv`] is rewritten to the literal `InList` form
// BEFORE the pipeline is evaluated, which is why `eval_expr`'s `InParam` arm is
// an unbound-param error rather than a lookup. An unbound `InParam` is left
// intact — the host's prune then sees it still naming its own param, so one
// `step_params`-driven prune covers scalar and list params alike, with no
// second rule.

/// The list-param environment — binds each `ColExpr::InParam` name to the
/// selection substituted for it. A `BTreeMap` for the same determinism reason
/// [`EvalEnv`] is one.
pub type ListEnv = BTreeMap<String, Vec<Cell>>;

/// Substitute every `InParam` bound in `list_env` with the literal `InList`
/// form, through one expression. An unbound list param is returned intact (its
/// subject still rewritten), so it keeps naming its own param for the caller's
/// prune.
fn substitute_list_params_expr(list_env: &ListEnv, e: &ColExpr) -> ColExpr {
    match e {
        ColExpr::Col { .. } | ColExpr::Lit { .. } | ColExpr::Param { .. } => e.clone(),
        ColExpr::Binary { op, left, right } => ColExpr::Binary {
            op: *op,
            left: Box::new(substitute_list_params_expr(list_env, left)),
            right: Box::new(substitute_list_params_expr(list_env, right)),
        },
        ColExpr::Not { expr } => ColExpr::Not {
            expr: Box::new(substitute_list_params_expr(list_env, expr)),
        },
        ColExpr::Coalesce { exprs } => ColExpr::Coalesce {
            exprs: exprs
                .iter()
                .map(|x| substitute_list_params_expr(list_env, x))
                .collect(),
        },
        ColExpr::Case { cases, else_expr } => ColExpr::Case {
            cases: cases
                .iter()
                .map(|br| CaseArm {
                    when: substitute_list_params_expr(list_env, &br.when),
                    then: substitute_list_params_expr(list_env, &br.then),
                })
                .collect(),
            else_expr: Box::new(substitute_list_params_expr(list_env, else_expr)),
        },
        ColExpr::Cast { column_type, expr } => ColExpr::Cast {
            column_type: *column_type,
            expr: Box::new(substitute_list_params_expr(list_env, expr)),
        },
        ColExpr::Apply { func, args } => ColExpr::Apply {
            func: *func,
            args: args
                .iter()
                .map(|x| substitute_list_params_expr(list_env, x))
                .collect(),
        },
        ColExpr::InList { subject, items } => ColExpr::InList {
            subject: Box::new(substitute_list_params_expr(list_env, subject)),
            items: items
                .iter()
                .map(|x| substitute_list_params_expr(list_env, x))
                .collect(),
        },
        ColExpr::InParam { subject, name } => {
            let subject = Box::new(substitute_list_params_expr(list_env, subject));
            match list_env.get(name) {
                Some(items) => ColExpr::InList {
                    subject,
                    items: items
                        .iter()
                        .map(|cell| ColExpr::Lit { cell: cell.clone() })
                        .collect(),
                },
                None => ColExpr::InParam {
                    subject,
                    name: name.clone(),
                },
            }
        }
        ColExpr::IsNull { expr } => ColExpr::IsNull {
            expr: Box::new(substitute_list_params_expr(list_env, expr)),
        },
        ColExpr::Quotient {
            dividend,
            divisor,
            rounding,
        } => ColExpr::Quotient {
            dividend: Box::new(substitute_list_params_expr(list_env, dividend)),
            divisor: Box::new(substitute_list_params_expr(list_env, divisor)),
            rounding: rounding.clone(),
        },
        ColExpr::Rounded { expr, rounding } => ColExpr::Rounded {
            expr: Box::new(substitute_list_params_expr(list_env, expr)),
            rounding: rounding.clone(),
        },
    }
}

/// Substitute every `InParam` bound in `list_env` through ONE expression — the
/// `Binding::Expr` entry point (§3.3.2), where there is no pipeline to walk.
/// The rule is the pipeline's, applied to the one expression the case carries.
pub fn substitute_list_params_in_expr(list_env: &ListEnv, e: &ColExpr) -> ColExpr {
    substitute_list_params_expr(list_env, e)
}

/// Evaluate ONE scalar expression against `env` alone — the `Binding::Expr`
/// evaluator (§3.3.2). It is `eval_expr` over an EMPTY frame, which is exactly
/// what the case means: an `Expr` evaluates against its params and has no row.
///
/// A `col` reference is therefore unresolvable here, and that is not a gap this
/// function papers over — the decoder refuses one before a tree carrying it can
/// exist, so reaching this path with a `Col` means the two disagree, and the
/// unknown-column error the shared walk raises is the honest report of that.
pub fn eval_scalar_expr(e: &ColExpr, env: &EvalEnv) -> Result<Cell, EvalError> {
    eval_expr(&[], &[], e, env)
}

/// Substitute every `InParam` bound in `list_env` through the whole pipeline —
/// the Rust mirror of Core `Transform.substituteListParams`. Only `Filter` /
/// `Derive` carry a `ColExpr`; every other step is returned unchanged.
pub fn substitute_list_params(
    list_env: &ListEnv,
    pipeline: &[TransformStep],
) -> Vec<TransformStep> {
    pipeline
        .iter()
        .map(|t| match t {
            TransformStep::Filter { pred } => TransformStep::Filter {
                pred: substitute_list_params_expr(list_env, pred),
            },
            TransformStep::Derive { name, expr } => TransformStep::Derive {
                name: name.clone(),
                expr: substitute_list_params_expr(list_env, expr),
            },
            TransformStep::Project { .. }
            | TransformStep::GroupBy { .. }
            | TransformStep::Join { .. }
            | TransformStep::Window { .. }
            | TransformStep::Pivot { .. }
            | TransformStep::Unpivot { .. }
            | TransformStep::Sort { .. }
            | TransformStep::Distinct
            | TransformStep::Limit { .. }
            | TransformStep::Union { .. }
            | TransformStep::Intersect { .. }
            | TransformStep::Except { .. } => t.clone(),
        })
        .collect()
}

#[cfg(test)]
mod power_tests {
    //! Phase 2400: the special-value rows the wire cannot carry (a NaN or an
    //! infinity, so the law file cannot), the pinned product tree past the
    //! cube, and the int32 band an int power stays inside.
    use super::*;

    fn is_nan(c: &Cell) -> bool {
        matches!(c, Cell::Float(f) if f.is_nan())
    }

    #[test]
    fn the_rows_off_the_wire() {
        let (nan, inf) = (f64::NAN, f64::INFINITY);
        assert!(is_nan(&pow_float(1.0, nan)));
        assert!(is_nan(&pow_float(nan, 0.0)));
        assert_eq!(pow_float(-1.0, inf), Cell::Float(1.0));
        assert_eq!(pow_float(-1.0, -inf), Cell::Float(1.0));
        assert!(exp_host(nan).is_nan());
        assert_eq!(exp_host(709.8), inf);
        assert_eq!(log_host(f64::ln, inf), Cell::Float(inf));
        assert!(is_nan(&log_host(f64::log10, nan)));
        let pow = |a: Cell, b: Cell| apply_scalar(ScalarFn::Pow, &[a, b]);
        assert!(is_nan(&pow(Cell::Float(nan), Cell::Int(0)).unwrap()));
        assert_eq!(pow(Cell::Float(1e-300), Cell::Int(-2)).unwrap(), NULL);
        assert!(pow(Cell::Int(46_341), Cell::Int(2)).is_err());
        assert_eq!(
            pow(Cell::Int(-2), Cell::Int(31)).unwrap(),
            Cell::Int(-2_147_483_648)
        );
        let x = 1.1_f64;
        assert_eq!(pow_go(x, 4).to_bits(), ((x * x) * (x * x)).to_bits());
        assert_eq!(pow_go(x, 5).to_bits(), (x * ((x * x) * (x * x))).to_bits());
    }
}
