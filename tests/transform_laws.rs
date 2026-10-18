//! `laws/transformLaws` — this host's evaluator against the reference's drawn
//! answers (Phase 2001).
//!
//! The family is read THROUGH `laws/manifest.json`: the file must agree with its
//! manifest row (family id, kit version, seed, iterations, vector count), and no
//! count is written here. Every `evalPipeline` vector runs: both inputs decode
//! with this host's codec (carried, as the `dataframeParity` leg carries them, on
//! a DataGrid whose source is a `Transform` binding) and round-trip byte for
//! byte; an `ok` verdict requires the evaluated table to encode byte for byte to
//! `expected.table`, and an `error` verdict requires this host to refuse — the
//! error class is never compared, because the file carries none.
//!
//! What is NOT run is reported by name and counted, never silent:
//! - every vector whose id names a `-unit-` shape is carried by fuaran#2208: this
//!   host does not yet carry field metadata (fuaran#2207) or the unit algebra.
//!   The set is derived from the ids and cross-checked against the vectors'
//!   CONTENT (a source schema declaring a unit), so a new unit vector, or a
//!   renamed one, is noticed rather than run or skipped by accident;
//! - the `columnOp` and `delta` case kinds: this host carries no ColumnOp
//!   surface and no table-delta surface. The set of case kinds in the file must
//!   be exactly the three this leg knows, so a new kind fails loudly.

mod common;

use std::collections::BTreeSet;

use fuaran_rs::canonical::{JVal, parse, render_canonical};
use fuaran_rs::transform::{Table, eval_pipeline};
use fuaran_rs::wire::{Binding, DataSource, NodeKind, TransformSource, decode_node, encode_node};

const UNIT_REASON: &str = "carried by fuaran#2208 — this host does not yet carry field metadata (fuaran#2207) or the unit algebra";

