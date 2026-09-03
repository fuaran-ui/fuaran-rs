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

## Destination policy — ambient, not available

`WIRE_FORMAT.md` §14.1. The scheme floor (`render::sanitize`) answers *is this
URL safe to have*; the destination policy answers *is this destination one the
composition declared*, which is the question that closes exfiltration — an image
`src` is contacted by rendering alone, with no user act at all.

The policy is **ambient on the render context**: every `href`, `src`, markdown
destination and grid link the renderer emits is checked, with no caller opt-in
anywhere on the path. A guarantee that holds where it is remembered is not a
guarantee, so the policy is a field on the context every render already threads
rather than a parameter on the emission helpers.

```rust
// Nothing declared ⇒ deny_non_local_egress(). This IS the ambient default.
let html = render_to_html(&tree, &sources);

// A host widens it BY NAME — so `grep permissive` finds every host that did.
let policy = deny_non_local_egress()
    .allow_origin(EgressOrigin::HostSuffix("cdn.example".into()), &[EgressClass::Media]);
let html = render_to_html_with_egress(&policy, &tree, &sources);
```

Every convenience entry point — `render_to_html`, `render_hydratable`,
`render_with_islands`, and `ClientSession::new` (hence every FFI-driven session,
the browser module included) — defaults to `deny_non_local_egress()`, and each
has a `_with_egress` twin (`ClientSession::with_egress_policy`) a host reaches by
name. The default denies **leaving**, not linking: same-origin destinations
render unchanged, while a hostless scheme (`mailto:`, `tel:`) is refused, having
no host for a rule to name and so being permittable only wholesale.

A refused destination RENDERS as a refusal — `about:blank#fuaran-egress-refused`
plus a `data-fuaran-egress-refused` attribute naming the class and the host —
never as a silent neuter: "nothing happened" and "this was refused" are different
facts, and only one of them is debuggable. The marker carries the class and the
host or scheme and **never the path or query**, which is exactly where the
payload of a refused exfiltration attempt would be sitting.

**Three things about it are load-bearing and easy to undo by accident.**

- **A `download` anchor is `Hyperlink`, not `Download`.** The class names the
  sink the browser reaches, and a download anchor is still a hyperlink the reader
  must act on. Scoping it separately would let a policy that denied hyperlinks
  admit the same destination by flipping one boolean on the tree.
- **The pure `markdown::to_html` is still the permissive case**, byte-for-byte,
  and the corpus gate asserts the equivalence rather than assuming it. A
  *renderer* entry point defaults the other way because it walks a DECODED tree,
  where the author is not the trust boundary. The renderer never reaches the pure
  form — a decoded markdown body's destinations are as ambient as an `href`.
- **The policy is threaded as a borrowed parameter**, never a `static mut` or a
  thread-local: two renders under two different policies may run concurrently.

**Two deliberate divergences from the reference host, stated rather than
discovered.**

- **A floor rejection now renders as a refusal too.** Before the call sites
  consulted the policy, a `javascript:` URL emitted the bare `about:blank` the
  floor returns. Once a site consults the policy, "this destination was refused"
  is one fact with one rendering, and splitting it by *which* gate refused would
  make the more dangerous case the one that renders as an ordinary blank. The
  marker value for it is `unsafe-url`. `render::sanitize`'s own answer is
  unchanged — the floor still returns `about:blank`, and the `sanitization/`
  corpus still pins that; what changed is what an emission site does with it.
- **This host has no email HTML projection**, so the reference host's deliberate
  omission of the refusal marker in that projection (`data-*` attributes do not
  survive the sanitisers most mail clients run) has nothing to attach to here.
  `render::email` is a plain-TEXT digest that emits no `href` or `src` at all: a
  link contributes its label, and the destination never reaches the output.

