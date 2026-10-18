//! This host's DIALECT over the wire corpus's `idl.json` (Phase 2315): the
//! representation choices `fuaran-rs` makes that the generator's uniform mapping
//! rules do not. The rules live in `idl_codegen.rs`, one function each; this file
//! lists only where this host departs from them, one entry per departure, each
//! with its reason.
//!
//! **A dialect is not a mirror.** A mirror restates the vocabulary, so it drifts
//! on every change; a dialect names only the positions the rules map differently.
//! Adding a kind, a field or a case to the IDL needs no entry here. The generator
//! checks every entry against the IDL it reads and refuses, naming the entry:
//!
//! - an entry naming a kind, record, union, case or field the IDL does not have;
//! - an entry the uniform rules would now produce anyway (the departure is gone,
//!   so the entry is stale and must be deleted);
//! - a note naming an item the generated module does not emit.
//!
//! The representation choices are held exactly as the public types already had
//! them. Shrinking this file (for example making every optional-with-default slot
//! a plain value) changes public types, so it rides a version cut, not a
//! regeneration.
//!
//! Owner keys: a kind by its tag (`"Metric"`), a record by its name, a union case
//! as `"Union.Case"`, a tree op as `"TreeOp.Tag"`, and the node envelope as
//! `"Node"`. A Rust member named `"0"`, `"1"`, … is positional: a case whose
//! members are all positional is a tuple variant.

/// The IDL's node category spellings mapped to this host's
/// [`NodeCategory`](super::NodeCategory) variants and the article the
/// unknown-kind hint uses, in the order the hint lists them. The IDL calls the
/// structural kinds `Meta`; this host's public enum has always called them
/// `Structural`, and a public variant name is kept. An IDL category missing here
/// is a generator error, never a silent bucket.
pub(super) const CATEGORIES: &[(&str, &str, &str)] = &[
    ("Layout", "Layout", "a"),
    ("Display", "Display", "a"),
    ("Input", "Input", "an"),
    ("Visualisation", "Visualisation", "a"),
    ("Meta", "Structural", "a"),
];

/// IDL type name to this host's Rust type name, where the type-name rule (a kind
/// is `<Tag>Spec`, a record or union keeps its name) does not hold. Each is a
/// public name this host has always used.
pub(super) const TYPE_NAMES: &[(&str, &str)] = &[
    ("DataGrid", "GridSpec"),
    ("GuestChannel", "MountChannel"),
    ("Scalar", "FragmentScalar"),
];

/// IDL records this host has no Rust type for: each is inlined into the one
/// position that carries it, as that position's dialect entry says.
pub(super) const INLINED: &[(&str, &str)] = &[
    (
        "ButtonGroupItem",
        "inlined into CellKindErased::ButtonGroup, which carries the labels only",
    ),
    (
        "DateTimeRangePair",
        "inlined into the DateTimeRange control's value binding",
    ),
    (
        "RangePair",
        "inlined into the Range control's value binding",
    ),
];

/// IDL unions this host models by hand in `model.rs`, because its case set is not
/// the IDL's. The tag set is still generated, for the decoder's refusal hints.
pub(super) const HAND_TYPES: &[(&str, &str)] = &[(
    "FragmentArg",
    "modelled as Value(FragmentScalar) | Slot { tree }: the four scalar cases are one \
     variant over FragmentScalar",
)];

/// Kinds whose `NodeKind` variant carries something other than `<Tag>Spec`. No
/// spec struct is generated for them.
pub(super) const KIND_PAYLOADS: &[(&str, &str, &str)] = &[(
    "Filters",
    "Vec<FilterSpec>",
    "the variant holds the `items` list directly; the kind has no other field",
)];

/// Derives beyond the rule's `Debug, Clone, PartialEq`, per generated type.
pub(super) const DERIVES: &[(&str, &[&str], &str)] = &[
    (
        "Accessibility",
        &["Default"],
        "every member is optional, and builders start from the empty record",
    ),
    (
        "DrawStyle",
        &["Default"],
        "every member is optional, and a shape without style is the default",
    ),
    (
        "StateBehaviour",
        &["Default"],
        "an absent `state` is the empty behaviour",
    ),
    (
        "SemanticStyle",
        &["Copy", "Eq"],
        "a small value type compared by value; its Default is hand-written in model.rs",
    ),
    (
        "StyleDelta",
        &["Copy", "Eq", "Default"],
        "a small value type; the empty delta is the default",
    ),
    (
        "ChartAnnotationX",
        &["Eq"],
        "both cases carry strings, and annotations are compared by value",
    ),
];

const CLOSURE_IMPLIED: &str = "the IDL declares this closure slot optional, but this host carries no member for      it: its presence is implied and the encoder always writes the sentinel. No corpus      fixture omits it, so an Option<Closure> member would change a public type with      nothing to certify the absent case; it waits for that fixture";
const CLOSURE_UNMODELLED: &str = "a closure slot this host does not carry: the closure is a host-side function the \
     wire never shows (only its sentinel), and nothing here reads its presence";
const NODE_ID: &str = "spelled with the NodeId alias (the same type as String), which documents that the \
     value names a node";
const POSITIONAL: &str = "a one-member case carried positionally (a tuple variant)";
const BOUND_VALUE: &str = "always present: the control's value binding is required by this host, and an \
     absent one is refused at decode";

/// One entry per IDL field this host maps differently from the uniform rules:
/// `(owner, IDL field, the Rust members it becomes, reason)`. An empty member list
/// means the field has no Rust member at all; two IDL fields naming the same Rust
/// member fold into it.
pub(super) type FieldEntry = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

