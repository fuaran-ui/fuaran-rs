//! Certifies the DAG record codec against `wire-format-fixtures/dag/`
//! (round-trip byte-equal) and the 3-way tree merge against
//! `merge-conformance/` (merged tree byte-equal + `sha256(tree)` outcome hash).

use std::path::{Path, PathBuf};

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::dag::{MergeResult, decode_record, encode_record, merge3_way};
use fuaran_rs::opstream::sha256_hex;
use fuaran_rs::wire::{decode_node, encode_node};

fn corpus() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        if dir
            .join("wire-format-fixtures")
            .join("manifest.json")
            .is_file()
        {
            return Some(dir.join("wire-format-fixtures"));
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

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
}

#[test]
fn dag_records_round_trip_byte_identical() {
    let Some(root) = corpus() else {
        eprintln!("corpus not found; skipping");
        return;
    };
    let manifest = parse(&read(&root, "dag/manifest.json")).expect("dag manifest parses");
    let Some(JVal::Arr(fixtures)) = manifest.field("fixtures") else {
        panic!("dag manifest has no fixtures");
    };
    let mut ran = 0;
    for fixture in fixtures {
        let input_file = str_field(fixture, "inputFile").expect("inputFile");
        let expected_file =
            str_field(fixture, "expectedFile").unwrap_or_else(|| input_file.clone());
        let input = read(&root, &format!("dag/{input_file}"));
        let expected = read(&root, &format!("dag/{expected_file}"));
        let record = decode_record(&input).expect("dag record decodes");
        assert_eq!(
            encode_record(&record),
            expected.trim_end(),
            "{input_file} re-encodes byte-identically"
        );
        ran += 1;
    }
    assert!(ran > 0);
    eprintln!("dag corpus: {ran} records round-trip byte-identical");
}

#[test]
fn three_way_merges_match_the_conformance_corpus() {
    let Some(root) = corpus() else {
        eprintln!("corpus not found; skipping");
        return;
    };
    let manifest =
        parse(&read(&root, "merge-conformance/manifest.json")).expect("merge manifest parses");
    let Some(JVal::Arr(fixtures)) = manifest.field("fixtures") else {
        panic!("merge manifest has no fixtures");
    };
    let mut ran = 0;
    for fixture in fixtures {
        // Only the auto-merge (merge-3way) fixtures carry an expected tree; the
        // validator-gated fixture is a separate (validator) leg, covered when the
        // merge feeds the validator — skipped here explicitly.
        if str_field(fixture, "kind").as_deref() != Some("merge-3way") {
            continue;
        }
        let id = str_field(fixture, "id").expect("id");
        let base = decode_node(&read(
            &root,
            &format!(
                "merge-conformance/{}",
                str_field(fixture, "baseFile").unwrap()
            ),
        ))
        .expect("base decodes");
        let a = decode_node(&read(
            &root,
            &format!("merge-conformance/{}", str_field(fixture, "aFile").unwrap()),
        ))
        .expect("a decodes");
        let b = decode_node(&read(
            &root,
            &format!("merge-conformance/{}", str_field(fixture, "bFile").unwrap()),
        ))
        .expect("b decodes");
        let expected = read(
            &root,
            &format!(
                "merge-conformance/{}",
                str_field(fixture, "expectedFile").unwrap()
            ),
        );
        let expected = expected.trim_end();
        let outcome_hash = str_field(fixture, "outcomeHash").expect("outcomeHash");

        match merge3_way(&base, &a, &b) {
            MergeResult::Merged(tree) => {
                let encoded = encode_node(&tree);
                assert_eq!(encoded, expected, "{id}: merged tree byte-identical");
                assert_eq!(
                    sha256_hex(&encoded),
                    outcome_hash,
                    "{id}: outcome hash matches the cross-host golden"
                );
            }
            MergeResult::Conflicts(c) => {
                panic!("{id}: expected a clean auto-merge, got conflicts {c:?}")
            }
        }
        ran += 1;
    }
    assert!(ran > 0, "no merge-3way fixtures ran");
    eprintln!("merge corpus: {ran} auto-merges byte-identical + outcome-hash verified");
}

#[test]
fn a_genuine_facet_conflict_is_reported_not_silently_picked() {
    // A and B set the SAME node's tone to DIFFERENT values → a style.tone
    // conflict (not an auto-blend, which only happens on distinct sub-fields).
    let base = decode_node(
        r#"{"id":"n","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"x"}}}"#,
    )
    .unwrap();
    let a = decode_node(
        r#"{"id":"n","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"x"}},"style":{"emphasis":"Normal","tone":"Brand","weight":"Standard"}}"#,
    )
    .unwrap();
    let b = decode_node(
        r#"{"id":"n","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"x"}},"style":{"emphasis":"Normal","tone":"Success","weight":"Standard"}}"#,
    )
    .unwrap();
    match merge3_way(&base, &a, &b) {
        MergeResult::Conflicts(c) => {
            assert_eq!(c.len(), 1);
            assert_eq!(c[0].node_id, "n");
            assert_eq!(c[0].facet, "style.tone");
        }
        MergeResult::Merged(_) => panic!("expected a style.tone conflict"),
    }
}

#[test]
fn distinct_style_subfields_auto_blend() {
    // A sets tone, B sets voice on the same node → the sub-fields merge
    // independently (no conflict) — the "you win the conflict" auto-blend.
    let base = decode_node(
        r#"{"id":"n","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"x"}}}"#,
    )
    .unwrap();
    let a = decode_node(
        r#"{"id":"n","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"x"}},"style":{"emphasis":"Normal","tone":"Brand","weight":"Standard"}}"#,
    )
    .unwrap();
    let b = decode_node(
        r#"{"id":"n","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"x"}},"style":{"emphasis":"Normal","tone":"Default","voice":"Display","weight":"Standard"}}"#,
    )
    .unwrap();
    match merge3_way(&base, &a, &b) {
        MergeResult::Merged(tree) => {
            let encoded = encode_node(&tree);
            assert!(encoded.contains("\"tone\":\"Brand\""));
            assert!(encoded.contains("\"voice\":\"Display\""));
        }
        MergeResult::Conflicts(c) => panic!("expected an auto-blend, got {c:?}"),
    }
}
