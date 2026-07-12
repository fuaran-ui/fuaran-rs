//! Certifies the `fuaran-rs` host against the shared `wire-format-fixtures` corpus.
//! Stage-0: a smoke leg that locates the corpus manifest and confirms it declares
//! fixtures. The byte-for-byte round-trip and reject legs land with the codec
//! (roadmap floor) and are skipped here.
//!
//! Rust's standard library has no JSON parser, so the stage-0 smoke leg does a
//! dependency-free presence + fixture-count check; the real manifest parse arrives
//! with the codec floor, alongside the hand-written canonical JSON layer.

use std::path::PathBuf;

/// Walks up from the crate directory looking for the shared `wire-format-fixtures`
/// corpus (a sibling of the `fuaran-rs` repo under Fuaran-UI). Returns `None` when
/// absent, so the repo stays standalone-testable — the corpus legs skip (pass)
/// rather than fail when the repo is checked out alone.
fn find_corpus() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let manifest = dir.join("wire-format-fixtures").join("manifest.json");
        if manifest.is_file() {
            return Some(manifest);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Stage-0 smoke leg: prove the harness can locate the shared corpus manifest and
/// that it declares fixtures. Round-trip + reject legs land with the codec floor.
#[test]
fn corpus_manifest_loads() {
    let Some(manifest) = find_corpus() else {
        eprintln!(
            "wire-format-fixtures corpus not found alongside the repo; skipping (standalone checkout)"
        );
        return;
    };
    let raw = std::fs::read_to_string(&manifest).expect("reading manifest");
    assert!(!raw.trim().is_empty(), "corpus manifest is empty");
    // Dependency-free fixture count: the manifest lists one `"id"` per fixture.
    let fixture_count = raw.matches("\"id\"").count();
    assert!(fixture_count > 0, "corpus manifest declares no fixtures");
    eprintln!(
        "corpus located: {fixture_count} fixtures declared (round-trip + reject legs pending the codec floor)"
    );
}

/// Placeholder for the byte-for-byte round-trip and reject legs, which require the
/// codec (roadmap floor). Nothing to assert yet.
#[test]
fn codec_round_trip_pending() {
    eprintln!(
        "node/op round-trip + reject legs pending the codec floor — see CLAUDE.md and the fuaran roadmap"
    );
}
