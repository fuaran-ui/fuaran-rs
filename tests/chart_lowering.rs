//! Chart → Drawing lowering certification (Phase 551, lower-in-host posture).
//!
//! The Rust lowering (`fuaran_rs::render::chart_lowering::lower_chart`) is pinned
//! by the language-neutral `wire-format-fixtures/chart-lowering/*` fixture family:
//! each case ships an `<name>.input.json` (the ChartSpec + data rows, the neutral
//! cross-host contract) and an `<name>.expected.json` (the canonical Drawing wire
//! JSON the lowering must produce). This suite discovers and certifies EVERY
//! committed pair — a golden added to the corpus without a matching arm here
//! fails loudly, never silently narrows coverage — asserting the Rust lowering
//! reproduces each F# golden byte-for-byte (the same R2-determinism discipline
//! the F#, TypeScript, and Python hosts certify against).
//!
//! The corpus is a sibling checkout; the suite skips (never fails) when it is
//! absent, keeping the crate standalone-testable.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::chart_lowering::{
    ChartAxisUnitMode, ChartLowerStyle, ChartTitles, lower_chart_with, project_row,
};
use fuaran_rs::wire::{
    Binding, ChartDataLabels, ChartKind, ChartLegendPosition, ChartXScale, Format, Node, NodeKind,
    SemanticStyle, StateBehaviour, StaticValue, TextSource,
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

/// Every committed `<name>.input.json` / `<name>.expected.json` pair, sorted.
/// A dangling half (an input with no golden, or vice versa) is a panic — the
/// corpus is the authoritative enumeration and must be self-consistent.
fn discover_cases(dir: &std::path::Path) -> Vec<String> {
    let mut inputs: Vec<String> = Vec::new();
    let mut expected: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir).expect("chart-lowering dir readable") {
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
        "chart-lowering corpus has a dangling input/golden half"
    );
    assert!(!inputs.is_empty(), "chart-lowering corpus is empty");
    inputs
}

/// The corpus carries a `Format` in canonical `$type` wire JSON; the lowering
/// takes the typed enum. Only the numeric arms appear in chart inputs.
fn value_format_of(j: &JVal) -> Format {
    let tag = match j.field("$type") {
        Some(JVal::Str(t)) => t.as_str(),
        _ => panic!("valueFormat missing $type"),
    };
    let decimals = match j.field("decimals") {
        Some(JVal::Num(d)) => Some(*d as i64),
        _ => None,
    };
    match tag {
        "Number" => Format::Number { decimals },
        "Percent" => Format::Percent { decimals },
        "Currency" => Format::Currency {
            iso_code: match j.field("isoCode") {
                Some(JVal::Str(c)) => c.clone(),
                _ => panic!("Currency valueFormat missing isoCode"),
            },
        },
        other => panic!("chart-lowering input: unsupported valueFormat {other}"),
    }
}

/// The corpus carries the four `TextSource`-typed chart declarations — `title`,
/// `subtitle`, `xTitle`, `yTitle` — in canonical wire JSON, exactly as
/// `valueFormat` above carries a `Format`. A BARE STRING is the canonical
/// `Literal` spelling (§16 lenient-accept, normative for every conformant
/// host); the object arms are `Literal` / `Bound` / `I18n`.
///
/// Building every arm here is the whole point of Phase 1143. The lowering
/// carries a non-literal declaration into the drawing UNRESOLVED (clauses 1–2
/// of `CHART-LOWERING-TEXT-CONTRACT.md`), so a harness that matched only the
/// bare string handed it `None` — and the fixture that exists to pin the carry
/// lowered as though the author had declared nothing at all, drawing the
/// capitalised field-name fallback in place of the author's meaning. That is
/// precisely the DROP the contract forbids, arrived at through the harness
/// rather than the host. An unsupported arm therefore PANICS rather than
/// degrading to `None`, for the same reason `value_format_of` does: a silent
/// `None` is what hid this.
fn text_source_of(j: &JVal) -> TextSource {
    if let JVal::Str(s) = j {
        return TextSource::Literal(s.clone());
    }
    let tag = match j.field("$type") {
        Some(JVal::Str(t)) => t.as_str(),
        _ => panic!("chart-lowering input: TextSource is neither a bare string nor $type-tagged"),
    };
    match tag {
        "Literal" => match j.field("text") {
            Some(JVal::Str(s)) => TextSource::Literal(s.clone()),
            _ => panic!("chart-lowering input: Literal TextSource missing text"),
        },
        "Bound" => match j.field("binding") {
            Some(b) => TextSource::Bound(Box::new(binding_of(b))),
            None => panic!("chart-lowering input: Bound TextSource missing binding"),
        },
        "I18n" => TextSource::I18n {
            key: match j.field("key") {
                Some(JVal::Str(k)) => k.clone(),
                _ => panic!("chart-lowering input: I18n TextSource missing key"),
            },
            args: match j.field("args") {
                None => Vec::new(),
                Some(JVal::Obj(entries)) => entries.clone(),
                Some(_) => panic!("chart-lowering input: I18n args is not an object"),
            },
        },
        other => panic!("chart-lowering input: unsupported TextSource arm {other}"),
    }
}

