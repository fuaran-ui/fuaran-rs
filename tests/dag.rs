//! Certifies the DAG record codec against `wire-format-fixtures/dag/`
//! (round-trip byte-equal) and the 3-way tree merge against
//! `merge-conformance/` (merged tree byte-equal + `sha256(tree)` outcome hash).

use std::path::{Path, PathBuf};

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::dag::{MergeResult, decode_record, encode_record, merge3_way};
use fuaran_rs::opstream::{Actor, sha256_hex};
use fuaran_rs::wire::{DecodeErrorCode, decode_node, encode_node};

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

// ─── The typed actor on the DAG record (Phase 1144 / 1168) ───────────────────
//
// Six checks, each pinning something the four curated corpus fixtures do not
// reach on their own. The corpus is the oracle for the BYTES; these pin the
// refusal contract, which has no reject vector in the `dag/` family.

/// The corpus family must exercise BOTH actor cases. `dag-linear-step` was
/// deliberately moved from a human to an agent actor when Phase 1144 re-minted
/// the family, precisely so the four-member agent shape is cross-host verified;
/// a future regeneration that collapsed it back to all-human would leave the
/// agent branch of every host's codec uncertified while still passing the
/// round-trip leg above.
#[test]
fn the_dag_corpus_exercises_both_actor_cases() {
    let Some(root) = corpus() else {
        eprintln!("corpus not found; skipping");
        return;
    };
    let manifest = parse(&read(&root, "dag/manifest.json")).expect("dag manifest parses");
    let Some(JVal::Arr(fixtures)) = manifest.field("fixtures") else {
        panic!("dag manifest has no fixtures");
    };
    let mut humans = 0;
    let mut agents = 0;
    for fixture in fixtures {
        let input_file = str_field(fixture, "inputFile").expect("inputFile");
        let record =
            decode_record(&read(&root, &format!("dag/{input_file}"))).expect("dag record decodes");
        match record.actor {
            Actor::Human { .. } => humans += 1,
            Actor::Agent { .. } => agents += 1,
        }
    }
    assert!(humans > 0, "no human-actor fixture in the dag/ family");
    assert!(agents > 0, "no agent-actor fixture in the dag/ family");
}

/// Top-level keys are Ordinal-sorted, which is what puts `actor` at the FRONT
/// where the pre-1144 `userId` sat at the back. Pinned over a record carrying
/// every optional member, so a future key that sorted ahead of `actor` fails
/// here rather than shifting bytes silently.
#[test]
fn dag_record_top_level_keys_are_ordinal_sorted() {
    let json = concat!(
        r#"{"actor":{"kind":"agent","model":"claude","version":"4.8","id":"planner"},"#,
        r#""hash":"h3","op":{"$type":"RemoveNode","target":"x"},"outcomeHash":"o1","#,
        r#""parents":["h1","h2"],"promptId":"p-9","#,
        r#""resultEnvelope":{"$type":"Failure","code":"E","message":"m"},"#,
        r#""streamId":"s","timestamp":1700000000,"tombstoned":true}"#
    );
    let record = decode_record(json).expect("decodes");
    let encoded = encode_record(&record);
    assert_eq!(encoded, json, "round-trips byte-identically");

    let keys = top_level_keys(&encoded);
    assert_eq!(
        keys,
        vec![
            "actor",
            "hash",
            "op",
            "outcomeHash",
            "parents",
            "promptId",
            "resultEnvelope",
            "streamId",
            "timestamp",
            "tombstoned",
        ],
        "top-level key order"
    );
    let mut sorted = keys.clone();
    sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    assert_eq!(keys, sorted, "top-level keys are Ordinal-sorted");
}

/// The emitted object's depth-1 keys, in emission order. Deliberately a scan of
/// the BYTES rather than a re-parse: the property under test is what the encoder
/// wrote, and a parser that sorts or reorders would hide exactly the defect this
/// pins.
fn top_level_keys(encoded: &str) -> Vec<String> {
    let bytes = encoded.as_bytes();
    let mut keys = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut key_start: Option<usize> = None;
    let mut opened_at_depth_1 = false;
    for (i, &b) in bytes.iter().enumerate() {
        let c = b as char;
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
                if let (true, Some(start)) = (opened_at_depth_1, key_start) {
                    keys.push(encoded[start..i].to_string());
                }
                key_start = None;
                opened_at_depth_1 = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                key_start = Some(i + 1);
                opened_at_depth_1 =
                    depth == 1 && i > 0 && (bytes[i - 1] == b'{' || bytes[i - 1] == b',');
            }
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            _ => {}
        }
    }
    keys
}