**How this relates to the bounded loop's destination floor.** They are the same
decision at two seams, not two policies. `render::egress::refuse_classified` is
the single function that answers "does this policy permit this class to reach
this destination", and both the renderer and `bounded::EffectPolicy`'s
`EgressFloor` call it. A host that has one `EgressPolicy` hands the same value to
both — `EgressFloor::Declared(policy)` — and gets one answer, with each
client-effect arm judged under the class §14.1 already scopes rules to
(`Navigate` / `PushState` as `Route`, `Download` as `Download`, `ReadFileBody` as
`FileRead`). What stays seam-local is the RECORD: a renderer refusal is a refusal
URL plus a marker attribute, and a loop refusal is a `Denial::GateRefused` naming
the origin, because a loop emits no markup for a marker to ride on.

That is also why `Action::Navigate` in the interpreter still applies the scheme
floor and only the scheme floor. The interpreter's predicate is fixed by the
program wire specification (§10.5 governs the *response* to a rejection, not the
predicate), and a step must still REPORT an effect it reached even when the host
declines it — a declined effect dropped in the fold is indistinguishable from one
that was never reached. So the route's destination policy is consulted one step
later, at the performer seam, and deliberately not twice.

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
> envelope; the refusal exactly; and, where a scenario names the host policy it
> presumes, the denials its performer seam produced, decoded into this host's own
> denial vocabulary — and the first divergence is reported with its step index
> and the member that differed. The claim is certified on **both** targets this
> host ships: natively, and on `wasm32` by executing the same comparison inside
> the module.

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

That last sentence is checked rather than merely stated, because a scenario can
record what the seam declined. Where it does, it names the policy it presumes and
this host **constructs** what the name denotes — refusing a name it does not
recognise, since falling back to its own default would report a scenario it could
not evaluate as one it passed. A denial names the capability and, where the
refusal's ground was the destination, that destination's **origin**: the host, or
the class of destination where there is none. Never the path and never the query,
which is exactly where the payload of a refused exfiltration attempt would be.

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

## Edge hosting — a session that can be evicted

