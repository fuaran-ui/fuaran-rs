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
//! families). The apply engine, validator, and emission tiers are roadmap work.

/// The bounded program loop (client placement) — behaviour carried as data:
/// the closed action walk, the per-interaction budget, the default-deny effect
/// vocabulary, and the binding re-resolution pass that makes a state write
/// visible. Carries this host's bounded-path conformance declaration.
pub mod bounded;
pub mod canonical;
pub mod client;
pub mod dag;
pub mod diff;
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
pub mod ops;
pub mod opstream;
pub mod render;
pub mod serverdriven;
pub mod teleport;
pub mod theme;
pub mod transform;
pub mod validator;
pub mod wire;

/// The pre-release version of the `fuaran-rs` host.
pub const VERSION: &str = "0.0.1-alpha";
