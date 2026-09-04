# Authoring Fuaran trees in Rust

`fuaran-rs` is the **dual-role host** of the Fuaran UI wire format. The same
dependency-light crate serves a native backend, an edge worker, an embedded
target — and, compiled to `wasm32`, a browser-native client that renders and
drives a tree with no JavaScript frontend. Nothing in the authoring surface
changes between those roles; what changes is where the session lives.

This page is the authoring reference: the typed model, the round trip, and the
mutation algebra. For the browser leg see [The WASM browser client](WASM-CLIENT.md).

Names first, because the split catches people out: the crate publishes to
crates.io as **`fuaran-ui`**, this repository is **`fuaran-rs`**, and the import
path is **`fuaran_rs`**. So `cargo add fuaran-ui`, then `use fuaran_rs::…`.

## The headline: the wire's closed DUs are native `enum`s

Every closed vocabulary in the wire format — `NodeKind`, the per-kind `Spec`
records, `Binding`, `Action`, `TextSource`, `TreeOp` — is a native Rust `enum`
with one variant per `$type` case. That is not a stylistic choice, and it is the
one structural advantage this host has over every other:

```rust
pub enum NodeKind {
    // Layout
    Box(BoxSpec),
    SplitPanel(SplitPanelSpec),
    Tabs(TabsSpec),
    // … Display, Input, Visualisation, Structural
    Mount(MountSpec),
}
```

A `match` over it with no `_` arm is **checked at compile time**. When a kind is
added to the wire format, every such match stops compiling — the codec arm, the
renderer arm, the class-name arm, each one named by the compiler with its file
and line. In a language without sum types that same change is a runtime fallback
nobody notices until a fixture reds, and the exhaustiveness guard a host has to
build to compensate is a test rather than a proof.

The crate leans on this deliberately. `NodeKind::type_name` and
`NodeKind::category` are written as `default`-free matches precisely so that a
new kind cannot slip through them, and the same discipline runs through the
encoder, the decoder and the renderer.

**So when you write your own walk over a decoded tree, do not add a `_ => …`
arm.** You are giving up the guarantee the host is built around, and the cost
lands on whoever adopts the next wire version.

## Building a tree

A node is a struct; its kind is an enum variant carrying that kind's spec:

```rust
use fuaran_rs::wire::{
    encode_node, Accessibility, BoxLayout, BoxRole, BoxSpec, HeadingSpec,
    HeadingVariant, MarkdownSpec, Node, NodeKind, TextSource,
};

let tree = Node {
    id: "root".into(),
    kind: NodeKind::Box(BoxSpec {
        children: vec![
            Node {
                id: "title".into(),
                kind: NodeKind::Heading(HeadingSpec {
                    level: 2,
                    text: TextSource::Literal("Channel performance".into()),
                    variant: HeadingVariant::Standard,
                }),
                state: Default::default(),
                style: Default::default(),
                accessibility: None,
                tooltip: None,
            },
            Node {
                id: "note".into(),
                kind: NodeKind::Markdown(MarkdownSpec {
                    text: TextSource::Literal("**Hello** from a Fuaran tree.".into()),
                }),
                state: Default::default(),
                style: Default::default(),
                accessibility: None,
                tooltip: None,
            },
        ],
        heading: None,
        layout: BoxLayout::Auto,
        role: BoxRole::Dashboard,
        keep_together: false,
        break_before: false,
    }),
    state: Default::default(),
    style: Default::default(),
    accessibility: Some(Accessibility { role: Some("main".into()), ..Default::default() }),
    tooltip: None,
};
```

Two things in there are recent additions and are the reason a spec struct is worth
re-reading rather than remembering. `BoxSpec`'s `keep_together` and `break_before`
are paged-medium declarations — this container stays on one page; this container
starts a fresh one — and both omit at `false` on the wire, so an ordinary screen
tree carries neither. There is deliberately no break-*after* member anywhere: a
break after this container is a break before the next one.

A spec struct is a plain `struct` with no `Default`, so the compiler names every
field you left out. That is the intended experience: when the wire format grows a
member, your construction sites stop compiling and you decide what each should
say, rather than silently inheriting someone else's zero value.

The `Node` envelope is:

```rust
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub state: StateBehaviour,        // restored to empty when the wire omits it
    pub style: SemanticStyle,         // restored to all-default when the wire omits it
    pub accessibility: Option<Accessibility>,
    pub tooltip: Option<TextSource>,  // a DESCRIPTION, never a name
}
```

