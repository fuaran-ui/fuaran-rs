//! Certifies the deterministic markdown renderer against the shared
//! cross-host corpus (`wire-format-fixtures/markdown/corpus.json`): every
//! fixture's `render(source)` must equal its pinned `html` byte-for-byte.
//! Skips cleanly when the corpus is absent (standalone checkout).

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::markdown::to_html;

fn find_corpus() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let file = dir
            .join("wire-format-fixtures")
            .join("markdown")
            .join("corpus.json");
        if file.is_file() {
            return Some(file);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn str_field(v: &JVal, key: &str) -> Option<String> {
    match v.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

#[test]
fn markdown_corpus_renders_byte_identical() {
    let Some(path) = find_corpus() else {
        eprintln!("markdown corpus not found; skipping (standalone checkout)");
        return;
    };
    let raw = std::fs::read_to_string(&path).expect("reading markdown corpus");
    let corpus = parse(&raw).expect("markdown corpus parses");
    let Some(JVal::Arr(fixtures)) = corpus.field("fixtures") else {
        panic!("markdown corpus declares no fixtures array");
    };
    let mut failures: Vec<String> = Vec::new();
    for fixture in fixtures {
        let id = str_field(fixture, "id").expect("fixture id");
        let source = str_field(fixture, "source").expect("fixture source");
        let expected = str_field(fixture, "html").expect("fixture html");
        let actual = to_html(&source);
        if actual != expected {
            failures.push(format!(
                "{id}:\n  source   {source:?}\n  expected {expected:?}\n  actual   {actual:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} markdown fixtures diverged:\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n")
    );
    eprintln!(
        "markdown corpus: {} fixtures byte-identical",
        fixtures.len()
    );
}
