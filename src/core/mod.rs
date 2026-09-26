//! The Core boundary (Phase 1863) — this host's twins of the Core reference
//! subsystems, gathered in one place that reaches nothing else in the crate.
//!
//! What it holds:
//! - [`dataframe`] — the dataframe model (cells, schema, column expressions,
//!   transform steps), the twin of `Fuaran.Core.DataFrame`'s model;
//! - [`transform`] — the Transform evaluator and list-param substitution;
//! - [`function`] — the signature-searchable function registry
//!   (`findBySignature` + compose-path resolution);
//! - [`number`] — the canonical number form (`formatFiniteDouble`).
//!
//! It mirrors `Fuaran.Core`, the reference these twins certify against. The
//! module is private: the published paths are the public modules that re-export
//! it (`crate::transform`, `crate::function`, `crate::wire`,
//! `crate::canonical`), so no public name moved when the twins did.
//!
//! The rule the boundary exists for is one-way: code in here uses the standard
//! library and other `core` modules only, never a domain module of this host
//! (`wire`, `render`, `ops`, …). `tests/core_boundary.rs` holds it, so the
//! day these twins are lifted into a crate of their own, the lift is a copy.

pub mod dataframe;
pub mod function;
pub mod number;
pub mod transform;

/// Writes a closed, bare-string wire enum: the `enum` itself, its wire-stable
/// spelling, the full spelling list and the parser. Shared by the dataframe
/// model here and the wire model outside, which is why it sits on this side
/// of the boundary. Imported by path (`use super::bare_enum`,
/// `use crate::core::bare_enum`), never by textual scope, so a reader sees
/// where it comes from.
macro_rules! bare_enum {
    ($(#[$doc:meta])* $name:ident { $($case:ident => $wire:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($case),+
        }

        impl $name {
            /// The wire-stable string form.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$case => $wire),+
                }
            }

            /// Every valid wire spelling, for `UNKNOWN_DU_CASE` hints.
            pub const WIRE_NAMES: &'static [&'static str] = &[$($wire),+];

            /// Parse the wire spelling; `None` on an unknown case.
            pub fn from_wire(s: &str) -> Option<Self> {
                match s {
                    $($wire => Some($name::$case),)+
                    _ => None,
                }
            }
        }
    };
}

pub(crate) use bare_enum;
