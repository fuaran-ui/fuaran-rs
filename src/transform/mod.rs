//! The columnar dataframe evaluator — a pure fold over the columnar model that
//! runs a serialisable `Binding.Transform` pipeline **as data**, client- or
//! server-side, with no external engine. This is the compute substrate behind
//! the Living Sheet (a spreadsheet whose cells are a transform pipeline) and the
//! Pattern Bank's computed metric — a host evaluates the wire's transform to
//! rows deterministically.
//!
//! A faithful port of the cross-host reference evaluator: the pinned semantics
//! (null/NA propagation, int↔float coercion, group/sort stability,
//! round-half-away, division-by-zero, the canonical float layout) match exactly,
//! so the output table encodes byte-identically under the shared canonical codec
//! (the `§11.1` transform-laws parity contract). Where the reference dispatches
//! verbs and operators on strings, this host dispatches on the native `enum`s —
//! recovering compile-time exhaustiveness (a new verb/op is a build error).
//!
//! Total: every recoverable failure is an [`EvalError`], never a panic.
//!
//! The implementation lives behind the crate's Core boundary
//! (`src/core/transform.rs`, Phase 1863); this module is its published face and
//! re-exports it whole, so every path below is the one it has always been.

pub use crate::core::transform::*;