fn str_field(v: &JVal, key: &str) -> Option<String> {
    match v.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn num_field(v: &JVal, key: &str) -> Option<f64> {
    match v.field(key) {
        Some(JVal::Num(n)) => Some(*n),
        _ => None,
    }
}

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn grid_json(source: &str, pipeline: &str) -> String {
    format!(
        r#"{{"id":"laws","kind":{{"$type":"DataGrid","columns":[],"editable":false,"rowKey":"<closure>","source":{{"$type":"Transform","pipeline":{pipeline},"source":{source}}}}}}}"#
    )
}

/// The canonical bytes at `kind.source.<member>` of an encoded carrier grid.
fn grid_member(encoded: &str, member: &str) -> String {
    let doc = parse(encoded).expect("the encoded grid parses");
    let value = doc
        .field("kind")
        .and_then(|k| k.field("source"))
        .and_then(|s| s.field(member))
        .unwrap_or_else(|| panic!("the encoded grid carries no kind.source.{member}"));
    render_canonical(value)
}

/// Does a canonical source declare a unit on any schema entry?
fn declares_a_unit(source: &str) -> bool {
    let doc = parse(source).expect("input.source parses");
    match doc.field("schema") {
        Some(JVal::Arr(entries)) => entries.iter().any(|e| e.field("unit").is_some()),
        _ => false,
    }
}

/// The table the carrier grid's embedded source holds, re-encoded canonically.
fn encode_table(carrier: &fuaran_rs::wire::Node, out: Table) -> String {
    let mut carrier = carrier.clone();
    if let NodeKind::DataGrid(spec) = &mut carrier.kind {
        spec.source = Binding::Transform {
            params: None,
            pipeline: vec![],
            source: TransformSource::Data(DataSource::Embedded {
                schema: out.schema,
                columns: out.columns,
            }),
        };
    }
    grid_member(&encode_node(&carrier), "source")
}

#[test]
fn the_evaluator_matches_the_reference_on_every_transform_law_vector() {
    let laws = common::corpus_root().join("laws");
    let manifest = parse(&read(&laws.join("manifest.json"))).expect("laws/manifest.json parses");
    let Some(JVal::Arr(families)) = manifest.field("families") else {
        panic!("laws/manifest.json declares no families");
    };
    let row = families
        .iter()
        .find(|f| str_field(f, "id").as_deref() == Some("transformLaws"))
        .expect("laws/manifest.json declares the transformLaws family");
    let file = str_field(row, "file").expect("the transformLaws row names its file");
    let doc = parse(&read(&laws.join(&file))).expect("the transform-law vectors parse");

    // The file agrees with its manifest row.
    assert_eq!(
        str_field(&doc, "family").as_deref(),
        Some("transformLaws"),
        "laws/{file} names another family"
    );
    assert_eq!(
        str_field(&doc, "kitVersion"),
        str_field(row, "kitVersion"),
        "laws/{file} and its manifest row disagree on kitVersion"
    );
    for key in ["seed", "iterations"] {
        assert_eq!(
            num_field(&doc, key),
            num_field(row, key),
            "laws/{file} and its manifest row disagree on {key}"
        );
    }
    let Some(JVal::Arr(vectors)) = doc.field("vectors") else {
        panic!("laws/{file} carries no vectors");
    };
    assert_eq!(
        Some(vectors.len() as f64),
        num_field(row, "vectors"),
        "laws/{file} carries a vector count its manifest row does not declare"
    );

    // The case kinds are exactly the three this leg knows.
    let kinds: BTreeSet<String> = vectors
        .iter()
        .map(|v| str_field(v, "case").expect("vector case"))
        .collect();
    let known: BTreeSet<String> = ["evalPipeline", "columnOp", "delta"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        kinds, known,
        "laws/{file} carries a case kind this leg does not know"
    );
    let eval_count = vectors
        .iter()
        .filter(|v| str_field(v, "case").as_deref() == Some("evalPipeline"))
        .count();
    assert_eq!(
        Some(eval_count as f64),
        num_field(&doc, "iterations"),
        "`iterations` counts the evalPipeline vectors"
    );

    let mut failures: Vec<String> = vec![];
    let (mut ran, mut by_id, mut by_content) = (0usize, BTreeSet::new(), BTreeSet::new());
    let mut not_run_kinds: Vec<(String, String)> = vec![];
    for v in vectors {
        let id = str_field(v, "id").expect("vector id");
        let case = str_field(v, "case").expect("vector case");
        let input = v.field("input").expect("vector input");
        match case.as_str() {
            "columnOp" => {
                not_run_kinds.push((id, "this host carries no ColumnOp surface".to_string()));
                continue;
            }
            "delta" => {
                not_run_kinds.push((id, "this host carries no table-delta surface".to_string()));
                continue;
            }
            _ => {}
        }
        let expected = v.field("expected").expect("vector expected");
        let source = str_field(input, "source").expect("input.source");
        let pipeline = str_field(input, "pipeline").expect("input.pipeline");
        if declares_a_unit(&source) {
            by_content.insert(id.clone());
        }
        if id.contains("-unit-") {
            by_id.insert(id.clone());
            continue;
        }
        ran += 1;
        let verdict = str_field(expected, "verdict").expect("expected.verdict");
        let node = match decode_node(&grid_json(&source, &pipeline)) {
            Ok(n) => n,
            Err(e) => {
                if verdict != "error" {
                    failures.push(format!("{id}: the carrier grid did not decode: {e:?}"));
                }
                continue;
            }
        };
        let encoded = encode_node(&node);
        for (member, want) in [("source", &source), ("pipeline", &pipeline)] {
            let got = grid_member(&encoded, member);
            if &got != want {
                failures.push(format!(
                    "{id}: {member} round-trip\n  got  {got}\n  want {want}"
                ));
            }
        }
        let NodeKind::DataGrid(spec) = &node.kind else {
            panic!("{id}: the carrier grid decoded to another kind");
        };
        let Binding::Transform {
            pipeline: steps,
            source: TransformSource::Data(DataSource::Embedded { schema, columns }),
            ..
        } = &spec.source
        else {
            panic!("{id}: the carrier grid's source is not an embedded Transform");
        };
        let table = Table {
            schema: schema.clone(),
            columns: columns.clone(),
        };
        match (eval_pipeline(steps, &table), verdict.as_str()) {
            (Err(e), "ok") => failures.push(format!(
                "{id}: the reference evaluated; this host refused: {e:?}"
            )),
            (Ok(_), "error") => {
                failures.push(format!("{id}: the reference refused; this host evaluated"))
            }
            (Err(_), _) => {}
            (Ok(out), _) => {
                let got = encode_table(&node, out);
                let want = str_field(expected, "table").expect("expected.table");
                if got != want {
                    failures.push(format!(
                        "{id}: evaluator ≠ reference\n  got  {got}\n  want {want}"
                    ));
                }
            }
        }
    }

    // The unit skip set, derived two ways, must be one set.
    assert_eq!(
        by_id, by_content,
        "the `-unit-` ids and the vectors whose source declares a unit disagree — a unit vector \
         was added or renamed; decide whether this host runs it"
    );
    for id in &by_id {
        eprintln!("transformLaws: NOT RUN {id} — {UNIT_REASON}");
    }
    for (id, why) in &not_run_kinds {
        eprintln!("transformLaws: NOT RUN {id} — {why}");
    }
    assert!(
        failures.is_empty(),
        "{} of {ran} transformLaws evalPipeline vectors failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!(
        "transformLaws leg: {ran} evalPipeline vectors agree with the reference; {} unit vectors \
         carried by fuaran#2208; {} columnOp/delta vectors not run (no surface)",
        by_id.len(),
        not_run_kinds.len()
    );
}
