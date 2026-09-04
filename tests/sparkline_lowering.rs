//! `Sparkline → Drawing` lowering certification (Phase 1099).
//!
//! The Rust lowering (`fuaran_rs::render::sparkline_lowering::try_lower_sparkline`)
//! is pinned by the language-neutral `wire-format-fixtures/sparkline-lowering/*`
//! family the reference host authored: each case ships an `<name>.input.json`
//! (the RESOLVED series — a host runs its own binding resolution first, so no
//! vector carries a binding) and an `<name>.expected.json` (the canonical wire
//! JSON of the `Drawing` node the lowering must produce, or the literal `null`
//! for a case with nothing to draw). This suite discovers and certifies EVERY
//! committed pair — a golden added to the corpus without a matching arm here
//! fails loudly, never silently narrows coverage.
//!
//! **This is the first direct test of this arm.** Before Phase 1099 the host's
//! sparkline had no test at all: the corpus round-trip walk exercises the DECODE
//! of a `Sparkline` node and says nothing whatever about the picture, so a
//! hand-written builder that had drifted from every other host would have been
//! caught by nothing here. The golden leg and the render leg below are the net,
//! built in the same pass that moved the code into it.
//!
//! The corpus is a sibling checkout; the suite skips (never fails) when it is
//! absent, keeping the crate standalone-testable — the same posture
//! `tests/chart_lowering.rs` takes.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::BindingSources;
use fuaran_rs::render::sparkline_lowering::try_lower_sparkline;
use fuaran_rs::wire::{
    Binding, Node, NodeKind, SemanticStyle, SparklineSpec, StateBehaviour, StaticValue,
};

