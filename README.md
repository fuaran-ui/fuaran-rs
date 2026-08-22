# fuaran-rs

A **Rust host of the Fuaran UI wire format** — a dependency-light, idiomatic-Rust
reference implementation of the canonical-JSON contract a Rust service, a WASM
client, or an embedded host needs to read, write, and drive Fuaran UI trees.

`fuaran-rs` is a **sibling reference implementation**, not a transpile of any other
host: it is built to the language-neutral wire-format specification
(`WIRE_FORMAT.md`) and certified against the shared conformance corpus. Conformance
to the spec is the contract; idiomatic Rust is the deliverable.

## Get started

```sh
cargo add fuaran-ui
```

The crate publishes to crates.io as **`fuaran-ui`**; this repository is `fuaran-rs`
(the repo-name / crate-name split is common in Rust) and the import path is
`fuaran_rs`. Author a tree with native `enum` kinds, then encode it byte-identically
to every host:

```rust
use fuaran_rs::wire::{encode_node, BoxLayout, BoxRole, BoxSpec, HeadingSpec,
    HeadingVariant, Node, NodeKind, TextSource};

let tree = Node {
    id: "root".into(),
    kind: NodeKind::Box(BoxSpec {
        children: vec![Node {
            id: "title".into(),
            kind: NodeKind::Heading(HeadingSpec {
                level: 2,
                text: TextSource::Literal("Channel performance".into()),
                variant: HeadingVariant::Standard,
            }),
            state: Default::default(),
            style: Default::default(),
            accessibility: None,
        }],
        heading: None,
        layout: BoxLayout::Auto,
        role: BoxRole::Dashboard,
    }),
    state: Default::default(),
    style: Default::default(),
    accessibility: None,
};

let wire: String = encode_node(&tree);   // canonical wire JSON, byte-identical to every host
```

Full walkthrough — author → encode → render (headless or browser-native WASM) →
playground: <https://fuaran-ui.io/get-started/rust>.

## Why a Rust host

Rust reaches two places the other hosts do not:

- **Browser-native via WASM.** Compiled to `wasm32`, a Rust host can decode, apply
  tree-ops, and *drive* a Fuaran UI tree **client-side in the browser without a
  JavaScript frontend** — a conformant host that renders, not merely a headless
  backend. `fuaran-rs` is therefore both a server / edge / embedded host and a
  client-side one.
- **Systems / edge / embedded.** A single dependency-light crate drops into native
  services, CLI tools, and resource-constrained edge / embedded targets where a
  managed runtime is unwelcome.

Rust is also the **best structural fit** of any host for the typed tree: the closed
wire DUs (`NodeKind`, `Spec`, `TreeOp`, `Binding`, `Action`) map onto native Rust
`enum`s with **compile-time-exhaustive `match`** — so a newly-added kind that misses
a codec arm is a *build error*, recovering the exhaustiveness guarantee a language
without sum types has to trade away.

Delivery modes a Rust host fits especially well:

- **WASM client** — ship the codec + apply engine as a `wasm32` module; a thin
  generic loader renders the tree in-browser and interactions run locally.
- **Static + partial hydration** — emit a mostly-static page server-side and
  hydrate only the interactive regions with a small WASM bundle.
- **Server-driven** — hold the tree + state in native Rust, stream tree-op diffs to
  a thin generic client, and round-trip interactions.

## Status — codec floor + apply + validator + server renderer + WASM client shipped

Shipped:

- **`canonical`** — the full canonical-JSON layer: the make-or-break number
  formatter (the `.NET "R"` float layout the wire form requires), a hand-rolled
  stdlib-only JSON parser, and the byte-exact canonical renderer (Ordinal key
  sort, the pinned escapes).
- **`wire`** — the typed wire model (every closed wire DU as a native `enum`)
  plus the working codec: `decode_node` / `encode_node` / `decode_op` /
  `encode_op`, with the six-code `DecodeError` envelope and `$`-rooted paths.
  Corpus-certified: every node + op round-trip fixture re-encodes
  **byte-identically**, every reject fixture surfaces the canonical code + path.
- **`ops`** — the tree-op apply engine: `apply(tree, op)` over the full `TreeOp`
  algebra (structural ops, `UpdateProp` with the nested-path grammar,
  `ReplaceBinding`), total (structured `ApplyError`s, never a panic), atomic
  `Batch`, and the `can_apply` dry-run obeying the apply-envelope law.
- **`validator`** — the pre-emit structural validator surfacing the canonical
  `FUARAN###` defect codes over a decoded tree (node identity, bounded
  primitives, shape coherence, write-back / wire-survivability lints).
