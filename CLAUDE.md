# CLAUDE.md — fuaran-rs (Rust reference implementation)

This repo is the **Rust host of the Fuaran UI wire format** — a **co-equal sibling
to the F# (`Fuaran.UI`), TypeScript (`@fuaran-ui/*`), Python (`fuaran_py`), and Go
(`fuaran-go`) tiers**. Its identity is **two hosts in one crate**: a headless
backend / edge / embedded host *and* a **browser-native `wasm32` client** — the
canonical-JSON codec, a tree-op apply engine, a pre-emit validator, and both
server-side and WASM-client emission, all conformant to the shared wire format. What
ships **today**: the codec floor (corpus-certified across every family — node/op
round-trip, reject, lenient-accept, envelope, elicitation — on the 0.2.x canonical
bytes), the tree-op apply engine (+ `can_apply` dry-run), the pre-emit validator
(canonical `FUARAN###` codes), dataframe evaluation (`Binding.Transform`), the
server-side emission tier (parity-locked server-HTML renderer, corpus-certified
deterministic markdown renderer, hydration-ready emission, islands partial
hydration, golden-certified chart lowering), and the **browser-native `wasm32`
client** — a `ClientSession` (decode → render → drive) over a minimal C-ABI shim +
a thin hand-written JS loader, no `wasm-bindgen`.

**Framing — load-bearing, do not regress.** The emission surface is the **canonical
JSON wire format, for every host**. The language tiers are **human-developer
authoring surfaces** that produce that JSON. Rust's distinctive value is reach the
other hosts lack — `wasm32` gives a *client-side* conformant host (renders in-browser
without a JS frontend), and native compilation gives systems / edge / embedded hosts
— plus the tightest structural fit for the typed tree (native `enum`s + exhaustive
`match`).

This repo sits alongside the `fuaran`, `fuaran-ts`, `fuaran-py`, and `fuaran-go` tiers as a
co-equal conformant host. Cross-repo development conventions (port allocation, formatting, language-baseline pinning) live at the maintainers' workspace level and are not shipped here.

## Posture

- **Apache 2.0 from day one** — same posture as `fuaran-ts` / `fuaran-py` /
  `fuaran-go`, to make the reference-implementation claim unambiguous.
- **Sibling reference implementation, not a transpile.** `fuaran-rs` is built to the
  language-neutral wire-format spec (`../fuaran-dotnet/docs/WIRE_FORMAT.md`) + the
  conformance corpus (`../wire-format-fixtures/`), not generated from any other tier.
  There is no Rust transpile path and none is wanted — the hard part (the canonical
  number form) is hand-written for every host regardless.
- **Wire-format conformance is the stability contract.** The codec must
  encode / decode byte-identically against the shared corpus and surface the
  canonical reject code + `$`-rooted path for every malformed fixture — certified the
  same way the F#, TypeScript, Python, and Go hosts are.
- **Dependency-light.** The runtime host uses the Rust standard library only. Rust's
  stdlib has no JSON, so the canonical JSON layer is hand-written (needed for
  byte-exact canonical output anyway); third-party crates appear only as dev tooling
  if ever.

## Language baseline

Rust **edition 2024** (`rust-version` pinned in `Cargo.toml` — the Rust analogue of
the workspace's F#-10 / .NET-10 pinning; the `fuaran-ts` / `fuaran-py` / `fuaran-go`
siblings pin their own runtimes the same way). Model the closed wire DUs (`NodeKind`,
`Spec`, `TreeOp`, `Binding`, `Action`, …) as **native `enum`s, one variant per
`$type` case** — and lean on the compiler: a per-kind `match` is **exhaustive at
build time**, so `fuaran-rs` *recovers* the compile-time exhaustiveness a language
without sum types has to trade away (the Go host's one real regression vs the F#
host). Deny the codec any `_ =>` catch-all arm so a new kind is a build error until
its arm lands.

## Layout

