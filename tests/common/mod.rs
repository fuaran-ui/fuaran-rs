//! Shared integration-test support: the ONE locator for the shared wire-format
//! corpus (Phase 2301). Every suite that reads the corpus finds it here, so the
//! rule below is stated once and every leg obeys the same one.
//!
//! # The rule
//!
//! 1. `FUARAN_WIRE_FIXTURES`, when set and non-empty, is a CLAIM: it must name a
//!    directory holding `manifest.json`, or the leg fails naming the path. A
//!    claim is never second-guessed by a fallback — an operator who named a path
//!    has said where the oracle is, so a wrong path is a mistake to report.
//! 2. Otherwise the corpus is discovered by walking up from this crate's
//!    directory to the nearest `wire-format-fixtures/manifest.json`.
//! 3. Nothing claimed and nothing found is a FAILURE, never a skip (Phase 1938):
//!    a conformance check that passes without its oracle is worse than no check.
//!
//! [`resolve`] is the rule as a pure function of the claim and the starting
//! directory, which is what lets `tests/corpus_locator.rs` exercise every verdict
//! without mutating the process environment.

// Each integration-test binary compiles this module separately and uses only
// part of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The environment variable an operator names the corpus root with.
pub const CORPUS_ENV: &str = "FUARAN_WIRE_FIXTURES";

/// The corpus checkout's directory name, as discovered beside this repository.
/// This module is the only test source allowed to spell it as a literal.
pub const CORPUS_DIR: &str = "wire-format-fixtures";

/// The rule above, purely: `declared` is the value of [`CORPUS_ENV`] (if any),
/// `start` the directory the discovery walk begins at.
pub fn resolve(declared: Option<&str>, start: &Path) -> Result<PathBuf, String> {
    if let Some(declared) = declared
        && !declared.trim().is_empty()
    {
        let root = PathBuf::from(declared);
        if root.join("manifest.json").is_file() {
            return Ok(root);
        }
        return Err(format!(
            "{CORPUS_ENV} names '{}', which holds no manifest.json (the corpus's authoritative \
             fixture enumeration). A claimed corpus is never replaced by a discovered one: fix \
             the path, or unset {CORPUS_ENV} to discover the corpus beside this repository.",
            root.display()
        ));
    }
    let mut dir = start.to_path_buf();
    loop {
        let root = dir.join(CORPUS_DIR);
        if root.join("manifest.json").is_file() {
            return Ok(root);
        }
        if !dir.pop() {
            break;
        }
    }
    Err(format!(
        "the wire-format corpus is neither claimed nor found above '{}'. It is the conformance \
         oracle, so this leg FAILS rather than skips (Phase 1938): clone `{CORPUS_DIR}` beside \
         this repository, or name it with {CORPUS_ENV}.",
        start.display()
    ))
}

/// The shared corpus root. Panics on every absence — see the module docs.
pub fn corpus_root() -> PathBuf {
    resolve(
        std::env::var(CORPUS_ENV).ok().as_deref(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
    .unwrap_or_else(|e| panic!("{e}"))
}
