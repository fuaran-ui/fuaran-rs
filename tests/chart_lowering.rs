//! Chart → Drawing lowering certification (Phase 551, lower-in-host posture).
//!
//! The Rust lowering (`fuaran_rs::render::chart_lowering::lower_chart`) is pinned
//! by the language-neutral `wire-format-fixtures/chart-lowering/*` fixture family:
//! each case ships an `<name>.input.json` (the ChartSpec + data rows, the neutral
//! cross-host contract) and an `<name>.expected.json` (the canonical Drawing wire
//! JSON the lowering must produce). This suite asserts the Rust lowering
//! reproduces each F# golden byte-for-byte — the same R2-determinism discipline
//! the F#, TypeScript, and Python hosts certify against.
//!
//! The corpus is a sibling checkout; the suite skips (never fails) when it is
//! absent, keeping the crate standalone-testable.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::chart_lowering::{lower_chart, project_row};
use fuaran_rs::wire::{ChartKind, Node, NodeKind, SemanticStyle, StateBehaviour, TextSource};

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

fn chart_kind(wire: &str) -> ChartKind {
    match wire {
        "Bar" => ChartKind::Bar,
        "Line" => ChartKind::Line,
        "Area" => ChartKind::Area,
        "Pie" => ChartKind::Pie,
        "Scatter" => ChartKind::Scatter,
        "Heatmap" => ChartKind::Heatmap,
        other => panic!("unknown chart kind in fixture: {other}"),
    }
}

fn str_field<'a>(obj: &'a JVal, key: &str) -> &'a str {
    match obj.field(key) {
        Some(JVal::Str(v)) => v,
        _ => panic!("fixture missing string field {key}"),
    }
}

/// Lower one fixture input to the canonical Drawing-node wire JSON, mirroring the
/// reference harness: lower → wrap in a Drawing node id `chart-<name>` → encode.
fn lowered_json(name: &str, input: &str) -> String {
    let spec = parse(input).expect("fixture input parses");
    let kind = chart_kind(str_field(&spec, "kind"));
    let x_field = str_field(&spec, "xField").to_string();
    let y_fields: Vec<String> = match spec.field("yFields") {
        Some(JVal::Arr(items)) => items
            .iter()
            .map(|it| match it {
                JVal::Str(v) => v.clone(),
                _ => panic!("yFields entry not a string"),
            })
            .collect(),
        _ => panic!("fixture missing yFields array"),
    };
    let title: Option<TextSource> = match spec.field("title") {
        Some(JVal::Str(t)) => Some(TextSource::Literal(t.clone())),
        _ => None,
    };
    let rows: Vec<_> = match spec.field("data") {
        Some(JVal::Arr(items)) => items
            .iter()
            .map(|row| project_row(row, &x_field, &y_fields))
            .collect(),
        _ => panic!("fixture missing data array"),
    };

    let drawing = lower_chart(kind, &x_field, &y_fields, title.as_ref(), &rows);
    let node = Node {
        id: format!("chart-{name}"),
        kind: NodeKind::Drawing(drawing),
        state: StateBehaviour::default(),
        style: SemanticStyle::default(),
        accessibility: None,
    };
    fuaran_rs::wire::encode_node(&node)
}

const CASES: [&str; 4] = ["bar-single", "bar-multi", "line-single", "line-multi"];

#[test]
fn every_case_lowers_byte_identically_to_its_committed_golden() {
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let dir = corpus.join("chart-lowering");
    for name in CASES {
        let input = std::fs::read_to_string(dir.join(format!("{name}.input.json")))
            .unwrap_or_else(|e| panic!("{name}: input missing: {e}"));
        let expected = std::fs::read_to_string(dir.join(format!("{name}.expected.json")))
            .unwrap_or_else(|e| panic!("{name}: golden missing: {e}"));
        let produced = lowered_json(name, input.trim());
        assert_eq!(
            produced,
            expected.trim(),
            "{name}: lowering drifted from the F# golden"
        );
    }
}

#[test]
fn lowering_is_deterministic() {
    let Some(corpus) = find_corpus() else {
        return;
    };
    let dir = corpus.join("chart-lowering");
    for name in CASES {
        let Ok(input) = std::fs::read_to_string(dir.join(format!("{name}.input.json"))) else {
            continue;
        };
        let a = lowered_json(name, input.trim());
        let b = lowered_json(name, input.trim());
        assert_eq!(a, b, "{name}: non-deterministic");
    }
}