- **`render`** — the emission tier: the pure-string server-HTML renderer
  (parity-locked `fuaran-*` class vocabulary, inert interactivity, crawlable
  links, sanitiser posture), the deterministic cross-host markdown renderer
  (corpus-certified byte-for-byte), hydration-ready whole-tree emission, and
  **islands partial hydration** (`render_with_islands`: per-island
  `data-fuaran-island` boundary + scoped wire-JSON payload; zero islands ⇒
  byte-identical to a plain render). The reference stylesheet ships as a
  byte-copy at `css/fuaran.css` (parity-tested against the reference artefact).
- **`client`** — the browser-native (`wasm32`) client host: a `ClientSession`
  decodes a wire tree, renders it (the server-parity renderer, so client and
  server produce byte-identical HTML for the same tree), applies `TreeOp`s that
  mutate the held tree, and writes the reactive stores (state / filter / query)
  — the decode → render → drive loop, in the browser. The session core is pure
  target-agnostic Rust (native-tested); a minimal C-ABI shim
  (`src/client/wasm.rs`) exposes it over WASM linear memory, driven by a thin
  hand-written loader (`js/`) — **no `wasm-bindgen`, no framework**.

- **`bounded`** — the bounded program loop at the client placement: behaviour
  carried as data, run under a per-interaction resource budget. The closed
  action walk (whose only store mutation is the state write and whose only
  outward reach is the closed client-effect vocabulary), the default-deny effect
  policy, the binding re-resolution pass that makes a state write visible, and
  the loop that orders them — validate → interpret → effects → re-resolve. See
  the conformance declaration below.

Beyond this tier: the lenient-profile / envelope / elicitation conformance
families, dataframe (`Binding.Transform`) evaluation, and a host-locale seam
for `Binding.Format`.

## Bounded program loop — the conformance declaration

The **program wire specification** governs behaviour carried as data: the
handler declared form, both placements' closed effect vocabularies, the
invocation record and the outcome report. Alongside the document families it
certifies with round-trip and reject vectors, it carries a **driver-semantics**
scenario family that certifies what a bounded program *loop* does — a tree, an
ordered event script, and the per-step trace a conformant loop produces from
them.

That family is opt-in **by declaration**, because asserting what a loop does is
a stronger claim than round-tripping a document and a host that only decodes,
encodes, records, relays or validates those documents is out of scope for it
rather than failing it. The specification fixes no encoding for the declaration:
a conformance claim is a sentence somebody writes down. Here is this host's, and
it names both halves.

> **`fuaran-rs` implements the bounded path, and it reproduces the
> driver-semantics family of the program wire specification's conformance
> corpus.** Every scenario the corpus enumerates is driven through this loop and
> compared step by step — the resolved tree semantically, through this host's own
> decoder and encoder; the client effects byte-for-byte in their as-emitted
> envelope; the refusal exactly — and the first divergence is reported with its
> step index and the member that differed. The claim is certified on **both**
> targets this host ships: natively, and on `wasm32` by executing the same
> comparison inside the module.

The claim covers the **client** placement. Running host-registered handlers is a
separate obligation this host does not declare, so a call action resolves to the
documented no-op it has always been where nothing is registered.

What the claim asserts is that the loop folds the same way: same resolved tree,
same effects reached with the same values in the same order, same refusals, at
every step. It asserts **nothing** about what a host does with an effect it
reached — performance is host-defined, and a host that declines every effect is
conformant, because both vocabularies default to deny. What is not conformant is
silence: a declined effect is reported as a denial carrying the derived
capability, never dropped.

### Running it

The scenario corpus is a separate artefact and is not vendored here, so this is
a **local, operator-invoked** gate:

```powershell
$env:FUARAN_PROGRAM_SPEC = "<the program wire specification's directory>"
.\run.ps1 -DriverSemantics        # both targets: the native leg, then wasm32 under node
cargo test --test driver_semantics # the native leg alone
```

The corpus is also found automatically when it is checked out beside this
repository. Where a corpus is **claimed** and cannot be read, the leg **fails**
rather than skipping — a conformance check that passes without its oracle
reports the same green as one that ran. Where none is claimed at all, it reports
that it did not run and asserts nothing.

## WASM client

The client module compiles to a dependency-free `.wasm`:

```powershell
cargo build --target wasm32-unknown-unknown --release
# → target/wasm32-unknown-unknown/release/fuaran_rs.wasm  (~0.7 MB)
```

The `run.ps1` gate builds it automatically when the `wasm32-unknown-unknown`
target is installed (`rustup target add wasm32-unknown-unknown`). To try the
demo, copy the built `.wasm` beside `js/index.html` and serve `js/` over HTTP
(ES modules + WASM need a real origin, not `file://`):

```powershell
cp target/wasm32-unknown-unknown/release/fuaran_rs.wasm js/fuaran_rs.wasm
# then serve the js/ directory with any static file server
```