A worker runtime evicts a session between requests by design, so a session hosted
there is a thing that must be able to disappear and come back. `src/edge/` is that
half of the client tier: an `EdgeSession` that **owns** its store (so one activation
per handle is the compiler's guarantee, not a review comment), journals each
applied `TreeOp` to a durable store **before** the held tree moves, and rehydrates
from that journal — newest checkpoint plus suffix — on the next activation, having
verified the chain first.

**It names obligations, not a platform.** `DurableSessionStore` is a four-method
trait with no vendor in it: acquire an activation, read the journal, append a
record durably, record a snapshot. `InMemoryDurableStore` is the reference
implementation and implements the fence fully, so the protocol is exercised by
`tests/edge.rs` rather than merely described; a real host writes its own against
the same four sentences and links nothing here. The trait is synchronous for the
same reason `OpStreamSink` is — the contract is about *ordering*, and a platform
whose storage is asynchronous meets it by awaiting inside its own implementation.

**What the tier honestly claims.** A record is durable before the tree moves, so a
crash between the two replays to the same tree — apply is a total function of the
tree and the op and reads nothing else, and nothing in the module reads a clock,
allocates an id, or touches a filesystem. A **superseded activation is refused**
(`StoreError::NotOwner`) rather than interleaved: ownership cannot reach a second
*process* opening the same session id, so the store carries a monotonic
`ActivationToken` and every write presents it. A **refused op journals nothing** —
the journal is the applied history, and a record whose op does not apply would make
replay fail on its own evidence. And **reactive slots are not journaled**: `$state`
/ `$filters` / `$queries` are a view's live inputs rather than authored history, so
they do not survive an eviction and a host re-seeds them on activation.

The whole module compiles for `wasm32` with no target-specific code, which is what
makes the browser module and an edge worker the same session type.

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

## Retired wire vocabulary — the positional slot on `InsertChild` / `MoveNode`

`InsertChild` and `MoveNode` both **append**; `ReorderChildren` states order by naming
child ids. The integer `position` / `newPosition` these two ops once carried was removed
from the wire format, and this host **REFUSES** it: `WrongType` at `$.position` /
`$.newPosition`, with a message naming `ReorderChildren`. Placing a node anywhere but
last is `Batch [InsertChild …, ReorderChildren …]`.

There was a migration window during which every host accepted and ignored the field so
the hosts could adopt independently. It is **closed**. How it closed is worth knowing,
because it is not the obvious thing: this decoder reads named fields and ignores the
rest, so *not reading* the ordinal **was** the tolerance — there was never a read to
delete. Closing the window therefore meant ADDING a refusal, not removing an acceptance;
a host that merely stopped mentioning the field would have gone on accepting it forever,
indistinguishable from one that had never adopted.

The refusal is **by name** and is the enumerated-near-miss narrowing of §2 rule 2: a
genuinely unknown key is still tolerated, because a slot a future profile may add must
stay addable. It is checked **before** the required-field reads, so an op carrying both a
retired ordinal and another defect names the ordinal — identically ordered in every host,
so which defect surfaces first is deterministic.

**The apply engine has no ordinal handling left, and the reason it needs none is that the
wire cannot express one** — the field never reaches it. Certified by the corpus fixtures
`reject-op-insertchild-retired-position` / `reject-op-movenode-retired-newposition` and
pinned by `tests/retired_position.rs`.

This host declares no stability policy yet (pre-1.0), so the change is
recorded here rather than in a `STABILITY.md` it does not have.

## Typed actor on the DAG record — a re-addressing change

`dag::DagRecord` carried a bare-string `user_id` until Phase 1144 replaced it with the
typed `actor` the linear op-stream chain has carried since Phase 320 — the same
`Human | Agent` value, in the same **pinned** canonical encoding
(`opstream::encode_actor`), nested verbatim exactly as the `op` is. Top-level keys are
Ordinal-sorted, so `actor` sorts to the **front** where `userId` sat at the back. This
host adopts it in Phase 1168, byte-identical to the regenerated
`wire-format-fixtures/dag/` family.

**It re-addresses every DAG node.** The reference host folds the attribution member into
the DAG content address, so substituting a typed actor for a bare id changes the
pre-image — a pre-1144 `hash` is no longer reproducible and is not a valid parent link
for a post-1144 node. **Pre-1144 DAG addresses do not carry forward, and there is no
in-place upgrade for a persisted DAG.**

Decoding is deliberately **not** dual-read: a pre-1144 `userId` envelope is refused **by
name** rather than lifted to a `Human`. A lift would mint a record carrying a stored
`hash` no host can reproduce, turning a clear refusal here into a silent verification
failure somewhere else. Every malformed actor is likewise named and never defaulted — a
non-object is `WRONG_TYPE`, a missing `kind` or case field is `MISSING_FIELD`, and a
`kind` outside the closed pair is `UNKNOWN_DU_CASE`.

One thing this host does **not** do: it mints no DAG content address and verifies none.
The only pre-images `fuaran-rs` computes are the linear chain's (`opstream::chain`), so
`hash` is an opaque string it round-trips. The reference pre-image and the exact member
substitution are recorded beside the type in `src/dag/record.rs`; a Rust DAG addresser
would be a new capability, not part of this adoption.

Certified by the four `dag/` corpus fixtures (both actor cases — the family carries a
human and an agent deliberately) and pinned by `tests/dag.rs`.

This host declares no stability policy yet (pre-1.0), so the change is recorded here
rather than in a `STABILITY.md` it does not have; the version advances
`0.0.1-alpha.1` -> `0.0.2-alpha.1`.

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
│   ├── client/          # wasm32 client — ClientSession (mod.rs) + C-ABI shim (wasm.rs)
│   └── edge/            # edge hosting — EdgeSession + the DurableSessionStore obligations
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
│   ├── edge.rs          # the durability protocol — fencing, kill points, rehydration
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
