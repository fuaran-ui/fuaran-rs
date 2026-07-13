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
bare_enum!(StyleRole { None => "None", Eyebrow => "Eyebrow", Data => "Data", Lede => "Lede", Caption => "Caption" });
bare_enum!(FontVoice { Default => "Default", Display => "Display", Structural => "Structural" });
bare_enum!(ChartKind { Line => "Line", Bar => "Bar", Area => "Area", Pie => "Pie", Scatter => "Scatter", Heatmap => "Heatmap" });
bare_enum!(ImageVariant { Default => "Default", Avatar => "Avatar", Rounded => "Rounded" });
bare_enum!(MathDisplay { Inline => "Inline", Block => "Block" });
bare_enum!(DateVariant { Date => "Date", Time => "Time", DateTime => "DateTime" });
bare_enum!(FileReadEncoding { Text => "Text", Base64 => "Base64", DataUrl => "DataUrl" });
bare_enum!(LiveRegionKind { Polite => "polite", Assertive => "assertive", Off => "off" });
bare_enum!(DateStyle { Short => "Short", Medium => "Medium", Long => "Long", Full => "Full" });
bare_enum!(RelativeTimeUnit { Second => "Second", Minute => "Minute", Hour => "Hour", Day => "Day", Week => "Week", Month => "Month", Year => "Year" });
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
    },
    Selection {
        node_id: NodeId,
    },
    State {
        key: String,
        default_value: StaticValue,
    },
    Computed,
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
        source: DataSource,
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
    Number { decimals: Option<i64> },
    Currency { iso_code: String },
    Percent { decimals: Option<i64> },
    Date { date_style: DateStyle },
    RelativeTime { unit: RelativeTimeUnit },
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
    SetState {
        key: String,
        value: JVal,
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
bare_enum!(BinOp { Add => "add", Sub => "sub", Mul => "mul", Div => "div", Mod => "mod", Eq => "eq", Ne => "ne", Lt => "lt", Le => "le", Gt => "gt", Ge => "ge", And => "and", Or => "or" });
bare_enum!(ScalarFn { Abs => "abs", Round => "round", Floor => "floor", Ceil => "ceil", Length => "length", Lower => "lower", Upper => "upper", Substr => "substr", DatePart => "datePart" });
bare_enum!(AggFn { Sum => "sum", Mean => "mean", Min => "min", Max => "max", Count => "count", Median => "median", Stddev => "stddev", First => "first", Last => "last" });
bare_enum!(JoinKind { Inner => "inner", Left => "left", Right => "right", Outer => "outer" });
bare_enum!(WindowFn { RowNumber => "rowNumber", Rank => "rank", Lag => "lag", Lead => "lead", CumSum => "cumSum", RollingMean => "rollingMean" });
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
    pub source: Binding,
    pub format: CellFormat,
    pub tone: ToneVariant,
    pub weight: StyleWeight,
    pub emphasis: Emphasis,
    pub trend: Option<Binding>,
    pub trend_format: Option<CellFormat>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct LabelValueRowSpec {
    pub label: TextSource,
    pub source: Binding,
    pub format: CellFormat,
    pub emphasis: bool,
    pub help: Option<TextSource>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkSpec {
    pub href: Binding,
    pub label: TextSource,
    pub download: bool,
    pub rel: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageSpec {
    pub alt: TextSource,
    pub src: Binding,
    pub variant: ImageVariant,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    pub id: String,
    pub kind: FormFieldKind,
    pub label: TextSource,
    pub required: bool,
    pub help: Option<TextSource>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FormSpec {
    pub fields: Vec<FormField>,
    pub on_submit: Action,
    pub submit_label: TextSource,
    pub disabled: Option<Binding>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilterKind {
    TextFilter {
        value: Binding,
        on_change: Option<Closure>,
    },
    ChoiceFilter {
        options: Binding,
        value: Binding,
        on_change: Option<Closure>,
    },
    /// The bounds ride as a typed `{min,max}` object (Phase 423) for the
    /// `Static` binding every decoded chip carries.
    RangeFilter {
        min: f64,
        max: f64,
        on_change: Option<Closure>,
    },
    SegmentedFilter {
        options: Binding,
        orientation: Orientation,
        value: Binding,
        on_change: Option<Closure>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilterSpec {
    pub kind: FilterKind,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticRows {
    pub headers: Vec<TextSource>,
    pub rows: Vec<Vec<TextSource>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GridSpec {
    pub columns: Vec<ColumnErased>,
    pub editable: bool,
    pub source: Binding,
    pub on_row_click: Option<Closure>,
    pub row_key: Option<Closure>,
    pub row_key_field: Option<String>,
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
    pub state_key: String,
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
    LabelValueRow(LabelValueRowSpec),
    Link(LinkSpec),
    Image(ImageSpec),
    List(ListSpec),
    Toast(ToastSpec),
    CodeBlock(CodeBlockSpec),
    Math(MathSpec),
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
            NodeKind::LabelValueRow(_) => "LabelValueRow",
            NodeKind::Link(_) => "Link",
            NodeKind::Image(_) => "Image",
            NodeKind::List(_) => "List",
            NodeKind::Toast(_) => "Toast",
            NodeKind::CodeBlock(_) => "CodeBlock",
            NodeKind::Math(_) => "Math",
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
            | NodeKind::LabelValueRow(_)
            | NodeKind::Link(_)
            | NodeKind::Image(_)
            | NodeKind::List(_)
            | NodeKind::Toast(_)
            | NodeKind::CodeBlock(_)
            | NodeKind::Math(_) => NodeCategory::Display,
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
        position: i64,
        child: Node,
    },
    RemoveNode {
        target: NodeId,
    },
    MoveNode {
        target: NodeId,
        new_parent_id: NodeId,
        new_position: i64,
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
