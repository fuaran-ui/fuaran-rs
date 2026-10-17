//! The crate's one corpus locator (Phase 2301), held to its rule.
//!
//! `common::resolve` is the rule as a pure function, so each verdict is produced
//! here from a directory built to produce it rather than from whatever this
//! machine happens to hold — including the absence, which cannot be produced on
//! a machine with a real corpus, and the claimed-but-wrong path, which must FAIL
//! rather than fall back to a discovered corpus nobody named.
//!
//! The second half keeps it the ONE locator: no other test source may spell the
//! corpus directory's name as a string literal, because a suite that does is a
//! suite with its own lookup — one that can disagree with the rule above about
//! which corpus is under test, or whether its absence is a failure.

mod common;

use std::path::{Path, PathBuf};

use common::{CORPUS_DIR, CORPUS_ENV, resolve};

/// A scratch tree unique to this test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "fuaran-rs-corpus-locator-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    /// A directory holding a `manifest.json`, i.e. a readable corpus.
    fn corpus_at(&self, rel: &str) -> PathBuf {
        let dir = self.0.join(rel);
        std::fs::create_dir_all(&dir).expect("scratch corpus dir");
        std::fs::write(dir.join("manifest.json"), b"{}").expect("scratch manifest");
        dir
    }

    fn dir_at(&self, rel: &str) -> PathBuf {
        let dir = self.0.join(rel);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn nothing_claimed_and_nothing_found_fails_naming_the_variable() {
    let scratch = Scratch::new("absent");
    let start = scratch.dir_at("standalone/fuaran-rs");
    let err = resolve(None, &start).expect_err("an absent corpus must not resolve");
    assert!(err.contains(CORPUS_ENV), "{err}");
    // An empty or whitespace claim is not a claim: it falls through to discovery.
    let err = resolve(Some("   "), &start).expect_err("a blank claim must not resolve");
    assert!(err.contains(CORPUS_ENV), "{err}");
}

#[test]
fn a_corpus_beside_the_crate_is_discovered() {
    let scratch = Scratch::new("discovered");
    let start = scratch.dir_at("found/fuaran-rs");
    let corpus = scratch.corpus_at(&format!("found/{CORPUS_DIR}"));
    assert_eq!(resolve(None, &start), Ok(corpus.clone()));
    assert_eq!(resolve(Some(""), &start), Ok(corpus));
}

#[test]
fn a_valid_claim_is_returned() {
    let scratch = Scratch::new("claimed");
    let start = scratch.dir_at("standalone/fuaran-rs");
    let claimed = scratch.corpus_at("elsewhere/corpus");
    let declared = claimed.to_str().expect("utf-8 scratch path");
    assert_eq!(resolve(Some(declared), &start), Ok(claimed.clone()));
}

#[test]
fn a_claim_without_a_manifest_fails_and_never_falls_back() {
    let scratch = Scratch::new("wrong-claim");
    let start = scratch.dir_at("found/fuaran-rs");
    // A discoverable corpus is right there — and must NOT be used, because the
    // operator named a different one.
    scratch.corpus_at(&format!("found/{CORPUS_DIR}"));
    let wrong = scratch.dir_at("sideways/not-a-corpus");
    let declared = wrong.to_str().expect("utf-8 scratch path");

    let err = resolve(Some(declared), &start).expect_err("a wrong claim must fail");
    assert!(
        err.contains(declared),
        "the failure names the claimed path: {err}"
    );
    assert!(err.contains(CORPUS_ENV), "{err}");

    let missing = scratch.0.join("no/such/corpus");
    let declared = missing.to_str().expect("utf-8 scratch path");
    let err = resolve(Some(declared), &start).expect_err("a missing claim must fail");
    assert!(err.contains(declared), "{err}");
}

#[test]
#[should_panic(expected = "FUARAN_WIRE_FIXTURES")]
fn the_failure_is_a_panic_not_a_skip() {
    // The shape `corpus_root` gives every verdict above.
    let scratch = Scratch::new("panics");
    let start = scratch.dir_at("standalone/fuaran-rs");
    resolve(None, &start).unwrap_or_else(|e| panic!("{e}"));
}

/// Every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("reading tests/") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The offending lines of one source: a non-comment line opening a string
/// literal with the corpus directory's name — `join("…")`, `"…/nodes"`, and
/// every other spelling of a private lookup. Prose may name the corpus.
fn literal_spellings(text: &str) -> Vec<(usize, String)> {
    let needle = format!("\"{CORPUS_DIR}");
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//") && line.contains(&needle))
        .map(|(i, line)| (i + 1, line.trim().to_string()))
        .collect()
}

#[test]
fn no_test_source_but_the_locator_spells_the_corpus_directory() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let locator = tests.join("common").join("mod.rs");
    let mut sources = Vec::new();
    rust_sources(&tests, &mut sources);
    assert!(
        sources.len() > 10,
        "the scan found {} sources under {} — it is not looking where the suites are",
        sources.len(),
        tests.display()
    );

    let mut offenders = Vec::new();
    for path in sources.iter().filter(|p| **p != locator) {
        let text = std::fs::read_to_string(path).expect("reading a test source");
        for (line, code) in literal_spellings(&text) {
            offenders.push(format!("{}:{line}: {code}", path.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "these test sources carry their own corpus lookup; use common::corpus_root() \
         instead:\n{}",
        offenders.join("\n")
    );

    // The scan must be able to see the locator's own spelling, or it proves nothing.
    let own = std::fs::read_to_string(&locator).expect("reading the locator");
    assert!(
        !literal_spellings(&own).is_empty(),
        "the guard cannot see the spelling it exists to confine"
    );
}

/// The go-red for the guard: it flags a private lookup and passes prose.
#[test]
fn the_literal_guard_discriminates() {
    let lookup = format!("    let root = dir.join(\"{CORPUS_DIR}\");\n");
    assert_eq!(literal_spellings(&lookup).len(), 1);
    let path = format!("    let p = Path::new(\"{CORPUS_DIR}/nodes\");\n");
    assert_eq!(literal_spellings(&path).len(), 1);
    let prose = format!("//! certified against `{CORPUS_DIR}/dag/`\n    // see {CORPUS_DIR}\n");
    assert!(literal_spellings(&prose).is_empty());
}
