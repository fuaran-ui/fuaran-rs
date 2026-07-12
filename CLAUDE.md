# CLAUDE.md — fuaran-rs (Rust reference implementation)

This repo is the **Rust host of the Fuaran UI wire format** — a **co-equal sibling
to the F# (`Fuaran.UI`), TypeScript (`@fuaran-ui/*`), Python (`fuaran_py`), and Go
(`fuaran-go`) tiers**. Its identity is **two hosts in one crate**: a headless
backend / edge / embedded host *and* a **browser-native `wasm32` client** — the
canonical-JSON codec, a tree-op apply engine, a pre-emit validator, and both
server-side and WASM-client emission, all conformant to the shared wire format. What
ships **today** is the codec floor — the canonical-JSON layer (number form, parser,
byte-exact renderer) and the typed node/op codec, corpus-certified on the round-trip
and reject families; the apply engine, validator, and emission are roadmap work.

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
├── tests/conformance.rs # shared-corpus certification (round-trip + reject legs live)
├── Cargo.toml
├── run.ps1              # Stage-0 entry point — cargo fmt --check + clippy + build + test
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
browser**; three delivery modes fit:

- **WASM client** — the codec + apply engine ship as a `wasm32` module; a thin
  generic loader mounts the rendered tree, interactions run client-side.
- **Static + partial hydration** — a mostly-static server-rendered page hydrates only
  its interactive regions with a small WASM bundle.
- **Server-driven** — native Rust holds the tree + state, streams frame diffs to a
  thin client, and applies tree-ops in response to events.

All are beyond the stage-0 floor (they need the codec + apply engine + an emitter / a
`wasm32` build target) but are the headline reason a Rust host earns its place — see
`README.md`.

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
