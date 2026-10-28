//! `laws/decimal` — the exact decimal against the reference kit's answers
//! (Phase 2001), read THROUGH `laws/manifest.json`.
//!
//! Every vector of every case runs: `canonical`, `compare`, `add` and `toFloat`
//! against this host's decimal text (`transform::decimal`), `codecDecode` and
//! `codecEncode` through the dataframe codec (carried on a DataGrid whose source
//! is a `Transform` binding, as the other law legs carry a table), and
//! `aggregate` through the evaluator's aggregate. A `reject` vector requires a
//! refusal. Its class is compared where this host's refusal names one the file
//! uses — `notDecimal` and `pastFloatRange`, which the decimal text itself
//! distinguishes — and refusal alone is required where this host's codec and
//! evaluator refusal classes are not the reference's.

mod common;

use std::cmp::Ordering;

use fuaran_rs::canonical::{JVal, format_finite_double, parse, render_canonical};
use fuaran_rs::transform::{Table, aggregate, cell_token, decimal, validate_table};
use fuaran_rs::wire::{
    AggFn, Binding, Cell, ColumnType, DataColumn, DataSource, NodeKind, SchemaEntry,
    TransformSource, decode_node, encode_node,
};

fn str_field(v: &JVal, key: &str) -> Option<String> {
    match v.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn grid_json(source: &str) -> String {
    format!(
        r#"{{"id":"laws","kind":{{"$type":"DataGrid","columns":[],"editable":false,"rowKey":"<closure>","source":{{"$type":"Transform","pipeline":[],"source":{source}}}}}}}"#
    )
}

fn grid_source(table: Table) -> String {
    let node = decode_node(&grid_json(r#"{"columns":{},"schema":[]}"#))
        .expect("the empty carrier decodes");
    let mut carrier = node.clone();
    if let NodeKind::DataGrid(spec) = &mut carrier.kind {
        spec.source = Binding::Transform {
            params: None,
            pipeline: vec![],
            source: TransformSource::Data(DataSource::Embedded {
                schema: table.schema,
                columns: table.columns,
            }),
        };
    }
    let doc = parse(&encode_node(&carrier)).expect("the encoded grid parses");
    render_canonical(
        doc.field("kind")
            .and_then(|k| k.field("source"))
            .and_then(|s| s.field("source"))
            .expect("the encoded grid carries kind.source.source"),
    )
}

/// The `cells` of a `codecEncode` / `aggregate` vector as this host's cells.
/// A decimal's text is taken AS GIVEN — not canonicalised — so the guarded
/// encode sees exactly what the vector built.
fn cells_of(input: &JVal) -> Vec<Cell> {
    let Some(JVal::Arr(cells)) = input.field("cells") else {
        panic!("input.cells");
    };
    cells
        .iter()
        .map(|c| {
            let text = str_field(c, "text");
            match str_field(c, "kind").as_deref() {
                Some("null") => Cell::Null,
                Some("decimal") => Cell::Decimal(text.expect("a decimal cell's text")),
                Some("int") => Cell::Int(text.expect("an int cell's text").parse().expect("int")),
                other => panic!("unknown cell kind {other:?}"),
            }
        })
        .collect()
}

fn decimal_table(cells: Vec<Cell>) -> Table {
    Table {
        schema: vec![SchemaEntry::new("c".to_string(), ColumnType::Decimal)],
        columns: vec![DataColumn {
            name: "c".to_string(),
            column_type: ColumnType::Decimal,
            cells,
        }],
    }
}

/// The class a decimal-text refusal is: the text is not decimal, or (for the
/// nearest double) it is decimal and past the float range.
fn text_class(text: &str) -> &'static str {
    if decimal::try_canonical(text).is_none() {
        "notDecimal"
    } else {
        "pastFloatRange"
    }
}

/// One vector's verdict: `Ok(answer)` (compared to the expected member), or
/// `Err(class)` — `Some` where this host names the class, `None` where it only
/// refuses.
type Outcome = Result<(&'static str, String), Option<&'static str>>;

fn run(case: &str, input: &JVal) -> Outcome {
    let field = |k: &str| str_field(input, k).unwrap_or_else(|| panic!("input.{k}"));
    match case {
        "canonical" => {
            let text = field("text");
            decimal::try_canonical(&text)
                .map(|c| ("canonical", c))
                .ok_or(Some(text_class(&text)))
        }
        "compare" => {
            let (a, b) = (field("a"), field("b"));
            match decimal::compare(&a, &b) {
                Some(o) => Ok((
                    "order",
                    match o {
                        Ordering::Less => "-1",
                        Ordering::Equal => "0",
                        Ordering::Greater => "1",
                    }
                    .to_string(),
                )),
                None => Err(Some("notDecimal")),
            }
        }
        "add" => {
            let (a, b) = (field("a"), field("b"));
            decimal::add(&a, &b)
                .map(|s| ("sum", s))
                .ok_or(Some("notDecimal"))
        }
        "toFloat" => {
            let text = field("text");
            decimal::try_to_float(&text)
                .map(|f| ("float", format_finite_double(f)))
                .ok_or(Some(text_class(&text)))
        }
        "codecDecode" => {
            let document = field("document");
            let node = decode_node(&grid_json(&document)).map_err(|_| None)?;
            let NodeKind::DataGrid(spec) = &node.kind else {
                panic!("the carrier grid decoded to another kind");
            };
            let Binding::Transform {
                source: TransformSource::Data(DataSource::Embedded { schema, columns }),
                ..
            } = &spec.source
            else {
                panic!("the carrier grid's source is not an embedded table");
            };
            Ok((
                "canonical",
                grid_source(Table {
                    schema: schema.clone(),
                    columns: columns.clone(),
                }),
            ))
        }
        "codecEncode" => {
            let table = decimal_table(cells_of(input));
            validate_table(&table).map_err(|_| None)?;
            Ok(("canonical", grid_source(table)))
        }
        "aggregate" => {
            let func = AggFn::from_wire(&field("fn")).expect("a known aggregate");
            aggregate(func, ColumnType::Decimal, &cells_of(input))
                .map(|c| ("token", cell_token(&c)))
                .map_err(|_| None)
        }
        other => panic!("the decimal family carries a case this leg does not know: {other}"),
    }
}

#[test]
fn the_decimal_matches_the_reference_on_every_decimal_law_vector() {
    let laws = common::corpus_root().join("laws");
    let manifest = parse(&read(&laws.join("manifest.json"))).expect("laws/manifest.json parses");
    let Some(JVal::Arr(families)) = manifest.field("families") else {
        panic!("laws/manifest.json declares no families");
    };
    let row = families
        .iter()
        .find(|f| str_field(f, "id").as_deref() == Some("decimal"))
        .expect("laws/manifest.json declares the decimal family");
    let file = str_field(row, "file").expect("the decimal row names its file");
    let doc = parse(&read(&laws.join(&file))).expect("the decimal vectors parse");
    assert_eq!(str_field(&doc, "family").as_deref(), Some("decimal"));
    assert_eq!(
        str_field(&doc, "kitVersion"),
        str_field(row, "kitVersion"),
        "laws/{file} and its manifest row disagree on kitVersion"
    );
    let Some(JVal::Arr(vectors)) = doc.field("vectors") else {
        panic!("laws/{file} carries no vectors");
    };
    assert_eq!(
        Some(&JVal::Num(vectors.len() as f64)),
        row.field("vectors"),
        "laws/{file} carries a vector count its manifest row does not declare"
    );

    let mut failures: Vec<String> = vec![];
    for v in vectors {
        let id = str_field(v, "id").expect("vector id");
        let case = str_field(v, "case").expect("vector case");
        let input = v.field("input").expect("vector input");
        let expected = v.field("expected").expect("vector expected");
        let verdict = str_field(expected, "verdict").expect("expected.verdict");
        match (run(&case, input), verdict.as_str()) {
            (Ok((member, got)), "accept") => {
                let want = match expected.field(member) {
                    Some(JVal::Str(s)) => s.clone(),
                    Some(JVal::Num(n)) => format!("{n}"),
                    other => panic!("{id}: expected.{member} is {other:?}"),
                };
                if got != want {
                    failures.push(format!("{id}: got {got}, want {want}"));
                }
            }
            (Ok((_, got)), _) => failures.push(format!(
                "{id}: the reference refused; this host answered {got}"
            )),
            (Err(class), "reject") => {
                let want = str_field(expected, "error");
                if let (Some(class), Some(want)) = (class, want.as_deref())
                    && class != want
                {
                    failures.push(format!("{id}: refused as {class}, the reference as {want}"));
                }
            }
            (Err(class), _) => failures.push(format!(
                "{id}: the reference accepted; this host refused ({class:?})"
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} decimal vectors failed:\n{}",
        failures.len(),
        vectors.len(),
        failures.join("\n")
    );
    eprintln!(
        "decimal leg: {} vectors agree with the reference",
        vectors.len()
    );
}