`state`, `style`, `accessibility` and `tooltip` are envelope siblings of `kind`,
not fields inside a spec. `Default::default()` on the first two is the common
case and encodes to nothing — the canonical form omits what is default, so a
plainly-styled node produces the bare `{"$type":"…", …}` object.

`tooltip` is worth one sentence of its own: it is a hint, uniform across kinds,
which is why it sits on the envelope. A host projects it as `aria-describedby`
and **never** as `aria-label` — it describes, it does not name.

### Text and bindings

`TextSource` has three cases:

```rust
pub enum TextSource {
    Literal(String),
    Bound(Box<Binding>),
    I18n { key: String, args: Vec<(String, JVal)> },
}
```

A `Literal` is the canonical spelling for constant text — on the wire it is a
bare string, not an envelope. Anything live goes through `Binding`, whose cases
include `Static`, `State { key, default_value }`, `Query { name, depends_on }`,
`Filter { name, default_value }`, `Selection { node_id, default_value, field }`,
`Format`, `Transform`, `I18n`, `Local`, `Computed`, and the host-furnished
`Now`.

`Now` deserves a note because it is the one binding that looks like a clock read
and is not: it resolves once per render pass from a host-pinned ISO-8601 string,
never from a call into the system clock inside the renderer. That is what keeps
server-side output reproducible for a pinned instant.

## Encoding and decoding

```rust
pub fn encode_node(n: &Node) -> String;
pub fn decode_node(json: &str) -> Result<Node, DecodeError>;
pub fn encode_op(op: &TreeOp) -> String;
pub fn decode_op(json: &str) -> Result<TreeOp, DecodeError>;
```

`encode_node` is infallible — a value of type `Node` is a tree that exists, so
there is nothing left to fail on. That is the type system paying for itself: the
structural hosts have to return a `Result` here because their model admits
documents the wire does not.

`decode_node` returns a `DecodeError` carrying the canonical code and a
`$`-rooted path, the same codes every conformant host surfaces for the same
input: `INVALID_JSON`, `MISSING_FIELD`, `WRONG_TYPE`, `UNKNOWN_DU_CASE`,
`WRONG_NODE_KIND`, `EMPTY_NODE_ID`, `LIMIT_EXCEEDED`, plus the out-of-band
`FOREIGN_PROFILE` for a versioning envelope this host cannot speak.

The corpus round-trip legs in `tests/conformance.rs` hold the byte-level
contract: every node and op fixture re-encodes **byte-identically**, and every
reject fixture surfaces the canonical code at the expected path.

## Validating before you emit

Decoding checks input. `validator` checks a tree you built:

```rust
use fuaran_rs::validator::{validate, validate_with, Severity, ValidateOptions};

for f in validate(&tree) {
    eprintln!("{:?} {} on {}: {}", f.severity, f.code, f.node_id, f.message);
}
```

```rust
pub struct Finding {
    pub severity: Severity,   // Warning | Error
    pub code: &'static str,   // FUARAN###, or EMPTY_NODE_ID for the §8 identity re-check
    pub node_id: String,
    pub message: String,
}
```

`validate_op` does the same for a single op. `validate_with(&tree,
ValidateOptions { orchestrated: true })` hardens the wire-survivability
advisories (`FUARAN084`) from Warning to Error — the mode to use when the tree
was emitted by a model rather than written by a person, where an escape that
merely *usually* survives is not good enough.

The codes are cross-host, so a finding here is the finding a reviewer on any
other host would get.

## Mutating a tree

```rust
use fuaran_rs::ops::{apply, can_apply};

let outcome = apply(&tree, &op)?;   // ApplyOutcome { new_tree, emitted_telemetry }
let next = outcome.new_tree;
```

`apply` is total: a refusal is a structured `ApplyError { code, message,
batch_index }`, never a panic, and the input tree is borrowed rather than
consumed so nothing is lost on a refusal. A `Batch` is atomic — it applies whole
or not at all, and `batch_index` names the inner op that refused.

`can_apply` is the dry run, and the apply-envelope law (`can_apply` ≡ `apply`
succeeds) is pinned by `tests/apply.rs` rather than assumed.

### `InsertChild` and `MoveNode` append — there is no positional slot

