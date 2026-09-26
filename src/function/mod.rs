//! The signature-searchable function registry (Phase 558) — the Rust host of the
//! F# reference `Fuaran.Core.FunctionRegistry.findBySignature` (Phase 50/512)
//! plus deterministic compose-path resolution (the twin of the Python
//! `fuaran_ui.function` registry, Phase 523).
//!
//! Composition-by-lookup, not composition-by-generation: register functions by
//! the node-kind they *produce* and the typed *holes* they require, then ask the
//! registry "what can I run to produce X with the context I have?" — a total,
//! in-memory structural search, no model call, no server — and compose a result
//! by chaining matched functions rather than prompting. This is the Pattern
//! Bank's deterministic no-model-call fast path.
//!
//! Reference semantics (canonical = F#):
//! - a query is `(result_type, available)` — the node-kind to produce (`None` =
//!   any) plus the context holes on offer; only a function's REQUIRED holes gate
//!   a match; matching is by absolute address.
//! - [`MatchMode::Subsumes`] — result type matches (or wildcard) and every
//!   required hole is satisfiable from context (`available ⊆ required` for value
//!   spaces, a slot-kind match for slots).
//! - [`MatchMode::Exact`] — the required-hole address set equals the context set
//!   and each pair is shape-equal (kind + space + slot).
//! - candidates return in deterministic lexicographic id order (no ranking).
//! - a compose that cannot reach the target returns a typed [`ComposeResult::NoPath`],
//!   never a guess — the closed wire outcomes modelled as a native, exhaustive
//!   `enum`.
//!
//! Certified against the shared `wire-format-fixtures/function-registry` goldens —
//! shape-identical resolution across the F#, py, ts, go, rs hosts. NOTE on the one
//! host divergence: the F# reference `spaceSubsumes` treats an `AnyString`
//! required space as subsuming an `Enum` available; the Python host does not. This
//! host follows the F# reference (the canonical semantics); the shared goldens
//! deliberately avoid that single edge so every host agrees on every fixture.
//!
//! The implementation lives behind the crate's Core boundary
//! (`src/core/function.rs`, Phase 1863); this module is its published face and
//! re-exports it whole, so every path below is the one it has always been.

pub use crate::core::function::*;