```
fuaran-rs/
├── src/lib.rs           # crate doc + VERSION
├── src/canonical/       # canonical-JSON layer — float.rs (number form) + json.rs (parser + canonical renderer)
├── src/wire/            # model.rs (typed tree, native enums) + decode.rs / encode.rs + the DecodeError envelope
├── src/ops/             # tree-op apply engine — total reducer + ApplyError envelope + can_apply dry-run
├── src/validator/       # pre-emit structural validator — canonical FUARAN### defect codes
├── src/render/          # emission tier — server.rs (HTML walk + islands) + markdown.rs (corpus-certified)
│                        #   + sanitize.rs (injection floor) + egress.rs (destination policy, §14.1)
│                        #   + bindings.rs / class_names.rs / html.rs
├── src/bounded/         # the bounded program loop (client placement) — actions.rs (the one evaluating
│                        #   match) + budget.rs + effect.rs + resolve.rs + validate.rs + program.rs
│                        #   + trace.rs (the IO-free scenario runner + first-divergence comparison)
├── src/ffi/             # target-neutral C-ABI (Phase 537) — fuaran_* over an opaque ClientSession, all targets
├── src/client/          # mod.rs (ClientSession, target-agnostic) + wasm.rs (wasm32 re-export of src/ffi/)
├── include/fuaran.h     # hand-written C header for src/ffi/ — the native binding surface + ownership/threading contract
├── css/fuaran.css       # byte-copy of the reference stylesheet (parity-tested against the reference artefact)
├── js/                  # thin hand-written WASM loader (fuaran-loader.js) + client-loop demo (index.html)
├── tests/               # conformance.rs + apply.rs + validator.rs + markdown.rs + render.rs + client.rs + ffi.rs
├── Cargo.toml           # lib + cdylib + staticlib crate types; release profile tuned for a small wasm artefact
├── run.ps1              # Stage-0 entry point — fmt/clippy/build/test/wasm; -CrossTargets/-Package for native mobile
├── LICENSE              # Apache 2.0 + Diametrical Ltd copyright
├── README.md
└── CLAUDE.md
```

## Build / verify pipeline

```powershell
.\run.ps1                 # cargo fmt --all --check + cargo clippy -D warnings + cargo build + cargo test
.\run.ps1 -SkipTests      # switches: -SkipFormat / -SkipBuild / -SkipTests
```

Or drive the toolchain directly: `cargo fmt --all --check`, `cargo clippy
--all-targets -- -D warnings`, `cargo build`, `cargo test`.

## Formatting mandate

The workspace formatting mandate (Fantomas for F#, Prettier for TS, ruff for Python,
gofmt for Go) maps here to **rustfmt** — every commit is preceded by `cargo fmt
--all` over the changed files. The `run.ps1` gate is `cargo fmt --all --check`, paired
with a `cargo clippy -D warnings` lint gate.

## Wire format

The canonical wire format is owned by the F# `fuaran` tier
(`../fuaran-dotnet/docs/WIRE_FORMAT.md`) with the workspace-level `../wire-format-fixtures/`
corpus as the executable conformance suite. `fuaran-rs` is one conformant host: it
must round-trip the corpus byte-for-byte and surface the canonical reject code + path
for every malformed fixture. The **forward-coupling rule** (`WIRE_FORMAT.md` §11)
means a new `NodeKind` / `Spec` / `TreeOp` / `Binding` / `Action` case must move every
host in one change — `fuaran-rs` is now one of those hosts.

### Conformance coverage (codec floor)

Shipped: the canonical-JSON layer (`canonical::format_finite_double` pinned to the
corpus `metric-float-*` divergence-zone vectors, plus the hand-rolled parser and
byte-exact canonical renderer) and the typed node/op codec
(`wire::{decode_node, encode_node, decode_op, encode_op}`). `tests/conformance.rs`
certifies against the shared corpus: every **node-round-trip** and **op-round-trip**
fixture re-encodes byte-identically, and every **reject** fixture surfaces the
canonical error code + `$`-rooted path prefix (the harness locates
`../wire-format-fixtures/` via `manifest.json` — the authoritative enumeration — and
skips when absent). The **lenient-accept**, **envelope**, and **elicitation**
families certify the same way — the full corpus enumeration runs, none skipped.

