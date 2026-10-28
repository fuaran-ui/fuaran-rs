//! `laws/fieldLaws` — the schema entry's field metadata against the reference
//! kit's answers (Phase 2207, WIRE_FORMAT §3.6.26), read THROUGH
//! `laws/manifest.json`.
//!
//! Every vector of every case runs through this host's own code: `fieldEncode`
//! builds a [`SchemaEntry`] from its members and encodes it through the
//! dataframe codec (carried on a DataGrid whose source is a `Transform` binding,
//! as the decimal leg carries a table); `codecDecode` decodes a zero-row source
//! and re-encodes it; `fingerprint`, `delta` and `classify` decode their schema
//! arrays through the same codec and run `wire::schema`. A `reject` vector
//! requires a refusal, and its class is compared: this host surfaces a columnar
//! source defect as `WRONG_TYPE`, which §3.6.26 maps to the reference's
//! `MalformedShape`, so any other code is a different class.

mod common;

use std::collections::BTreeSet;

use fuaran_rs::canonical::{JVal, parse, render_canonical};
use fuaran_rs::wire::{
    Binding, ColumnType, DataColumn, DataSource, DecodeErrorCode, NodeKind, SchemaEntry,
    TransformSource, decode_node, encode_node, schema, unit,
};

/// The case set this leg certifies. A case the file gains and this leg does
/// not know fails the run.
const CASES: &[&str] = &[
    "fieldEncode",
    "codecDecode",
    "fingerprint",
    "delta",
    "classify",
];

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

