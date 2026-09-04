//! Certifies the DAG record codec against `wire-format-fixtures/dag/`
//! (round-trip byte-equal) and the 3-way tree merge against
//! `merge-conformance/` — EVERY family the manifest enumerates: the
//! `merge-3way` auto-merges (merged tree byte-equal + `sha256(tree)` outcome
//! hash), the `merge-validator-gated` verdicts (introduced-defect set
//! byte-equal + `sha256(verdict)`), and the `merge-refusal` two-sided
//! envelopes (refusal envelope byte-equal + `sha256(envelope)`, and the
//! swapped merge transposes it).

use std::path::{Path, PathBuf};

use fuaran_rs::canonical::{JVal, ordinal_cmp, parse};
use fuaran_rs::dag::{
    MergeConflict, MergeResult, decode_record, encode_envelope, encode_record, merge3_way,
    sort_canonical,
};
use fuaran_rs::opstream::{Actor, sha256_hex};
use fuaran_rs::wire::{DecodeErrorCode, Node, NodeKind, ToneVariant, decode_node, encode_node};

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

// ─── the merge-conformance corpus: EVERY family the manifest enumerates ──────
//
// The manifest holds two arrays — `fixtures` (auto-merge + validator-gated) and
// `refusalFixtures` (the Phase 1497 refusal envelopes, deliberately under their
// own key so a host that iterates `fixtures` expecting every entry to auto-merge
// stays correct). Both are walked here, each entry dispatched on its `kind`, and
// an UNRECOGNISED kind fails: a family this host cannot certify must announce
// itself, never pass by falling through a filter.

/// A defect found by a domain validator over a candidate tree — the port of the
/// reference host's `MergeDefect`, diffed on `(code, node_id, facet)`.
#[derive(Clone, PartialEq, Eq)]
struct MergeDefect {
    code: String,
    node_id: String,
    facet: String,
    message: String,
}

/// The sample DOMAIN validator the gated family certifies against: "at most one
/// `Brand`-toned pane per dashboard". Each offending child is a defect on its
/// `style.tone` cell. The corpus documents this exact invariant for a host to
/// port; it is deliberately tiny and lives here, in the test, because it is a
/// conformance fixture rather than library surface.
fn gated_validator(tree: &Node) -> Vec<MergeDefect> {
    match &tree.kind {
        NodeKind::Box(spec) => {
            let brand: Vec<&Node> = spec
                .children
                .iter()
                .filter(|c| c.style.tone == ToneVariant::Brand)
                .collect();
            if brand.len() > 1 {
                brand
                    .iter()
                    .map(|c| MergeDefect {
                        code: "TESTBRAND001".to_string(),
                        node_id: c.id.clone(),
                        facet: "style.tone".to_string(),
                        message: format!(
                            "Pane '{}' shares Brand tone with a sibling — at most one Brand pane per dashboard.",
                            c.id
                        ),
                    })
                    .collect()
            } else {
                vec![]
            }
        }
        _ => vec![],
    }
}

/// Defects present in `merged` but in NEITHER parent — the ones the merge
/// INTRODUCED. A defect already present in a parent was not caused by the merge.
fn introduced_defects(a: &Node, b: &Node, merged: &Node) -> Vec<MergeDefect> {
    let identity = |d: &MergeDefect| (d.code.clone(), d.node_id.clone(), d.facet.clone());
    let mut parent_keys: Vec<(String, String, String)> = gated_validator(a)
        .iter()
        .chain(gated_validator(b).iter())
        .map(identity)
        .collect();
    parent_keys.sort();
    let mut introduced: Vec<MergeDefect> = gated_validator(merged)
        .into_iter()
        .filter(|d| !parent_keys.contains(&identity(d)))
        .collect();
    introduced.sort_by(|x, y| {
        ordinal_cmp(&x.node_id, &y.node_id)
            .then_with(|| ordinal_cmp(&x.facet, &y.facet))
            .then_with(|| ordinal_cmp(&x.code, &y.code))
    });
    introduced
}

/// Mirror of the reference host's `encodeVerdict`: the defect set as a sorted
/// array of `{code,facet,message,nodeId}` objects, byte-stable across hosts so
/// `sha256_hex` over it is the cross-host verdict hash.
fn encode_verdict(defects: &[MergeDefect]) -> String {
    fn esc(out: &mut String, s: &str) {
        out.push('"');
        for ch in s.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    let mut out = String::from("[");
    for (i, d) in defects.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"code\":");
        esc(&mut out, &d.code);
        out.push_str(",\"facet\":");
        esc(&mut out, &d.facet);
        out.push_str(",\"message\":");
        esc(&mut out, &d.message);
        out.push_str(",\"nodeId\":");
        esc(&mut out, &d.node_id);
        out.push('}');
    }
    out.push(']');
    out
}

/// The `(base, a, b)` triad of a merge-conformance fixture.
fn triad(root: &Path, fixture: &JVal) -> (Node, Node, Node) {
    let load = |key: &str| {
        let rel = str_field(fixture, key).unwrap_or_else(|| panic!("fixture has no {key}"));
        decode_node(&read(root, &format!("merge-conformance/{rel}")))
            .unwrap_or_else(|e| panic!("{rel} decodes: {e:?}"))
    };
    (load("baseFile"), load("aFile"), load("bFile"))
}