### Destination policy at the markdown link / image seam (`src/render/egress.rs`)

`WIRE_FORMAT.md` §14.1. The scheme floor (`src/render/sanitize.rs`) answers *is this
URL safe to have*; the policy answers *is this destination one the composition
declared*, which is the question that closes exfiltration — an image `src` is
contacted by rendering alone, with no user act. `markdown::to_html_with_egress` takes
a host-constructed `EgressPolicy` and checks every link (`Hyperlink`) and image
(`Media`) destination; a refusal renders the inert
`about:blank#fuaran-egress-refused` plus a trailing `data-fuaran-egress-refused`
marker naming the class and the host.

**Four things about it are load-bearing and easy to undo by accident.**

- **`to_html` IS the permissive case**, byte-for-byte, and the corpus gate asserts
  the equivalence rather than assuming it. Flipping the pure function's default would
  rewrite existing fixtures in every host in one act, and a mass churn is where a real
  divergence hides. A host rendering a *decoded* body reaches
  `to_html_with_egress` deliberately.
- **The scheme floor's own answer is unchanged.** A URL the floor rejects still
  renders the bare `about:blank`, with **no** marker — a different fact from a policy
  refusal, and one the `sanitization/` corpus already pins.
- **A refusal marker carries the class and the host or scheme, NEVER the path or the
  query.** The query string of a refused exfiltration attempt is the payload itself,
  so a refusal record that quoted it would become the disclosure it exists to prevent.
- **The policy is threaded as a borrowed parameter**, never a `static mut` or a
  thread-local: two renders under two different policies may run concurrently.

`tests/markdown.rs` maps a fixture's optional `policy` name to a policy it
**constructs** — the corpus never carries one as data — and an unrecognised name
**fails**: a silent fallback to permissive would report a fixture the host cannot
evaluate as one it passed. A guard also asserts the corpus still carries a
non-permissive fixture, without which the whole gate could run on the permissive path
and stay green on a host that never implemented §14.1.

## The bounded program loop + its conformance leg (`src/bounded/`)

`src/bounded/` is the **client placement of the bounded program loop** — behaviour
carried as data: the closed action walk (`actions.rs`, the one evaluating `match`
over `Action` in this crate), the per-interaction resource budget (`budget.rs`),
the default-deny client-effect vocabulary and policy (`effect.rs`), the binding
re-resolution pass that makes a state write visible (`resolve.rs`), the inbound
trust boundary (`validate.rs`), and the loop that orders them (`program.rs`).

**Four things about it are load-bearing and easy to undo by accident.**

- **The client-effect envelope is NOT canonical, on purpose.** `kind` rather than
  `$type`, declaration-ordered members, short escapes for the three common control
  characters. Its encoder is deliberately separate from `canonical::` rather than a
  flag on it, so encoding an effect canonically fails to compile rather than
  silently erasing an exception a rendering surface already reads. Unifying it is a
  migration with a version, never a tidy-up.
- **`resolve.rs`'s coverage floor is a recorded negative result.** The kinds it does
  NOT reach are pinned by a conformance scenario, so widening the floor changes a
  recorded expectation rather than moving behaviour silently. Both its per-kind
  match and `validate.rs`'s event-legitimacy table are exhaustive with **no
  catch-all**, so a new `NodeKind` is a build error until somebody decides.
- **`trace.rs` does no IO**, so the identical comparison compiles for `wasm32`. The
  tree is compared **semantically** (decoded and re-encoded through this host's own
  codec — this host is measured against its own bytes) and the effects
  **byte-for-byte**; normalising the effects would erase the exception above. A
  step's **denials** are neither: their envelope is canonical with no exception, so
  they are embedded as objects and **decoded** into `Denial` before comparison —
  which is what makes the comparison assert that this host recognises the
  vocabulary rather than that two strings matched. Their absence and their
  emptiness are different facts (unobserved vs observed-and-nothing-declined),
  which is the one place on this wire where an omitted array is not the same as an
  empty one; a reader that defaulted the member would turn a silence into a claim.