/// The actor's OWN members keep their pinned order (`kind` first, then the case
/// fields) — deliberately NOT Ordinal-sorted, which would emit
/// `{"id":…,"kind":…,"model":…,"version":…}` and break every host's bytes.
#[test]
fn the_nested_actor_keeps_its_pinned_member_order() {
    let json = concat!(
        r#"{"actor":{"kind":"agent","model":"claude","version":"4.8","id":"planner"},"#,
        r#""hash":"h","op":{"$type":"RemoveNode","target":"x"},"parents":[],"#,
        r#""resultEnvelope":{"$type":"Success"},"streamId":"s","timestamp":1,"#,
        r#""tombstoned":false}"#
    );
    let encoded = encode_record(&decode_record(json).expect("decodes"));
    assert!(
        encoded.contains(
            r#""actor":{"kind":"agent","model":"claude","version":"4.8","id":"planner"}"#
        ),
        "agent actor emitted in pinned order, got {encoded}"
    );
    let human = json.replace(
        r#"{"kind":"agent","model":"claude","version":"4.8","id":"planner"}"#,
        r#"{"kind":"human","id":"u1"}"#,
    );
    let encoded = encode_record(&decode_record(&human).expect("decodes"));
    assert!(
        encoded.contains(r#""actor":{"kind":"human","id":"u1"}"#),
        "human actor emitted in pinned order, got {encoded}"
    );
}

/// A pre-1144 `userId` envelope is refused BY NAME, not lifted to a `Human`.
/// A lift would mint a record whose stored `hash` no host can reproduce, which
/// turns a clear refusal into a silent verification failure downstream.
#[test]
fn a_pre_1144_user_id_envelope_is_refused_by_name() {
    let json = concat!(
        r#"{"hash":"h","op":{"$type":"RemoveNode","target":"x"},"parents":[],"#,
        r#""resultEnvelope":{"$type":"Success"},"streamId":"s","timestamp":1,"#,
        r#""tombstoned":false,"userId":"u1"}"#
    );
    let err = decode_record(json).expect_err("a pre-1144 envelope is refused");
    assert_eq!(err.code, DecodeErrorCode::MissingField);
    assert_eq!(err.path, "$.actor");
    assert!(
        err.message.contains("userId") && err.message.contains("do not carry forward"),
        "the refusal names the cause and the consequence, got: {}",
        err.message
    );
}

/// Every malformed actor is NAMED, never defaulted — the actor is inside the
/// reference host's content address, so a guessed one silently invalidates the
/// record's own hash.
#[test]
fn a_malformed_actor_is_named_never_defaulted() {
    let with = |actor: &str| {
        format!(
            concat!(
                r#"{{"actor":{},"hash":"h","op":{{"$type":"RemoveNode","target":"x"}},"#,
                r#""parents":[],"resultEnvelope":{{"$type":"Success"}},"streamId":"s","#,
                r#""timestamp":1,"tombstoned":false}}"#
            ),
            actor
        )
    };
    let cases: Vec<(&str, DecodeErrorCode, &str)> = vec![
        // Not an object at all.
        (r#""u1""#, DecodeErrorCode::WrongType, "$.actor"),
        // Object, but no discriminator.
        (
            r#"{"id":"u1"}"#,
            DecodeErrorCode::MissingField,
            "$.actor.kind",
        ),
        // A kind outside the closed pair.
        (
            r#"{"kind":"robot","id":"u1"}"#,
            DecodeErrorCode::UnknownDuCase,
            "$.actor.kind",
        ),
        // Human missing its one field.
        (
            r#"{"kind":"human"}"#,
            DecodeErrorCode::MissingField,
            "$.actor.id",
        ),
        // Agent missing a case field.
        (
            r#"{"kind":"agent","model":"claude","id":"planner"}"#,
            DecodeErrorCode::MissingField,
            "$.actor.version",
        ),
        // A case field of the wrong type.
        (
            r#"{"kind":"human","id":7}"#,
            DecodeErrorCode::WrongType,
            "$.actor.id",
        ),
    ];
    for (actor, code, path) in cases {
        let err =
            decode_record(&with(actor)).expect_err(&format!("actor {actor} should be refused"));
        assert_eq!(err.code, code, "code for actor {actor}");
        assert_eq!(err.path, path, "path for actor {actor}");
    }
}

/// `actor` is required outright: an envelope carrying neither `actor` nor the
/// legacy `userId` is a plain missing-field refusal, distinct from the
/// pre-1144 one above so the two diagnoses stay tellable apart.
#[test]
fn a_record_with_no_actor_at_all_is_refused() {
    let json = concat!(
        r#"{"hash":"h","op":{"$type":"RemoveNode","target":"x"},"parents":[],"#,
        r#""resultEnvelope":{"$type":"Success"},"streamId":"s","timestamp":1,"#,
        r#""tombstoned":false}"#
    );
    let err = decode_record(json).expect_err("a record with no actor is refused");
    assert_eq!(err.code, DecodeErrorCode::MissingField);
    assert_eq!(err.path, "$.actor");
    assert!(
        !err.message.contains("userId"),
        "must not blame userId when there was none: {}",
        err.message
    );
}
