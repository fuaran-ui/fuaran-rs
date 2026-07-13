//! The `wasm32` client entry points — the browser's view of the C-ABI surface.
//!
//! As of Phase 537 the C-ABI is **target-neutral** and lives in
//! [`crate::ffi`](../../ffi/index.html): nothing in the shim was ever
//! `wasm32`-specific, so it now compiles for native staticlib / cdylib
//! consumers (the Swift XCFramework + Kotlin cargo-ndk tiers) as well as the
//! browser module. This module survives as the shim's **historical path** and
//! the browser build's re-export point — the thin JS loader
//! (`js/fuaran-loader.js`) links the exact same exported symbols
//! (`fuaran_alloc` / `fuaran_dealloc` / `fuaran_session_*` / `fuaran_last_error`),
//! which are defined once in [`crate::ffi`] and emitted into the `wasm32`
//! `cdylib` unchanged.
//!
//! Compiled only for `target_arch = "wasm32"` (the re-export is a source-level
//! convenience; the symbols themselves are target-neutral). See
//! `include/fuaran.h` for the C declarations + ownership/threading contract.

pub use crate::ffi::*;
