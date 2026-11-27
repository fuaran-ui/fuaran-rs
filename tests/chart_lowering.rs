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
//! The corpus is located by `common::corpus_root`; an absent corpus fails the
//! suite rather than skipping it (Phase 2301).

mod common;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::BindingSources;
use fuaran_rs::render::chart_lowering::{
    ChartAxisUnitMode, ChartLowerStyle, ChartTable, ChartTitles, TABLE_ROW_LIMIT, lower_chart_with,
    project_row, table_model,
};
use fuaran_rs::render::egress::deny_non_local_egress;
use fuaran_rs::render::server::{ChartTablePolicy, render_to_html_with_chart_table};
use fuaran_rs::wire::{
    Binding, ChartAnnotation, ChartAnnotationRange, ChartAnnotationX, ChartDataLabels, ChartKind,
    ChartLegendPosition, ChartXScale, Format, I18nArg, Node, NodeKind, SemanticStyle,
    StateBehaviour, StaticValue, TextSource,
};

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
            decimals,
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
            // Phase 1661 — an argument is an `I18nArg`. These inputs carry only
            // literal arguments, which is the bare arm.
            args: match j.field("args") {
                None => Vec::new(),
                Some(JVal::Obj(entries)) => entries
                    .iter()
                    .map(|(k, v)| (k.clone(), I18nArg::Literal(v.clone())))
                    .collect(),
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

/// An annotation's x address, from the corpus's canonical wire JSON. An arm the
/// family has not exercised panics rather than being approximated, on
/// `text_source_of`'s reasoning: a silent fallback is what hides a drop.
fn annotation_x_of(j: &JVal) -> ChartAnnotationX {
    let tag = match j.field("$type") {
        Some(JVal::Str(t)) => t.as_str(),
        _ => panic!("chart-lowering input: ChartAnnotationX missing $type"),
    };
    match tag {
        "Category" => match j.field("key") {
            Some(JVal::Str(k)) => ChartAnnotationX::Category(k.clone()),
            _ => panic!("chart-lowering input: Category address missing key"),
        },
        "Date" => match j.field("iso") {
            Some(JVal::Str(s)) => ChartAnnotationX::Date(s.clone()),
            _ => panic!("chart-lowering input: Date address missing iso"),
        },
        other => panic!("chart-lowering input: unsupported ChartAnnotationX arm {other}"),
    }
}

/// A range band's pair — the case carries the AXIS as well as the pair.
fn annotation_range_of(j: &JVal) -> ChartAnnotationRange {
    let tag = match j.field("$type") {
        Some(JVal::Str(t)) => t.as_str(),
        _ => panic!("chart-lowering input: ChartAnnotationRange missing $type"),
    };
    let num = |key: &str| -> f64 {
        match j.field(key) {
            Some(JVal::Num(v)) => *v,
            _ => panic!("chart-lowering input: ValueRange missing numeric {key}"),
        }
    };
    let addr = |key: &str| -> ChartAnnotationX {
        match j.field(key) {
            Some(v) => annotation_x_of(v),
            None => panic!("chart-lowering input: XRange missing {key}"),
        }
    };
    match tag {
        "ValueRange" => ChartAnnotationRange::ValueRange {
            from: num("from"),
            to: num("to"),
        },
        "XRange" => ChartAnnotationRange::XRange {
            from: addr("from"),
            to: addr("to"),
        },
        other => panic!("chart-lowering input: unsupported ChartAnnotationRange arm {other}"),
    }
}

/// A chart's data-addressed annotation. Its LABEL rides `text_source_of`, so every
/// arm crosses UNRESOLVED — the Phase 1143 contract at a new slot.
fn annotation_of(j: &JVal) -> ChartAnnotation {
    let tag = match j.field("$type") {
        Some(JVal::Str(t)) => t.as_str(),
        _ => panic!("chart-lowering input: ChartAnnotation missing $type"),
    };
    let label = j.field("label").map(text_source_of);
    match tag {
        "ReferenceLine" => match j.field("value") {
            Some(JVal::Num(v)) => ChartAnnotation::ReferenceLine { value: *v, label },
            _ => panic!("chart-lowering input: ReferenceLine missing numeric value"),
        },
        "EventMarker" => match j.field("at") {
            Some(at) => ChartAnnotation::EventMarker {
                at: annotation_x_of(at),
                label,
            },
            None => panic!("chart-lowering input: EventMarker missing at"),
        },
        "RangeBand" => match j.field("range") {
            Some(range) => ChartAnnotation::RangeBand {
                range: annotation_range_of(range),
                label,
            },
            None => panic!("chart-lowering input: RangeBand missing range"),
        },
        other => panic!("chart-lowering input: unsupported ChartAnnotation arm {other}"),
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
    // Phase 1490/1491/1492 — `annotations` is a WIRE field carried in canonical
    // `$type` JSON, omitted when the case declares none (so every pre-1490 case
    // AND every pre-1490 golden is unchanged). Every LABEL crosses UNRESOLVED,
    // whichever arm it carries — the Phase 1143 text contract at a new slot, which
    // is why `text_source_of` is reused rather than a bare-string reader.
    let annotations: Vec<ChartAnnotation> = match spec.field("annotations") {
        Some(JVal::Arr(items)) => items.iter().map(annotation_of).collect(),
        None => Vec::new(),
        Some(_) => panic!("chart-lowering input: annotations is not an array"),
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
            annotations: if annotations.is_empty() {
                None
            } else {
                Some(&annotations)
            },
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
        visible: None,
        fallback: None,
    };
    fuaran_rs::wire::encode_node(&node)
}

#[test]
fn every_case_lowers_byte_identically_to_its_committed_golden() {
    let corpus = common::corpus_root();
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
    let corpus = common::corpus_root();
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

// ── Phase 2804 — the data-table alternative ──────────────────────────────────
//
// The table model against the corpus's `chart-lowering/table-model/` goldens (the
// reference host's Phase 2776 subfamily: each case a byte copy of an existing
// input beside `<name>.table.json`, the model in canonical JSON or `null` where
// the cost caps refuse the chart), and the server renderer's disclosure against
// the reference SERVER renderer's bytes for the documents in
// `tests/fixtures/chart_table/` (`<name>.reference.txt`, produced by
// `render-oracle.fsx` there; its header carries the command).

/// The cases this host does not certify, and why: each declares a field this
/// host's lowering does not yet read (series read off the rows, the heatmap grid,
/// withheld rows, the interval / size / path / sort channels — the subfamilies
/// this suite does not run either), so its drawing here is drawn without it and
/// the chart offers no table rather than one that disagrees with its drawing.
const TABLE_MODEL_NOT_CERTIFIED: &[&str] = &[
    "derived-mixed-columns",
    "derived-over-series-cap",
    "heatmap-missing-cells",
    "heatmap-refusal-cell-cap",
    "bar-withheld",
    "pie-withheld",
    "scatter-withheld",
    "bar-sort-desc",
    "bar-interval",
    "scatter-all-channels",
];

/// A neutral chart-lowering input as the wire `Chart` it describes, and its rows.
/// The `table-model/` cases: every input, each with its `<name>.table.json`.
fn table_model_cases(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("table-model dir readable")
        .filter_map(|e| {
            e.ok()?
                .file_name()
                .into_string()
                .ok()?
                .strip_suffix(".input.json")
                .map(String::from)
        })
        .collect();
    names.sort();
    for name in &names {
        assert!(
            dir.join(format!("{name}.table.json")).is_file(),
            "{name}: no table golden"
        );
    }
    names
}

fn chart_of_input(input: &str) -> (fuaran_rs::wire::ChartSpec, Vec<JVal>) {
    let JVal::Obj(members) = parse(input).expect("input parses") else {
        panic!("an input is an object");
    };
    let mut rows = Vec::new();
    let mut kind = vec![("$type".to_string(), JVal::Str("Chart".into()))];
    for (k, v) in members {
        match k.as_str() {
            "data" => {
                if let JVal::Arr(items) = &v {
                    rows.clone_from(items);
                }
                kind.push((
                    "source".into(),
                    JVal::Obj(vec![
                        ("$type".into(), JVal::Str("Static".into())),
                        ("value".into(), v),
                    ]),
                ));
            }
            // A style selector, not a wire field.
            "axisUnitMode" => {}
            _ => kind.push((k, v)),
        }
    }
    let doc = JVal::Obj(vec![
        ("id".into(), JVal::Str("c".into())),
        ("kind".into(), JVal::Obj(kind)),
    ]);
    let node = fuaran_rs::wire::decode_node(&fuaran_rs::canonical::render_canonical(&doc))
        .expect("the input decodes as a Chart");
    let NodeKind::Chart(spec) = node.kind else {
        panic!("the input is a Chart");
    };
    (spec, rows)
}

fn table_json(table: Option<&ChartTable>) -> String {
    let Some(t) = table else {
        return "null".into();
    };
    let TextSource::Literal(caption) = &t.caption else {
        panic!("a table-model caption is a Literal in every corpus case");
    };
    let strs = |v: &[String]| JVal::Arr(v.iter().map(|s| JVal::Str(s.clone())).collect());
    fuaran_rs::canonical::render_canonical(&JVal::Obj(vec![
        ("caption".into(), JVal::Str(caption.clone())),
        ("columns".into(), strs(&t.columns)),
        ("rowCount".into(), JVal::Num(t.row_count as f64)),
        (
            "rows".into(),
            JVal::Arr(t.rows.iter().map(|r| strs(r)).collect()),
        ),
    ]))
}

#[test]
fn every_certified_table_model_case_yields_its_golden_byte_for_byte() {
    let dir = common::corpus_root()
        .join("chart-lowering")
        .join("table-model");
    let names = table_model_cases(&dir);
    // A subfamily that lost its cases would make the sweep vacuous, and an
    // exclusion naming a case that is gone would outlive its reason.
    assert!(
        names.len() >= 25,
        "table-model/ holds {} cases",
        names.len()
    );
    for excluded in TABLE_MODEL_NOT_CERTIFIED {
        assert!(
            names.iter().any(|n| n == excluded),
            "{excluded} is no longer a case"
        );
    }
    let mut certified = 0;
    for name in names
        .iter()
        .filter(|n| !TABLE_MODEL_NOT_CERTIFIED.contains(&n.as_str()))
    {
        let input = std::fs::read_to_string(dir.join(format!("{name}.input.json"))).expect("input");
        let golden =
            std::fs::read_to_string(dir.join(format!("{name}.table.json"))).expect("golden");
        let (spec, rows) = chart_of_input(input.trim());
        assert_eq!(
            table_json(table_model(&spec, &rows).as_ref()),
            golden.trim(),
            "{name}"
        );
        certified += 1;
    }
    assert!(certified >= 14, "only {certified} cases certified");
}

#[test]
fn an_uncertified_case_offers_no_table_rather_than_a_disagreeing_one() {
    let dir = common::corpus_root()
        .join("chart-lowering")
        .join("table-model");
    for name in TABLE_MODEL_NOT_CERTIFIED {
        let input = std::fs::read_to_string(dir.join(format!("{name}.input.json"))).expect("input");
        let (spec, rows) = chart_of_input(input.trim());
        assert_eq!(table_model(&spec, &rows), None, "{name}");
    }
}

#[test]
fn the_caption_is_the_title_unresolved_or_minted_from_the_kind_and_the_caps_apply() {
    let doc = |kind: &str, extra: &str, rows: &str| {
        chart_of_input(&format!(
            r#"{{"kind":"{kind}","xField":"x","yFields":["a","b"],"stacked":true{extra},"data":{rows}}}"#
        ))
    };
    let (spec, rows) = doc("Area", "", r#"[{"x":"p","a":1,"b":2}]"#);
    let table = table_model(&spec, &rows).expect("a table");
    assert_eq!(
        table.caption,
        TextSource::Literal("Stacked area chart data".into())
    );
    let (spec, rows) = doc(
        "Bar",
        r#","title":{"$type":"Bound","binding":{"$type":"State","key":"t"}}"#,
        "[]",
    );
    assert_eq!(table_model(&spec, &rows), None, "no rows, no table");
    let (spec, _) = doc(
        "Bar",
        r#","title":{"$type":"Bound","binding":{"$type":"State","key":"t"}}"#,
        "[]",
    );
    let one = vec![parse(r#"{"x":"p","a":1,"b":2}"#).unwrap()];
    assert!(matches!(
        table_model(&spec, &one).unwrap().caption,
        TextSource::Bound(_)
    ));
    let many: Vec<JVal> = (0..=TABLE_ROW_LIMIT + 2)
        .map(|i| parse(&format!(r#"{{"x":"r{i}","a":{i},"b":0}}"#)).unwrap())
        .collect();
    let long = table_model(&spec, &many).unwrap();
    assert_eq!(
        (long.rows.len(), long.row_count),
        (TABLE_ROW_LIMIT, TABLE_ROW_LIMIT + 3)
    );
}

fn chart_table_fixture(name: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/chart_table/{name}")).expect("fixture")
}

fn chart_table_documents() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir("tests/fixtures/chart_table")
        .expect("fixtures")
        .filter_map(|e| {
            e.ok()?
                .file_name()
                .into_string()
                .ok()?
                .strip_suffix(".json")
                .map(String::from)
        })
        .collect();
    names.sort();
    names
}

fn render_fixture(name: &str, chart_table: ChartTablePolicy) -> String {
    let node = fuaran_rs::wire::decode_node(&chart_table_fixture(&format!("{name}.json")))
        .expect("decodes");
    render_to_html_with_chart_table(
        &deny_non_local_egress(),
        chart_table,
        &node,
        &BindingSources::default(),
    )
}

/// The disclosure and the host element's opening: from `<details` to its close.
fn disclosure(html: &str) -> Option<&str> {
    let start = html.find("<details class=\"fuaran-chart-table\"")?;
    let stop = html[start..].find("</details>")? + start + "</details>".len();
    Some(&html[start..stop])
}

/// The documents whose charts this host offers a table for: those declaring no
/// field its lowering does not read.
const CHART_TABLE_OFFERED: &[&str] = &[
    "bar-single",
    "bar-stacked-currency",
    "line-limit",
    "line-temporal",
    "pie-percent",
];

#[test]
fn the_disclosure_is_the_reference_server_renderers_byte_for_byte() {
    let names = chart_table_documents();
    assert!(names.len() >= 8, "the fixture folder lost its documents");
    for name in &names {
        let html = render_fixture(name, ChartTablePolicy::Offer);
        if CHART_TABLE_OFFERED.contains(&name.as_str()) {
            let reference = chart_table_fixture(&format!("{name}.reference.txt"));
            assert_eq!(disclosure(&html), disclosure(&reference), "{name}");
            assert!(disclosure(&html).is_some(), "{name}: no disclosure");
            let id = format!("chart-{name}-data-table");
            assert!(html.contains(&format!(
                "<div class=\"fuaran-chart-table-host\"><div><svg class=\"fuaran-drawing\" role=\"img\" aria-details=\"{id}\""
            )));
            assert_eq!(html.matches(&format!("id=\"{id}\"")).count(), 1, "{name}");
            assert!(!html.contains("<script"), "{name}");
        } else {
            assert_eq!(
                disclosure(&html),
                None,
                "{name}: a chart this host draws without a field offers no table"
            );
        }
    }
}

#[test]
fn with_the_table_off_every_chart_renders_its_pre_phase_bytes() {
    let mut checked = 0;
    for name in chart_table_documents() {
        let Ok(before) =
            std::fs::read_to_string(format!("tests/fixtures/chart_table/{name}.pre-phase.txt"))
        else {
            continue;
        };
        assert_eq!(
            render_fixture(&name, ChartTablePolicy::Off),
            before,
            "{name}"
        );
        checked += 1;
    }
    assert!(checked >= 7, "only {checked} pre-phase captures");
}

/// The accessibility contract's `chartTable` rules, each checked structurally
/// over every offered disclosure; a rule the contract names that this suite has
/// no check for fails, so the contract and this host cannot drift apart.
#[test]
fn the_a11y_contracts_chart_table_rules_hold() {
    let contract =
        parse(&std::fs::read_to_string(common::corpus_root().join("a11y-contract.json")).unwrap())
            .unwrap();
    let JVal::Arr(rules) = contract
        .field("chartTable")
        .and_then(|c| c.field("rules"))
        .expect("rules")
        .clone()
    else {
        panic!("rules is an array");
    };
    let ids: Vec<String> = rules
        .iter()
        .filter_map(|r| match r.field("id") {
            Some(JVal::Str(s)) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(ids.len() >= 8);
    let between = |s: &str, open: &str, close: &str| -> Vec<String> {
        s.split(open)
            .skip(1)
            .filter_map(|p| p.split(close).next().map(String::from))
            .collect()
    };
    for name in CHART_TABLE_OFFERED {
        let html = render_fixture(name, ChartTablePolicy::Offer);
        let d = disclosure(&html).expect("a disclosure");
        let id = between(d, "id=\"", "\"")
            .into_iter()
            .next()
            .unwrap_or_default();
        let thead = between(d, "<thead>", "</thead>").concat();
        let header = between(&thead, "<th scope=\"col\">", "</th>");
        let body_rows = between(&between(d, "<tbody>", "</tbody>").concat(), "<tr>", "</tr>");
        for rule in &ids {
            let ok = match rule.as_str() {
                "disclosure-summary" => d.contains(
                    "\"><summary class=\"fuaran-chart-table-toggle\">View as table</summary>",
                ),
                "referenced-from-chart" => {
                    !id.is_empty()
                        && html.matches(&format!("id=\"{id}\"")).count() == 1
                        && html.contains(&format!("role=\"img\" aria-details=\"{id}\""))
                }
                "one-table" => d.matches("<table").count() == 1,
                "caption" => {
                    d.contains("<table class=\"fuaran-chart-table-grid\"><caption>")
                        && !d.contains("<caption></caption>")
                }
                "column-headers" => {
                    !header.is_empty()
                        && header.iter().all(|h| !h.is_empty())
                        && thead.matches("<th").count() == header.len()
                        && !thead.contains("<td")
                }
                "row-headers" => body_rows
                    .iter()
                    .all(|r| r.starts_with("<th scope=\"row\">") && r.matches("<th").count() == 1),
                "rectangular" => body_rows
                    .iter()
                    .all(|r| r.matches("<th").count() + r.matches("<td").count() == header.len()),
                "no-empty-body" => !body_rows.is_empty(),
                other => panic!("the contract names {other}, which this suite has no check for"),
            };
            assert!(ok, "{name}: {rule}");
        }
    }
}
