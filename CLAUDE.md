# CLAUDE.md — fuaran-rs (Rust reference implementation)

This repo is the **Rust host of the Fuaran UI wire format** — a **co-equal sibling
to the F# (`Fuaran.UI`), TypeScript (`@fuaran-ui/*`), Python (`fuaran_py`), and Go
(`fuaran-go`) tiers**. Its identity is **two hosts in one crate**: a headless
backend / edge / embedded host *and* a **browser-native `wasm32` client** — the
canonical-JSON codec, a tree-op apply engine, a pre-emit validator, and both
server-side and WASM-client emission, all conformant to the shared wire format. What
ships **today**: the codec floor (corpus-certified round-trip + reject), the tree-op
apply engine (+ `can_apply` dry-run), the pre-emit validator (canonical `FUARAN###`
codes), the server-side emission tier (parity-locked server-HTML renderer,
corpus-certified deterministic markdown renderer, hydration-ready emission, islands
partial hydration), and the **browser-native `wasm32` client** — a `ClientSession`
(decode → render → drive) over a minimal C-ABI shim + a thin hand-written JS loader,
no `wasm-bindgen`. The remaining conformance families (lenient / envelope /
elicitation) and dataframe evaluation are roadmap work.

**Framing — load-bearing, do not regress.** The emission surface is the **canonical
JSON wire format, for every host**. The language tiers are **human-developer
authoring surfaces** that produce that JSON. Rust's distinctive value is reach the
other hosts lack — `wasm32` gives a *client-side* conformant host (renders in-browser
without a JS frontend), and native compilation gives systems / edge / embedded hosts
— plus the tightest structural fit for the typed tree (native `enum`s + exhaustive
`match`).

This repo sits under the Fuaran-UI sub-estate at `../`, alongside the `fuaran`,
`fuaran-ts`, `fuaran-py`, and `fuaran-go` tiers. Cross-repo conventions (port
allocation, Sync All, the formatting mandate, the language-baseline pinning, the OSS
publication boundary) live in the workspace `CLAUDE.md` (`../../../CLAUDE.md`) and the
Fuaran-UI sub-estate `CLAUDE.md` (`../CLAUDE.md`). Read those first.

## Posture

- **Apache 2.0 from day one** — same posture as `fuaran-ts` / `fuaran-py` /
  `fuaran-go`, to make the reference-implementation claim unambiguous.
- **Sibling reference implementation, not a transpile.** `fuaran-rs` is built to the
  language-neutral wire-format spec (`../fuaran/docs/WIRE_FORMAT.md`) + the
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
│                        #   + sanitize.rs (injection floor) + bindings.rs / class_names.rs / html.rs
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
(`../fuaran/docs/WIRE_FORMAT.md`) with the workspace-level `../wire-format-fixtures/`
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
skips when absent). The lenient-accept, envelope, and elicitation families are later
tiers, counted and skipped explicitly by the harness.

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