- **A scenario's host policy arrives as a NAME, and an unrecognised one fails.**
  `EffectPolicy::named` constructs what the name denotes — the corpus never carries
  a policy as data, because a corpus that did would be specifying one. This is the
  same arrangement `tests/markdown.rs` already uses for the egress fixtures, and
  for the same reason: a silent fallback to permissive would report a scenario this
  host could not evaluate as one it passed.
- **The conformance leg is LOCAL and operator-invoked.** The scenario corpus is a
  separate artefact this repository does not vendor and its public workflow does not
  check out, so `tests/driver_semantics.rs` runs when the corpus is present locally
  (`FUARAN_PROGRAM_SPEC`, or a checkout beside this repository) and reports "NOT RUN"
  otherwise. A **claimed** corpus that cannot be read **fails** — never skips. Do not
  add a corpus checkout to the public workflow.

`run.ps1 -DriverSemantics` runs both targets: the native leg, then the `wasm32` leg,
which builds the module with the `driver-semantics-abi` feature (off by default, and
deliberately not part of the stable `include/fuaran.h` surface) and executes the same
comparison inside it under node. The `§10.2` bounded-path declaration itself lives in
`README.md` and in `src/bounded/mod.rs`'s header — a conformance claim is a sentence
somebody writes down, and both halves of it must be named.

## Interactivity — a client-side host, not only a headless one

Unlike a purely-headless backend host, a Rust host is not confined to the server.
Compiled to `wasm32` it decodes, applies tree-ops, and drives a tree **in the
browser** — **shipped** as `src/client/`: a `ClientSession` (decode → render → apply
op / write store → re-render) with a dependency-free C-ABI shim over WASM linear
memory (`src/client/wasm.rs`, `cfg(target_arch = "wasm32")`) driven by a thin
hand-written loader (`js/fuaran-loader.js`), no `wasm-bindgen`. The session core is
pure target-agnostic Rust (native-tested by `tests/client.rs`); the WASM layer is a
marshalling shim. Three delivery modes build on it:

- **WASM client** — the codec + renderer + apply engine ship as the `wasm32` module;
  the loader mounts the render and drives the session client-side (`js/index.html`
  demonstrates the loop). **Shipped.**
- **Static + partial hydration** — a mostly-static server-rendered page (the islands
  emission, `render::server::render_with_islands`) hydrates only its interactive
  regions with the WASM module. The server + islands halves ship; the client-side
  `hydrateIslands` glue that mounts per-island is a follow-on.
- **Server-driven** — native Rust holds the tree + state, streams frame diffs to a
  thin client. Built on the shipped apply engine; the streaming transport is a
  follow-on.

**Interaction model — load-bearing.** The renderer emits **inert** HTML (closures
never ride the wire, §4). A client drives interactivity by writing the reactive
stores (`ClientSession::set_state` / `set_filter`) and re-rendering — the wire's
*write-back default* (an omitted handler over a writable `Binding.State` /
`Binding.Filter` slot writes the change to that slot). Structural edits arrive as
`TreeOp`s through the same total apply engine the headless host uses. The parity-
locked render carries no per-control slot attribute, so **event → store auto-wiring
is app-specific** — the loader stays generic and leaves that mapping to the app.
Adding a `data-*` slot hint to auto-wire would be a renderer-vocabulary change and
therefore a cross-host parity change — do not add one unilaterally.

## Native C-ABI surface (`src/ffi/` + `include/fuaran.h`)

The C-ABI export surface — `fuaran_alloc` / `fuaran_dealloc`, `fuaran_session_new` /
`_free` / `_render` / `_tree_json` / `_apply_op` / `_set_state` / `_set_filter` /
`_set_query`, and `fuaran_last_error` — lives in **`src/ffi/`** and is **target-
neutral**: it compiles into the `wasm32` browser module *and* the native `staticlib`
/ `cdylib` (`crate-type = ["lib", "cdylib", "staticlib"]`). Nothing in it is
`wasm32`-specific; `src/client/wasm.rs` is now just the browser build's re-export of
`crate::ffi` at the shim's historical path (the JS loader links the same symbol
names, unchanged). The hand-written **`include/fuaran.h`** is the C declaration of
that surface — `cbindgen` may run as a dev-tooling *check* but is never a build
dependency (stdlib-only mandate). Native staticlib / dynamic consumers (the Swift and
Kotlin binding tiers) link these symbols; a native FFI smoke test (`tests/ffi.rs`)
drives a session through the raw surface to certify the ABI end-to-end.

