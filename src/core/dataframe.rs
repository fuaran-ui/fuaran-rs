//! The dataframe model — the typed cells, schema, column expressions and
//! transform steps the Transform evaluator folds over. The Core twin of the
//! reference `Fuaran.Core.DataFrame` model; the wire codec for these types stays
//! in `crate::wire`, which re-exports them, so every `fuaran_rs::wire::` path is
//! unchanged.

use super::bare_enum;

// ─── Compute layer (Phases 282/284/424) ──────────────────────────────────────

// The three sub-second timestamp units are appended after `timestamp` (Phase
// 2206, fuaran-core#422), so the spelling list keeps its earlier prefix.
bare_enum!(ColumnType { Int => "int", Float => "float", Bool => "bool", Str => "string", Date => "date", Timestamp => "timestamp", TimestampMs => "timestamp_ms", TimestampUs => "timestamp_us", TimestampNs => "timestamp_ns" });

impl ColumnType {
    /// The timestamp family (WIRE_FORMAT §3.6.25): the fraction digits a
    /// timestamp unit holds — 0, 3, 6 or 9 — or `None` for a type that is not a
    /// timestamp. Every unit carries one canonical instant text per instant, so
    /// a timestamp cell is `Cell::Timestamp` whichever unit its column declares;
    /// a coarser unit widens into a finer one.
    pub fn timestamp_digits(self) -> Option<u32> {
        match self {
            ColumnType::Timestamp => Some(0),
            ColumnType::TimestampMs => Some(3),
            ColumnType::TimestampUs => Some(6),
            ColumnType::TimestampNs => Some(9),
            _ => None,
        }
    }

    /// The coarsest timestamp unit holding `digits` fraction digits exactly.
    pub fn timestamp_of_digits(digits: u32) -> ColumnType {
        match digits {
            0 => ColumnType::Timestamp,
            1..=3 => ColumnType::TimestampMs,
            4..=6 => ColumnType::TimestampUs,
            _ => ColumnType::TimestampNs,
        }
    }
}

/// The fraction digits of a CANONICAL instant text (`YYYY-MM-DDThh:mm:ssZ`, or
/// `YYYY-MM-DDThh:mm:ss.FZ` with one to nine digits and no trailing zero,
/// naming a moment that exists), or `None` for any other text.
pub fn instant_fraction_digits(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    let n = b.len();
    if n < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |at: usize, len: usize| -> Option<u32> {
        b[at..at + len].iter().try_fold(0u32, |acc, &c| {
            c.is_ascii_digit().then(|| acc * 10 + u32::from(c - b'0'))
        })
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, se) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in = match mo {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=12).contains(&mo) || d < 1 || d > days_in || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    if n == 20 {
        return (b[19] == b'Z').then_some(0);
    }
    if n > 30 || b[19] != b'.' || b[n - 1] != b'Z' || b[n - 2] == b'0' {
        return None;
    }
    b[20..n - 1]
        .iter()
        .all(u8::is_ascii_digit)
        .then(|| (n - 21) as u32)
}
bare_enum!(BinOp { Add => "add", Sub => "sub", Mul => "mul", Div => "div", Mod => "mod", Eq => "eq", Ne => "ne", Lt => "lt", Le => "le", Gt => "gt", Ge => "ge", And => "and", Or => "or", Contains => "contains", StartsWith => "startsWith", EndsWith => "endsWith" });
bare_enum!(ScalarFn { Abs => "abs", Round => "round", Floor => "floor", Ceil => "ceil", Length => "length", Lower => "lower", Upper => "upper", Substr => "substr", DatePart => "datePart", Concat => "concat", Trim => "trim", Replace => "replace", DateDiffDays => "dateDiffDays" });
bare_enum!(AggFn { Sum => "sum", Mean => "mean", Min => "min", Max => "max", Count => "count", Median => "median", Stddev => "stddev", First => "first", Last => "last" });
bare_enum!(JoinKind { Inner => "inner", Left => "left", Right => "right", Outer => "outer" });
// `cumulSum` is the canonical tag (operator rename 2026-07-19); the legacy
// `cumSum` spelling decodes as a lenient alias and normalises on re-encode.
bare_enum!(WindowFn { RowNumber => "rowNumber", Rank => "rank", Lag => "lag", Lead => "lead", CumulSum => "cumulSum", RollingMean => "rollingMean" });
bare_enum!(SortDir { Asc => "asc", Desc => "desc" });

/// A typed dataframe cell.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Date(String),
    Timestamp(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaEntry {
    pub name: String,
    pub column_type: ColumnType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataColumn {
    pub name: String,
    pub column_type: ColumnType,
    pub cells: Vec<Cell>,
}

/// A columnar data source: embedded `{schema, columns}` or host-resolved `ref`.
#[derive(Debug, Clone, PartialEq)]
pub enum DataSource {
    Ref {
        name: String,
    },
    Embedded {
        schema: Vec<SchemaEntry>,
        columns: Vec<DataColumn>,
    },
}

/// A scalar column expression of the dataframe algebra.
#[derive(Debug, Clone, PartialEq)]
pub enum ColExpr {
    Col {
        name: String,
    },
    Param {
        name: String,
    },
    Lit {
        cell: Cell,
    },
    Binary {
        op: BinOp,
        left: Box<ColExpr>,
        right: Box<ColExpr>,
    },
    Not {
        expr: Box<ColExpr>,
    },
    Coalesce {
        exprs: Vec<ColExpr>,
    },
    Case {
        cases: Vec<CaseArm>,
        else_expr: Box<ColExpr>,
    },
    Cast {
        column_type: ColumnType,
        expr: Box<ColExpr>,
    },
    Apply {
        func: ScalarFn,
        args: Vec<ColExpr>,
    },
    /// SQL three-valued membership over a literal list (Phase 91).
    InList {
        subject: Box<ColExpr>,
        items: Vec<ColExpr>,
    },
    /// Membership over a bound multi-select list param — resolves by
    /// substitution to `InList` before evaluation; one that reaches the
    /// evaluator is unbound (the scalar-`Param` strictness).
    InParam {
        subject: Box<ColExpr>,
        name: String,
    },
    IsNull {
        expr: Box<ColExpr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseArm {
    pub when: ColExpr,
    pub then: ColExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColPair {
    pub a: String,
    pub b: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Agg {
    pub name: String,
    pub func: AggFn,
    pub of: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub col: String,
    pub dir: SortDir,
}

/// One dataframe transform step.
#[derive(Debug, Clone, PartialEq)]
pub enum TransformStep {
    Filter {
        pred: ColExpr,
    },
    Project {
        cols: Vec<ColPair>,
    },
    Derive {
        name: String,
        expr: ColExpr,
    },
    GroupBy {
        keys: Vec<String>,
        aggs: Vec<Agg>,
    },
    Join {
        source: DataSource,
        on: Vec<ColPair>,
        how: JoinKind,
    },
    Window {
        partition_by: Vec<String>,
        order_by: Vec<SortKey>,
        func: WindowFn,
        of: String,
        alias: String,
    },
    Pivot {
        index: Vec<String>,
        on: String,
        values: String,
        agg: AggFn,
    },
    Unpivot {
        id_vars: Vec<String>,
        value_vars: Vec<String>,
    },
    Sort {
        by: Vec<SortKey>,
    },
    Distinct,
    Limit {
        n: i64,
        offset: i64,
    },
    Union {
        source: DataSource,
    },
}
