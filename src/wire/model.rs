//! The typed wire tree — every closed wire DU modelled as a native Rust `enum`,
//! one variant per `$type` case. This is the Rust host's distinctive reach: a
//! per-kind `match` in the codec is **compile-time exhaustive**, so a
//! newly-added kind that misses a codec arm is a build error, not a corpus-only
//! catch. No codec `match` over these enums takes a `_ =>` catch-all arm.
//!
//! The model is **storage-shape erased** (`WIRE_FORMAT.md` §1): the wire carries
//! no typed-message information — every function-typed payload encodes as the
//! `"<closure>"` sentinel (§4). A closure-bearing slot is therefore modelled as
//! presence only: [`Closure`] marks "a closure rode the wire here", and decodes
//! /re-encodes to the same sentinel, keeping the round-trip byte-stable.

use std::collections::BTreeMap;

use crate::canonical::JVal;

/// A node identity string (non-empty by decode invariant, §8).
pub type NodeId = String;

/// Marks a closure-bearing slot whose function value was present on the wire.
/// The value itself is unobservable (§4) — only presence round-trips, as the
/// `"<closure>"` sentinel. Slots whose closure is *always* emitted carry no
/// field at all; `Option<Closure>` models the optional-handler slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closure;

// ─── Bare-string enums (§3.5) ────────────────────────────────────────────────

macro_rules! bare_enum {
    ($(#[$doc:meta])* $name:ident { $($case:ident => $wire:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($case),+
        }

        impl $name {
            /// The wire-stable string form.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$case => $wire),+
                }
            }

            /// Every valid wire spelling, for `UNKNOWN_DU_CASE` hints.
            pub const WIRE_NAMES: &'static [&'static str] = &[$($wire),+];

            /// Parse the wire spelling; `None` on an unknown case.
            pub fn from_wire(s: &str) -> Option<Self> {
                match s {
                    $($wire => Some($name::$case),)+
                    _ => None,
                }
            }
        }
    };
}

bare_enum!(Orientation { Vertical => "Vertical", Horizontal => "Horizontal" });
bare_enum!(ScrollOrientation { Vertical => "Vertical", Horizontal => "Horizontal", Both => "Both" });
bare_enum!(BadgeVariant { Neutral => "Neutral", Brand => "Brand", Success => "Success", Warning => "Warning", Critical => "Critical", Info => "Info" });
bare_enum!(ButtonVariant { Primary => "Primary", Secondary => "Secondary", Tertiary => "Tertiary", Destructive => "Destructive" });
bare_enum!(HeadingVariant { Standard => "Standard", Eyebrow => "Eyebrow", Caption => "Caption", Lead => "Lead" });
bare_enum!(ToneVariant { Default => "Default", Subdued => "Subdued", Brand => "Brand", Success => "Success", Warning => "Warning", Critical => "Critical", Info => "Info" });
bare_enum!(StyleWeight { Compact => "Compact", Standard => "Standard", Spacious => "Spacious" });
bare_enum!(Emphasis { Quiet => "Quiet", Normal => "Normal", Loud => "Loud" });
bare_enum!(TextAnchor { Start => "Start", Middle => "Middle", End => "End" });
bare_enum!(StyleRole { None => "None", Eyebrow => "Eyebrow", Data => "Data", Lede => "Lede", Caption => "Caption" });
bare_enum!(FontVoice { Default => "Default", Display => "Display", Structural => "Structural" });
bare_enum!(ChartKind { Line => "Line", Bar => "Bar", Area => "Area", Pie => "Pie", Scatter => "Scatter", Heatmap => "Heatmap" });
// Phase 880 — which edge the chart's legend occupies, or `None`, which suppresses
// it entirely. A WIRE vocabulary on the same side of the D8 line as `title`: WHERE
// an author wants the legend is their meaning, where the geometry that puts it
// there (column widths, pitches, how the plot shrinks) stays the host's, in the
// lowering's `ChartLowerStyle`. Absent means "the host default" (`Right`), NOT "no
// legend" — suppression is the explicit `None`.
bare_enum!(ChartLegendPosition { Top => "Top", Right => "Right", Bottom => "Bottom", None => "None" });
// Phase 881 — whether a chart writes its values onto the picture, and where. A
// WIRE vocabulary on the semantic side of the D8 line: whether the reader is
// meant to READ THE NUMBERS or read the shape is the author's meaning; the type
// size, the offsets and the fit rule that realise it stay the host's, in the
// lowering. THE CASE SET IS TWO, AND THAT IS THE POINT — there is deliberately
// no all-points case, so no shape of this enum can request a number on every
// interior point. Absent means `Off`, which is also the default.
bare_enum!(ChartDataLabels { Off => "Off", Ends => "Ends" });
// Phase 882 — what a chart's x column MEANS: discrete `Category` bands or
// `Temporal` dates read on a continuous day-scale. A WIRE vocabulary on the
// semantic side of the D8 line: whether a column is a set of categories or a run
// of dates is a fact about the data the author is DECLARING; where the ticks
// land, how they are formatted and how much margin they need are the host's, in
// the lowering. DECLARED, NEVER INFERRED — the chart's schema is statically known
// only for an embedded table with an empty pipeline, so an inferred axis would
// make one wire tree draw a band axis or a temporal one depending on where its
// rows came from, and sniffing the cell strings for an ISO-8601 shape is a guess
// dressed as a rule. The declaration is GROUNDED instead (FUARAN097 refuses a
// temporal axis over a non-date column). Absent means `Category`, which is also
// the default, so every pre-882 chart is byte-identical on the wire and in the
// picture.
bare_enum!(ChartXScale { Category => "Category", Temporal => "Temporal" });
bare_enum!(ImageVariant { Default => "Default", Avatar => "Avatar", Rounded => "Rounded" });
// Phase 1077 — the three `Image` presentation slots (WIRE_FORMAT.md §3.6.2).
// TOKENS, never CSS values: `aspectRatio` names one of four ratios and cannot
// carry `"16 / 9"`, a pair, or a number, which is the free-form escape this
// format does not have. Each is omitted at its identity default on BOTH
// boundaries, so a pre-1077 document decodes to today's behaviour and
// re-encodes to the bytes it already had.
//
// `fit` says how the decoded pixels fill the box; `aspectRatio` says what box
// is RESERVED before the bytes land. Neither is derived from the other — a host
// stylesheet may size the element some other way — which is why they are two
// independently-declarable slots rather than one.
bare_enum!(ImageFit { Natural => "Natural", Cover => "Cover", Contain => "Contain" });
bare_enum!(ImageAspect {
    Natural => "Natural", Square => "Square", FourThree => "FourThree",
    ThreeTwo => "ThreeTwo", SixteenNine => "SixteenNine"
});
// `Eager` is the default DELIBERATELY, and it is not the "unoptimised" value:
// deferring an above-the-fold image delays the largest contentful paint rather
// than helping it, and only the author knows where the image sits. So `Eager`
// emits no attribute at all and leaves the browser's own default in place,
// while `Lazy` is a positive declaration. A host MUST NOT infer laziness from
// position, viewport, or anything else the tree does not say.
bare_enum!(ImageLoading { Eager => "Eager", Lazy => "Lazy" });
// Anti-scraper render strategy for a `Link` — `email` marks a `mailto:` link
// whose address must not appear in plaintext in emitted markup (the renderers
// own the emission strategy). Lower-case on the wire, like `SortDirection`.
bare_enum!(LinkProtection { Email => "email" });
bare_enum!(MathDisplay { Inline => "Inline", Block => "Block" });
bare_enum!(DateVariant { Date => "Date", Time => "Time", DateTime => "DateTime" });
bare_enum!(FileReadEncoding { Text => "Text", Base64 => "Base64", DataUrl => "DataUrl" });
// `FormFieldKind` names the CONTROL; `FormField.rule` names the ACCEPTED SET.
bare_enum!(TextFormat { Email => "email", Url => "url", Tel => "tel" });
bare_enum!(CompareOp {
    Eq => "eq", Neq => "neq", Lt => "lt", Lte => "lte", Gt => "gt", Gte => "gte"
});
bare_enum!(LiveRegionKind { Polite => "polite", Assertive => "assertive", Off => "off" });
// Phase 801 — the closed sort direction on `staticRows.defaultSort`. Lower-case on
// the wire, like `LiveRegionKind` and unlike most enums here.
bare_enum!(SortDirection { Asc => "asc", Desc => "desc" });
bare_enum!(DateStyle { Short => "Short", Medium => "Medium", Long => "Long", Full => "Full" });
bare_enum!(RelativeTimeUnit { Second => "Second", Minute => "Minute", Hour => "Hour", Day => "Day", Week => "Week", Month => "Month", Year => "Year" });
// Phase 819 — the Duration format enums (`Format.Duration` / `CellFormat.Duration`):
// the numeric source counts `unit`s, rendered per the bounded `style`.
bare_enum!(DurationUnit { Seconds => "Seconds", Minutes => "Minutes", Hours => "Hours" });
bare_enum!(DurationStyle { Compact => "Compact", Clock => "Clock", Long => "Long" });
// Phase 821 — the standalone `Icon` display kind's size modifier.
bare_enum!(IconSize { Small => "Small", Medium => "Medium", Large => "Large" });
// Phase 867 — which way a `Metric`'s quantity IMPROVES (WIRE_FORMAT.md §3.6.1).
// Distinct from `tone`, which says how the reading STANDS: a host derives neither
// from the other, and nothing here ever writes back to `tone`. A renderer
// composes `sentiment = sign(trend) × polarity` and paints the TREND ELEMENT
// alone; the numeric text, sign included, is untouched. Absent means
// `HigherIsBetter`, so every pre-867 `Metric` is byte-identical on the wire.
//
// `Neutral` IS RESERVED AND IS DELIBERATELY NOT A CASE, and its absence is the
// whole reason this slot is an enum rather than `inverted: bool` — a later
// admission is then a bare-string ADDITION, not a type replacement. Modelling
// the reservation as absence (rather than as a case the decoder refuses) is what
// keeps that promise honest in this host specifically: the case set IS the
// accepted wire set, so `"Neutral"` fails `UNKNOWN_DU_CASE` against the two
// canonical spellings, `WIRE_NAMES` cannot advertise a spelling the format does
// not accept, and no `match` anywhere in the crate carries a dead arm waiting for
// a case the wire will not produce. Admitting it later is one line here and a
// compiler error at every site that must then decide what it means — which is
// exactly the exhaustiveness property this host exists to demonstrate.
// NO alias arm is registered for it: an alias would ACCEPT the reserved spelling
// under a canonical case, silently deciding the question the reservation holds open.
bare_enum!(TrendPolarity { HigherIsBetter => "HigherIsBetter", LowerIsBetter => "LowerIsBetter" });
bare_enum!(HashStrictness { StrictReplay => "StrictReplay", AdvisoryWarning => "AdvisoryWarning", Enforced => "Enforced" });
bare_enum!(BoxRole { Group => "Group", Card => "Card", Dashboard => "Dashboard", Separator => "Separator" });
bare_enum!(HostEffect { Pure => "Pure", ReadsHost => "ReadsHost", WritesHost => "WritesHost" });
bare_enum!(DeterminismSource { Deterministic => "Deterministic", Clock => "Clock", Random => "Random", Network => "Network" });
bare_enum!(ChannelDirection { OutOnly => "OutOnly", TwoWay => "TwoWay" });

