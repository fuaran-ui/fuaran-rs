//! The typed wire tree — every closed wire DU modelled as a native Rust `enum`,
//! one variant per `$type` case. The types themselves are GENERATED
//! (`generated.rs`, from the corpus's `idl.json` through this host's dialect,
//! `dialect.rs`); this file holds what the IDL does not define — the
//! host-modelled types the dialect names, and the behaviour impls. This is the Rust host's distinctive reach: a
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

// The structural layer generated from the corpus's `idl.json` — the bare-string
// enums, the kind and discriminator sets and every wire type. Re-exported from
// here so every path that named a model item still does.
pub use super::generated::*;

/// A node identity string (non-empty by decode invariant, §8).
pub type NodeId = String;

/// Marks a closure-bearing slot whose function value was present on the wire.
/// The value itself is unobservable (§4) — only presence round-trips, as the
/// `"<closure>"` sentinel. Slots whose closure is *always* emitted carry no
/// field at all; `Option<Closure>` models the optional-handler slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closure;

// ─── Bare-string enums (§3.5) ────────────────────────────────────────────────
//
// The enums themselves are GENERATED (`generated.rs`, from the corpus's
// `idl.json`): the case set, the host case names and the wire spellings are the
// IDL's, never this file's. What the IDL does not carry is WHY a vocabulary has
// the shape it has, and that stays here, beside the model it explains.
//
// `ChartLegendPosition` —
// Phase 880 — which edge the chart's legend occupies, or `None`, which suppresses
// it entirely. A WIRE vocabulary on the same side of the D8 line as `title`: WHERE
// an author wants the legend is their meaning, where the geometry that puts it
// there (column widths, pitches, how the plot shrinks) stays the host's, in the
// lowering's `ChartLowerStyle`. Absent means "the host default" (`Right`), NOT "no
// legend" — suppression is the explicit `None`.
//
// `ChartDataLabels` —
// Phase 881 — whether a chart writes its values onto the picture, and where. A
// WIRE vocabulary on the semantic side of the D8 line: whether the reader is
// meant to READ THE NUMBERS or read the shape is the author's meaning; the type
// size, the offsets and the fit rule that realise it stay the host's, in the
// lowering. THE CASE SET IS TWO, AND THAT IS THE POINT — there is deliberately
// no all-points case, so no shape of this enum can request a number on every
// interior point. Absent means `Off`, which is also the default.
//
// `ChartXScale` —
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
//
// `ImageFit` —
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
//
// `ImageLoading` —
// `Eager` is the default DELIBERATELY, and it is not the "unoptimised" value:
// deferring an above-the-fold image delays the largest contentful paint rather
// than helping it, and only the author knows where the image sits. So `Eager`
// emits no attribute at all and leaves the browser's own default in place,
// while `Lazy` is a positive declaration. A host MUST NOT infer laziness from
// position, viewport, or anything else the tree does not say.
//
// `TrackKind` —
// Phase 1110 - the closed timed-text track vocabulary (WIRE_FORMAT.md 3.6.6).
// `metadata` is deliberately NOT a case: its cues are rendered by no user agent
// and read only by script, so a declarative document naming it would state an
// intent no conformant host could honour. The set is closed at four; a fifth is
// an addition, not a spelling a decoder may guess at.
//
// `EmbedPermission` —
// Phase 1111 - the closed sandbox-relaxation vocabulary (WIRE_FORMAT.md 3.6.8).
// A BARE enum, so an unrecognised token reports at the ELEMENT's own path with
// no `$type` suffix. A decoder MUST NOT silently drop an unrecognised
// permission: that would turn a document asking for something this vocabulary
// has no name for into a document asking for LESS, which reads as success.
//
// `ModalityKind` —
// WIRE_FORMAT.md 3.6.11 - the modality a `Modal` declares. Omitted at `Modal`,
// which is the blocking modality every pre-modality document meant.
//
// `TextDirection` —
// Phase 1472 - the node-level declared text direction (WIRE_FORMAT.md 3.1).
// Lower-case on the wire, like `LiveRegionKind` and `SortDirection`, because
// these are the HTML `dir` tokens themselves. Omitted at `Auto`, which leaves
// the inherited direction in place - the pre-1472 rendering.
//
// `LinkProtection` —
// Anti-scraper render strategy for a `Link` — `email` marks a `mailto:` link
// whose address must not appear in plaintext in emitted markup (the renderers
// own the emission strategy). Lower-case on the wire, like `SortDirection`.
//
// `DateTimeVariant` —
// Phase 1811 — `DateTimeVariant` (was `DateVariant`): the temporal breadth of a
// `DateTime` / `DateTimeRange` field. The three cases did not move.
//
// `NavigateTarget` —
// Phase 1536 — `Action::Navigate`'s destination window. A CLOSED enum of two,
// deliberately not HTML's `target` attribute: that vocabulary also carries
// `_parent` and `_top`, which are frame-busting gestures a hosted tree must not
// be able to ask for, so `_blank` is refused rather than aliased — accepting
// any of it would teach an emitter the wrong vocabulary. Omitted at `Self_`,
// which is the pre-1536 behaviour.
//
// `CaptureSource` —
// Phase 1116 — the recording device a `FileUpload` asks the platform for
// (WIRE_FORMAT.md §3.6.18). A BARE enum, so an unrecognised spelling reports at
// `$.kind.capture` with no `.$type` suffix. There is deliberately no
// display-capture case and there will not be one by widening this member: a
// screen capture reaches every window the reader has open rather than one
// device behind the picker, so it is a different class of thing.
//
// `TextFormat` —
// `FormFieldKind` names the CONTROL; `FormField.rule` names the ACCEPTED SET.
//
// `SortDirection` —
// Phase 801 — the closed sort direction on `staticRows.defaultSort`. Lower-case on
// the wire, like `LiveRegionKind` and unlike most enums here.
//
// `TimeStyle` —
// Phase 1810 — the time-of-day half of `Format::DateTime`'s `dateStyle` /
// `timeStyle` pair. The same four cases as `DateStyle`, deliberately a SEPARATE
// enum: the two name breadths of different things.
//
// `TimeGrain` —
// Phase 1533 — the resolution a `Binding::Now` declares for the host-furnished
// instant. FOUR members and not `RelativeTimeUnit`'s seven, deliberately: this
// is a TRUNCATION of a calendar instant, and a week, a month or a year has no
// truncation every host agrees on (which weekday starts a week; which
// calendar). The four here truncate the canonical ISO-8601 form by prefix.
//
// `DurationUnit` —
// Phase 819 — the Duration format enums (`Format.Duration` / `CellFormat.Duration`):
// the numeric source counts `unit`s, rendered per the bounded `style`.
//
// `IconSize` —
// Phase 821 — the standalone `Icon` display kind's size modifier.
//
// `TrendPolarity` —
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

