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

## Status — codec floor shipped

Shipped:

- **`canonical`** — the full canonical-JSON layer: the make-or-break number
  formatter (the `.NET "R"` float layout the wire form requires), a hand-rolled
  stdlib-only JSON parser, and the byte-exact canonical renderer (Ordinal key
  sort, the pinned escapes).
- **`wire`** — the typed wire model (every closed wire DU as a native `enum`)
  plus the working codec: `decode_node` / `encode_node` / `decode_op` /
  `encode_op`, with the six-code `DecodeError` envelope and `$`-rooted paths.
- **conformance** (`tests/conformance.rs`) — certification against the shared
  `wire-format-fixtures` corpus: every node + op round-trip fixture re-encodes
  **byte-identically**, and every reject fixture surfaces the canonical error
  code + path prefix. Skips cleanly when the repo is checked out alone.

The tree-op apply engine, pre-emit validator, lenient-profile / envelope /
elicitation conformance families, and server-HTML / WASM-client emission are
roadmap work beyond the floor.

## Layout

```
fuaran-rs/
├── Cargo.toml
├── src/
│   ├── lib.rs           # crate doc + VERSION
│   ├── canonical/       # canonical-JSON layer — number form, parser, canonical renderer
│   │   ├── mod.rs
│   │   ├── float.rs
│   │   └── json.rs
│   └── wire/            # typed wire model + Node / TreeOp codec + DecodeError envelope
│       ├── mod.rs
│       ├── model.rs
│       ├── decode.rs
│       ├── encode.rs
│       └── result.rs
├── tests/
│   └── conformance.rs   # shared-corpus certification (round-trip + reject legs)
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