/// Walk up from the crate dir to the shared corpus (mirrors conformance.rs).
fn find_corpus() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let root = dir.join("wire-format-fixtures");
        if root.join("manifest.json").is_file() {
            return Some(root);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Every committed `<name>.input.json` / `<name>.expected.json` pair, sorted.
/// A dangling half is a panic — the corpus is the authoritative enumeration and
/// must be self-consistent.
fn discover_cases(dir: &std::path::Path) -> Vec<String> {
    let mut inputs: Vec<String> = Vec::new();
    let mut expected: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir).expect("sparkline-lowering dir readable") {
        let name = entry.expect("dir entry").file_name();
        let name = name.to_string_lossy().to_string();
        if let Some(stem) = name.strip_suffix(".input.json") {
            inputs.push(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(".expected.json") {
            expected.push(stem.to_string());
        }
    }
    inputs.sort();
    expected.sort();
    assert_eq!(
        inputs, expected,
        "sparkline-lowering corpus has a dangling input/golden half"
    );
    assert!(!inputs.is_empty(), "sparkline-lowering corpus is empty");
    inputs
}

/// One series element. A non-finite value is the same string sentinel the wire
/// format spells it as everywhere else, so this reads the vectors with the
/// decoder rule the host already has.
///
/// Anything else PANICS rather than being dropped. A `filter_map` here would
/// silently shorten the series and still produce a plausible picture — which is
/// the exact shape of the decode divergence Phase 1099 closes on another host,
/// and a harness must not be able to hide it.
fn series_element(v: &JVal) -> f64 {
    match v {
        JVal::Num(n) => *n,
        JVal::Str(s) if s == "NaN" => f64::NAN,
        JVal::Str(s) if s == "Infinity" => f64::INFINITY,
        JVal::Str(s) if s == "-Infinity" => f64::NEG_INFINITY,
        other => panic!("sparkline-lowering input: unsupported series element {other:?}"),
    }
}

fn series_of(input: &str) -> Vec<f64> {
    let root = parse(input).expect("sparkline-lowering input is well-formed JSON");
    match root.field("series") {
        Some(JVal::Arr(items)) => items.iter().map(series_element).collect(),
        other => panic!("sparkline-lowering input carries no `series` array: {other:?}"),
    }
}

/// The corpus states the answer as a `Drawing` NODE, so the projection is a node
/// encode rather than a bare spec encode.
fn encode_as_node(name: &str, drawing: fuaran_rs::wire::DrawingSpec) -> String {
    let node = Node {
        id: format!("sparkline-{name}"),
        kind: NodeKind::Drawing(drawing),
        state: StateBehaviour::default(),
        style: SemanticStyle::default(),
        accessibility: None,
        tooltip: None,
    };
    fuaran_rs::wire::encode_node(&node)
}

/// Lower a resolved series and project it the way the corpus states the answer:
/// the canonical wire JSON of a `Drawing` node whose id is `sparkline-<name>`,
/// or the literal `null` when there is nothing to draw.
fn lowered_json(name: &str, input: &str) -> String {
    match try_lower_sparkline(&series_of(input)) {
        None => "null".to_string(),
        Some(drawing) => encode_as_node(name, drawing),
    }
}

#[test]
fn every_case_lowers_byte_identically_to_its_committed_golden() {
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let dir = corpus.join("sparkline-lowering");
    for name in discover_cases(&dir) {
        let input = std::fs::read_to_string(dir.join(format!("{name}.input.json")))
            .unwrap_or_else(|e| panic!("{name}: input missing: {e}"));
        let expected = std::fs::read_to_string(dir.join(format!("{name}.expected.json")))
            .unwrap_or_else(|e| panic!("{name}: golden missing: {e}"));
        let produced = lowered_json(&name, input.trim());
        assert_eq!(
            produced,
            expected.trim(),
            "{name}: lowering drifted from the reference golden"
        );
    }
}

#[test]
fn lowering_is_deterministic() {
    let Some(corpus) = find_corpus() else {
        return;
    };
    let dir = corpus.join("sparkline-lowering");
    for name in discover_cases(&dir) {
        let Ok(input) = std::fs::read_to_string(dir.join(format!("{name}.input.json"))) else {
            continue;
        };
        let a = lowered_json(&name, input.trim());
        let b = lowered_json(&name, input.trim());
        assert_eq!(a, b, "{name}: non-deterministic");
    }
}

/// The go-red probe, and it is not optional: a golden comparison whose only
/// evidence is that it passed says nothing about whether it CAN fail. Every
/// drawn vector gets one more sample appended to its series, and the comparison
/// must reject the result.
///
/// **The obvious perturbation is the wrong one, and this test caught it.** The
/// first shape here bumped the LAST value by one — and it is invisible on three
/// of the six drawn vectors, because the lowering normalises: `two-points`'
/// endpoints are the min and the max whatever their values, `single-point` is
/// centred at the mid-line whatever its value, and every coordinate of
/// `nonfinite-sentinel` is already `NaN`. Half the family would have been
/// certified by a probe that could not move it. Appending a sample changes the
/// POINT COUNT, which no normalisation can absorb.
///
/// So it asserts it perturbed something rather than trusting that it did: the
/// count of vectors actually discriminated must equal the count of drawn
/// vectors. That equality is what turned the weak probe red instead of letting
/// it pass on the three vectors it did move.
#[test]
fn the_golden_comparison_discriminates() {
    let Some(corpus) = find_corpus() else {
        return;
    };
    let dir = corpus.join("sparkline-lowering");
    let mut drawn = 0usize;
    let mut rejected = 0usize;
    for name in discover_cases(&dir) {
        let input = std::fs::read_to_string(dir.join(format!("{name}.input.json")))
            .unwrap_or_else(|e| panic!("{name}: input missing: {e}"));
        let expected = std::fs::read_to_string(dir.join(format!("{name}.expected.json")))
            .unwrap_or_else(|e| panic!("{name}: golden missing: {e}"));
        let expected = expected.trim().to_string();
        let mut series = series_of(input.trim());
        // `empty` has nothing to perturb: its golden is `null`, the ABSENCE of a
        // drawing rather than a drawing that could move. Named rather than
        // skipped silently.
        if series.is_empty() {
            assert_eq!(expected, "null", "{name}: empty series, non-null golden");
            continue;
        }
        drawn += 1;
        series.push(0.0);
        let Some(drawing) = try_lower_sparkline(&series) else {
            panic!("{name}: a perturbed non-empty series lowered to nothing");
        };
        if encode_as_node(&name, drawing) != expected {
            rejected += 1;
        }
    }
    assert!(drawn > 0, "no drawn vector to perturb");
    assert_eq!(
        rejected, drawn,
        "the golden comparison did not reject a perturbed series — it cannot fail, \
         so it proves nothing"
    );
}

// ─── The host arm ────────────────────────────────────────────────────────────
//
// The goldens certify the LOWERING. They say nothing about whether the renderer
// reaches it, so these pin the wiring: a `Sparkline` node rendered through the
// public entry point must carry the container the stylesheet targets, with the
// shared builder's svg as its DIRECT child, and none of the retired builder's
// own vocabulary.

fn render_series(values: &[f64]) -> String {
    let node = Node {
        id: "spark".to_string(),
        kind: NodeKind::Sparkline(SparklineSpec {
            source: Binding::Static {
                value: StaticValue::FloatSeq(values.to_vec()),
            },
        }),
        state: StateBehaviour::default(),
        style: SemanticStyle::default(),
        accessibility: None,
        tooltip: None,
    };
    fuaran_rs::render::render_to_html(&node, &BindingSources::default())
}

#[test]
fn the_rendered_sparkline_is_the_lowered_drawing_inside_the_hook_container() {
    let html = render_series(&[1.0, 2.0, 3.0, 2.0, 4.0]);
    assert!(
        html.contains("<div class=\"fuaran-sparkline\"><svg class=\"fuaran-drawing\""),
        "the drawing must be a DIRECT child of the hook — the stylesheet rule is \
         `.fuaran-sparkline > .fuaran-drawing`. Got: {html}"
    );
    // The geometry the `normal` golden fixes, reaching the DOM.
    assert!(
        html.contains("points=\"0,29 25,19.67 50,10.33 75,19.67 100,1\""),
        "the rendered polyline must carry the lowered geometry. Got: {html}"
    );
}

#[test]
fn no_hand_written_sparkline_vocabulary_survives_in_the_output() {
    let html = render_series(&[1.0, 2.0, 3.0]);
    assert!(
        !html.contains("fuaran-sparkline-line"),
        "the retired builder's own class is still being emitted: {html}"
    );
    assert!(
        !html.contains("preserveAspectRatio"),
        "the retired builder's own attribute is still being emitted: {html}"
    );
}

#[test]
fn an_empty_series_keeps_the_em_dash_fallback() {
    let html = render_series(&[]);
    assert!(
        html.contains("<div class=\"fuaran-sparkline fuaran-sparkline-empty\">"),
        "an empty series must keep its declared fallback element. Got: {html}"
    );
    assert!(
        !html.contains("fuaran-drawing"),
        "an empty series must draw nothing at all — not an empty canvas. Got: {html}"
    );
}