fn fixture_file(root: &Path, fixture: &JVal, key: &str) -> String {
    let rel = str_field(fixture, key).unwrap_or_else(|| panic!("fixture has no {key}"));
    read(root, &format!("merge-conformance/{rel}"))
        .trim_end()
        .to_string()
}

/// The manifest's own count of entries carrying `kind` under `key` — the
/// expected run count, read from the corpus rather than written down here, so
/// adding a fixture cannot leave this leg quietly certifying the old set.
fn manifest_count(manifest: &JVal, key: &str, kind: &str) -> usize {
    let Some(JVal::Arr(entries)) = manifest.field(key) else {
        panic!("merge manifest has no {key}");
    };
    entries
        .iter()
        .filter(|f| str_field(f, "kind").as_deref() == Some(kind))
        .count()
}

#[test]
fn every_merge_conformance_family_is_certified() {
    let Some(root) = corpus() else {
        eprintln!("corpus not found; skipping");
        return;
    };
    let manifest =
        parse(&read(&root, "merge-conformance/manifest.json")).expect("merge manifest parses");

    let (mut auto, mut gated, mut refused) = (0usize, 0usize, 0usize);

    for (key, entries) in ["fixtures", "refusalFixtures"].map(|key| {
        let Some(JVal::Arr(entries)) = manifest.field(key) else {
            panic!("merge manifest has no {key}");
        };
        (key, entries)
    }) {
        for fixture in entries {
            let id = str_field(fixture, "id").expect("id");
            let kind = str_field(fixture, "kind")
                .unwrap_or_else(|| panic!("{id}: fixture carries no kind"));
            let (base, a, b) = triad(&root, fixture);

            match kind.as_str() {
                "merge-3way" => {
                    let expected = fixture_file(&root, fixture, "expectedFile");
                    let outcome_hash = str_field(fixture, "outcomeHash").expect("outcomeHash");
                    let MergeResult::Merged(tree) = merge3_way(&base, &a, &b) else {
                        panic!("{id}: expected a clean auto-merge, got a refusal");
                    };
                    let encoded = encode_node(&tree);
                    assert_eq!(encoded, expected, "{id}: merged tree byte-identical");
                    assert_eq!(
                        sha256_hex(&encoded),
                        outcome_hash,
                        "{id}: outcome hash matches the cross-host golden"
                    );
                    auto += 1;
                }
                "merge-validator-gated" => {
                    let expected = fixture_file(&root, fixture, "verdictFile");
                    let verdict_hash = str_field(fixture, "verdictHash").expect("verdictHash");
                    let MergeResult::Merged(tree) = merge3_way(&base, &a, &b) else {
                        panic!("{id}: gated fixture is not structurally clean");
                    };
                    let defects = introduced_defects(&a, &b, &tree);
                    assert!(
                        !defects.is_empty(),
                        "{id}: the fixture must INTRODUCE a defect — an empty verdict asserts nothing"
                    );
                    let encoded = encode_verdict(&defects);
                    assert_eq!(encoded, expected, "{id}: verdict byte-identical");
                    assert_eq!(
                        sha256_hex(&encoded),
                        verdict_hash,
                        "{id}: verdict hash matches the cross-host golden"
                    );
                    gated += 1;
                }
                "merge-refusal" => {
                    let expected = fixture_file(&root, fixture, "envelopeFile");
                    let envelope_hash = str_field(fixture, "envelopeHash").expect("envelopeHash");
                    let forward = refusal_of(&base, &a, &b, &id);
                    let encoded = encode_envelope(&forward);
                    assert_eq!(encoded, expected, "{id}: refusal envelope byte-identical");
                    assert_eq!(
                        sha256_hex(&encoded),
                        envelope_hash,
                        "{id}: envelope hash matches the cross-host golden"
                    );

                    // Swapping the branches TRANSPOSES each entry's sides and
                    // rewrites nothing else. Asserted rather than committed
                    // twice: two files that were transpositions of each other
                    // would pin the same fact in a form a host could satisfy by
                    // emitting both from one side.
                    let mut fwd = forward;
                    let mut rev = refusal_of(&base, &b, &a, &id);
                    sort_canonical(&mut fwd);
                    sort_canonical(&mut rev);
                    assert_eq!(fwd.len(), rev.len(), "{id}: same refusal set on the swap");
                    for (f, r) in fwd.iter().zip(rev.iter()) {
                        assert_eq!(
                            (&f.node_id, &f.facet, f.class),
                            (&r.node_id, &r.facet, r.class),
                            "{id}: same cell"
                        );
                        assert_eq!(f.base, r.base, "{id}: same base");
                        assert_eq!(f.a, r.b, "{id}: forward a == swapped b");
                        assert_eq!(f.b, r.a, "{id}: forward b == swapped a");
                    }
                    refused += 1;
                }
                other => panic!(
                    "{id}: merge-conformance kind '{other}' (manifest key '{key}') is not certified \
                     by this host — a family must be asserted or skipped BY NAME with a reason, \
                     never passed over silently"
                ),
            }
        }
    }

    assert_eq!(
        auto,
        manifest_count(&manifest, "fixtures", "merge-3way"),
        "every merge-3way fixture the manifest enumerates ran"
    );
    assert_eq!(
        gated,
        manifest_count(&manifest, "fixtures", "merge-validator-gated"),
        "every merge-validator-gated fixture the manifest enumerates ran"
    );
    assert_eq!(
        refused,
        manifest_count(&manifest, "refusalFixtures", "merge-refusal"),
        "every merge-refusal fixture the manifest enumerates ran"
    );
    assert!(
        auto > 0 && gated > 0 && refused > 0,
        "every family is non-empty"
    );
    eprintln!(
        "merge corpus: {auto} auto-merges, {gated} gated verdicts, {refused} refusal envelopes \
         — byte-identical + hash-verified (0 skipped)"
    );
}