**Buffer-return ABI is pointer-width dependent — load-bearing.** Every text-returning
function hands back a Rust-owned `(ptr, len)` pair the caller frees with
`fuaran_dealloc(ptr, len)`. On `wasm32` (32-bit pointers) the pair is a **packed
`u64`** (`ptr` high 32, `len` low 32) — the exact form the JS loader has always read,
so the browser ABI is byte-for-byte unchanged. On native 64-bit targets a full
pointer cannot share a `u64` with a length without truncation, so the pair is a
`#[repr(C)]` two-word struct `FuaranBuf { ptr, len }` returned by value. The
`FuaranBuf` type in `src/ffi/` is `cfg`-aliased to the right representation per
target; do not "unify" it back to a bare `u64` — that silently corrupts native
pointers.

**Ownership + threading contract (also in `include/fuaran.h`).**

- **Buffers.** *Input* buffers (`fuaran_alloc`) are caller-owned — Rust borrows for
  one call; the caller frees after. *Output* buffers (inside a returned `FuaranBuf`)
  are Rust-owned — the caller reads `len` bytes (there is **no** trailing NUL; always
  honour `len`, never `strlen`) then frees with `fuaran_dealloc`. Every buffer is
  freed exactly once; a session handle exactly once via `fuaran_session_free`.
- **Threading.** A `ClientSession` handle is **single-owner**: confine it (and every
  call taking it) to **one thread / executor** for its whole lifetime — no `Send` /
  `Sync` guarantee crosses the boundary. A Swift `actor` wrapper or a Kotlin
  single-threaded dispatcher is the intended confinement. `fuaran_last_error` reads a
  **per-thread** slot, so read it on the same thread that made the failing
  `fuaran_session_new`. (On the single-threaded `wasm32` host the per-thread slot is
  effectively one global.)

**v0 native surface decision (Phase 537).** v0 is exactly the session surface above —
the adopted architecture drives all native rendering through a session
(`session_apply_op` → read back `session_tree_json`), so no extra entry points are
needed. Stateless candidates (`fuaran_validate`, `fuaran_encode_canonical`,
`fuaran_apply`) are listed **reserved** in the header, not implemented — add them only
on demand.

**Cross-target build legs + packaging.** `run.ps1 -CrossTargets` builds the six mobile
release legs (`aarch64-apple-ios{,-sim}`, `aarch64-apple-darwin`,
`aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android`); each
**skips cleanly** with a named-toolchain message when its Rust target or native
toolchain (Xcode / NDK / cargo-ndk) is absent, so the Windows dev box stays green. The
Android `.so` legs link with **16KB page-size alignment**
(`-C link-arg=-Wl,-z,max-page-size=16384`, required on modern Android). `run.ps1
-Package` assembles the Apple XCFramework (macOS + `xcodebuild` only) and the Android
`jniLibs/<abi>/` layout from the built legs into `packaging/` (gitignored — regenerated
output, not source).

## Cross-repo dependencies

No upstream dependency on any other sibling. At test time it reads the
workspace-relative corpus at `../wire-format-fixtures/` (skipped when absent, so the
repo is standalone-testable). It produces a Cargo crate, not a NuGet pack — the
workspace `pack-all.ps1` treats it as a no-op.

## Public vocabulary discipline

`fuaran-rs` is OSS-public (Apache 2.0). Per the workspace OSS publication boundary,
**shipped artefacts** (source, README, package metadata) reference only "the Fuaran UI
wire format" generically — never a private sibling / package name, a commercial
product name, or a strategic-command name. The specific banned list lives in the
workspace OSS publication boundary doc, not here. This `CLAUDE.md` lives in the public
repo, so it observes the same boundary — it names no private sibling, package,
product, or command.
