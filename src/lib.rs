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

pub mod canonical;
pub mod client;
pub mod dag;
pub mod introspect;
pub mod ops;
pub mod opstream;
pub mod render;
pub mod theme;
pub mod validator;
pub mod wire;

/// The pre-release version of the `fuaran-rs` host.
pub const VERSION: &str = "0.0.1-alpha";