Both ops put the node **last**, and a wire document carrying `position` or
`newPosition` is *refused* (`WRONG_TYPE` at `$.position` / `$.newPosition`),
checked before the required-field reads. The slot is retired vocabulary, not an
optional extra.

Placing anywhere else is two ops: `Batch [InsertChild …, ReorderChildren …]`.
Rather than assemble that by hand, `ops::placement` derives it:

```rust
use fuaran_rs::ops::placement::{move_op, Placement, Target};

let target = Target::new("sidebar", Placement::Before("filters".into()));
let op = move_op(&tree, "legend", &target)?;
```

`Placement` is `Last | First | Before(id) | After(id)`. Beside `move_op` sit
`place_op`, `nudge_op`, `can_place` (the pre-check, no dry-run apply) and the
clone verbs `duplicate_op` / `paste_op`, which mint fresh ids through a
`FreshIds` seam — `DerivedIds` (`<id>-copy`, probing) by default, or
`SequentialIds::new(prefix)` when you need deterministic replay.

These are helpers, not new wire: what comes out is an ordinary op any host
applies, and a `PlaceError` names the apply-time refusal it pre-states
(`MoveIntoDescendant`, `UnknownAnchor`, `ChildlessKind`, and the rest).

## What this host renders in-host

Two postures worth knowing, because they differ from the other backend host and
the difference is deliberate:

- **A `Chart` is lowered in-host.** This host takes the *lower-in-host* posture:
  a `Chart` reaching the renderer is deterministically lowered to a canonical
  `Drawing` and painted as inline SVG. The reason is the browser leg — a WASM
  client that could not paint a chart would not reach parity with the clients it
  sits beside, so the lowering earns its pixels here where in a purely headless
  host it would not.
- **A `Sparkline` is lowered in-host too, and through the same builder.** The
  series is lowered to a canonical `Drawing` (`render::sparkline_lowering`) and
  painted as inline SVG inside a `fuaran-sparkline` container — so the picture
  agrees with every other host's by construction rather than by a hand-written
  copy kept in step. An unresolved or empty series keeps the em-dash placeholder:
  that fallback is a *host* element rather than a `Shape`, so the lowering
  reports it in the type (`None`) instead of drawing an empty canvas nobody can
  read.

  The `fuaran-sparkline` class is a CONTAINER, not the drawn element. That is
  where the 100×30 sizing and the inherited `color` the lowering's
  `currentColor` stroke reads have always lived, and it is what the byte-copied
  reference stylesheet targets — `.fuaran-sparkline > .fuaran-drawing`, a
  DIRECT-child selector, which is why the arm splices the bare svg rather than
  reusing the `Drawing` node's own `<div>` wrapper.

  The contract is `wire-format-fixtures/sparkline-lowering/*`, and
  `tests/sparkline_lowering.rs` certifies every committed vector byte-for-byte
  plus the wiring that reaches it. Before that suite this arm had **no** test at
  all — the corpus round-trip walk exercises the decode of a `Sparkline` and says
  nothing about the picture.

## The crate's shape

`crate-type = ["lib", "cdylib", "staticlib"]`, and all three matter:

- **`lib`** — native consumers and the test suite.
- **`cdylib`** — the `wasm32` browser module and native dynamic consumers.
- **`staticlib`** — `libfuaran_rs.a`, which the native Swift and Kotlin surfaces
  link over the C-ABI in `src/ffi/` (declared in `include/fuaran.h`). Those
  surfaces decode and render; this crate owns truth and mutation for them.

Dependencies: **none**. Rust's standard library has no JSON, so the canonical
layer is hand-written — which it would have to be anyway, since no general
serialiser produces the byte-exact number and key layout the wire requires.

Edition 2024, `rust-version = "1.85"`. The host is pre-1.0 (`fuaran_rs::VERSION`
is `0.0.2-alpha`) and declares no `STABILITY.md`; that version carries a breaking
change to the DAG record surface, so pin deliberately.

## Verifying

```powershell
.\run.ps1                   # cargo fmt --check -> clippy -> build -> test -> wasm build
.\run.ps1 -SkipTests        # switches: -SkipFormat / -SkipBuild / -SkipTests
.\run.ps1 -DriverSemantics  # adds the bounded-loop conformance legs
```

`run.ps1` builds the `wasm32` module automatically when the target is installed
(`rustup target add wasm32-unknown-unknown`). The conformance legs certify
against the shared corpus and skip cleanly when the repo is checked out alone.