/// The refusal set for a triad. Fails loudly if the triad MERGES: a refusal
/// fixture that stopped refusing would otherwise be compared as an empty
/// envelope, which is a green assertion about nothing.
fn refusal_of(base: &Node, a: &Node, b: &Node, id: &str) -> Vec<MergeConflict> {
    match merge3_way(base, a, b) {
        MergeResult::Conflicts(c) => c,
        MergeResult::Merged(_) => panic!("{id}: refusal fixture auto-merged"),
    }
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
            // Two-sided: BOTH branches' values ride the refusal, and neither
            // precedence slot is populated — a value in either IS a precedence
            // claim, and the author-agnostic merge holds no pin.
            assert_eq!(c[0].base, "Default");
            assert_eq!(c[0].a.as_ref().map(|s| s.value.as_str()), Some("Brand"));
            assert_eq!(c[0].b.as_ref().map(|s| s.value.as_str()), Some("Success"));
            assert!(!c[0].primacy_held);
            assert_eq!(c[0].primary, None);
            assert_eq!(c[0].secondary, None);
            assert_eq!(c[0].secondary_tag, None);
        }
        MergeResult::Merged(_) => panic!("expected a style.tone conflict"),
    }
}

/// Both branches insert the SAME id with DIFFERENT content: refused NAMING the
/// id, on the `insert` facet, with each side's content in its own slot — never
/// an arrival-order-dependent A-side pick, and never a whole-parent `children`
/// refusal that names the parent instead of the contended id.
#[test]
fn a_same_id_insert_with_different_content_is_refused_naming_the_id() {
    let dash = |extra: &str| {
        format!(
            r#"{{"id":"dash","kind":{{"$type":"Box","children":[{{"id":"keep","kind":{{"$type":"Markdown","text":"K"}}}}{extra}],"layout":{{"$type":"Auto"}},"role":"Dashboard"}}}}"#
        )
    };
    let base = decode_node(&dash("")).unwrap();
    let a = decode_node(&dash(
        r#",{"id":"new","kind":{"$type":"Markdown","text":"A wrote this"}}"#,
    ))
    .unwrap();
    let b = decode_node(&dash(
        r#",{"id":"new","kind":{"$type":"Markdown","text":"B wrote this"}}"#,
    ))
    .unwrap();
    match merge3_way(&base, &a, &b) {
        MergeResult::Conflicts(c) => {
            assert_eq!(c.len(), 1, "one refusal, on the contended id: {c:?}");
            assert_eq!(c[0].node_id, "new");
            assert_eq!(c[0].facet, "insert");
            // The id exists on neither side of the LCA, so it has no base value.
            assert_eq!(c[0].base, "");
            assert!(
                c[0].a.as_ref().unwrap().value.contains("A wrote this")
                    && c[0].b.as_ref().unwrap().value.contains("B wrote this"),
                "each side carries its OWN content: {c:?}"
            );
        }
        MergeResult::Merged(t) => panic!("expected an insert refusal, merged {}", encode_node(&t)),
    }
}

/// Both branches reaching the SAME child-id list with agreeing content is
/// AGREEMENT, not a conflict — the guard every other facet already had. Without
/// it, `merge3_way base a a` refused for any branch that touched children.
#[test]
fn branches_that_agree_on_the_children_list_merge_rather_than_refuse() {
    let dash = |extra: &str| {
        format!(
            r#"{{"id":"dash","kind":{{"$type":"Box","children":[{{"id":"keep","kind":{{"$type":"Markdown","text":"K"}}}}{extra}],"layout":{{"$type":"Auto"}},"role":"Dashboard"}}}}"#
        )
    };
    let base = decode_node(&dash("")).unwrap();
    let added = dash(r#",{"id":"new","kind":{"$type":"Markdown","text":"same"}}"#);
    let a = decode_node(&added).unwrap();
    let b = decode_node(&added).unwrap();
    match merge3_way(&base, &a, &b) {
        MergeResult::Merged(t) => assert_eq!(encode_node(&t), added.trim_end()),
        MergeResult::Conflicts(c) => panic!("expected agreement, got {c:?}"),
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