// ─── Text / bindings / actions ───────────────────────────────────────────────

/// A text-producing position (`§3.3`).
#[derive(Debug, Clone, PartialEq)]
pub enum TextSource {
    Literal(String),
    Bound(Box<Binding>),
    /// `args` is a structured-JSON map (rule 12) — always emitted, `{}` when empty.
    I18n {
        key: String,
        args: Vec<(String, JVal)>,
    },
}

/// One grid / chart row: an *open* name→cell map (unlike a fixed-field record),
/// so a row is name-addressable — the shape the declarative column floor
/// (`field`, `rowKeyField`) has always assumed. Cells stay faithful `JVal`s; the
/// residual-`"<opaque>"` boundary is narrowed to the CELL, where a nested array
/// or object re-encodes as the sentinel (`obj_value`).
pub type Row = Vec<(String, JVal)>;

/// A `Binding.Static` / `Binding.State` payload. The language-enumerated slot
/// shapes (Phase 429) are typed variants; everything else rides the faithful
/// parsed value (`Ast`), whose non-primitive forms re-encode as `"<opaque>"`
/// (§5 — the residual-opaque boundary, by design).
#[derive(Debug, Clone, PartialEq)]
pub enum StaticValue {
    /// The untyped slot payload: the faithful parsed value. Primitives (string
    /// / bool / number / null) re-encode natively; arrays / objects collapse to
    /// the `"<opaque>"` sentinel per §2 rule 11.
    Ast(JVal),
    /// `SelectOption list` slots (options sources).
    Options(Vec<SelectOption>),
    /// `string option` slots (choice values); `None` re-encodes as `null`.
    StringOpt(Option<String>),
    /// `string list` slots (multi-select values).
    StringList(Vec<String>),
    /// `float seq` slots (sparkline series).
    FloatSeq(Vec<f64>),
    /// `MapMarker seq` slots (map sources).
    Markers(Vec<MapMarker>),
    /// `Row seq` slots (grid / chart row sources) — fuaran#665 moved these off
    /// the residual-`"<opaque>"` boundary. An empty feed encodes `[]`, never
    /// `null`; the legacy sentinel decodes to the empty feed indefinitely.
    Rows(Vec<Row>),
    /// `float * float` slots (the 0.2.0 `FormFieldKind.Range` dual-thumb pair).
    /// The canonical `Static` pair rides as the BARE `{"max":…,"min":…}` object
    /// — no `Static` envelope (the Phase 423 range shape, kept at 0.2.0).
    FloatPair(f64, f64),
    /// `string * string` slots (the 0.7.0 `FormFieldKind.DateRange` ISO pair).
    /// The canonical `Static` pair rides as the BARE `{"from":…,"to":…}` object
    /// — no `Static` envelope (exactly the `FloatPair` posture above). The pair
    /// is ORDERED: a literal `from` sorting after its `to` is a decode error.
    StringPair(String, String),
}