pub(super) const FIELDS: &[FieldEntry] = &[
    // ── Kinds ──────────────────────────────────────────────────────────────
    (
        "Custom",
        "exposedNodeIds",
        &[("exposed_node_ids", "Vec<NodeId>")],
        "an absent list decodes as the empty list and the encoder omits an empty one, so \
         the member is a plain list; spelled with NodeId",
    ),
    (
        "FragmentRef",
        "args",
        &[("args", "Vec<(String, FragmentArg)>")],
        "an absent map decodes as the empty map and the encoder omits an empty one",
    ),
    (
        "Mount",
        "inputs",
        &[("inputs", "Vec<(String, FragmentArg)>")],
        "an absent map decodes as the empty map and the encoder omits an empty one",
    ),
    ("Mount", "onBubble", &[], CLOSURE_IMPLIED),
    (
        "Switch",
        "on",
        &[("on", "Binding")],
        "always present: the state-keyed form (`stateKey`) is decoded into it as a State \
         binding",
    ),
    (
        "Switch",
        "stateKey",
        &[],
        "folded into `on`: the state-keyed form is one of the `on` binding's shapes",
    ),
    // ── Records ────────────────────────────────────────────────────────────
    (
        "Accessibility",
        "describedBy",
        &[("described_by", "Option<NodeId>")],
        NODE_ID,
    ),
    (
        "Accessibility",
        "labelledBy",
        &[("labelled_by", "Option<NodeId>")],
        NODE_ID,
    ),
    (
        "Accessibility",
        "role",
        &[("role", "Option<String>")],
        "a hosted ARIA role whose wire form is its token string, so the host carries the \
         string",
    ),
    (
        "MapMarker",
        "label",
        &[("label", "TextSource")],
        "the host reads a full TextSource here; a plain string is its Literal case, which \
         encodes back to the same string",
    ),
    (
        "SelectOption",
        "label",
        &[("label", "TextSource")],
        "the host reads a full TextSource here; a plain string is its Literal case, which \
         encodes back to the same string",
    ),
    (
        "SwitchCase",
        "match",
        &[("condition", "SwitchCondition")],
        "the match/when pair is one SwitchCondition, so exactly one of the two is ever \
         present",
    ),
    (
        "SwitchCase",
        "when",
        &[],
        "folded into `condition` with `match`",
    ),
    // ── Action ─────────────────────────────────────────────────────────────
    ("Action.Chain", "ops", &[("0", "Vec<Action>")], POSITIONAL),
    (
        "Action.CommitLocal",
        "nodeId",
        &[("node_id", "NodeId")],
        NODE_ID,
    ),
    ("Action.Focus", "nodeId", &[("node_id", "NodeId")], NODE_ID),
    ("Action.ReadFileBody", "onRead", &[], CLOSURE_UNMODELLED),
    (
        "Action.SetState",
        "valueFrom",
        &[("value_from", "Option<Box<Binding>>")],
        "boxed to keep Action small: a Binding is the largest payload any action carries",
    ),
    // ── Binding ────────────────────────────────────────────────────────────
    ("Binding.Computed", "fn", &[], CLOSURE_UNMODELLED),
    (
        "Binding.Filter",
        "defaultValue",
        &[("default_value", "Option<StaticValue>")],
        "the type variable is resolved by the slot's static shape (StaticValue), not erased",
    ),
    ("Binding.Local", "format", &[], CLOSURE_UNMODELLED),
    ("Binding.Local", "parse", &[], CLOSURE_UNMODELLED),
    (
        "Binding.Selection",
        "defaultValue",
        &[("default_value", "Option<StaticValue>")],
        "the type variable is resolved by the slot's static shape (StaticValue), not erased",
    ),
    (
        "Binding.Selection",
        "nodeId",
        &[("node_id", "NodeId")],
        NODE_ID,
    ),
    (
        "Binding.State",
        "defaultValue",
        &[
            ("default_value", "StaticValue"),
            ("default_declared", "bool"),
        ],
        "the type variable is resolved by the slot's static shape; an absent default is \
         the shape's empty value, and `default_declared` records whether the wire carried \
         one so the encoder writes it back only then",
    ),
    (
        "Binding.Static",
        "value",
        &[("value", "StaticValue")],
        "the type variable is resolved by the slot's static shape (StaticValue), and the \
         shape decides how an absent value reads",
    ),
    (
        "Binding.Expr",
        "expr",
        &[("expr", "ColExpr")],
        "the hosted expression is decoded into the compute layer's typed ColExpr",
    ),
    (
        "Binding.Transform",
        "pipeline",
        &[("pipeline", "Vec<TransformStep>")],
        "the hosted steps are decoded into the compute layer's typed TransformStep",
    ),
    (
        "Binding.Transform",
        "source",
        &[("source", "TransformSource")],
        "the hosted source is decoded into TransformSource (embedded data or a live \
         binding)",
    ),
    // ── CellFormat / CellKindErased ────────────────────────────────────────
    ("CellFormat.Custom", "fn", &[], CLOSURE_UNMODELLED),
    ("CellKindErased.Button", "onClick", &[], CLOSURE_UNMODELLED),
    (
        "CellKindErased.ButtonGroup",
        "buttons",
        &[("labels", "Vec<TextSource>")],
        "each ButtonGroupItem is inlined to its label: its only other member is a closure \
         this host does not carry",
    ),
    ("CellKindErased.Checkbox", "get", &[], CLOSURE_UNMODELLED),
    (
        "CellKindErased.Checkbox",
        "onToggle",
        &[],
        CLOSURE_UNMODELLED,
    ),
    ("CellKindErased.Custom", "fn", &[], CLOSURE_UNMODELLED),
    ("CellKindErased.Editable", "onEdit", &[], CLOSURE_UNMODELLED),
    ("CellKindErased.Link", "hrefFn", &[], CLOSURE_UNMODELLED),
    ("CellKindErased.Link", "labelFn", &[], CLOSURE_UNMODELLED),
    ("CellKindErased.Pill", "labelFn", &[], CLOSURE_UNMODELLED),
    ("CellKindErased.Pill", "toneFn", &[], CLOSURE_UNMODELLED),
    (
        "CellKindErased.Progress",
        "fractionFn",
        &[],
        CLOSURE_UNMODELLED,
    ),
    (
        "CellKindErased.Progress",
        "labelFn",
        &[],
        CLOSURE_UNMODELLED,
    ),
    (
        "CellKindErased.TonedPill",
        "default",
        &[("default_tone", "ToneVariant")],
        "renamed: `default` reads as the Default trait beside the map it qualifies",
    ),
    (
        "CellKindErased.TonedPill",
        "map",
        &[("map", "BTreeMap<String, ToneVariant>")],
        "a lookup table keyed by cell value, so a sorted map rather than an ordered list",
    ),
    // ── ChartAnnotationX / CurveCommand / Scalar / TextSource ─────────────
    (
        "ChartAnnotationX.Category",
        "key",
        &[("0", "String")],
        POSITIONAL,
    ),
    (
        "ChartAnnotationX.Date",
        "iso",
        &[("0", "String")],
        POSITIONAL,
    ),
    (
        "CurveCommand.LineTo",
        "to",
        &[("0", "DrawPoint")],
        POSITIONAL,
    ),
    (
        "CurveCommand.MoveTo",
        "to",
        &[("0", "DrawPoint")],
        POSITIONAL,
    ),
    ("Scalar.Bool", "value", &[("0", "bool")], POSITIONAL),
    ("Scalar.Float", "value", &[("0", "f64")], POSITIONAL),
    ("Scalar.Int", "value", &[("0", "i64")], POSITIONAL),
    ("Scalar.Str", "value", &[("0", "String")], POSITIONAL),
    (
        "TextSource.Bound",
        "binding",
        &[("0", "Box<Binding>")],
        "carried positionally, and boxed to keep TextSource small: it sits in most specs",
    ),
    (
        "TextSource.I18n",
        "args",
        &[("args", "Vec<(String, I18nArg)>")],
        "the hosted argument is an I18nArg: a literal JSON value or a binding",
    ),
    ("TextSource.Literal", "text", &[("0", "String")], POSITIONAL),
    // ── FormFieldKind ──────────────────────────────────────────────────────
    (
        "FormFieldKind.Checkbox",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.Choice",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.Color",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.Combobox",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.DateTime",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.DateTimeRange",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.Number",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.Range",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.RangedNumber",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.Rating",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.SegmentedChoice",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.Text",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.TextArea",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.Toggle",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    (
        "FormFieldKind.Tokens",
        "value",
        &[("value", "Binding")],
        BOUND_VALUE,
    ),
    // ── TreeOp ─────────────────────────────────────────────────────────────
    ("TreeOp.Batch", "ops", &[("0", "Vec<TreeOp>")], POSITIONAL),
    (
        "TreeOp.EditNode",
        "target",
        &[("target", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.InsertChild",
        "parentId",
        &[("parent_id", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.MoveNode",
        "newParentId",
        &[("new_parent_id", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.MoveNode",
        "target",
        &[("target", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.PatchStyle",
        "target",
        &[("target", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.RemoveNode",
        "target",
        &[("target", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.ReorderChildren",
        "newOrder",
        &[("new_order", "Vec<NodeId>")],
        NODE_ID,
    ),
    (
        "TreeOp.ReorderChildren",
        "parentId",
        &[("parent_id", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.ReplaceBinding",
        "target",
        &[("target", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.UpdateProp",
        "target",
        &[("target", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.UpdateState",
        "target",
        &[("target", "NodeId")],
        NODE_ID,
    ),
    (
        "TreeOp.UpdateStyle",
        "target",
        &[("target", "NodeId")],
        NODE_ID,
    ),
    // ── The node envelope ──────────────────────────────────────────────────
    (
        "Node",
        "state",
        &[("state", "StateBehaviour")],
        "an absent state is the empty StateBehaviour (`is_empty`), which the encoder omits",
    ),
    (
        "Node",
        "style",
        &[("style", "SemanticStyle")],
        "an absent style is the default SemanticStyle (`is_default`), which the encoder \
         omits",
    ),
];

/// Prose the IDL does not carry: the rustdoc of a generated item, keyed by its
/// Rust path — `Type`, `Type.field`, `Type::Variant` or `Type::Variant.field`.
/// The generator emits each as the item's doc comment and refuses a key that names
/// nothing it emits.
pub(super) const NOTES: &[(&str, &str)] = &[
    ("TextSource", r#"A text-producing position (`§3.3`)."#),
    (
        "TextSource::I18n",
        r#"`args` is a name-keyed ARGUMENT map — always emitted, `{}` when empty.
Since Phase 1661 an argument is an [`I18nArg`], not a bare `JVal`."#,
    ),
    (
        "Binding",
        r#"A value binding (`§3.3`), one variant per wire `$type` case."#,
    ),
    (
        "Binding::Query.depends_on",
        r#"Phase 421 — the declared filter edge; omitted when empty."#,
    ),
    (
        "Binding::Filter.default_value",
        r#"0.2.0 — the value the resolver yields (and the renderer seeds the
store with) before the filter is first written. Typed via the
slot's static encoding; omitted when `None`."#,
    ),
    (
        "Binding::Selection.default_value",
        r#"0.2.9 (Phase 629) — the `Filter.defaultValue` convention: yielded
until the user first selects a row on `nodeId`. Omitted when `None`."#,
    ),
    (
        "Binding::Selection.field",
        r#"0.2.10 (Phase 632) — the declarative row-field projection: present
⇒ the accessor projects that field off the clicked row. Omitted
when `None`."#,
    ),
    (
        "Binding::State",
        r#"`default_value` is the RESOLUTION default — the slot's typed
representation an unwritten key yields (§3.3), which at `bool` is
`false` and at a numeric slot is `0`. `default_declared` is the separate
WIRE fact: whether the document actually carried a `defaultValue`.

They are two different facts and conflating them cost a byte-level
round trip: with only the value, an absent default at a typed slot
re-encoded as the fabricated placeholder, so
`{"$type":"State","key":k}` came back as
`{"$type":"State","defaultValue":0,"key":k}`. Carrying the placeholder
AND the declaration keeps resolution unchanged — `visible` on an
unwritten key still resolves `false` and removes the node, per §3.6 —
while the encoder emits only what the document wrote.

Phase 1656 — §5 now states that posture normatively for every host, and
pins it with `nodes/state-absent-default`,
`lenient/lenient-1656-state-default-null` and
`reject/reject-state-default-without-key`. The pair above is what makes
this host's answer expressible: on the WIRE fact it is exactly an
`Option<StaticValue>`, and it additionally carries the resolution
default an `Option` would discard, which `resolve` cannot recompute
because it has no access to the slot. `default_declared` is false for
every spelling of absence — the member missing, the member or either
alias present as `null` — and for a declared value this slot's parser
cannot read, which is the reference host's answer."#,
    ),
    (
        "Binding::Now",
        r#"Phase 765 — the host-furnished current INSTANT is never on the wire.
Resolved once per render pass from the host's pinned ISO-8601 string —
never a clock read in the renderer, so SSR output stays reproducible for
a pinned instant.

Phase 1533 — `grain` is the one thing the wire DOES carry, and only when
it is not the `Second` default, so a grain-less `Now` is still the bare
`{"$type":"Now"}`. It declares the RESOLUTION the document wants: the
host truncates its instant before the value is read, so a `Day`-grain
`Now` is the `YYYY-MM-DD` a day-difference verb accepts."#,
    ),
    (
        "Binding::I18n.args",
        r#"Per-argument bindings; omitted when absent."#,
    ),
    (
        "Binding::Local",
        r#"The controlled input's edit buffer (WIRE_FORMAT.md §3.3.3). `format` /
`parse` / `on_commit` are closures and never arrive at a decoding host,
so a decoded `Local` uses the IDENTITY codec unless `codec` replaces it.

`codec` is a `Format` restricted to the cases with a TOTAL,
LOCALE-INDEPENDENT inverse — `Number` alone today — because whatever the
buffer renders it must also parse back from what the reader typed. It
carries no `LocaleSource` for that reason: a buffer writing `1,234.5`
where the reader types `1.234,5` is the round-trip hole the slot exists
to close.

`commit_to` is the State key the flush writes to, and is MUTUALLY
EXCLUSIVE with the `on_commit` closure — carrying both is a refusal
rather than a precedence rule, because the wire cannot carry the closure
and two hosts would write to different places from identical bytes."#,
    ),
    (
        "Binding::Local.on_commit",
        r#"Presence only — the closure never crosses the wire (§4)."#,
    ),
    (
        "Binding::Transform.params",
        r#"Phase 424 — omitted when empty."#,
    ),
    (
        "Binding::Expr",
        r#"Phase 1534 — ONE scalar expression evaluated to ONE value
(WIRE_FORMAT.md §3.3.2). `expr` is the SAME `ColExpr` algebra a
`Transform` pipeline step carries, in the same encoding: this case
introduces no operator and no expression language of its own, so an
expression means here exactly what it means inside a `derive`.

`params` is the same slot `Transform` carries, omitted when empty, and
it is the reactive edge — a write to any param's source re-evaluates.

Two DECODE refusals, both because an `Expr` has no row: a `col`
reference anywhere in `expr`, and a `param` the binding's own list does
not bind. Left admitted, each would decode to an expression whose
evaluation could only ever fail, once per render, on every host."#,
    ),
    (
        "Binding::Expr.params",
        r#"Omitted when empty, exactly as `Transform`'s is."#,
    ),
    (
        "Format::DateTime",
        r#"Phase 1810 — `dateStyle` / `timeStyle` are BOTH optional: `timeStyle`
alone is a time of day, both together a date-time, `dateStyle` alone the
shape every pre-1810 document carries. Neither present is a validator
concern (FUARAN155 on the reference host), not a decode refusal.
Phase 1811 — `DateTime` (was `Date`): an honest name once it renders a time."#,
    ),
    (
        "Format::Duration",
        r#"Phase 819 — locale-independent duration formatting."#,
    ),
    (
        "Format::Since",
        r#"Phase 1533 — the INSTANT-reading twin of `RelativeTime`.

`RelativeTime`'s source is a signed COUNT of its unit, already computed
by whoever produced it; this one's source is an instant in whole
Unix-epoch seconds (`Date`'s convention) and the count is the delta the
HOST takes against its own furnished instant.

`unit: None` is NOT a default — it is the auto-selection request,
resolved from the fixed threshold ladder in WIRE_FORMAT.md §4b."#,
    ),
    (
        "CallResultTarget",
        r#"The declarative result target of an `Action.Call` (Phase 428)."#,
    ),
    (
        "Action",
        r#"A user-interaction action (`§3.3`), one variant per wire `$type` case."#,
    ),
    (
        "Action::Dispatch",
        r#"The whole message payload is a closure (§4) — presence only."#,
    ),
    (
        "Action::Notify",
        r#"`payload` is a structured-JSON position (rule 12)."#,
    ),
    (
        "Action::Navigate",
        r#"Phase 1536 — `route` is a `TextSource`, not a bare string, so a tree can
name a destination it computes from what the reader is looking at. The
bare JSON string IS `Literal`'s canonical form, so every document
written before the widening decodes exactly as it did.

`target` is the CLOSED enum `Self | Blank`, not HTML's `target`
attribute: that vocabulary also contains `_parent` and `_top`, which are
frame-busting gestures a hosted tree must not be able to ask for."#,
    ),
    (
        "Action::SetState",
        r#"Phase 818 — `value` (a literal, written verbatim) XOR `value_from` (a
Binding evaluated at dispatch time inside the existing gate); decode
enforces exactly one present."#,
    ),
    (
        "Action::WriteToClipboard",
        r#"Phase 1126 — the payload is a `TextSource`. A `text` that is neither a
string nor a `$type`-tagged `TextSource` is `WRONG_TYPE` and is never
coerced: a host that read the widening as "this member is now open"
would put a JSON literal on the reader's clipboard, and a clipboard is a
channel the reader later pastes somewhere with authority."#,
    ),
    (
        "Action::Print",
        r#"Phase 1124 — the payload-free print. The ONE `Action` arm strict about
unrecognised members: page range, size, margins and copies are the
host's page setup and the reader's dialogue, so there is nothing here a
future host could learn, and accepting a member would leave the emitter
believing it had constrained a printing it had not."#,
    ),
    (
        "Action::Confirm",
        r#"Phase 1537 — ask, then act. `prompt` is a `TextSource` so the question
can name what the reader selected; an absent `on_cancel` means "nothing
happens", which an absent action already expresses.

Confirmation is bounded at ONE question — a `Confirm` reachable from
either continuation is refused, and the check walks the DECODED
continuation so a `Chain` cannot hide the nesting."#,
    ),
    (
        "Action::Focus",
        r#"Phase 1537 — a bare node id, the `CommitLocal` shape. It addresses a node
in THIS document, so there is nothing for a binding to compute and no
`TextSource` here."#,
    ),
    (
        "Action::ReadFileBody",
        r#"Only the file id + encoding cross the wire; the blob and `onRead` do not."#,
    ),
    (
        "CellFormat::DateTime",
        r#"Phase 1811 — `DateTime` (was `Date`): the host-formatter pattern it
carries renders a date, a time or both."#,
    ),
    (
        "CellFormat::Duration",
        r#"Phase 819 — trendable duration cells: raw float counts `unit`s,
rendered per `style`."#,
    ),
    (
        "CellFormat::RelativeTime",
        r#"Phase 819 — cell-vocabulary parity with `Format::RelativeTime`."#,
    ),
    (
        "CellFormat::Custom",
        r#"The formatter is a closure (§4) — presence only."#,
    ),
    (
        "MetricSpec.value",
        r#"0.2.0 rename law — a *scalar displayed value* is named `value` on the
wire (`source` is reserved for collection feeds). The retired
`Metric.source` spelling is a hard decode error, not an alias."#,
    ),
    (
        "MetricSpec.trend_polarity",
        r#"Phase 867 — which way this quantity IMPROVES (§3.6.1). Omitted-when-
`HigherIsBetter` on the wire, like the other stylistic slots above, so
the field is a total `TrendPolarity` rather than an `Option`: the wire's
"absent means `HigherIsBetter`" is a DEFAULT, not a third state, and
modelling it as `None` would put a decision back into every reader.

The slot is inert without `trend` — a `Metric` that declares a polarity
and carries no trend is legal and says nothing (§3.6.1 clause 4)."#,
    ),
    (
        "IconSpec",
        r#"Phase 821 — the standalone icon-only display kind. `size`
omitted-when-`Medium`, `tone` omitted-when-`Default`, `label`
omitted-when-decorative (`None` ⇒ `aria-hidden`)."#,
    ),
    (
        "LabelValueRowSpec.value",
        r#"0.2.0 rename law — scalar displayed value ⇒ `value` (see [`MetricSpec`])."#,
    ),
    (
        "LabelValueRowSpec.emphasis",
        r#"The behavioural bool (not the `Emphasis` style DU). 0.2.2 — omitted on
the wire when `false`, aligning with `Fact`'s identical flag."#,
    ),
    (
        "FactSpec",
        r#"The labeled TEXT fact — `Metric`'s complementary type. `value` is a
[`TextSource`] (the same vocabulary the labels use, so `Bound` / `I18n`
values ride through); `emphasis` is the behavioural bool (omit-when-false),
`tone` omit-when-default; `help` / `icon` optional."#,
    ),
    (
        "LinkSpec.protection",
        r#"Omitted on the wire when `None` (every pre-existing tree byte-identical)."#,
    ),
    (
        "SrcSetEntry",
        r#"Phase 1080 — one `srcSet` candidate (§3.6.4): an alternate rendition of the
SAME picture at a declared intrinsic pixel width. Both members are required
*within* the entry, and `width` is a POSITIVE integer — the `w` descriptor a
client selects on, so `0` is refused as firmly as a negative: a `0w`
candidate is not a small image, it is one a client can never select."#,
    ),
    (
        "ImageSpec.fit",
        r#"Phase 1077 (§3.6.2) — omitted at `Natural` on both boundaries."#,
    ),
    (
        "ImageSpec.aspect_ratio",
        r#"Phase 1077 (§3.6.2) — omitted at `Natural` on both boundaries."#,
    ),
    (
        "ImageSpec.loading",
        r#"Phase 1077 (§3.6.2) — omitted at `Eager` on both boundaries."#,
    ),
    (
        "ImageSpec.caption",
        r#"Phase 1078 (§3.6.3) — optional CONTENT, not an identity default, so it
takes the ordinary optional-field posture: omitted when absent. It is a
full [`TextSource`] and not a string, which is the rule a second host is
most likely to break — narrowing the slot costs nothing until somebody
needs a locale."#,
    ),
    (
        "ImageSpec.src_set",
        r#"Phase 1080 (§3.6.4) — the missing-list-field decode class: an ABSENT
`srcSet` IS the empty list, never `None`, and a present `null` is
refused. Absence already has a spelling, and a second one would let two
conformant hosts emit different canonical bytes for one document. The
AUTHORED ORDER is preserved here and on encode; ascending-by-width is
the RENDERER's canonicalisation, not the codec's."#,
    ),
    (
        "ImageSpec.expandable",
        r#"Phase 1079 (§3.6.5) — omitted at `false`. Declares that the full-size
asset is REACHABLE from the rendered image (a real anchor), not that a
lightbox appears; nothing here crosses the dispatch gate."#,
    ),
    (
        "MediaKind",
        r#"Phase 1076 — which media surface a [`MediaSpec`] is (§3.6.6). A
`$type`-DISCRIMINATED union nested at `kind.kind`, so an unknown case
reports at `$.kind.kind.$type` rather than at the bare slot.

**`Audio` has no autoplay pathway — in the type, on the wire, or in the
emission.** That is stronger than a default of `false`: a slot defaulting to
off is one a document can switch on, and there is no document this format
wants to be able to state in which a page begins making sound unbidden. The
case declares no such field, so `{"$type":"Audio","autoplay":true}` decodes
to an audio surface that does not autoplay — the value has nowhere to land,
and no renderer arm below can acquire a branch for it by a later edit."#,
    ),
    (
        "MediaKind::Video.autoplay",
        r#"A DECLARATION whose rendering is constrained: a host that honours it
MUST emit it together with a muted attribute (§3.6.6). There is
deliberately no separate `muted` slot on the wire — a second knob
free to disagree with the first would add only the combination no
host may render."#,
    ),
    (
        "MediaKind::Video.poster",
        r#"A second URL through the §19 floor; a REFUSED one is dropped rather
than neutered (a `<video>` with no poster shows its first frame,
which is a working rendering)."#,
    ),
    (
        "MediaSpec",
        r#"Phase 1076 — the playback surface (§3.6.6). ONE kind, two variants: every
slot a video surface and an audio surface share is stated once here, and
only the genuinely-differing slots live in the [`MediaKind`] variant."#,
    ),
    (
        "MediaSpec.label",
        r#"REQUIRED, and the one place the media contract differs from `Image`'s.
An image can honestly be decorative and say so with an empty `alt`; a
media element is a TRANSPORT — a control a reader focuses, plays, pauses
and seeks — so it is never decorative, and a document omitting it is
refused rather than defaulted to a fabricated name."#,
    ),
    (
        "MediaSpec.controls",
        r#"Omitted at TRUE on the wire (the second such slot in the vocabulary,
after `Toast.dismissable`): a media element without a transport cannot
be paused, seeked or muted by a keyboard user at all, so the accessible
setting is what a document gets for free."#,
    ),
    (
        "MediaSpec.loop",
        r#"Ordinary polarity — omitted at `false`. Named `r#loop` so the field and
the wire key are the same word."#,
    ),
    (
        "MediaSpec.tracks",
        r#"Phase 1110 - the element's timed-text tracks, in the AUTHORED order the
wire carries. Omitted when EMPTY: an absent list and an empty one denote
the same document, so a decoder restores `[]` and never a null."#,
    ),
    (
        "MediaSpec.transcript",
        r#"Phase 1110 - the text alternative. An ordinary optional: absent means the
document offers no transcript, which is a different statement from
offering an empty one."#,
    ),
    (
        "TrackEntry",
        r#"Phase 1110 - one timed-text track (WIRE_FORMAT.md 3.6.6). Four of the five
members are REQUIRED, which makes it the strictest record on the wire;
`default` is the one omitted-at-`false` slot.

`src_lang` is required on EVERY kind, where HTML makes `srclang` mandatory
only on a subtitles track: a track with no language is one nothing downstream
can route, and there is no value to default to that would not be an invented
claim about someone else's recording."#,
    ),
    (
        "EmbedSpec",
        r#"Phase 1111 - the sandboxed third-party embed (WIRE_FORMAT.md 3.6.8).

`title` is REQUIRED: a frame is a focus container a reader tabs INTO, so
there is no decorative embed the way there is a decorative image, and an
invented title is a claim about somebody else's document.

`permissions` is omitted at the EMPTY list, and empty means TOTAL DENIAL -
the wire-cheapest document is also the most locked-down one. `aspect_ratio`
REUSES [`ImageAspect`] rather than minting a parallel enum with identical
cases, and omits at `Natural` for the reason every other omit-at-default slot
does."#,
    ),
    (
        "TreeItem",
        r#"Phase 1120 - one `Tree` row (WIRE_FORMAT.md 3.6.12).

`children` is a list of the SAME record, which makes this the format's first
self-referential shape; it omits at the EMPTY LIST, so a leaf carries two
keys and nothing else. `id` is required because it is what the two State
slots NAME; `label` is a `TextSource` because it is content."#,
    ),
    (
        "TreeSpec",
        r#"Phase 1120 - recursive disclosure with tree semantics (WIRE_FORMAT.md
3.6.12).

The kind carries no `expandable` and no `selectable` boolean, and none is
coming: a behaviour the reader drives is declared as a named State key the
host both writes and reads. `expanded_state_key` names a State value holding
a JSON ARRAY OF ROW IDS (absent => the tree renders FULLY EXPANDED and does
not toggle); `selection_state_key` names one holding a BARE ROW-ID STRING
(absent => the tree does not select and emits no `aria-selected`)."#,
    ),
    (
        "FormFieldKind::Checkbox",
        r#"Phase 2177 — the change handler is `onChange`, as on every form-field
kind (it was `onToggle` on this case and `Toggle`)."#,
    ),
    (
        "FormFieldKind::Toggle",
        r#"Phase 766 — the switch affordance: `Checkbox`'s exact wire mechanics
(bool value slot, optional `onChange`, the write-back default) under a
distinct tag, so the renderer can announce `role="switch"` semantics."#,
    ),
    (
        "FormFieldKind::Combobox",
        r#"Phase 1113 - the searchable form of `Choice` (WIRE_FORMAT.md 3.6.9).
`options` / `value` / `on_change` are `Choice`'s, deliberately and
normatively: the constrained combobox IS a searchable select, so a
document migrating between the two changes its `$type` and nothing else,
and a host implementing a different value contract here would break
exactly that migration.

`allow_free_text` omits at `false`, and the polarity is load-bearing -
the SHORTEST combobox document is the CONSTRAINED one, and admitting
off-list values is the thing an emitter has to ask for."#,
    ),
    (
        "FormFieldKind::Range",
        r#"0.2.0 — the dual-thumb numeric range control (absorbed the retired
`FilterKind.RangeFilter`). A `Static` pair rides as the bare
`{"max":…,"min":…}` object; optional bounds omitted when absent."#,
    ),
    (
        "FormFieldKind::DateTime",
        r#"Phase 1811 — `DateTime` (was `Date`): the field already took a date, a
time of day or both via `variant`; the name now says so."#,
    ),
    (
        "FormFieldKind::DateTimeRange",
        r#"0.7.0 — the single-control date range (`DateTimeRange` since Phase 1811):
`Range`'s pair mechanics with `DateTime`'s value conventions (an identical
field list to `DateTime`). A `Static`
pair rides as the bare `{"from":…,"to":…}` object; `variant` is always
emitted; `min`/`max` (ISO strings) and `step` (seconds) bound BOTH ends and
are omitted when absent. In a filter context the pair binds ONE filter
param, not two — the reason the case exists rather than two coordinated
`DateTime` fields."#,
    ),
    (
        "FormFieldKind::Tokens",
        r#"Phase 1121 — SEVERAL values accumulated as removable chips, over a
suggestion set that may be open, searchable, asynchronous, or absent
entirely (WIRE_FORMAT.md §3.6.19).

Every member is optional, so `{"$type":"Tokens"}` is a complete
document. `allow_free_text` omits at **`true`** — the OPPOSITE polarity
to `Combobox`, and the one thing about this case a host is most likely
to get wrong: `Combobox.options` is required so "constrained" is its
resting state, where `suggestions` is optional so "open" is this one's.
The default follows the required-ness of the set.

`value` is a `Binding<string list>` and the list is ORDERED — chips
appear where the reader added them, so a host must not sort or
de-duplicate it."#,
    ),
    (
        "FormFieldKind::Rating",
        r#"Phase 1130 — a subjective score on a small ordinal scale
(WIRE_FORMAT.md §3.6.17). The line against `RangedNumber` is who the
number belongs to: a rating is a judgement a person GIVES, a ranged
number is a measurement they REPORT.

`max` is the case's only required member and is refused below 1 — a
scale with no positions has nothing to draw and no keystroke that could
change anything. `value` is a float even where nothing can type a
fraction, because the commonest rating a reader sees is an AVERAGE
arriving through a `Query`. `allow_half` governs ENTRY, never display."#,
    ),
    (
        "FormFieldKind::Color",
        r#"Phase 1130 — the platform's own colour picker (WIRE_FORMAT.md §3.6.17).
A CONTROL, not a `rule.format`: a swatch that opens the operating
system's picker, which no format on a text field can produce.

Both members optional. The value is `#rrggbb` and nothing else — the one
form a native colour input can hold or return — and case is PRESERVED
rather than normalised, so `#FFAA00` round-trips byte-identically."#,
    ),
    (
        "CompareRule",
        r#"The cross-field operand. `against` is a `Binding`, and that IS the cross-field
mechanism rather than an accident of typing: any read slot may take a Binding,
and the auto-bind rule already puts every form field's value in State under the
field's own id, so `{"$type":"State","key":"<sibling id>"}` reads the sibling
with no coordination vocabulary at all."#,
    ),
    (
        "FieldRule",
        r#"A field's declared constraint — the ACCEPTED SET, where `FormFieldKind` names
the control. Every slot is optional structurally; the two well-formedness
refusals are relations BETWEEN slots and so live in the decoder's policy layer
rather than in this shape."#,
    ),
    (
        "FilterSpec",
        r#"0.2.0 filters-unification: a filter chip's control is an ordinary
[`FormFieldKind`] — the parallel `FilterKind` DU (`TextFilter` /
`ChoiceFilter` / `RangeFilter` / `SegmentedFilter`) is retired, its
discriminators a hard `UNKNOWN_DU_CASE`. A chip control with no `value`
key decodes to the auto binding `Filter(<chip name>)`."#,
    ),
    (
        "SelectSpec.multiple",
        r#"Emitted only when `true` (Phase 291), so single-select stays byte-identical."#,
    ),
    (
        "FileUploadSpec.drop_target",
        r#"Phase 1115 - an ADDITIONAL ingress route, never a replacement for the
picker. Omits at `false`, and the polarity is load-bearing: the shortest
upload document is the plain picker, which is what every document written
before this revision says."#,
    ),
    (
        "FileUploadSpec.accept_paste",
        r#"Phase 1115 - the paste ingress route. Same polarity, same reason."#,
    ),
    (
        "FileUploadSpec.capture",
        r#"Phase 1116 — WHICH of the reader's own recording devices the platform
should open in place of the file browser (WIRE_FORMAT.md §3.6.18). The
third ingress route, and the only one that PRODUCES a file rather than
moving one that already exists.

OPTIONAL rather than omit-at-default, and the distinction is real: "say
nothing" is a state of its own, because an upload naming no device is
asking for the file browser, which is not one of the two devices wearing
a default."#,
    ),
    (
        "FileUploadSpec.destination",
        r#"Phase 1117 — the host-registered destination an upload streams to
(§3.6.20). A NAME, and a name because it must never be an ADDRESS: a
wire document comes from an arbitrary emitter, and a URL here would let
that emitter choose where a reader's file goes.

The empty string is REFUSED rather than read as absence — reading it as
absence silently turns an upload the author meant to stream into a
client-only one while every visible thing about the control still works.
An UNREGISTERED non-empty id is deliberately not a decode refusal:
whether an id is registered is a fact about the HOST, so a decoder that
judged it would make one document's validity depend on who read it."#,
    ),
    (
        "FileUploadSpec.max_bytes",
        r#"Phase 1548 — the largest single file this control accepts, in bytes
(§3.6.23). PER FILE, not per selection: it bounds each file the reader
picks, which is what makes it the quantity the `file-read` route can be
measured against and what makes it meaningful on a single-file upload. A
control bounding a whole multiple selection states both members and
declares `max_bytes * max_files`.

Absent declares NO ceiling — the pre-1548 control, bounded only by
whatever the host already enforces. A present value is POSITIVE, and the
floor is a decode rule rather than a type because this wire has no
refined-integer form; zero is refused as firmly as a negative, since a
ceiling of zero is a control that can accept nothing.

`i64` here carries a slot the format declares as a signed 32-BIT integer
(§7.1), exactly as every other integer member of this model does — the
range is enforced at decode, not by the Rust type."#,
    ),
    (
        "FileUploadSpec.max_files",
        r#"Phase 1548 — how many files this control accepts in one selection.
Absent declares no ceiling. Meaningful only alongside `multiple`: a
single-file upload admits one file by construction, so a ceiling there is
INERT rather than wrong, and is documented rather than refused. Positive
on the same terms as `max_bytes`."#,
    ),
    (
        "CellKindErased::Editable",
        r#"`onEdit` closure — always emitted."#,
    ),
    (
        "CellKindErased::Checkbox",
        r#"`get` + `onToggle` closures — always emitted."#,
    ),
    (
        "CellKindErased::Button",
        r#"`onClick` closure — always emitted."#,
    ),
    (
        "CellKindErased::ButtonGroup",
        r#"Each button's `onClick` closure — always emitted."#,
    ),
    (
        "CellKindErased::TonedPill",
        r#"Phase 750 — the declarative twin of `Pill`, and the ONE cell kind holding no
closure, which is exactly why it survives the wire. `field` names the row
property that is both the pill's label and the map key; `map` carries value →
tone; `default_tone` covers a value the map does not mention and is omitted on
the wire at `ToneVariant::Default`.

A `BTreeMap` rather than a `Vec` of pairs deliberately: the tone map is a
key-addressed set, so two spellings differing only in key order are the SAME
map, and a `Vec`'s derived `PartialEq` would call them different — a difference
the round-trip property tests would surface as a phantom divergence."#,
    ),
    (
        "ColumnErased.value",
        r#"Phase 425 — `value` (closure) + `field` (declarative) are sibling slots."#,
    ),
    (
        "ColumnErased.sortable",
        r#"Phase 861 — per-column sort NARROWING on the bound path. Absent inherits
the grid-level intent, which is why an explicit `false` is carried on the
wire rather than omitted-when-default: omitting it would erase the
narrowing the author declared."#,
    ),
    (
        "ColumnErased.editable",
        r#"Phase 863 — per-column editability NARROWING; absent inherits the
grid-level `editable` flag. Deliberately not spelled `readOnly`: an
inverting alias that guesses wrong makes a read-only column editable."#,
    ),
    (
        "StaticRows.sortable",
        r#"Phase 801 — the table INVITES interactive column sorting. Intent, not a
behaviour guarantee: a host with no sorting affordance renders the
authored order and stays conformant."#,
    ),
    (
        "StaticRows.default_sort",
        r#"Phase 801 — the table's initial order. Configuration, distinct from the
transform pipeline's data `sort`."#,
    ),
    (
        "DefaultSort",
        r#"Phase 801 — a static table's declared initial order. `column` is a
non-negative index into `StaticRows::headers`; an index past the end is
deliberately not a decode error (a sibling-value relation is not something a
per-object codec judges)."#,
    ),
    (
        "GridSpec.sort_state_key",
        r#"Phase 818 — the grid-sort header affordance for a data-bound grid:
names the State key carrying the sort descriptor
`{"column": <index>, "direction": "asc"|"desc"}` the runtime sorts its
resolved rows by (and whose headers write it). Omitted on the wire when
absent; `static_rows`' own Phase-801 sort intent is untouched."#,
    ),
    (
        "GridSpec.default_sort",
        r#"Phase 861 — the BOUND path's declared initial order, the same record the
`static_rows` spelling carries at its own path."#,
    ),
    (
        "GridSpec.page_size",
        r#"Phase 862 — how many rows a page holds. A page of zero or fewer rows
names no page at all, so a non-positive value is a decode error."#,
    ),
    (
        "GridSpec.page_state_key",
        r#"Phase 862 — names the State key carrying `{"page": N}` (1-based). The
page POSITION lives in State so a pager can move it; a literal page
number is deliberately not expressible."#,
    ),
    (
        "GridSpec.edit_state_key",
        r#"Phase 863 — the declared edit DESTINATION: a State key on the grid, not
a per-cell host closure (which carries no destination across the wire)."#,
    ),
    (
        "GridSpec.reorderable",
        r#"Phase 934 — declarative row reorder; omitted-when-false, the same
convention as `editable`. Edits and reorders share one destination."#,
    ),
    (
        "GridSpec.keep_rows_together",
        r#"Phase 1473 — a PAGED-medium declaration, omitted at `false`. It names the
one fact a host cannot recover from a rendering: a formatter laying out
pages sees boxes, and nothing in the rendering carries back that the
three lines of a totals block are ONE THING that reads wrong when halved.
"Not stated" and "explicitly off" are the same state, so there is no
third value and a present member of the wrong JSON kind is REFUSED
rather than coerced.
No row of this grid is split across a page boundary. Applied to the
grid's ROWS, never to the grid as a whole — which is why it is a separate
member from `BoxSpec.keep_together` rather than reachable by wrapping."#,
    ),
    (
        "GridSpec.exportable",
        r#"Phase 1123 — this grid's rows may be taken out of the page as a file.
Omitted at `false`, so every pre-1123 document is byte-identical."#,
    ),
    (
        "GridSpec.transfer_in_key",
        r#"Phase 1125 — the two sides of ONE shared State key. A grid declaring
`transfer_out_key` K may RELEASE rows onto K; one declaring
`transfer_in_key` K ACCEPTS rows arriving on it. They are separate
decoder arms, and the corpus vectors them separately for that reason."#,
    ),
    (
        "GridSpec.repeat_header",
        r#"Phase 1473 — the column headers repeat at the top of every page the grid
continues onto. What survives on `DataGrid` is precisely what no
arrangement of existing kinds reaches."#,
    ),
    (
        "GridSpec.window_state_key",
        r#"Phase 1892 — names the State key carrying the row-window descriptor
`{"offset": N, "count": M}`. Omitted on the wire when absent. The
server renderer applies the window (Phase 1912)."#,
    ),
    (
        "GridSpec.row_total",
        r#"Phase 1892 — the declared row total, an int `Binding` (the same slot
shape as `Tabs.activeIndex` / `Stepper.activeStep`). Omitted on the
wire when absent."#,
    ),
    (
        "ChartSpec.value_format",
        r#"Phase 876 — the VALUE axis's number format, reusing the existing
`Format` vocabulary. A semantic declaration ("these numbers are pounds /
a ratio"), which is why it is a wire field where the chart STYLE is not.
Absent means no declared meaning — the lowering still applies its
canonical default rendering."#,
    ),
    (
        "ChartSpec.x_title",
        r#"Phase 878 — the x axis's NAME. On the same side of the semantic line as
`title`: what an axis is CALLED is the author's meaning, where and how it
is drawn stays the host's. Absent falls back to the capitalised field
name, so an axis is never nameless."#,
    ),
    (
        "ChartSpec.y_title",
        r#"Phase 878 — the value axis's NAME. Absent falls back to the capitalised
FIRST y-field; with no y-fields at all there is no honest fallback and no
y title draws."#,
    ),
    (
        "ChartSpec.subtitle",
        r#"Phase 878 — a muted qualifier line under the visible `title`. Its
PRESENCE also suppresses the display-unit slot: the subtitle is the
author's own place to state a unit, and the machine restating it two
lines away is the clutter the rule exists to prevent."#,
    ),
    (
        "ChartSpec.legend_position",
        r#"Phase 880 — WHERE the legend sits, and whether it sits anywhere at all.
Semantic for the same reason the titles above are (D8): the edge an author
wants the legend on is their meaning; the column widths and pitches that
realise it are the host's.

Absent means "the host's default" (`Right`) — NOT "no legend"; suppression
is the explicit [`ChartLegendPosition::None`]. So absence stays the
ordinary shape and is omitted on the wire, and an author who wants no
legend says so."#,
    ),
    (
        "ChartSpec.data_labels",
        r#"Phase 881 — whether the values are written onto the picture. Semantic in
the same way (D8): whether a reader is meant to read the NUMBERS or the
shape is the author's meaning; the type size, the offsets and the fit rule
that decide whether a given label draws are the host's.

Absent means [`ChartDataLabels::Off`], which is ALSO the default — the one
place this differs from `legend_position`, deliberately: a legend is chrome
an author opts OUT of, where a data label is ink an author opts IN to. So an
absent field is byte-identical to the pre-881 wire and to the pre-881
picture."#,
    ),
    (
        "ChartSpec.x_scale",
        r#"Phase 882 — what the x column MEANS: discrete categories, or dates on a
continuous temporal scale. Semantic in the same way (D8): whether a column
is a set of categories or a run of dates is a fact about the data; the tick
ladder, the label format and the margins that realise it are the host's.

Absent means [`ChartXScale::Category`], which is also the shipped default,
so an absent field is byte-identical to the pre-882 wire AND to the pre-882
picture. `Temporal` is a DECLARATION the pre-emit validator grounds against
the column type (FUARAN097) — never an inference from the data, which would
make the same tree draw differently depending on where its rows came from."#,
    ),
    (
        "ChartSpec.annotations",
        r#"Phase 1490 — the data-addressed annotations (§4l of
`docs/CHARTS-DRAWING-PRIMITIVE-DESIGN.md`): reference lines, event markers
and range bands, one closed union so a further member is a case rather
than a further widening of this record. Semantic in the same way the
fields above are (D8): WHERE in the data a threshold or an episode sits is
the author's meaning; the stroke weights, opacities and label offsets that
draw it are the host's.

Absent OMITS on the wire, so every pre-1490 chart is byte-identical on the
wire AND in the picture. An EMPTY list is a DIFFERENT document from an
absent field and round-trips as `"annotations":[]`."#,
    ),
    (
        "ChartAnnotationX",
        r#"Phase 1491 — an annotation's X ADDRESS (§4l "The three addressing forms"), in
the two forms the x axis already distinguishes: a `Category` key naming a
band, and an ISO-8601 `Date` read under `ChartXScale::Temporal`.

Its own type rather than two inline fields, because Phase 1492's range band
addresses an x-axis interval with a PAIR of these."#,
    ),
    (
        "ChartAnnotationRange",
        r#"Phase 1492 — a `RangeBand`'s PAIR (§4l "The three addressing forms", third
row): two of the same address form, on one axis.

THE AXIS IS THE CASE. §4l requires a band to declare which axis it sits on;
carrying that as a separate flag beside an untyped pair would admit a document
declaring the value axis and addressing it with two category keys. The union
tag declares the axis AND types the pair with it, so that document cannot be
written by any conformant emitter.

Both ends enter the domain before the axis is nice-d (§4l rule 3), and an
UNORDERED pair is refused rather than silently normalised — a band written
backwards is an author's mistake about their own data, and swapping the ends
would draw a picture they did not describe."#,
    ),
    (
        "ChartAnnotation",
        r#"Phase 1490 — a chart's data-addressed annotation (§4l). One closed union, so
the lowering's three arms are an EXHAUSTIVE `match` and a fourth member is a
compile error at every site rather than a silent omission at one.

An annotation names a place in the DATA's coordinates and, optionally, a
label; it carries no geometry and no style at all. That is what makes it
survive a data change, a theme flip, a restyle and a resize.

`ReferenceLine` is a horizontal line at `value` in the VALUE axis's own units;
`EventMarker` is a VERTICAL line at an x address, the mirror of the reference
line across the axes; `RangeBand` is a shaded interval on either axis and is
the one member that draws BEHIND every series. Every label rides the Phase
1143 text contract — carried, never resolved."#,
    ),
    (
        "BoxLayout",
        r#"How a `Box` arranges its children (Phase 390)."#,
    ),
    (
        "BoxLayout::Masonry",
        r#"WIRE_FORMAT §3.6.7 (Phase 1082) — the column-fill mode. `cols` is
REQUIRED and POSITIVE; there is deliberately no `template_columns` twin,
because the multi-column model realising masonry has no track list for
one to name."#,
    ),
    (
        "BoxSpec.keep_together",
        r#"Phase 1473 — a PAGED-medium declaration, omitted at `false`. It names the
one fact a host cannot recover from a rendering: a formatter laying out
pages sees boxes, and nothing in the rendering carries back that the
three lines of a totals block are ONE THING that reads wrong when halved.
"Not stated" and "explicitly off" are the same state, so there is no
third value and a present member of the wrong JSON kind is REFUSED
rather than coerced.
This container and its whole subtree stay on one page."#,
    ),
    (
        "BoxSpec.break_before",
        r#"Phase 1473 — this container starts at the top of a fresh page. There is
deliberately no break-AFTER member anywhere: a break after this container
is a break before the next one."#,
    ),
    (
        "ModalSpec.on_dismiss",
        r#"A wire-survivable `Action` (not a closure sentinel); optional since Phase 426."#,
    ),
    (
        "ModalSpec.modality",
        r#"WIRE_FORMAT.md 3.6.11 - omitted at `Modal`, the blocking modality every
pre-modality document meant. Only the blocking modality carries the
`aria-modal` inertness claim."#,
    ),
    (
        "ModalSpec.anchor",
        r#"WIRE_FORMAT.md 3.6.11 - the id of the element a `Popover` is positioned
against. An ordinary optional: a popover with no anchor is positioned by
the host's own default."#,
    ),
    ("CustomSpec.props", r#"A structured-JSON map (rule 12)."#),
    (
        "CustomSpec.exposed_node_ids",
        r#"Emitted only when non-empty."#,
    ),
    (
        "SwitchCase.condition",
        r#"EXACTLY ONE of `match_value` and `when` is present (WIRE_FORMAT.md
§3.6) — both together and neither at all are decode errors, the same
shape and the same reasoning as `SetState`'s `value` / `valueFrom` pair.

A case naming no condition is not a case that never matches; it is a
document whose author meant something the wire cannot say, and a host
that silently skipped it would render the `default` and report nothing.
A precedence rule for "both" would have to be specified, agreed on every
host and remembered by every author, for a document nobody meant to
write."#,
    ),
    (
        "SwitchSpec.on",
        r#"The branch selector — any Binding (Phase 768): a `State` selector keeps
the Phase 392 behaviour; a `Selection` selector lets the branch follow
the clicked row with no writer. The no-default `State` form keeps its
compact `stateKey` spelling on the wire (the encoder collapses it
back); any other selector encodes under an `on` key holding the
canonical Binding wire form."#,
    ),
    (
        "SwitchSpec.auto_advance_ms",
        r#"Phase 1122 — the timed carousel: advance to the next case every
this-many milliseconds, omitted at absence. It declares the one fact a
host cannot recover from the tree — every other half of a carousel was
already composable, and nothing in any arrangement of those says a timer
exists.

A DURATION, never a flag: "advances" with no interval would make a host
invent a period. Non-positive and fractional values are REFUSED rather
than canonicalised — `0` is what an emitter reaches for to mean "off"
and the language already has a spelling for off, an absent key."#,
    ),
    ("DrawPoint", r#"A point in `Drawing` coordinate space."#),
    ("ViewBox", r#"The SVG-style view box of a `Drawing`."#),
    (
        "DrawStyle",
        r#"Presentation style shared by every `Shape`; every field is optional (an
all-`None` style emits `{}`). The text-only fields (`text_anchor` /
`font_size` / `emphasis` / `font_family`, Phase 528.1) apply only to `Label`."#,
    ),
    (
        "DrawStyle.mark_id",
        r#"Phase 642 — keyed mark identity: a data-bearing shape's derivation-based
id (`series-field|category-key`), stable under row reorder and data
refresh (object constancy). Renderers emit it as `data-fuaran-mark`;
omitted-when-`None` (rule 4). Chrome (axes, gridlines, labels, legend)
deliberately stays unstamped."#,
    ),
    (
        "DrawStyle.rotation",
        r#"Phase 877 — clockwise `Label` text rotation in DEGREES about the label's
own anchor point (`Shape::Label`'s `x` / `y`). Renderers emit
`transform="rotate(θ x y)"` on the `<text>` element, so the anchor is the
pivot and `text_anchor` keeps its meaning in the rotated frame.

Applies only to `Label`, like the other text-only fields — and here that
is load-bearing rather than cosmetic: an SVG `transform` on a `<rect>`
would MOVE GEOMETRY, so this is the one text field whose uniform
emission would silently distort a drawing.

Omitted-when-`None` (rule 4). `Some(0.0)` is a legitimate PRESENT value
and a different wire shape from `None` — never collapse the two."#,
    ),
    (
        "DrawStyle.tip",
        r#"Phase 883 — the mark's hover-readable text. Renderers emit it as an SVG
`<title>` element that is the FIRST CHILD of that shape's own element:
the native browser tooltip and the element's accessible name, with no
script, so a statically-served page carries the readout too.

Unlike the Phase 528.1 text cluster and `rotation`, this applies to
EVERY shape, not just `Label` — the marks a reader hovers are bars,
wedges and points, and a `<title>` child is inert geometry-wise
everywhere.

The emission consequence to get right: a TIPPED shape cannot be emitted
self-closing — `<rect …/>` becomes `<rect …><title>…</title></rect>`.
An absent tip leaves the emitted bytes unchanged, self-closing tag
included.

Omitted-when-`None` (rule 4). `Some(TextSource::Literal(""))` is a
legitimate PRESENT value and a different wire shape from `None` — never
collapse the two."#,
    ),
    (
        "CurveCommand",
        r#"A single SVG path command inside a `Shape::Curve`. Closed + typed — an
unrecognised `$type` is `UNKNOWN_DU_CASE` at decode."#,
    ),
    (
        "Shape",
        r#"The closed, typed vector-graphics vocabulary (no raw-SVG / attribute-bag
escape hatch). An unrecognised `$type` is `UNKNOWN_DU_CASE` at decode."#,
    ),
    (
        "DrawingSpec",
        r#"A bounded, typed vector-graphics primitive (`DisplayKind.Drawing`). Geometry
is static numbers — a resolved artefact; only `DrawStyle` carries `Binding`s."#,
    ),
    (
        "NodeKind",
        r#"The node's primitive kind: the closed, flat `kind.$type` vocabulary. The
behavioural categories (Layout / Display / Input / Visualisation /
Structural) are a host-side classification (see [`NodeKind::category`]), not
a wire level.

The variants are written here because each carries a hand-modelled spec; the
SET is the IDL's. `NodeKind::type_name` and `NodeKind::category` are
generated from the IDL's kind rows as matches with no catch-all arm, so a
kind on one side and not the other does not compile."#,
    ),
    (
        "StateBehaviour.on_error",
        r#"The `ErrorPayload -> Node` callback is a closure (§4) — presence only."#,
    ),
    (
        "SemanticStyle.role",
        r#"Phase 147 — omitted on the wire at its default (`None`)."#,
    ),
    (
        "SemanticStyle.voice",
        r#"Phase 147 — omitted on the wire at its default (`Default`)."#,
    ),
    (
        "SemanticStyle.direction",
        r#"Phase 1472 - the DECLARED text direction, omitted at `Auto`. A node
declaring one is a bidi ISOLATION boundary: the host emits `dir` on the
node's own element so a run of the opposite direction cannot reorder the
text around it."#,
    ),
    (
        "StyleDelta",
        r#"Phase 2291 — the payload of `PatchStyle`, the field-level style edit: the six
`SemanticStyle` fields, each optional. `Some` SETS the field (a default value
included — it is spelled explicitly on the wire); `None` leaves the field as
it is. This is the difference from `UpdateStyle`, whose whole-record write
reads an absent member as the default."#,
    ),
    (
        "Accessibility.role",
        r#"The raw ARIA role string (named roles and the custom escape both encode
as the raw string, §10.2)."#,
    ),
    (
        "Accessibility.speak",
        r#"Phase 1812 — the node's SPOKEN rendering for a voice surface: an ordinary
`TextSource` (bare-string canonical for a literal, as `tooltip`). Inert to
every visual renderer: it MUST reach no attribute and no visible text."#,
    ),
    (
        "Node",
        r#"The typed UI tree node (§3.1). `motion` / `extraAttributes` are wire-omitted
by design (§9) and therefore not modelled by this host."#,
    ),
    (
        "Node.state",
        r#"Restored to empty when the wire omits it (the common case)."#,
    ),
    (
        "Node.style",
        r#"Restored to all-default when the wire omits it (the common case)."#,
    ),
    (
        "Node.tooltip",
        r#"Phase 1112 - the node-level tooltip TRAIT (WIRE_FORMAT.md 3.6). A hint is
uniform across kinds, so it sits on the envelope beside `accessibility`
rather than in any kind. It is a DESCRIPTION, never a NAME: a host
projects it as `aria-describedby` and never as `aria-label`."#,
    ),
    (
        "Node.visible",
        r#"Phase 1535 — whether the node is PRESENT in the rendered output at all
(WIRE_FORMAT.md §3.6). A node-level trait for the reason the trait tier
exists: "should this be here at all" is uniform across every kind, and
forty-odd per-spec spellings of it would be forty-odd independently
driftable decisions about one concept.

It is NOT `accessibility.hidden`, and the two have opposite polarity: a
resolved `false` here emits NOTHING — no element, no layout, no
accessibility-tree entry — where `hidden` is `aria-hidden` over a node
that IS rendered. Content the reader should not have now takes this;
decoration the reader should never hear takes `hidden`.

Any outcome other than a resolved `false` renders the node, unresolved
and errored included: hiding on absence is the one failure a reader
cannot see, cannot report and cannot work around."#,
    ),
    (
        "Node.fallback",
        r#"Phase 1812 — the author-declared degraded rendering: a full node a BEHIND
reader (one that meets this node's kind as a transport-only `Unknown`)
renders in place of its placeholder. A current reader decodes, preserves
and re-encodes it byte-for-byte and never renders it. Omitted when absent."#,
    ),
    (
        "TreeOp",
        r#"A tree edit, one variant per top-level wire `$type` case."#,
    ),
    (
        "TreeOp::UpdateProp",
        r#"`value` is a structured-JSON position (rule 12); `path` is a plain
string at codec level (grammar violations surface at apply time, §3.4)."#,
    ),
    (
        "TreeOp::PatchStyle",
        r#"Phase 2291 — the field-level style edit: a patch over the target's
current style, setting exactly the fields `style` names."#,
    ),
];

// ── Codec hooks (Phase 2333) ─────────────────────────────────────────────────
//
// The generator writes the UNIFORM half of the codec (`gen_encode` /
// `gen_decode` in the generated module). Every position whose wire handling is
// POLICY is declared here, naming the hand function in `encode.rs` or
// `decode.rs` that handles it; a field with no entry is coded by the uniform
// rules. Owner keys are the ones `FIELDS` uses.

/// What a hook departs from, which decides when the generator refuses it as stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Hook {
    /// The uniform rules have no codec for the position: its Rust members are a
    /// `FIELDS` departure, or its default is no generated constant. Refused as
    /// stale once the rules cover it.
    Shape,
    /// The uniform rules would code it, and this host's behaviour differs on
    /// purpose (leniency, a refusal, a fold, a typed slot).
    Policy,
}

/// When an owner-level decode check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Check {
    /// Before any field is read: `fn(path, fields) -> DResult<()>`.
    Pre,
    /// After the owner is built: `fn(path, fields, &Owner) -> DResult<()>`.
    Post,
}

/// `(owner, IDL field, hand function, kind, reason)`. An encode hook takes a
/// reference to each Rust member the field maps to and returns the field's wire
/// value, or `None` to omit it.
pub(super) const ENCODE_HOOKS: &[(&str, &str, &str, Hook, &str)] = &[
    (
        "Custom",
        "exposedNodeIds",
        "encode_exposed_node_ids",
        Hook::Shape,
        "optional in the IDL and a plain list here: an empty list is not written",
    ),
    (
        "FragmentDecl",
        "effect",
        "encode_fragment_effect",
        Hook::Shape,
        "omitted at the IDL's record default, which the defaults table does not render",
    ),
    (
        "FragmentRef",
        "args",
        "encode_fragment_args",
        Hook::Shape,
        "optional in the IDL and a plain map here: an empty map is not written",
    ),
    (
        "Mount",
        "inputs",
        "encode_fragment_args",
        Hook::Shape,
        "optional in the IDL and a plain map here: an empty map is not written",
    ),
    (
        "Stepper",
        "activeStep",
        "encode_stepper_active_step",
        Hook::Shape,
        "omitted at its default, the literal binding 0, which the defaults table does not render",
    ),
    (
        "Tabs",
        "activeIndex",
        "encode_tabs_active_index",
        Hook::Shape,
        "omitted at its default, the literal binding 0, which the defaults table does not          render",
    ),
    (
        "Accessibility",
        "role",
        "encode_aria_role",
        Hook::Shape,
        "a hosted JSON position this host carries as the role's token string",
    ),
    (
        "MapMarker",
        "label",
        "encode_text_label",
        Hook::Shape,
        "the host reads a full TextSource where the IDL types a string",
    ),
    (
        "SelectOption",
        "label",
        "encode_text_label",
        Hook::Shape,
        "the host reads a full TextSource where the IDL types a string",
    ),
    (
        "Binding.Static",
        "value",
        "encode_static_payload",
        Hook::Shape,
        "absence is a StaticValue sentinel here, so an absent payload omits the key",
    ),
    (
        "Binding.State",
        "defaultValue",
        "encode_state_default",
        Hook::Shape,
        "written exactly when the document declared it (the default_declared member)",
    ),
    (
        "Binding.Query",
        "dependsOn",
        "encode_depends_on",
        Hook::Policy,
        "an empty list says nothing and is not written",
    ),
    (
        "Binding.Expr",
        "params",
        "encode_binding_params",
        Hook::Policy,
        "§3.3 omits the slot when empty: Some(empty) is the same fact as None",
    ),
    (
        "Binding.Transform",
        "params",
        "encode_binding_params",
        Hook::Policy,
        "§3.3 omits the slot when empty: Some(empty) is the same fact as None",
    ),
    (
        "CellKindErased.ButtonGroup",
        "buttons",
        "encode_button_group",
        Hook::Shape,
        "each item is inlined to its label; the click handler is the sentinel",
    ),
    (
        "CellKindErased.TonedPill",
        "map",
        "encode_tone_map",
        Hook::Shape,
        "a BTreeMap lookup table, written as an object",
    ),
    (
        "Node",
        "state",
        "encode_node_state",
        Hook::Shape,
        "optional in the IDL and a plain record here: only a non-empty state is written",
    ),
    (
        "Node",
        "style",
        "encode_node_style",
        Hook::Shape,
        "optional in the IDL and a plain record here: only a non-default style is written",
    ),
];

/// `(owner, IDL field, hand function, kind, reason)`. A decode hook is
/// `fn(path, fields) -> DResult<_>` returning the members the field maps to (a
/// tuple for several, `()` for none).
pub(super) const DECODE_HOOKS: &[(&str, &str, &str, Hook, &str)] = &[
    (
        "DataGrid",
        "source",
        "decode_grid_source",
        Hook::Policy,
        "the rows feed decodes through the Rows payload slot; `data` and `rows` are its foreign spellings",
    ),
    (
        "Chart",
        "source",
        "decode_chart_source",
        Hook::Policy,
        "the rows feed decodes through the Rows payload slot; `data` is its foreign spelling",
    ),
    (
        "Select",
        "value",
        "decode_select_value",
        Hook::Policy,
        "the selection decodes through the StringOpt payload slot (no selection is a value), and the \
         single/multi rule applies: required unless `multiple` is true, dropped beside it",
    ),
    (
        "Custom",
        "exposedNodeIds",
        "decode_exposed_node_ids",
        Hook::Shape,
        "optional in the IDL and a plain list here: absent is the empty list",
    ),
    (
        "FragmentDecl",
        "effect",
        "decode_fragment_effect",
        Hook::Shape,
        "absent is the IDL's record default, which the defaults table does not render",
    ),
    (
        "FragmentRef",
        "args",
        "decode_fragment_ref_args",
        Hook::Shape,
        "optional in the IDL and a plain map here: absent is the empty map",
    ),
    (
        "Mount",
        "inputs",
        "decode_mount_inputs",
        Hook::Shape,
        "optional in the IDL and a plain map here: absent is the empty map",
    ),
    (
        "Stepper",
        "activeStep",
        "decode_stepper_active_step",
        Hook::Shape,
        "absent is the literal binding 0, which the defaults table does not render",
    ),
    (
        "Switch",
        "on",
        "decode_switch_on",
        Hook::Shape,
        "folds `stateKey` in: absent, the compact spelling names a State binding; an explicit `on` decodes untyped",
    ),
    (
        "Switch",
        "stateKey",
        "decode_switch_state_key",
        Hook::Shape,
        "folded into `on`, whose hook reads it",
    ),
    (
        "Tabs",
        "activeIndex",
        "decode_tabs_active_index",
        Hook::Shape,
        "absent is the literal binding 0, which the defaults table does not render",
    ),
    (
        "Accessibility",
        "role",
        "decode_aria_role",
        Hook::Shape,
        "a hosted JSON position this host carries as the role's token string",
    ),
    (
        "MapMarker",
        "label",
        "decode_text_label",
        Hook::Shape,
        "the host reads a full TextSource where the IDL types a string",
    ),
    (
        "SelectOption",
        "label",
        "decode_text_label",
        Hook::Shape,
        "the host reads a full TextSource where the IDL types a string",
    ),
    (
        "CellKindErased.ButtonGroup",
        "buttons",
        "decode_button_group",
        Hook::Shape,
        "each button object contributes its label; the click handler is the sentinel",
    ),
    (
        "CellKindErased.TonedPill",
        "map",
        "decode_toned_pill_map",
        Hook::Shape,
        "a BTreeMap lookup table under its canonical key or a foreign one",
    ),
    (
        "Node",
        "id",
        "decode_node_id",
        Hook::Policy,
        "an empty id is EMPTY_NODE_ID",
    ),
    (
        "Node",
        "state",
        "decode_node_state",
        Hook::Shape,
        "optional in the IDL and a plain record here: absent is the empty behaviour",
    ),
    (
        "Node",
        "style",
        "decode_node_style",
        Hook::Shape,
        "optional in the IDL and a plain record here: absent is the default style",
    ),
    (
        "Fact",
        "emphasis",
        "decode_fact_emphasis",
        Hook::Policy,
        "the behavioural bool, which a model also spells with the Emphasis style tokens",
    ),
    (
        "LabelValueRow",
        "emphasis",
        "decode_row_emphasis",
        Hook::Policy,
        "the behavioural bool, which a model also spells with the Emphasis style tokens",
    ),
    (
        "SrcSetEntry",
        "width",
        "decode_src_set_width",
        Hook::Policy,
        "a positive integer pixel width: zero and negative are WRONG_TYPE",
    ),
    (
        "FormField",
        "kind",
        "decode_form_field_control",
        Hook::Policy,
        "context: an absent control value auto-binds to the field id",
    ),
    (
        "FilterSpec",
        "kind",
        "decode_filter_control",
        Hook::Policy,
        "context: an absent control value auto-binds to the chip name",
    ),
    (
        "Shape.Group",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "Shape.Rectangle",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "Shape.Line",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "Shape.Polyline",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "Shape.Polygon",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "Shape.Curve",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "Shape.Circle",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "Shape.Ellipse",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "Shape.Label",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "Drawing",
        "style",
        "decode_style_or_default",
        Hook::Policy,
        "lenient: absent is the default style",
    ),
    (
        "BoxLayout.Grid",
        "cols",
        "decode_grid_cols",
        Hook::Policy,
        "absent beside a template is one column; `columns` is its foreign spelling",
    ),
    (
        "TextSource.Bound",
        "binding",
        "decode_text_bound",
        Hook::Policy,
        "read untyped: the bound value is rendered as text whatever it carries",
    ),
    (
        "TextSource.I18n",
        "args",
        "decode_text_i18n_args",
        Hook::Policy,
        "absent is the empty bag; each argument is discriminated by inspection",
    ),
];

/// `(owner, hand function, kind, reason)`: the whole owner encoded by hand. The
/// function takes a reference to every member of the owner, so a member the IDL
/// gains changes its arity.
pub(super) const ENCODE_OWNERS: &[(&str, &str, Hook, &str)] = &[
    (
        "Filters",
        "encode_filters_kind",
        Hook::Shape,
        "the variant holds the items list directly",
    ),
    (
        "Switch",
        "encode_switch_spec",
        Hook::Shape,
        "`on` and `stateKey` fold into one binding: an undeclared-default State binding is          written as the `stateKey` shorthand",
    ),
    (
        "SwitchCase",
        "encode_switch_case",
        Hook::Shape,
        "`match` and `when` fold into one SwitchCondition",
    ),
    (
        "FormFieldKind",
        "form_field_kind",
        Hook::Policy,
        "context: a control whose value is the auto-binding its field id or chip name          implies omits it, so the encoder takes the enclosing ControlAutoBind",
    ),
    (
        "FormField",
        "encode_form_field",
        Hook::Policy,
        "its control's auto-bound value is omitted, which needs the field id",
    ),
    (
        "FilterSpec",
        "encode_filter_spec",
        Hook::Policy,
        "its control's auto-bound value is omitted, which needs the chip name",
    ),
];

/// `(owner, hand function, kind, reason)`: the whole owner decoded by hand,
/// `fn(path, j) -> DResult<Owner>` (for a union case, `fn(path, fields)`). A hand
/// union decoder dispatches over the generated `<Union>Tag`.
pub(super) const DECODE_OWNERS: &[(&str, &str, Hook, &str)] = &[
    (
        "Filters",
        "decode_filters_kind",
        Hook::Shape,
        "the variant holds the items list directly",
    ),
    (
        "SwitchCase",
        "decode_switch_case_folded",
        Hook::Shape,
        "`match` and `when` fold into one SwitchCondition: exactly one is present",
    ),
    (
        "Binding",
        "",
        Hook::Policy,
        "context: every case decodes through the enclosing slot's StaticSlot (typed Static payloads and defaults), plus the bare-scalar and `Bound` spellings; the hand decoder dispatches over BindingTag",
    ),
    (
        "FormFieldKind",
        "",
        Hook::Policy,
        "context: an absent control value auto-binds to the enclosing field id or chip name, plus the Date/Time tag spellings; the hand decoder dispatches over FormFieldKindTag",
    ),
    (
        "Action.SetState",
        "decode_set_state",
        Hook::Policy,
        "exactly one of `value` and `valueFrom`",
    ),
    (
        "Action.Print",
        "decode_print",
        Hook::Policy,
        "the payload-free action refuses any member beside $type",
    ),
    (
        "Emphasis",
        "decode_emphasis",
        Hook::Policy,
        "also reads a bool (true is Loud) and the Strong/Bold/Subtle/Muted spellings, through the Static envelope",
    ),
    (
        "BoxRole",
        "decode_box_role",
        Hook::Policy,
        "reads through the Static envelope like a plain scalar",
    ),
    (
        "HashStrictness",
        "decode_hash_strictness",
        Hook::Policy,
        "reads through the Static envelope like a plain scalar",
    ),
    (
        "HostEffect",
        "decode_host_effect",
        Hook::Policy,
        "reads through the Static envelope like a plain scalar",
    ),
    (
        "DeterminismSource",
        "decode_determinism_source",
        Hook::Policy,
        "reads through the Static envelope like a plain scalar",
    ),
    (
        "ChannelDirection",
        "decode_channel_direction",
        Hook::Policy,
        "reads through the Static envelope like a plain scalar",
    ),
];

/// `(owner, hand function, when, reason)`: an owner-level decode check.
pub(super) const DECODE_CHECKS: &[(&str, &str, Check, &str)] = &[
    (
        "FormField",
        "check_form_field_near_misses",
        Check::Pre,
        "the near-miss refusals (validation/constraints/validate for rule)",
    ),
    (
        "ColumnErased",
        "check_column_near_misses",
        Check::Pre,
        "the grid vocabulary's near misses (readOnly)",
    ),
    (
        "DataGrid",
        "check_grid_near_misses",
        Check::Pre,
        "the grid-behaviour near misses (paging, sorting, editing spellings)",
    ),
    (
        "Accessibility",
        "check_a11y_near_misses",
        Check::Pre,
        "the ARIA attribute spellings of each member",
    ),
    (
        "TreeOp.InsertChild",
        "check_insert_child",
        Check::Pre,
        "the retired positional slot `position` is refused by name",
    ),
    (
        "TreeOp.MoveNode",
        "check_move_node",
        Check::Pre,
        "the retired positional slot `newPosition` is refused by name",
    ),
    (
        "Skeleton",
        "check_skeleton",
        Check::Post,
        "the §21.9 row bound",
    ),
    (
        "FieldRule",
        "check_field_rule",
        Check::Post,
        "a rule constrains something, and its length bounds are not inverted",
    ),
    (
        "FileUpload",
        "check_file_upload",
        Check::Post,
        "a named destination is non-empty and each ceiling positive",
    ),
    (
        "DefaultSort",
        "check_default_sort",
        Check::Post,
        "a non-negative header index",
    ),
    (
        "DataGrid",
        "check_grid",
        Check::Post,
        "a page holds at least one row",
    ),
    (
        "ChartAnnotationX.Date",
        "check_annotation_date",
        Check::Post,
        "a canonical ISO day",
    ),
    (
        "ChartAnnotationRange.ValueRange",
        "check_value_range",
        Check::Post,
        "finite ends, in order",
    ),
    (
        "ChartAnnotationRange.XRange",
        "check_x_range",
        Check::Post,
        "two dates run forwards",
    ),
    (
        "ChartAnnotation.ReferenceLine",
        "check_reference_line",
        Check::Post,
        "a finite value",
    ),
    (
        "BoxLayout.Masonry",
        "check_masonry",
        Check::Post,
        "a positive column count",
    ),
    (
        "StyleDelta",
        "check_style_delta",
        Check::Post,
        "a patch names at least one member",
    ),
    (
        "Action.Confirm",
        "check_confirm",
        Check::Post,
        "no confirmation nested inside another",
    ),
    (
        "Switch",
        "check_switch",
        Check::Post,
        "a positive auto-advance interval",
    ),
];

/// `(owner, hand function, reason)`: `fn(path) -> DResult<Guard>`, held while the
/// owner decodes.
pub(super) const DECODE_ENTER: &[(&str, &str, &str)] = &[
    (
        "Node",
        "enter_node",
        "the §21 node-depth and total-node bounds",
    ),
    ("TreeOp", "enter_op", "the §21.5 op-axis bound"),
    ("TreeItem", "enter_tree_item", "the tree item nesting bound"),
];

/// `(union, hand function, reason)`: `fn(path, j) -> Option<DResult<Union>>`, tried
/// before the discriminator is read (a shorthand spelling of a case).
pub(super) const UNION_PRE: &[(&str, &str, &str)] = &[
    (
        "SelectOption",
        "select_option_shorthand",
        "a bare string is the option whose value and label are that string",
    ),
    (
        "Format",
        "format_date_alias",
        "`Date` is the foreign spelling of `DateTime`",
    ),
    (
        "CellFormat",
        "cell_format_date_alias",
        "`Date` is the foreign spelling of `DateTime`",
    ),
    (
        "CellKindErased",
        "pill_with_tone_map",
        "a `Pill` carrying a tone map is the declarative TonedPill",
    ),
    (
        "BoxLayout",
        "grid_without_columns",
        "a `Grid` naming neither a column count nor a template is `Auto`",
    ),
];

/// `(owner, IDL field, foreign spellings, reason)`: names a model writes for the
/// field, decoded to it when the canonical name is absent and never encoded.
pub(super) const DECODE_ALIASES: &[(&str, &str, &[&str], &str)] = &[
    (
        "Action.Call",
        "endpoint",
        &["url"],
        "a spelling models write for this field",
    ),
    (
        "Action.Navigate",
        "route",
        &["href", "url", "to"],
        "a spelling models write for this field",
    ),
    (
        "Metric",
        "value",
        &["data"],
        "a spelling models write for this field",
    ),
    (
        "LabelValueRow",
        "value",
        &["data"],
        "a spelling models write for this field",
    ),
    (
        "Sparkline",
        "source",
        &["data"],
        "a spelling models write for this field",
    ),
    (
        "Callout",
        "heading",
        &["title"],
        "a spelling models write for this field",
    ),
    (
        "FormField",
        "id",
        &["name"],
        "a spelling models write for this field",
    ),
    (
        "Select",
        "source",
        &["options", "data"],
        "a spelling models write for this field",
    ),
    (
        "ColumnErased",
        "kind",
        &["type"],
        "a spelling models write for this field",
    ),
    (
        "ColumnErased",
        "label",
        &["header", "title"],
        "a spelling models write for this field",
    ),
    (
        "Map",
        "source",
        &["data", "markers"],
        "a spelling models write for this field",
    ),
    (
        "BoxLayout.Masonry",
        "cols",
        &["columns"],
        "a spelling models write for this field",
    ),
    (
        "Box",
        "heading",
        &["title"],
        "a spelling models write for this field",
    ),
    (
        "SummaryList",
        "heading",
        &["title"],
        "a spelling models write for this field",
    ),
    (
        "Disclosure",
        "heading",
        &["title"],
        "a spelling models write for this field",
    ),
    (
        "Modal",
        "heading",
        &["title"],
        "a spelling models write for this field",
    ),
];

/// `(enum, (foreign spelling, case), reason)`: a bare-enum decoder's foreign
/// spellings of a case, decoded to it and never encoded.
pub(super) type EnumAliases = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

pub(super) const ENUM_ALIASES: &[EnumAliases] = &[
    (
        "Orientation",
        &[
            ("Row", "Horizontal"),
            ("row", "Horizontal"),
            ("Column", "Vertical"),
            ("column", "Vertical"),
        ],
        "the CSS flex-direction prior: a row lays out horizontally, a column vertically",
    ),
    (
        "BadgeVariant",
        &[("Default", "Neutral"), ("Danger", "Critical")],
        "`Default` is the universal no-special-variant prior; `Danger` the Bootstrap prior for Critical",
    ),
    (
        "ButtonVariant",
        &[("Danger", "Destructive")],
        "Bootstrap's `Danger` names the same concept as Destructive",
    ),
    (
        "HeadingVariant",
        &[("Default", "Standard")],
        "`Default` is the identity case's prior",
    ),
    (
        "ToneVariant",
        &[
            ("Positive", "Success"),
            ("Danger", "Critical"),
            ("Negative", "Critical"),
            ("Neutral", "Default"),
        ],
        "faithful semantic mappings only",
    ),
];