`js/fuaran-loader.js` is a generic, hand-written loader: `loadFuaran(url)`
instantiates the module, `createSession(exports, tree)` opens a session over a
wire tree, and the `FuaranSession` methods (`render` / `applyOp` / `setState` /
`setFilter` / `setQuery` / `mount`) drive it. The module renders **inert** HTML
(closures never ride the wire, §4); interactivity is driven by writing the
reactive stores and re-rendering — exactly the wire's *write-back default*. The
three delivery modes (WASM client, static + partial hydration, server-driven)
all build on this session; `js/index.html` demonstrates the client loop.

## Chart-lowering posture — lower-in-host

A resolved `Drawing` node renders as first-party inline SVG on every host. A raw
`Chart` node is a *semantic* wire kind that must be *lowered* to a `Drawing`
before it can paint. `fuaran-rs` takes the **lower-in-host** posture: a `Chart`
reaching the renderer is lowered deterministically to a canonical `Drawing`
(`render::chart_lowering::lower_chart`) and rendered as inline SVG through the
shared Drawing renderer.

This is the posture the dual-role host's *client* leg earns: the browser-native
`wasm32` client renders through the **same** server renderer, so lowering here
brings the WASM client to parity with the "Chart-as-data" demo — a raw `Chart`
becomes a real rendered chart in the browser, not a placeholder. (The headless
`fuaran-go` host, which paints nothing itself, takes the cheaper
require-pre-lowered posture instead.)

The lowering is a byte-identical port of the F# reference (`Fuaran.UI.Charts.lower`):
a fixed pixel `viewBox`, a `{1,2,5}·10ⁿ` nice-tick rule, and round-half-up-to-2dp
coordinate rounding make it deterministic (`R2`). It is **certified byte-for-byte**
against the shared `wire-format-fixtures/chart-lowering/*` goldens by
`tests/chart_lowering.rs` (every committed input/golden pair, discovered from
the corpus), and the render-path lowering is pinned by `tests/render.rs`.
Lowered arms: `Bar` (grouped + stacked), `Line`, `Area` (overlaid + stacked),
`Scatter` (linear numeric x-scale, point marks), and `Pie` (polar,
cubic-approximated wedges); every data-bearing shape carries a derivation-based
`markId` (emitted as `data-fuaran-mark` in the SVG) for object constancy.
`Heatmap` yields an empty (but titled) drawing region, never a silent blank.

## Layout

```
fuaran-rs/
├── Cargo.toml
├── src/
│   ├── lib.rs           # crate doc + VERSION
│   ├── canonical/       # canonical-JSON layer — number form, parser, canonical renderer
│   ├── wire/            # typed wire model + Node / TreeOp codec + DecodeError envelope
│   ├── ops/             # tree-op apply engine + ApplyError envelope + dry-run
│   ├── validator/       # pre-emit structural validator (FUARAN### codes)
│   ├── render/          # server-HTML renderer + islands + markdown + sanitise floor
│   ├── bounded/         # the bounded program loop — interpreter, budget, effects, re-resolve
│   └── client/          # wasm32 client — ClientSession (mod.rs) + C-ABI shim (wasm.rs)
├── css/
│   └── fuaran.css       # byte-copy of the reference stylesheet (parity-tested)
├── js/
│   ├── fuaran-loader.js # thin hand-written WASM loader (no wasm-bindgen)
│   ├── driver-semantics.mjs # the wasm32 leg of the bounded-loop conformance check
│   └── index.html       # client-loop demo
├── tests/
│   ├── conformance.rs   # shared-corpus certification (round-trip + reject legs)
│   ├── apply.rs         # apply-engine behaviour + the apply-envelope law
│   ├── validator.rs     # per-rule fire / stay-silent pairs
│   ├── markdown.rs      # markdown corpus certification (byte-for-byte)
│   ├── render.rs        # renderer behaviour + islands laws + CSS byte-parity
│   ├── client.rs        # client-session decode → render → drive loop
│   └── driver_semantics.rs # bounded-loop conformance against the scenario corpus
├── run.ps1              # cargo fmt --check -> clippy -> build -> test -> wasm build
├── LICENSE              # Apache-2.0
├── README.md
└── CLAUDE.md
```

## Build / verify

```powershell
.\run.ps1              # cargo fmt --check -> cargo clippy -> cargo build -> cargo test
.\run.ps1 -SkipTests   # fast-iteration switches: -SkipFormat / -SkipBuild / -SkipTests
.\run.ps1 -DriverSemantics  # add the bounded-loop conformance legs (see the declaration above)
```

Requires the Rust toolchain (see `Cargo.toml` for the pinned edition / `rust-version`).
The runtime host uses the **standard library only** — no third-party crates (Rust's
stdlib has no JSON, so the canonical JSON layer is hand-written for byte-exact output
regardless).

## Licence

Apache-2.0. See [`LICENSE`](LICENSE).