/// A value binding (`§3.3`), one variant per wire `$type` case.
#[derive(Debug, Clone, PartialEq)]
pub enum Binding {
    Static {
        value: StaticValue,
    },
    Query {
        name: String,
        /// Phase 421 — the declared filter edge; omitted when empty.
        depends_on: Option<Vec<String>>,
    },
    Filter {
        name: String,
        /// 0.2.0 — the value the resolver yields (and the renderer seeds the
        /// store with) before the filter is first written. Typed via the
        /// slot's static encoding; omitted when `None`.
        default_value: Option<StaticValue>,
    },
    Selection {
        node_id: NodeId,
        /// 0.2.9 (Phase 629) — the `Filter.defaultValue` convention: yielded
        /// until the user first selects a row on `nodeId`. Omitted when `None`.
        default_value: Option<StaticValue>,
        /// 0.2.10 (Phase 632) — the declarative row-field projection: present
        /// ⇒ the accessor projects that field off the clicked row. Omitted
        /// when `None`.
        field: Option<String>,
    },
    State {
        key: String,
        default_value: StaticValue,
    },
    Computed,
    /// Phase 765 — the host-furnished current instant: the bare
    /// `{"$type":"Now"}` object, no wire fields. Resolved once per render pass
    /// from the host's pinned ISO-8601 string — never a clock read in the
    /// renderer, so SSR output stays reproducible for a pinned instant.
    Now,
    I18n {
        key: String,
        /// Per-argument bindings; omitted when absent.
        args: Option<Vec<(String, Binding)>>,
    },
    Local {
        flush_on: LocalFlushTrigger,
        initial_from: Box<Binding>,
    },
    Format {
        format: Format,
        locale: LocaleSource,
        source: Box<Binding>,
    },
    Transform {
        /// Phase 424 — omitted when empty.
        params: Option<Vec<TransformParam>>,
        pipeline: Vec<TransformStep>,
        source: TransformSource,
    },
    Invoke {
        capability_id: String,
        args: Vec<InvokeArg>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalFlushTrigger {
    OnBlur,
    OnSubmit,
    OnCommitAction,
    OnDebounce { milliseconds: i64 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Format {
    Number {
        decimals: Option<i64>,
    },
    Currency {
        iso_code: String,
    },
    Percent {
        decimals: Option<i64>,
    },
    Date {
        date_style: DateStyle,
    },
    RelativeTime {
        unit: RelativeTimeUnit,
    },
    /// Phase 819 — locale-independent duration formatting.
    Duration {
        style: DurationStyle,
        unit: DurationUnit,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocaleSource {
    Ambient,
    Explicit { tag: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransformParam {
    pub name: String,
    pub from: Binding,
}

/// Phase 818 — a `Binding::Transform`'s source slot (mirror of the reference
/// host's `TransformSource` DU). `Data` is the canonical columnar / `ref`
/// source (the pre-818 shape, byte-identical on the wire). `Live` preserves a
/// binding-shaped source (State / Selection / Query) verbatim so a runtime
/// re-evaluates the Transform with subscription semantics when the binding's
/// channel changes; `initial` is the decode-time snapshot table derived from
/// the binding's carried default data (never encoded — the binding IS the
/// wire form), which SSR / diagnostic evaluation reads, byte-identical to the
/// Phase-815 snapshot for the same input.
#[derive(Debug, Clone, PartialEq)]
pub enum TransformSource {
    Data(DataSource),
    Live {
        binding: Box<Binding>,
        initial: DataSource,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvokeArg {
    pub addr: String,
    pub value: String,
}

/// The declarative result target of an `Action.Call` (Phase 428).
#[derive(Debug, Clone, PartialEq)]
pub enum CallResultTarget {
    State { key: String },
    Query { name: String },
}

/// A user-interaction action (`§3.3`), one variant per wire `$type` case.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// The whole message payload is a closure (§4) — presence only.
    Dispatch,
    Call {
        endpoint: String,
        into: Option<CallResultTarget>,
        on_result: Option<Closure>,
    },
    /// `payload` is a structured-JSON position (rule 12).
    Notify {
        channel: String,
        payload: JVal,
    },
    Navigate {
        route: String,
    },
    /// Phase 818 — `value` (a literal, written verbatim) XOR `value_from` (a
    /// Binding evaluated at dispatch time inside the existing gate); decode
    /// enforces exactly one present.
    SetState {
        key: String,
        value: Option<JVal>,
        value_from: Option<Box<Binding>>,
    },
    AiTool {
        tool_name: String,
        args: JVal,
    },
    Chain(Vec<Action>),
    CommitLocal {
        node_id: NodeId,
    },
    WriteToClipboard {
        text: String,
    },
    /// Only the file id + encoding cross the wire; the blob and `onRead` do not.
    ReadFileBody {
        file_ref: String,
        encoding: FileReadEncoding,
    },
    Invoke {
        capability_id: String,
        args: Vec<InvokeArg>,
    },
}

// ─── Shared spec fragments ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: TextSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapMarker {
    pub label: TextSource,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CellFormat {
    None,
    Number {
        decimals: Option<i64>,
    },
    Currency {
        code: String,
    },
    Percent {
        decimals: Option<i64>,
    },
    SignificantDigits {
        digits: i64,
    },
    Date {
        format: String,
    },
    /// Phase 819 — trendable duration cells: raw float counts `unit`s,
    /// rendered per `style`.
    Duration {
        style: DurationStyle,
        unit: DurationUnit,
    },
    /// Phase 819 — cell-vocabulary parity with `Format::RelativeTime`.
    RelativeTime {
        unit: RelativeTimeUnit,
    },
    /// The formatter is a closure (§4) — presence only.
    Custom,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnWidth {
    Auto,
    Fixed { pixels: i64 },
    Flex { weight: f64 },
}

// ─── Compute layer (Phases 282/284/424) ──────────────────────────────────────

bare_enum!(ColumnType { Int => "int", Float => "float", Bool => "bool", Str => "string", Date => "date", Timestamp => "timestamp" });
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

// ─── Display specs ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct HeadingSpec {
    pub level: i64,
    pub text: TextSource,
    pub variant: HeadingVariant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownSpec {
    pub text: TextSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricSpec {
    pub label: TextSource,
    /// 0.2.0 rename law — a *scalar displayed value* is named `value` on the
    /// wire (`source` is reserved for collection feeds). The retired
    /// `Metric.source` spelling is a hard decode error, not an alias.
    pub value: Binding,
    pub format: CellFormat,
    pub tone: ToneVariant,
    pub weight: StyleWeight,
    pub emphasis: Emphasis,
    pub trend: Option<Binding>,
    pub trend_format: Option<CellFormat>,
    /// Phase 867 — which way this quantity IMPROVES (§3.6.1). Omitted-when-
    /// `HigherIsBetter` on the wire, like the other stylistic slots above, so
    /// the field is a total `TrendPolarity` rather than an `Option`: the wire's
    /// "absent means `HigherIsBetter`" is a DEFAULT, not a third state, and
    /// modelling it as `None` would put a decision back into every reader.
    ///
    /// The slot is inert without `trend` — a `Metric` that declares a polarity
    /// and carries no trend is legal and says nothing (§3.6.1 clause 4).
    pub trend_polarity: TrendPolarity,
    pub icon: Option<String>,
    pub subtext: Option<TextSource>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BadgeSpec {
    pub label: TextSource,
    pub variant: BadgeVariant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SparklineSpec {
    pub source: Binding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalloutSpec {
    pub body: TextSource,
    pub dismissable: bool,
    pub tone: ToneVariant,
    pub heading: Option<TextSource>,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgressSpec {
    pub fraction: Binding,
    pub indeterminate: bool,
    pub tone: ToneVariant,
    pub label: Option<TextSource>,
    pub caveat: Option<TextSource>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonSpec {
    pub rows: i64,
}

/// Phase 821 — the standalone icon-only display kind. `size`
/// omitted-when-`Medium`, `tone` omitted-when-`Default`, `label`
/// omitted-when-decorative (`None` ⇒ `aria-hidden`).
#[derive(Debug, Clone, PartialEq)]
pub struct IconSpec {
    pub icon: String,
    pub size: IconSize,
    pub tone: ToneVariant,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelValueRowSpec {
    pub label: TextSource,
    /// 0.2.0 rename law — scalar displayed value ⇒ `value` (see [`MetricSpec`]).
    pub value: Binding,
    pub format: CellFormat,
    /// The behavioural bool (not the `Emphasis` style DU). 0.2.2 — omitted on
    /// the wire when `false`, aligning with `Fact`'s identical flag.
    pub emphasis: bool,
    pub help: Option<TextSource>,
}

/// The labeled TEXT fact — `Metric`'s complementary type. `value` is a
/// [`TextSource`] (the same vocabulary the labels use, so `Bound` / `I18n`
/// values ride through); `emphasis` is the behavioural bool (omit-when-false),
/// `tone` omit-when-default; `help` / `icon` optional.
#[derive(Debug, Clone, PartialEq)]
pub struct FactSpec {
    pub label: TextSource,
    pub value: TextSource,
    pub emphasis: bool,
    pub tone: ToneVariant,
    pub help: Option<TextSource>,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkSpec {
    pub href: Binding,
    pub label: TextSource,
    pub download: bool,
    pub rel: Option<String>,
    pub target: Option<String>,
    /// Omitted on the wire when `None` (every pre-existing tree byte-identical).
    pub protection: Option<LinkProtection>,
}

/// Phase 1080 — one `srcSet` candidate (§3.6.4): an alternate rendition of the
/// SAME picture at a declared intrinsic pixel width. Both members are required
/// *within* the entry, and `width` is a POSITIVE integer — the `w` descriptor a
/// client selects on, so `0` is refused as firmly as a negative: a `0w`
/// candidate is not a small image, it is one a client can never select.
#[derive(Debug, Clone, PartialEq)]
pub struct SrcSetEntry {
    pub src: Binding,
    pub width: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageSpec {
    pub alt: TextSource,
    pub src: Binding,
    pub variant: ImageVariant,
    /// Phase 1077 (§3.6.2) — omitted at `Natural` on both boundaries.
    pub fit: ImageFit,
    /// Phase 1077 (§3.6.2) — omitted at `Natural` on both boundaries.
    pub aspect_ratio: ImageAspect,
    /// Phase 1077 (§3.6.2) — omitted at `Eager` on both boundaries.
    pub loading: ImageLoading,
    /// Phase 1078 (§3.6.3) — optional CONTENT, not an identity default, so it
    /// takes the ordinary optional-field posture: omitted when absent. It is a
    /// full [`TextSource`] and not a string, which is the rule a second host is
    /// most likely to break — narrowing the slot costs nothing until somebody
    /// needs a locale.
    pub caption: Option<TextSource>,
    /// Phase 1080 (§3.6.4) — the missing-list-field decode class: an ABSENT
    /// `srcSet` IS the empty list, never `None`, and a present `null` is
    /// refused. Absence already has a spelling, and a second one would let two
    /// conformant hosts emit different canonical bytes for one document. The
    /// AUTHORED ORDER is preserved here and on encode; ascending-by-width is
    /// the RENDERER's canonicalisation, not the codec's.
    pub src_set: Vec<SrcSetEntry>,
    /// Phase 1079 (§3.6.5) — omitted at `false`. Declares that the full-size
    /// asset is REACHABLE from the rendered image (a real anchor), not that a
    /// lightbox appears; nothing here crosses the dispatch gate.
    pub expandable: bool,
}

/// Phase 1076 — which media surface a [`MediaSpec`] is (§3.6.6). A
/// `$type`-DISCRIMINATED union nested at `kind.kind`, so an unknown case
/// reports at `$.kind.kind.$type` rather than at the bare slot.
///
/// **`Audio` has no autoplay pathway — in the type, on the wire, or in the
/// emission.** That is stronger than a default of `false`: a slot defaulting to
/// off is one a document can switch on, and there is no document this format
/// wants to be able to state in which a page begins making sound unbidden. The
/// case declares no such field, so `{"$type":"Audio","autoplay":true}` decodes
/// to an audio surface that does not autoplay — the value has nowhere to land,
/// and no renderer arm below can acquire a branch for it by a later edit.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaKind {
    Video {
        /// A DECLARATION whose rendering is constrained: a host that honours it
        /// MUST emit it together with a muted attribute (§3.6.6). There is
        /// deliberately no separate `muted` slot on the wire — a second knob
        /// free to disagree with the first would add only the combination no
        /// host may render.
        autoplay: bool,
        /// A second URL through the §19 floor; a REFUSED one is dropped rather
        /// than neutered (a `<video>` with no poster shows its first frame,
        /// which is a working rendering).
        poster: Option<Binding>,
    },
    Audio,
}

/// Phase 1076 — the playback surface (§3.6.6). ONE kind, two variants: every
/// slot a video surface and an audio surface share is stated once here, and
/// only the genuinely-differing slots live in the [`MediaKind`] variant.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaSpec {
    pub src: Binding,
    /// REQUIRED, and the one place the media contract differs from `Image`'s.
    /// An image can honestly be decorative and say so with an empty `alt`; a
    /// media element is a TRANSPORT — a control a reader focuses, plays, pauses
    /// and seeks — so it is never decorative, and a document omitting it is
    /// refused rather than defaulted to a fabricated name.
    pub label: TextSource,
    /// Omitted at TRUE on the wire (the second such slot in the vocabulary,
    /// after `Toast.dismissable`): a media element without a transport cannot
    /// be paused, seeked or muted by a keyboard user at all, so the accessible
    /// setting is what a document gets for free.
    pub controls: bool,
    /// Ordinary polarity — omitted at `false`. Named `r#loop` so the field and
    /// the wire key are the same word.
    pub r#loop: bool,
    pub kind: MediaKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListSpec {
    pub items: Vec<TextSource>,
    pub ordered: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToastSpec {
    pub message: TextSource,
    pub tone: ToneVariant,
    pub open: Binding,
    pub dismissable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeBlockSpec {
    pub code: String,
    pub language: String,
    pub line_numbers: bool,
    pub highlight_lines: Vec<i64>,
    pub copyable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MathSpec {
    pub source: String,
    pub display: MathDisplay,
}

// ─── Input specs ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum FormFieldKind {
    Text {
        value: Binding,
        on_change: Option<Closure>,
    },
    Number {
        value: Binding,
        on_change: Option<Closure>,
    },
    Checkbox {
        value: Binding,
        on_toggle: Option<Closure>,
    },
    /// Phase 766 — the switch affordance: `Checkbox`'s exact wire mechanics
    /// (bool value slot, optional `onToggle`, the write-back default) under a
    /// distinct tag, so the renderer can announce `role="switch"` semantics.
    Toggle {
        value: Binding,
        on_toggle: Option<Closure>,
    },
    Choice {
        options: Binding,
        value: Binding,
        on_change: Option<Closure>,
    },
    RangedNumber {
        value: Binding,
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
        on_change: Option<Closure>,
    },
    /// 0.2.0 — the dual-thumb numeric range control (absorbed the retired
    /// `FilterKind.RangeFilter`). A `Static` pair rides as the bare
    /// `{"max":…,"min":…}` object; optional bounds omitted when absent.
    Range {
        value: Binding,
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
        on_change: Option<Closure>,
    },
    SegmentedChoice {
        options: Binding,
        orientation: Orientation,
        value: Binding,
        on_change: Option<Closure>,
    },
    TextArea {
        rows: i64,
        value: Binding,
        on_change: Option<Closure>,
    },
    Date {
        value: Binding,
        variant: DateVariant,
        min: Option<String>,
        max: Option<String>,
        step: Option<f64>,
        on_change: Option<Closure>,
    },
    /// 0.7.0 — the single-control date range: `Range`'s pair mechanics with
    /// `Date`'s value conventions (an identical field list to `Date`). A `Static`
    /// pair rides as the bare `{"from":…,"to":…}` object; `variant` is always
    /// emitted; `min`/`max` (ISO strings) and `step` (seconds) bound BOTH ends and
    /// are omitted when absent. In a filter context the pair binds ONE filter
    /// param, not two — the reason the case exists rather than two coordinated
    /// `Date` fields.
    DateRange {
        value: Binding,
        variant: DateVariant,
        min: Option<String>,
        max: Option<String>,
        step: Option<f64>,
        on_change: Option<Closure>,
    },
}

/// The cross-field operand. `against` is a `Binding`, and that IS the cross-field
/// mechanism rather than an accident of typing: any read slot may take a Binding,
/// and the auto-bind rule already puts every form field's value in State under the
/// field's own id, so `{"$type":"State","key":"<sibling id>"}` reads the sibling
/// with no coordination vocabulary at all.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareRule {
    pub op: CompareOp,
    pub against: Binding,
}

/// A field's declared constraint — the ACCEPTED SET, where `FormFieldKind` names
/// the control. Every slot is optional structurally; the two well-formedness
/// refusals are relations BETWEEN slots and so live in the decoder's policy layer
/// rather than in this shape.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldRule {
    pub format: Option<TextFormat>,
    pub pattern: Option<String>,
    pub min_length: Option<i64>,
    pub max_length: Option<i64>,
    pub compare: Option<CompareRule>,
    pub message: Option<TextSource>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    pub id: String,
    pub kind: FormFieldKind,
    pub label: TextSource,
    pub required: bool,
    pub help: Option<TextSource>,
    pub rule: Option<FieldRule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FormSpec {
    pub fields: Vec<FormField>,
    pub on_submit: Action,
    pub submit_label: TextSource,
    pub disabled: Option<Binding>,
}

/// 0.2.0 filters-unification: a filter chip's control is an ordinary
/// [`FormFieldKind`] — the parallel `FilterKind` DU (`TextFilter` /
/// `ChoiceFilter` / `RangeFilter` / `SegmentedFilter`) is retired, its
/// discriminators a hard `UNKNOWN_DU_CASE`. A chip control with no `value`
/// key decodes to the auto binding `Filter(<chip name>)`.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterSpec {
    pub kind: FormFieldKind,
    pub label: TextSource,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ButtonSpec {
    pub label: TextSource,
    pub on_click: Action,
    pub variant: ButtonVariant,
    pub icon: Option<String>,
    pub disabled: Option<Binding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectSpec {
    pub label: TextSource,
    pub source: Binding,
    pub value: Binding,
    pub on_change: Option<Closure>,
    pub placeholder: Option<TextSource>,
    pub disabled: Option<Binding>,
    /// Emitted only when `true` (Phase 291), so single-select stays byte-identical.
    pub multiple: bool,
    pub values: Option<Binding>,
    pub on_change_multi: Option<Closure>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileUploadSpec {
    pub accept: Vec<String>,
    pub label: TextSource,
    pub multiple: bool,
    pub disabled: Option<Binding>,
    // `onSelect` is an always-emitted closure — no field.
}

// ─── Visualisation specs ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum CellKindErased {
    Text,
    Numeric,
    Date,
    /// `onEdit` closure — always emitted.
    Editable,
    /// `get` + `onToggle` closures — always emitted.
    Checkbox,
    /// `onClick` closure — always emitted.
    Button {
        label: TextSource,
    },
    /// Each button's `onClick` closure — always emitted.
    ButtonGroup {
        labels: Vec<TextSource>,
    },
    Link,
    Pill,
    /// Phase 750 — the declarative twin of `Pill`, and the ONE cell kind holding no
    /// closure, which is exactly why it survives the wire. `field` names the row
    /// property that is both the pill's label and the map key; `map` carries value →
    /// tone; `default_tone` covers a value the map does not mention and is omitted on
    /// the wire at `ToneVariant::Default`.
    ///
    /// A `BTreeMap` rather than a `Vec` of pairs deliberately: the tone map is a
    /// key-addressed set, so two spellings differing only in key order are the SAME
    /// map, and a `Vec`'s derived `PartialEq` would call them different — a difference
    /// the round-trip property tests would surface as a phantom divergence.
    TonedPill {
        field: String,
        map: BTreeMap<String, ToneVariant>,
        default_tone: ToneVariant,
    },
    Progress,
    Custom,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnErased {
    pub format: CellFormat,
    pub kind: CellKindErased,
    pub label: String,
    pub width: ColumnWidth,
    /// Phase 425 — `value` (closure) + `field` (declarative) are sibling slots.
    pub value: Option<Closure>,
    pub field: Option<String>,
    /// Phase 861 — per-column sort NARROWING on the bound path. Absent inherits
    /// the grid-level intent, which is why an explicit `false` is carried on the
    /// wire rather than omitted-when-default: omitting it would erase the
    /// narrowing the author declared.
    pub sortable: Option<bool>,
    /// Phase 863 — per-column editability NARROWING; absent inherits the
    /// grid-level `editable` flag. Deliberately not spelled `readOnly`: an
    /// inverting alias that guesses wrong makes a read-only column editable.
    pub editable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticRows {
    pub headers: Vec<TextSource>,
    pub rows: Vec<Vec<TextSource>>,
    /// Phase 801 — the table INVITES interactive column sorting. Intent, not a
    /// behaviour guarantee: a host with no sorting affordance renders the
    /// authored order and stays conformant.
    pub sortable: Option<bool>,
    /// Phase 801 — the table's initial order. Configuration, distinct from the
    /// transform pipeline's data `sort`.
    pub default_sort: Option<DefaultSort>,
}

/// Phase 801 — a static table's declared initial order. `column` is a
/// non-negative index into `StaticRows::headers`; an index past the end is
/// deliberately not a decode error (a sibling-value relation is not something a
/// per-object codec judges).
#[derive(Debug, Clone, PartialEq)]
pub struct DefaultSort {
    pub column: i64,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GridSpec {
    pub columns: Vec<ColumnErased>,
    pub editable: bool,
    pub source: Binding,
    pub on_row_click: Option<Closure>,
    pub row_key: Option<Closure>,
    pub row_key_field: Option<String>,
    /// Phase 818 — the grid-sort header affordance for a data-bound grid:
    /// names the State key carrying the sort descriptor
    /// `{"column": <index>, "direction": "asc"|"desc"}` the runtime sorts its
    /// resolved rows by (and whose headers write it). Omitted on the wire when
    /// absent; `static_rows`' own Phase-801 sort intent is untouched.
    pub sort_state_key: Option<String>,
    /// Phase 861 — the BOUND path's declared initial order, the same record the
    /// `static_rows` spelling carries at its own path.
    pub default_sort: Option<DefaultSort>,
    /// Phase 862 — how many rows a page holds. A page of zero or fewer rows
    /// names no page at all, so a non-positive value is a decode error.
    pub page_size: Option<i64>,
    /// Phase 862 — names the State key carrying `{"page": N}` (1-based). The
    /// page POSITION lives in State so a pager can move it; a literal page
    /// number is deliberately not expressible.
    pub page_state_key: Option<String>,
    /// Phase 863 — the declared edit DESTINATION: a State key on the grid, not
    /// a per-cell host closure (which carries no destination across the wire).
    pub edit_state_key: Option<String>,
    /// Phase 934 — declarative row reorder; omitted-when-false, the same
    /// convention as `editable`. Edits and reorders share one destination.
    pub reorderable: bool,
    pub static_rows: Option<StaticRows>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartSpec {
    pub kind: ChartKind,
    pub source: Binding,
    pub stacked: bool,
    pub x_field: String,
    pub y_fields: Vec<String>,
    pub title: Option<TextSource>,
    /// Phase 876 — the VALUE axis's number format, reusing the existing
    /// `Format` vocabulary. A semantic declaration ("these numbers are pounds /
    /// a ratio"), which is why it is a wire field where the chart STYLE is not.
    /// Absent means no declared meaning — the lowering still applies its
    /// canonical default rendering.
    pub value_format: Option<Format>,
    /// Phase 878 — the x axis's NAME. On the same side of the semantic line as
    /// `title`: what an axis is CALLED is the author's meaning, where and how it
    /// is drawn stays the host's. Absent falls back to the capitalised field
    /// name, so an axis is never nameless.
    pub x_title: Option<TextSource>,
    /// Phase 878 — the value axis's NAME. Absent falls back to the capitalised
    /// FIRST y-field; with no y-fields at all there is no honest fallback and no
    /// y title draws.
    pub y_title: Option<TextSource>,
    /// Phase 878 — a muted qualifier line under the visible `title`. Its
    /// PRESENCE also suppresses the display-unit slot: the subtitle is the
    /// author's own place to state a unit, and the machine restating it two
    /// lines away is the clutter the rule exists to prevent.
    pub subtitle: Option<TextSource>,
    /// Phase 880 — WHERE the legend sits, and whether it sits anywhere at all.
    /// Semantic for the same reason the titles above are (D8): the edge an author
    /// wants the legend on is their meaning; the column widths and pitches that
    /// realise it are the host's.
    ///
    /// Absent means "the host's default" (`Right`) — NOT "no legend"; suppression
    /// is the explicit [`ChartLegendPosition::None`]. So absence stays the
    /// ordinary shape and is omitted on the wire, and an author who wants no
    /// legend says so.
    pub legend_position: Option<ChartLegendPosition>,
    /// Phase 881 — whether the values are written onto the picture. Semantic in
    /// the same way (D8): whether a reader is meant to read the NUMBERS or the
    /// shape is the author's meaning; the type size, the offsets and the fit rule
    /// that decide whether a given label draws are the host's.
    ///
    /// Absent means [`ChartDataLabels::Off`], which is ALSO the default — the one
    /// place this differs from `legend_position`, deliberately: a legend is chrome
    /// an author opts OUT of, where a data label is ink an author opts IN to. So an
    /// absent field is byte-identical to the pre-881 wire and to the pre-881
    /// picture.
    pub data_labels: Option<ChartDataLabels>,
    /// Phase 882 — what the x column MEANS: discrete categories, or dates on a
    /// continuous temporal scale. Semantic in the same way (D8): whether a column
    /// is a set of categories or a run of dates is a fact about the data; the tick
    /// ladder, the label format and the margins that realise it are the host's.
    ///
    /// Absent means [`ChartXScale::Category`], which is also the shipped default,
    /// so an absent field is byte-identical to the pre-882 wire AND to the pre-882
    /// picture. `Temporal` is a DECLARATION the pre-emit validator grounds against
    /// the column type (FUARAN097) — never an inference from the data, which would
    /// make the same tree draw differently depending on where its rows came from.
    pub x_scale: Option<ChartXScale>,
    pub on_point_click: Option<Closure>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapSpec {
    pub centre_latitude: f64,
    pub centre_longitude: f64,
    pub source: Binding,
    pub zoom: i64,
    pub on_marker_click: Option<Closure>,
}

// ─── Layout specs ────────────────────────────────────────────────────────────

/// How a `Box` arranges its children (Phase 390).
#[derive(Debug, Clone, PartialEq)]
pub enum BoxLayout {
    Flex {
        direction: Orientation,
        gap: Option<i64>,
        wrap: bool,
    },
    Grid {
        cols: i64,
        gap: Option<i64>,
        template_columns: Option<String>,
    },
    /// WIRE_FORMAT §3.6.7 (Phase 1082) — the column-fill mode. `cols` is
    /// REQUIRED and POSITIVE; there is deliberately no `template_columns` twin,
    /// because the multi-column model realising masonry has no track list for
    /// one to name.
    Masonry {
        cols: i64,
        gap: Option<i64>,
    },
    Auto,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxSpec {
    pub children: Vec<Node>,
    pub heading: Option<TextSource>,
    pub layout: BoxLayout,
    pub role: BoxRole,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SplitPanelSpec {
    pub children: Vec<Node>,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabHeader {
    pub label: TextSource,
    pub icon: Option<String>,
    pub disabled: Option<Binding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabsSpec {
    pub children: Vec<Node>,
    pub orientation: Orientation,
    pub active_index: Binding,
    pub on_select: Option<Closure>,
    pub tab_headers: Option<Vec<TabHeader>>,
    pub tab_tags: Option<Vec<String>>,
    pub active_tag: Option<Binding>,
    pub on_select_tag: Option<Closure>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepperSpec {
    pub active_step: Binding,
    pub children: Vec<Node>,
    // `onSelect` is an always-emitted closure — no field.
}

#[derive(Debug, Clone, PartialEq)]
pub struct SummaryListSpec {
    pub children: Vec<Node>,
    pub heading: Option<TextSource>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisclosureSpec {
    pub children: Vec<Node>,
    pub default_open: bool,
    pub heading: TextSource,
    pub open: Binding,
    pub on_toggle: Option<Closure>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModalSpec {
    pub children: Vec<Node>,
    pub dismissable: bool,
    pub open: Binding,
    /// A wire-survivable `Action` (not a closure sentinel); optional since Phase 426.
    pub on_dismiss: Option<Action>,
    pub heading: Option<TextSource>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScrollAreaSpec {
    pub children: Vec<Node>,
    pub orientation: ScrollOrientation,
    pub max_height: Option<i64>,
    pub max_width: Option<i64>,
}

// ─── Structural kinds ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ContentHash {
    pub algorithm: String,
    pub hash: String,
    pub strictness: HashStrictness,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CustomSpec {
    pub module_id: String,
    pub component_id: String,
    /// A structured-JSON map (rule 12).
    pub props: Vec<(String, JVal)>,
    pub content_hash: Option<ContentHash>,
    /// Emitted only when non-empty.
    pub exposed_node_ids: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ErrorBoundarySpec {
    pub child: Box<Node>,
    pub fallback: Box<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    pub match_value: String,
    pub child: Node,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchSpec {
    /// The branch selector — any Binding (Phase 768): a `State` selector keeps
    /// the Phase 392 behaviour; a `Selection` selector lets the branch follow
    /// the clicked row with no writer. The no-default `State` form keeps its
    /// compact `stateKey` spelling on the wire (the encoder collapses it
    /// back); any other selector encodes under an `on` key holding the
    /// canonical Binding wire form.
    pub on: Binding,
    pub cases: Vec<SwitchCase>,
    pub default: Box<Node>,
}

// Parameterised fragments (holes / effect / args).

#[derive(Debug, Clone, PartialEq)]
pub enum HoleValueSpace {
    IntRange { min: i64, max: i64 },
    FloatRange { min: f64, max: f64 },
    StringLen { min_len: i64, max_len: i64 },
    Enum { choices: Vec<String> },
    AnyString,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FragmentScalar {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HoleDecl {
    Value {
        name: String,
        space: HoleValueSpace,
        default: Option<FragmentScalar>,
    },
    Slot {
        name: String,
        kind_constraint: Option<String>,
    },
    Repeat {
        name: String,
        count_space: HoleValueSpace,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectClass {
    pub host_effect: HostEffect,
    pub determinism: DeterminismSource,
}

impl EffectClass {
    pub const PURE_DETERMINISTIC: EffectClass = EffectClass {
        host_effect: HostEffect::Pure,
        determinism: DeterminismSource::Deterministic,
    };

    pub fn is_pure_deterministic(&self) -> bool {
        *self == Self::PURE_DETERMINISTIC
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FragmentArg {
    Value(FragmentScalar),
    Slot { tree: Box<Node> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FragmentDeclSpec {
    pub name: String,
    pub body: Box<Node>,
    pub holes: Vec<HoleDecl>,
    pub effect: EffectClass,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FragmentRefSpec {
    pub name: String,
    pub args: Vec<(String, FragmentArg)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MountChannel {
    pub direction: ChannelDirection,
    pub message_shape: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MountSpec {
    pub scope_id: String,
    pub inputs: Vec<(String, FragmentArg)>,
    pub channel: MountChannel,
    pub capabilities: Vec<String>,
    // `onBubble` is an always-emitted closure — no field.
}

// ─── Drawing (Phase 524) — bounded, typed vector graphics ────────────────────

/// A point in `Drawing` coordinate space.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawPoint {
    pub x: f64,
    pub y: f64,
}

/// The SVG-style view box of a `Drawing`.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewBox {
    pub min_x: f64,
    pub min_y: f64,
    pub width: f64,
    pub height: f64,
}

/// Presentation style shared by every `Shape`; every field is optional (an
/// all-`None` style emits `{}`). The text-only fields (`text_anchor` /
/// `font_size` / `emphasis` / `font_family`, Phase 528.1) apply only to `Label`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DrawStyle {
    pub fill: Option<Binding>,
    pub stroke: Option<Binding>,
    pub stroke_width: Option<Binding>,
    pub opacity: Option<Binding>,
    pub text_anchor: Option<TextAnchor>,
    pub font_size: Option<f64>,
    pub emphasis: Option<Emphasis>,
    pub font_family: Option<String>,
    /// Phase 642 — keyed mark identity: a data-bearing shape's derivation-based
    /// id (`series-field|category-key`), stable under row reorder and data
    /// refresh (object constancy). Renderers emit it as `data-fuaran-mark`;
    /// omitted-when-`None` (rule 4). Chrome (axes, gridlines, labels, legend)
    /// deliberately stays unstamped.
    pub mark_id: Option<String>,
    /// Phase 877 — clockwise `Label` text rotation in DEGREES about the label's
    /// own anchor point (`Shape::Label`'s `x` / `y`). Renderers emit
    /// `transform="rotate(θ x y)"` on the `<text>` element, so the anchor is the
    /// pivot and `text_anchor` keeps its meaning in the rotated frame.
    ///
    /// Applies only to `Label`, like the other text-only fields — and here that
    /// is load-bearing rather than cosmetic: an SVG `transform` on a `<rect>`
    /// would MOVE GEOMETRY, so this is the one text field whose uniform
    /// emission would silently distort a drawing.
    ///
    /// Omitted-when-`None` (rule 4). `Some(0.0)` is a legitimate PRESENT value
    /// and a different wire shape from `None` — never collapse the two.
    pub rotation: Option<f64>,
    /// Phase 883 — the mark's hover-readable text. Renderers emit it as an SVG
    /// `<title>` element that is the FIRST CHILD of that shape's own element:
    /// the native browser tooltip and the element's accessible name, with no
    /// script, so a statically-served page carries the readout too.
    ///
    /// Unlike the Phase 528.1 text cluster and `rotation`, this applies to
    /// EVERY shape, not just `Label` — the marks a reader hovers are bars,
    /// wedges and points, and a `<title>` child is inert geometry-wise
    /// everywhere.
    ///
    /// The emission consequence to get right: a TIPPED shape cannot be emitted
    /// self-closing — `<rect …/>` becomes `<rect …><title>…</title></rect>`.
    /// An absent tip leaves the emitted bytes unchanged, self-closing tag
    /// included.
    ///
    /// Omitted-when-`None` (rule 4). `Some(TextSource::Literal(""))` is a
    /// legitimate PRESENT value and a different wire shape from `None` — never
    /// collapse the two.
    pub tip: Option<TextSource>,
}

/// A single SVG path command inside a `Shape::Curve`. Closed + typed — an
/// unrecognised `$type` is `UNKNOWN_DU_CASE` at decode.
#[derive(Debug, Clone, PartialEq)]
pub enum CurveCommand {
    MoveTo(DrawPoint),
    LineTo(DrawPoint),
    CubicTo {
        control1: DrawPoint,
        control2: DrawPoint,
        to: DrawPoint,
    },
    QuadraticTo {
        control: DrawPoint,
        to: DrawPoint,
    },
    Close,
}

/// The closed, typed vector-graphics vocabulary (no raw-SVG / attribute-bag
/// escape hatch). An unrecognised `$type` is `UNKNOWN_DU_CASE` at decode.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Group {
        children: Vec<Shape>,
        style: DrawStyle,
    },
    Rectangle {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        corner_radius: Option<f64>,
        style: DrawStyle,
    },
    Line {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        style: DrawStyle,
    },
    Polyline {
        points: Vec<DrawPoint>,
        style: DrawStyle,
    },
    Polygon {
        points: Vec<DrawPoint>,
        style: DrawStyle,
    },
    Curve {
        commands: Vec<CurveCommand>,
        style: DrawStyle,
    },
    Circle {
        cx: f64,
        cy: f64,
        r: f64,
        style: DrawStyle,
    },
    Ellipse {
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        style: DrawStyle,
    },
    Label {
        x: f64,
        y: f64,
        text: TextSource,
        style: DrawStyle,
    },
}

/// A bounded, typed vector-graphics primitive (`DisplayKind.Drawing`). Geometry
/// is static numbers — a resolved artefact; only `DrawStyle` carries `Binding`s.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawingSpec {
    pub view_box: ViewBox,
    pub shapes: Vec<Shape>,
    pub style: DrawStyle,
    pub title: Option<TextSource>,
    pub description: Option<TextSource>,
}

// ─── NodeKind — the flat wire vocabulary (§3.2) ──────────────────────────────

/// The node's primitive kind: the closed, flat `kind.$type` vocabulary. The
/// four behavioural categories (Layout / Display / Input / Visualisation) are a
/// host-side classification (see [`NodeKind::category`]), not a wire level.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    // Layout
    Box(BoxSpec),
    SplitPanel(SplitPanelSpec),
    Tabs(TabsSpec),
    Stepper(StepperSpec),
    SummaryList(SummaryListSpec),
    Disclosure(DisclosureSpec),
    Modal(ModalSpec),
    ScrollArea(ScrollAreaSpec),
    // Display
    Heading(HeadingSpec),
    Markdown(MarkdownSpec),
    Metric(MetricSpec),
    Badge(BadgeSpec),
    Sparkline(SparklineSpec),
    Callout(CalloutSpec),
    Progress(ProgressSpec),
    Skeleton(SkeletonSpec),
    Icon(IconSpec),
    LabelValueRow(LabelValueRowSpec),
    Fact(FactSpec),
    Link(LinkSpec),
    Image(ImageSpec),
    Media(MediaSpec),
    List(ListSpec),
    Toast(ToastSpec),
    CodeBlock(CodeBlockSpec),
    Math(MathSpec),
    Drawing(DrawingSpec),
    // Input
    Form(FormSpec),
    Filters(Vec<FilterSpec>),
    Button(ButtonSpec),
    FileUpload(FileUploadSpec),
    Select(SelectSpec),
    // Visualisation
    DataGrid(GridSpec),
    Chart(ChartSpec),
    Map(MapSpec),
    // Structural
    Custom(CustomSpec),
    ErrorBoundary(ErrorBoundarySpec),
    Switch(SwitchSpec),
    FragmentDecl(FragmentDeclSpec),
    FragmentRef(FragmentRefSpec),
    Mount(MountSpec),
}

/// The behavioural category a primitive belongs to — recovered on decode, never
/// a wire level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeCategory {
    Layout,
    Display,
    Input,
    Visualisation,
    Structural,
}

impl NodeKind {
    /// The wire `$type` discriminator name of this kind (e.g. `"Metric"`,
    /// `"DataGrid"`, `"Box"`) — the stable structural name introspection and
    /// structural search key off.
    pub fn type_name(&self) -> &'static str {
        match self {
            NodeKind::Box(_) => "Box",
            NodeKind::SplitPanel(_) => "SplitPanel",
            NodeKind::Tabs(_) => "Tabs",
            NodeKind::Stepper(_) => "Stepper",
            NodeKind::SummaryList(_) => "SummaryList",
            NodeKind::Disclosure(_) => "Disclosure",
            NodeKind::Modal(_) => "Modal",
            NodeKind::ScrollArea(_) => "ScrollArea",
            NodeKind::Heading(_) => "Heading",
            NodeKind::Markdown(_) => "Markdown",
            NodeKind::Metric(_) => "Metric",
            NodeKind::Badge(_) => "Badge",
            NodeKind::Sparkline(_) => "Sparkline",
            NodeKind::Callout(_) => "Callout",
            NodeKind::Progress(_) => "Progress",
            NodeKind::Skeleton(_) => "Skeleton",
            NodeKind::Icon(_) => "Icon",
            NodeKind::LabelValueRow(_) => "LabelValueRow",
            NodeKind::Fact(_) => "Fact",
            NodeKind::Link(_) => "Link",
            NodeKind::Image(_) => "Image",
            NodeKind::Media(_) => "Media",
            NodeKind::List(_) => "List",
            NodeKind::Toast(_) => "Toast",
            NodeKind::CodeBlock(_) => "CodeBlock",
            NodeKind::Math(_) => "Math",
            NodeKind::Drawing(_) => "Drawing",
            NodeKind::Form(_) => "Form",
            NodeKind::Filters(_) => "Filters",
            NodeKind::Button(_) => "Button",
            NodeKind::FileUpload(_) => "FileUpload",
            NodeKind::Select(_) => "Select",
            NodeKind::DataGrid(_) => "DataGrid",
            NodeKind::Chart(_) => "Chart",
            NodeKind::Map(_) => "Map",
            NodeKind::Custom(_) => "Custom",
            NodeKind::ErrorBoundary(_) => "ErrorBoundary",
            NodeKind::Switch(_) => "Switch",
            NodeKind::FragmentDecl(_) => "FragmentDecl",
            NodeKind::FragmentRef(_) => "FragmentRef",
            NodeKind::Mount(_) => "Mount",
        }
    }

    /// The host-side behavioural classification of this primitive (§3.2).
    pub fn category(&self) -> NodeCategory {
        match self {
            NodeKind::Box(_)
            | NodeKind::SplitPanel(_)
            | NodeKind::Tabs(_)
            | NodeKind::Stepper(_)
            | NodeKind::SummaryList(_)
            | NodeKind::Disclosure(_)
            | NodeKind::Modal(_)
            | NodeKind::ScrollArea(_) => NodeCategory::Layout,
            NodeKind::Heading(_)
            | NodeKind::Markdown(_)
            | NodeKind::Metric(_)
            | NodeKind::Badge(_)
            | NodeKind::Sparkline(_)
            | NodeKind::Callout(_)
            | NodeKind::Progress(_)
            | NodeKind::Skeleton(_)
            | NodeKind::Icon(_)
            | NodeKind::LabelValueRow(_)
            | NodeKind::Fact(_)
            | NodeKind::Link(_)
            | NodeKind::Image(_)
            | NodeKind::Media(_)
            | NodeKind::List(_)
            | NodeKind::Toast(_)
            | NodeKind::CodeBlock(_)
            | NodeKind::Math(_)
            | NodeKind::Drawing(_) => NodeCategory::Display,
            NodeKind::Form(_)
            | NodeKind::Filters(_)
            | NodeKind::Button(_)
            | NodeKind::FileUpload(_)
            | NodeKind::Select(_) => NodeCategory::Input,
            NodeKind::DataGrid(_) | NodeKind::Chart(_) | NodeKind::Map(_) => {
                NodeCategory::Visualisation
            }
            NodeKind::Custom(_)
            | NodeKind::ErrorBoundary(_)
            | NodeKind::Switch(_)
            | NodeKind::FragmentDecl(_)
            | NodeKind::FragmentRef(_)
            | NodeKind::Mount(_) => NodeCategory::Structural,
        }
    }
}

/// The emittable NodeKind vocabulary — one entry per [`NodeKind`] variant's wire
/// `$type` name (the values [`NodeKind::type_name`] returns). Pinned against the
/// generated `wire-format-fixtures` manifest `kinds` enumeration by
/// `tests/conformance.rs` (Phase 548 cross-host kind-set attestation): a new
/// variant added without an entry here fails that pin with the kind named. Rust
/// has no enum-variant reflection, so this list is maintained in lockstep with the
/// `type_name` match under the §11 forward-coupling discipline (adding a NodeKind
/// already touches `type_name` + `category` + every exhaustive `match`; this is
/// one more).
pub const CANONICAL_NODE_KINDS: &[&str] = &[
    // Layout
    "Box",
    "SplitPanel",
    "Tabs",
    "Stepper",
    "SummaryList",
    "Disclosure",
    "Modal",
    "ScrollArea",
    // Display
    "Heading",
    "Markdown",
    "Metric",
    "Badge",
    "Sparkline",
    "Callout",
    "Progress",
    "Skeleton",
    "Icon",
    "LabelValueRow",
    "Fact",
    "Link",
    "Image",
    "Media",
    "List",
    "Toast",
    "CodeBlock",
    "Math",
    "Drawing",
    // Input
    "Form",
    "Filters",
    "Button",
    "FileUpload",
    "Select",
    // Visualisation
    "DataGrid",
    "Chart",
    "Map",
    // Structural
    "Custom",
    "ErrorBoundary",
    "Switch",
    "FragmentDecl",
    "FragmentRef",
    "Mount",
];

/// The emittable FormFieldKind control vocabulary — one entry per
/// [`FormFieldKind`] variant's wire `$type` name. Pinned against the generated
/// `wire-format-fixtures` manifest `formFieldKinds` enumeration by
/// `tests/conformance.rs` (Phase 746 control-vocabulary attestation), the
/// [`CANONICAL_NODE_KINDS`] twin: the kind-set pin only ever covered NodeKind, so
/// a control-vocabulary commit that skipped this host stayed silent until a
/// fixture happened to exercise it. Rust has no enum-variant reflection, so this
/// list is maintained in lockstep with the `decode_form_field_kind` dispatch under
/// the §11 forward-coupling discipline — and that decoder's unknown-case hint is
/// DERIVED from this list, so hint and dispatch cannot diverge (they had: the
/// hand-typed hint was already missing `DateRange`'s predecessor state).
///
/// Since the filters/forms unification there is ONE control vocabulary, so this
/// covers both the `Form.fields` and `Filters.items` carriers.
pub const CANONICAL_FORM_FIELD_KINDS: &[&str] = &[
    "Text",
    "Number",
    "Checkbox",
    "Toggle",
    "Choice",
    "Range",
    "RangedNumber",
    "SegmentedChoice",
    "TextArea",
    "Date",
    "DateRange",
];

// ─── Node envelope (§3.1) ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StateBehaviour {
    pub on_loading: Option<Box<Node>>,
    pub on_empty: Option<Box<Node>>,
    /// The `ErrorPayload -> Node` callback is a closure (§4) — presence only.
    pub on_error: Option<Closure>,
}

impl StateBehaviour {
    pub fn is_empty(&self) -> bool {
        self.on_loading.is_none() && self.on_empty.is_none() && self.on_error.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticStyle {
    pub emphasis: Emphasis,
    pub tone: ToneVariant,
    pub weight: StyleWeight,
    /// Phase 147 — omitted on the wire at its default (`None`).
    pub role: StyleRole,
    /// Phase 147 — omitted on the wire at its default (`Default`).
    pub voice: FontVoice,
}

impl Default for SemanticStyle {
    fn default() -> Self {
        SemanticStyle {
            emphasis: Emphasis::Normal,
            tone: ToneVariant::Default,
            weight: StyleWeight::Standard,
            role: StyleRole::None,
            voice: FontVoice::Default,
        }
    }
}

impl SemanticStyle {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Accessibility {
    pub label: Option<Binding>,
    pub labelled_by: Option<NodeId>,
    pub described_by: Option<NodeId>,
    /// The raw ARIA role string (named roles and the custom escape both encode
    /// as the raw string, §10.2).
    pub role: Option<String>,
    pub live_region: Option<LiveRegionKind>,
    pub hidden: Option<Binding>,
}

/// The typed UI tree node (§3.1). `motion` / `extraAttributes` are wire-omitted
/// by design (§9) and therefore not modelled by this host.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Restored to empty when the wire omits it (the common case).
    pub state: StateBehaviour,
    /// Restored to all-default when the wire omits it (the common case).
    pub style: SemanticStyle,
    pub accessibility: Option<Accessibility>,
}

// ─── TreeOp (§3.4) ───────────────────────────────────────────────────────────

/// A tree edit, one variant per top-level wire `$type` case.
#[derive(Debug, Clone, PartialEq)]
pub enum TreeOp {
    EditNode {
        target: NodeId,
        new_kind: NodeKind,
    },
    /// `value` is a structured-JSON position (rule 12); `path` is a plain
    /// string at codec level (grammar violations surface at apply time, §3.4).
    UpdateProp {
        target: NodeId,
        path: String,
        value: JVal,
    },
    ReplaceBinding {
        target: NodeId,
        slot: String,
        binding: Binding,
    },
    UpdateStyle {
        target: NodeId,
        style: SemanticStyle,
    },
    UpdateState {
        target: NodeId,
        state: StateBehaviour,
    },
    InsertChild {
        parent_id: NodeId,
        child: Node,
    },
    RemoveNode {
        target: NodeId,
    },
    MoveNode {
        target: NodeId,
        new_parent_id: NodeId,
    },
    ReorderChildren {
        parent_id: NodeId,
        new_order: Vec<NodeId>,
    },
    ReplaceRoot {
        node: Node,
    },
    Batch(Vec<TreeOp>),
}
