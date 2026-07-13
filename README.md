# fuaran-rs

A **Rust host of the Fuaran UI wire format** — a dependency-light, idiomatic-Rust
reference implementation of the canonical-JSON contract a Rust service, a WASM
client, or an embedded host needs to read, write, and drive Fuaran UI trees.

`fuaran-rs` is a **sibling reference implementation**, not a transpile of any other
host: it is built to the language-neutral wire-format specification
(`WIRE_FORMAT.md`) and certified against the shared conformance corpus. Conformance
to the spec is the contract; idiomatic Rust is the deliverable.

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

## Status — codec floor + apply + validator + server renderer shipped

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

Beyond this tier: the lenient-profile / envelope / elicitation conformance
families, dataframe (`Binding.Transform`) evaluation, a host-locale seam for
`Binding.Format`, and the `wasm32` client build.

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
│   └── render/          # server-HTML renderer + islands + markdown + sanitise floor
├── css/
│   └── fuaran.css       # byte-copy of the reference stylesheet (parity-tested)
├── tests/
│   ├── conformance.rs   # shared-corpus certification (round-trip + reject legs)
│   ├── apply.rs         # apply-engine behaviour + the apply-envelope law
│   ├── validator.rs     # per-rule fire / stay-silent pairs
│   ├── markdown.rs      # markdown corpus certification (byte-for-byte)
│   └── render.rs        # renderer behaviour + islands laws + CSS byte-parity
├── run.ps1              # cargo fmt --check -> clippy -> build -> test
├── LICENSE              # Apache-2.0
├── README.md
└── CLAUDE.md
```

## Build / verify

```powershell
.\run.ps1              # cargo fmt --check -> cargo clippy -> cargo build -> cargo test
.\run.ps1 -SkipTests   # fast-iteration switches: -SkipFormat / -SkipBuild / -SkipTests
```

Requires the Rust toolchain (see `Cargo.toml` for the pinned edition / `rust-version`).
The runtime host uses the **standard library only** — no third-party crates (Rust's
stdlib has no JSON, so the canonical JSON layer is hand-written for byte-exact output
regardless).

## Licence

Apache-2.0. See [`LICENSE`](LICENSE).