// ─── Host-modelled types (named by the dialect) ──────────────────────────────

/// One `TextSource.I18n` argument (§5, Phase 1661).
///
/// The slot is discriminated BY INSPECTION on the wire — an object carrying a
/// `$type` member is the binding arm, ANY other JSON value the literal arm — so
/// it is discriminated by variant here, and a literal encodes BARE, which is
/// why every literal-args document ever emitted is byte-identical across the
/// widening.
#[derive(Debug, Clone, PartialEq)]
pub enum I18nArg {
    /// The LITERAL argument: a bare JSON value, faithful at any depth within
    /// the §21 limits (rule 12), and rejecting a `null` at the null's own path.
    ///
    /// A variant of its own rather than `Binding::Static`, for a reason worth
    /// stating where the choice is made: that variant carries a [`StaticValue`]
    /// whose untyped `Ast` arm collapses an array or object to `"<opaque>"`
    /// under rule 11 — correct at an obj-erased seam, and wrong here, where it
    /// would silently narrow what this host can round-trip at a position rule 12
    /// promises is faithful.
    Literal(JVal),
    /// A store-reading argument, carrying its own `$type` envelope on the wire —
    /// including a valueless `Static`, whose absence is structural (Phase 677)
    /// and has no bare spelling.
    Bound(Binding),
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

// ─── Compute layer (Phases 282/284/424) ──────────────────────────────────────

// The dataframe model is a Core twin, so it lives behind the Core boundary
// (`crate::core::dataframe`) and is re-exported here: every `wire::` path to it
// is unchanged, and the codec in this module reads it like any other wire type.
pub use crate::core::dataframe::*;

/// Phase 1535 — the two spellings of a case's condition. They interleave
/// freely in one ordered `cases` array, and first-match-wins runs over the
/// array in AUTHORED order: a host evaluates case *n* fully before considering
/// case *n+1*, and must not batch all the matches ahead of all the predicates.
#[derive(Debug, Clone, PartialEq)]
pub enum SwitchCondition {
    /// Compares the switch's resolved selector against a literal string.
    Match(String),
    /// Evaluates a `Binding<bool>` and takes the case on a RESOLVED `true`
    /// only — a resolved `false`, an unresolved binding and an errored one all
    /// fall through. There is no truthiness rule: `0`, `""` and `"false"` are
    /// refused by the coercion rather than read as `false`.
    ///
    /// It consults no selector at all, so a switch whose cases are ALL
    /// predicates needs no `on` and has no state key for anything to write.
    When(Binding),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FragmentArg {
    Value(FragmentScalar),
    Slot { tree: Box<Node> },
}

/// The behavioural category a primitive belongs to — recovered on decode, never
/// a wire level. The IDL spells `Structural` as `Meta`; the public name is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeCategory {
    Layout,
    Display,
    Input,
    Visualisation,
    Structural,
}

// ─── Behaviour ───────────────────────────────────────────────────────────────

impl EffectClass {
    pub const PURE_DETERMINISTIC: EffectClass = EffectClass {
        host_effect: HostEffect::Pure,
        determinism: DeterminismSource::Deterministic,
    };

    pub fn is_pure_deterministic(&self) -> bool {
        *self == Self::PURE_DETERMINISTIC
    }
}

impl StateBehaviour {
    pub fn is_empty(&self) -> bool {
        self.on_loading.is_none() && self.on_empty.is_none() && self.on_error.is_none()
    }
}

impl Default for SemanticStyle {
    fn default() -> Self {
        SemanticStyle {
            emphasis: Emphasis::Normal,
            tone: ToneVariant::Default,
            weight: StyleWeight::Standard,
            role: StyleRole::None,
            voice: FontVoice::Default,
            direction: TextDirection::Auto,
        }
    }
}

impl SemanticStyle {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl StyleDelta {
    /// A delta naming no field — not an edit, and refused on decode.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The delta patched over `style`: every field the delta sets replaces the
    /// style's value, every other field is carried through unchanged.
    pub fn apply_to(&self, style: SemanticStyle) -> SemanticStyle {
        SemanticStyle {
            direction: self.direction.unwrap_or(style.direction),
            emphasis: self.emphasis.unwrap_or(style.emphasis),
            role: self.role.unwrap_or(style.role),
            tone: self.tone.unwrap_or(style.tone),
            voice: self.voice.unwrap_or(style.voice),
            weight: self.weight.unwrap_or(style.weight),
        }
    }
}