/// The `Binding` behind a `Bound` text declaration. Only the `Static` arm
/// appears in this fixture family — a chart title's binding is what the drawing
/// carries UNRESOLVED, so the corpus pins the carry and not any resolver — and
/// an arm the family has not exercised panics rather than being approximated.
fn binding_of(j: &JVal) -> Binding {
    let tag = match j.field("$type") {
        Some(JVal::Str(t)) => t.as_str(),
        _ => panic!("chart-lowering input: Binding missing $type"),
    };
    match tag {
        "Static" => Binding::Static {
            value: StaticValue::Ast(j.field("value").cloned().unwrap_or(JVal::Null)),
        },
        other => {
            panic!("chart-lowering input: unsupported Binding arm {other} behind a Bound text")
        }
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
    let stacked = matches!(spec.field("stacked"), Some(JVal::Bool(true)));
    // Phase 878 / 1143 — the four `TextSource`-typed declarations, carried in
    // canonical wire JSON and omitted when absent.
    // An explicit `null` is how three cases in this family spell "no title";
    // it reads as ABSENT here, as it always has, and never as a declaration.
    let text_field = |key: &str| -> Option<TextSource> {
        match spec.field(key) {
            None | Some(JVal::Null) => None,
            Some(j) => Some(text_source_of(j)),
        }
    };
    let title = text_field("title");
    let x_title = text_field("xTitle");
    let y_title = text_field("yTitle");
    let subtitle = text_field("subtitle");
    // Phase 880 — `legendPosition` is a bare wire string in the fixture input,
    // omitted when the case takes the host default.
    let legend_position: Option<ChartLegendPosition> = match spec.field("legendPosition") {
        Some(JVal::Str(p)) => Some(
            ChartLegendPosition::from_wire(p)
                .unwrap_or_else(|| panic!("unknown legendPosition in fixture: {p}")),
        ),
        _ => None,
    };
    // Phase 881 — `dataLabels` is a bare wire string in the fixture input,
    // omitted when the case declares none (which means `Off`, the default).
    let data_labels: Option<ChartDataLabels> = match spec.field("dataLabels") {
        Some(JVal::Str(d)) => Some(
            ChartDataLabels::from_wire(d)
                .unwrap_or_else(|| panic!("unknown dataLabels in fixture: {d}")),
        ),
        _ => None,
    };
    // Phase 882 — `xScale` is a bare wire string in the fixture input, omitted
    // when the case declares none (which means `Category`, the default).
    let x_scale: Option<ChartXScale> = match spec.field("xScale") {
        Some(JVal::Str(x)) => Some(
            ChartXScale::from_wire(x).unwrap_or_else(|| panic!("unknown xScale in fixture: {x}")),
        ),
        _ => None,
    };
    let rows: Vec<_> = match spec.field("data") {
        Some(JVal::Arr(items)) => items
            .iter()
            .map(|row| project_row(row, &x_field, &y_fields))
            .collect(),
        _ => panic!("fixture missing data array"),
    };

    // Phase 876 — `valueFormat` is a WIRE field carried in canonical `Format`
    // JSON; `axisUnitMode` is a harness-only STYLE selector (the chart style is
    // a lowering parameter, never wire), present so the corpus can pin every
    // mode. Both absent on the pre-876 cases.
    let value_format = spec.field("valueFormat").map(value_format_of);
    let style = ChartLowerStyle {
        axis_unit_mode: match spec.field("axisUnitMode") {
            Some(JVal::Str(m)) => match m.as_str() {
                "WordsWithSymbol" => ChartAxisUnitMode::WordsWithSymbol,
                "SIAbbreviation" => ChartAxisUnitMode::SIAbbreviation,
                "CompactPerTick" => ChartAxisUnitMode::CompactPerTick,
                "Off" => ChartAxisUnitMode::Off,
                _ => ChartAxisUnitMode::Words,
            },
            _ => ChartAxisUnitMode::Words,
        },
        ..ChartLowerStyle::default()
    };

    let drawing = lower_chart_with(
        kind,
        stacked,
        &x_field,
        &y_fields,
        title.as_ref(),
        &ChartTitles {
            x_title: x_title.as_ref(),
            y_title: y_title.as_ref(),
            subtitle: subtitle.as_ref(),
            legend_position,
            data_labels,
            x_scale,
        },
        value_format.as_ref(),
        &style,
        &rows,
    );
    let node = Node {
        id: format!("chart-{name}"),
        kind: NodeKind::Drawing(drawing),
        state: StateBehaviour::default(),
        style: SemanticStyle::default(),
        accessibility: None,
        tooltip: None,
    };
    fuaran_rs::wire::encode_node(&node)
}

#[test]
fn every_case_lowers_byte_identically_to_its_committed_golden() {
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let dir = corpus.join("chart-lowering");
    for name in discover_cases(&dir) {
        let input = std::fs::read_to_string(dir.join(format!("{name}.input.json")))
            .unwrap_or_else(|e| panic!("{name}: input missing: {e}"));
        let expected = std::fs::read_to_string(dir.join(format!("{name}.expected.json")))
            .unwrap_or_else(|e| panic!("{name}: golden missing: {e}"));
        let produced = lowered_json(&name, input.trim());
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
    for name in discover_cases(&dir) {
        let Ok(input) = std::fs::read_to_string(dir.join(format!("{name}.input.json"))) else {
            continue;
        };
        let a = lowered_json(&name, input.trim());
        let b = lowered_json(&name, input.trim());
        assert_eq!(a, b, "{name}: non-deterministic");
    }
}