/// The canonical bytes of an embedded zero-row source holding `schema`, as this
/// host's codec writes it.
fn source_of(schema: Vec<SchemaEntry>) -> JVal {
    let columns = schema
        .iter()
        .map(|e| DataColumn {
            name: e.name.clone(),
            column_type: e.column_type,
            cells: vec![],
        })
        .collect();
    let mut carrier = decode_node(&grid_json(r#"{"columns":{},"schema":[]}"#))
        .expect("the empty carrier decodes");
    if let NodeKind::DataGrid(spec) = &mut carrier.kind {
        spec.source = Binding::Transform {
            params: None,
            pipeline: vec![],
            source: TransformSource::Data(DataSource::Embedded { schema, columns }),
        };
    }
    let doc = parse(&encode_node(&carrier)).expect("the encoded grid parses");
    doc.field("kind")
        .and_then(|k| k.field("source"))
        .and_then(|s| s.field("source"))
        .expect("the encoded grid carries kind.source.source")
        .clone()
}

/// The canonical bytes of a schema array, as this host's codec writes it.
fn schema_bytes(schema: Vec<SchemaEntry>) -> String {
    render_canonical(
        source_of(schema)
            .field("schema")
            .expect("the source carries its schema"),
    )
}

/// Decode a source document through this host's codec: the schema, or the
/// refusal's class.
fn decode_source(document: &str) -> Result<Vec<SchemaEntry>, String> {
    let node = decode_node(&grid_json(document)).map_err(|e| match e.code {
        DecodeErrorCode::WrongType => "MalformedShape".to_string(),
        other => format!("{other:?}"),
    })?;
    let NodeKind::DataGrid(spec) = &node.kind else {
        panic!("the carrier grid decoded to another kind");
    };
    let Binding::Transform {
        source: TransformSource::Data(DataSource::Embedded { schema, .. }),
        ..
    } = &spec.source
    else {
        panic!("the carrier grid's source is not an embedded table");
    };
    Ok(schema.clone())
}

/// A schema array in wire form, decoded through this host's codec as a zero-row
/// source carrying it.
fn decode_schema(entries: &JVal) -> Vec<SchemaEntry> {
    let JVal::Arr(items) = entries else {
        panic!("a schema is an array, got {entries:?}");
    };
    let columns: Vec<String> = items
        .iter()
        .map(|e| {
            let name = str_field(e, "name").expect("a schema entry's name");
            format!(
                r#"{}:{{"validity":[],"values":[]}}"#,
                render_canonical(&JVal::Str(name))
            )
        })
        .collect();
    let document = format!(
        r#"{{"columns":{{{}}},"schema":{}}}"#,
        columns.join(","),
        render_canonical(entries)
    );
    decode_source(&document).unwrap_or_else(|class| panic!("the schema refused as {class}"))
}

/// The entry a `fieldEncode` vector's members build.
fn entry_of(input: &JVal) -> SchemaEntry {
    let field = |k: &str| str_field(input, k);
    let ty = field("type").expect("input.type");
    let mut e = SchemaEntry::new(
        field("name").expect("input.name"),
        ColumnType::from_wire(&ty).unwrap_or_else(|| panic!("unknown column type {ty}")),
    );
    e.unit = field("unit").map(|u| unit::parse(&u).unwrap_or_else(|r| panic!("unit {u}: {r:?}")));
    e.label = field("label");
    e.description = field("description");
    if let Some(JVal::Obj(members)) = input.field("ext") {
        for (k, v) in members {
            let JVal::Str(v) = v else {
                panic!("input.ext.{k} is not a string");
            };
            e.ext.insert(k.clone(), v.clone());
        }
    }
    e
}

/// One vector's verdict: `Ok` with each expected member's answer, or `Err` with
/// the refusal's class.
type Outcome = Result<Vec<(&'static str, String)>, String>;

fn run(case: &str, input: &JVal) -> Outcome {
    let schema_at = |k: &str| decode_schema(input.field(k).unwrap_or_else(|| panic!("input.{k}")));
    match case {
        "fieldEncode" => {
            let JVal::Arr(entries) = parse(&schema_bytes(vec![entry_of(input)])).unwrap() else {
                panic!("the encoded schema is not an array");
            };
            Ok(vec![("canonical", render_canonical(&entries[0]))])
        }
        "codecDecode" => {
            let document = str_field(input, "document").expect("input.document");
            let decoded = decode_source(&document)?;
            Ok(vec![("canonical", render_canonical(&source_of(decoded)))])
        }
        "fingerprint" => Ok(vec![(
            "fingerprint",
            schema::fingerprint(&schema_at("schema")),
        )]),
        "delta" => {
            let (old, target) = (schema_at("old"), schema_at("target"));
            let delta = schema::diff(&old, &target);
            let patched = schema::patch(&old, &delta)
                .unwrap_or_else(|e| panic!("the delta does not replay over old: {e:?}"));
            Ok(vec![
                ("canonical", schema::encode_delta(&delta)),
                ("patched", schema_bytes(patched)),
            ])
        }
        "classify" => {
            let (old, target) = (schema_at("old"), schema_at("target"));
            let Some(JVal::Arr(deps)) = input.field("dependsOn") else {
                panic!("input.dependsOn");
            };
            let deps: Vec<String> = deps
                .iter()
                .map(|d| match d {
                    JVal::Str(s) => s.clone(),
                    other => panic!("dependsOn holds {other:?}"),
                })
                .collect();
            let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
            let compat = schema::classify(&deps, &schema::diff(&old, &target));
            Ok(vec![("compat", compat.tag().to_string())])
        }
        other => panic!("the fieldLaws family carries a case this leg does not know: {other}"),
    }
}

#[test]
fn the_field_metadata_matches_the_reference_on_every_field_law_vector() {
    let laws = common::corpus_root().join("laws");
    let manifest = parse(&read(&laws.join("manifest.json"))).expect("laws/manifest.json parses");
    let Some(JVal::Arr(families)) = manifest.field("families") else {
        panic!("laws/manifest.json declares no families");
    };
    let row = families
        .iter()
        .find(|f| str_field(f, "id").as_deref() == Some("fieldLaws"))
        .expect("laws/manifest.json declares the fieldLaws family");
    let file = str_field(row, "file").expect("the fieldLaws row names its file");
    let doc = parse(&read(&laws.join(&file))).expect("the fieldLaws vectors parse");
    assert_eq!(str_field(&doc, "family").as_deref(), Some("fieldLaws"));
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

    let mut seen = BTreeSet::new();
    let mut failures: Vec<String> = vec![];
    for v in vectors {
        let id = str_field(v, "id").expect("vector id");
        let case = str_field(v, "case").expect("vector case");
        let input = v.field("input").expect("vector input");
        let expected = v.field("expected").expect("vector expected");
        let verdict = str_field(expected, "verdict").expect("expected.verdict");
        seen.insert(case.clone());
        match (run(&case, input), verdict.as_str()) {
            (Ok(answers), "accept") => {
                for (member, got) in answers {
                    let want = match expected.field(member) {
                        Some(JVal::Str(s)) => s.clone(),
                        Some(other @ (JVal::Arr(_) | JVal::Obj(_))) => render_canonical(other),
                        other => panic!("{id}: expected.{member} is {other:?}"),
                    };
                    if got != want {
                        failures.push(format!("{id}: {member} got {got}, want {want}"));
                    }
                }
            }
            (Ok(_), _) => failures.push(format!("{id}: the reference refused; this host answered")),
            (Err(class), "reject") => {
                let want = str_field(expected, "error").expect("expected.error");
                if class != want {
                    failures.push(format!("{id}: refused as {class}, the reference as {want}"));
                }
            }
            (Err(class), _) => failures.push(format!(
                "{id}: the reference accepted; this host refused ({class})"
            )),
        }
    }
    let pinned: BTreeSet<String> = CASES.iter().map(|c| c.to_string()).collect();
    assert_eq!(
        seen, pinned,
        "laws/{file} no longer carries exactly the case set this leg certifies"
    );
    assert!(
        failures.is_empty(),
        "{} of {} fieldLaws vectors failed:\n{}",
        failures.len(),
        vectors.len(),
        failures.join("\n")
    );
    eprintln!(
        "fieldLaws leg: {} vectors agree with the reference",
        vectors.len()
    );
}
