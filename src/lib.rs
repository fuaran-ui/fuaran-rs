//! `fuaran-rs` is the Rust host of the Fuaran UI wire format — a dependency-light,
//! idiomatic-Rust reference implementation of the canonical-JSON contract a Rust
//! service, a WASM client, or an embedded host needs to read, write, and drive
//! Fuaran UI trees.
//!
//! `fuaran-rs` is a sibling reference implementation, not a transpile of any other
//! host: it is built to the language-neutral wire-format specification
//! (`WIRE_FORMAT.md`) and certified against the shared conformance corpus. See
//! `README.md` and `CLAUDE.md`.
//!
//! Status: the codec floor is shipped — the canonical JSON layer ([`canonical`])
//! and the typed node/op codec ([`wire`]): [`wire::decode_node`] /
//! [`wire::encode_node`] / [`wire::decode_op`] / [`wire::encode_op`], certified
//! byte-for-byte against the shared conformance corpus (round-trip + reject
//! families).
//!
//! So are the three tiers this line used to call roadmap work: the tree-op apply
//! engine ([`ops`]), the pre-emit [`validator`], and server-side and
//! browser-native emission ([`render`], [`serverdriven`], [`client`], [`ffi`]) —
//! each certified against the same corpus. What a reader should still calibrate
//! against is coverage rather than existence: the [`render`] tier's per-kind
//! fidelity is measured by the corpus's own obligation roster, not claimed
//! uniformly, and this host is a *headless and WASM* host — it holds no
//! server-side session state beyond what [`edge`] journals.

/// The bounded program loop (client placement) — behaviour carried as data:
/// the closed action walk, the per-interaction budget, the default-deny effect
/// vocabulary, and the binding re-resolution pass that makes a state write
/// visible. Carries this host's bounded-path conformance declaration.
pub mod bounded;
pub mod canonical;
pub mod client;
pub mod dag;
pub mod diff;
/// Edge hosting of the certified core — a single-owner session whose op-stream
/// is journaled to the platform's durable store before its held tree moves, and
/// which rehydrates from that journal after an eviction. The store is a trait
/// with a reference implementation, so the tier names obligations rather than a
/// platform.
pub mod edge;
pub mod elicitation;
pub mod envelope;
/// The target-neutral C-ABI export surface (Phase 537) — `extern "C"`
/// `fuaran_*` functions over an opaque [`client::ClientSession`], compiled for
/// the `wasm32` browser client *and* native staticlib / cdylib consumers (the
/// Swift / Kotlin native surfaces). See `include/fuaran.h`.
pub mod ffi;
/// The signature-searchable function registry (Phase 558) — `findBySignature`
/// (EXACT/SUBSUMES) + deterministic compose-path resolution, the Rust twin of
/// the F# `Fuaran.Core.FunctionRegistry` reference.
pub mod function;
pub mod gate;
pub mod introspect;
pub mod limits;
pub mod ops;
pub mod opstream;
pub mod render;
pub mod serverdriven;
pub mod teleport;
pub mod theme;
pub mod transform;
pub mod validator;
pub mod wire;

/// The pre-release version of this host — DERIVED from `Cargo.toml`, never
/// written out here.
///
/// It was a hand-written literal until it read `0.0.4-alpha` against a manifest
/// two releases further on, with nothing anywhere asserting the two against each
/// other: a constant that restates a number the build system already owns will
/// drift, and a stale one is worse than none, since a consumer reading it pins
/// deliberately on a value that is wrong. `tests/version.rs` reads the manifest
/// text and pins the equality, so re-introducing a literal fails the gate.
///
/// The per-release notes below record the releases that carried a contract
/// change and stop at `0.0.4-alpha`; they are not a complete changelog, and the
/// manifest — not this list — says which version the crate is.
///
/// `0.0.4-alpha` is ADDITIVE over `0.0.3-alpha`: the accessible summary (§4i)
/// gains one clause per annotation member, so a chart CARRYING annotations
/// lowers with a longer `description` and a chart carrying none lowers
/// byte-for-byte as before. No type moves; the corpus goldens carry the change.
///
/// `0.0.3-alpha` is ADDITIVE over `0.0.2-alpha`: `ChartSpec.annotations` and the
/// three closed enums behind it (`ChartAnnotation`, `ChartAnnotationX`,
/// `ChartAnnotationRange`), their codec, and the lowering arms that draw them.
/// A pre-1490 document decodes, re-encodes and lowers byte-for-byte as before.
///
/// `0.0.2-alpha` carried the Phase 1168 BREAKING change to the DAG record
/// surface (`dag::DagRecord`'s bare `user_id` becomes the typed `actor`, and
/// pre-1144 DAG content addresses do not carry forward). Recorded in
/// `README.md` — this host declares no `STABILITY.md`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
