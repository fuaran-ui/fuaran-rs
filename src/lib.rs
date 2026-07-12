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
//! Status: stage-0 bootstrap. The canonical number formatter (the [`canonical`]
//! module) is the first shipped brick; the node/op codec, apply engine, and
//! validator are roadmap work (the "floor" tier). Nothing here claims a working
//! codec yet.

pub mod canonical;
pub mod wire;

/// The pre-release version of the `fuaran-rs` host.
pub const VERSION: &str = "0.0.1-alpha";
