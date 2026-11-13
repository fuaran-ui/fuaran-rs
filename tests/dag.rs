//! Certifies the DAG record codec against `wire-format-fixtures/dag/`
//! (round-trip byte-equal, the post-2645 records) and merge by fold against
//! `merge-conformance/`'s `foldFixtures` — since fuaran#2645 the manifest's only
//! family (version 2): derived trees, refusal envelopes and gated verdicts,
//! byte-equal and hash-verified in both branch orders. The three-way tree merge
//! and its triad families were retired with it (fuaran#2646).

mod common;

use std::path::Path;

use fuaran_rs::canonical::{JVal, ordinal_cmp, parse, render_canonical};
use fuaran_rs::dag::{
    ConflictShape, FoldFault, FoldReconciled, FoldRefusal, FoldRefusalClass, FoldSide,
    GatedFoldFault, OpConflict, decode_record, encode_fold_envelope, encode_record, reconcile_fold,
    reconcile_fold_gated,
};
use fuaran_rs::opstream::{Actor, sha256_hex};
use fuaran_rs::wire::{
    DecodeErrorCode, Node, NodeKind, ToneVariant, TreeOp, decode_node, decode_op, encode_node,
    encode_op,
};

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
    let root = common::corpus_root();
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

/// The merge-conformance manifest is version 2 (fuaran#2645): `foldFixtures`
/// alone. A triad family reappearing would be a family this host no longer
/// certifies, so it fails here BY NAME rather than passing unread.
#[test]
fn the_merge_corpus_is_the_fold_family_alone() {
    let root = common::corpus_root();
    let manifest =
        parse(&read(&root, "merge-conformance/manifest.json")).expect("merge manifest parses");
    assert!(
        matches!(manifest.field("version"), Some(JVal::Num(v)) if *v == 2.0),
        "merge-conformance manifest is version 2"
    );
    for retired in ["fixtures", "refusalFixtures", "totalityFixtures"] {
        assert!(
            manifest.field(retired).is_none(),
            "merge-conformance carries the retired tree-merge family '{retired}'"
        );
    }
}

/// A merge node's op is its reconciliation script as a `Batch` (§5.3.3): the
/// `dag-merge-script` record carries a one-op script, `dag-merge` an empty one.
#[test]
fn a_merge_record_carries_its_script_as_a_batch() {
    let root = common::corpus_root();
    for (file, ops) in [("dag/dag-merge.json", 0), ("dag/dag-merge-script.json", 1)] {
        let record = decode_record(&read(&root, file)).expect("decodes");
        assert_eq!(
            record.parents.len(),
            2,
            "{file}: a merge node has two parents"
        );
        match &record.op {
            TreeOp::Batch(members) => assert_eq!(members.len(), ops, "{file}: script length"),
            other => panic!("{file}: a merge node's op is a Batch, got {other:?}"),
        }
    }
}

/// A pre-2645 record — one carrying the retired `outcomeHash` or `tombstoned`
/// member — is refused BY NAME: its address does not recompute under the cut.
#[test]
fn a_pre_2645_record_is_refused_by_name() {
    let post = concat!(
        r#"{"actor":{"kind":"human","id":"u1"},"hash":"h","#,
        r#""op":{"$type":"RemoveNode","target":"x"},"parents":[],"#,
        r#""resultEnvelope":{"$type":"Success"},"streamId":"s","timestamp":1"#
    );
    for (member, extra) in [
        ("tombstoned", r#","tombstoned":false}"#),
        ("outcomeHash", r#","outcomeHash":"o1"}"#),
    ] {
        let err = decode_record(&format!("{post}{extra}"))
            .expect_err(&format!("a record carrying {member} is refused"));
        assert_eq!(err.path, format!("$.{member}"));
        assert!(
            err.message.contains("pre-2645") && err.message.contains("do not carry forward"),
            "the refusal names the cut, got: {}",
            err.message
        );
    }
    assert!(
        decode_record(&format!("{post}}}")).is_ok(),
        "the post-cut record decodes"
    );
}

// ─── the sample validator the `merge-fold-gated` fixtures certify against ──

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
    let root = common::corpus_root();
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
        r#""hash":"h3","op":{"$type":"RemoveNode","target":"x"},"#,
        r#""parents":["h1","h2"],"promptId":"p-9","#,
        r#""resultEnvelope":{"$type":"Failure","code":"E","message":"m"},"#,
        r#""streamId":"s","timestamp":1700000000}"#
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
            "parents",
            "promptId",
            "resultEnvelope",
            "streamId",
            "timestamp",
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
        r#""resultEnvelope":{"$type":"Success"},"streamId":"s","timestamp":1}"#
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
        r#""userId":"u1"}"#
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
                r#""timestamp":1}}"#
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
        r#""resultEnvelope":{"$type":"Success"},"streamId":"s","timestamp":1}"#
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

// ─── Merge by fold: the `foldFixtures` family (WIRE_FORMAT §34.6, Phase 2644) ──
//
// Every entry of `merge-conformance/manifest.json`'s `foldFixtures`, read under
// its own key BY NAME, each dispatched on its kind:
//
// - `merge-fold-derive`: `reconcile_fold` derives, the tree is byte-identical to
//   `expectedFile` and its sha256 is `outcomeHash` — in BOTH branch orders.
// - `merge-fold-refusal`: the policy refuses, the envelope is byte-identical to
//   `envelopeFile` and its sha256 is `envelopeHash`; the swapped reconcile
//   refuses with every entry's left and right swapped and nothing else changed.
// - `merge-fold-gated`: the fold derives and the sample validator above finds
//   defects neither head carries; the verdict is byte-identical to `verdictFile`
//   and its sha256 is `verdictHash`, in both branch orders, and ungated the same
//   deltas derive.
//
// An empty family FAILS rather than skips: this host certifies it, so an empty
// list means the corpus moved out from under the leg.

/// A canonical `TreeOp` array file, decoded op by op.
fn fold_ops(root: &Path, fixture: &JVal, key: &str) -> Vec<TreeOp> {
    let rel = str_field(fixture, key).unwrap_or_else(|| panic!("fixture has no {key}"));
    let text = read(root, &format!("merge-conformance/{rel}"));
    let Ok(JVal::Arr(items)) = parse(&text) else {
        panic!("{rel} is not a JSON array");
    };
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            decode_op(&render_canonical(item))
                .unwrap_or_else(|e| panic!("{rel}[{i}] decodes: {e:?}"))
        })
        .collect()
}

fn fold_base(root: &Path, fixture: &JVal) -> Node {
    let rel = str_field(fixture, "baseFile").expect("baseFile");
    decode_node(&read(root, &format!("merge-conformance/{rel}")))
        .unwrap_or_else(|e| panic!("{rel} decodes: {e:?}"))
}

fn refused(r: Result<FoldReconciled, FoldFault>, what: &str) -> Vec<FoldRefusal> {
    match r {
        Err(FoldFault::Refused(refusals)) => refusals,
        other => panic!("{what}: expected a refusal, got {other:?}"),
    }
}

#[test]
fn every_fold_fixture_is_certified_in_both_orders() {
    let root = common::corpus_root();
    let manifest =
        parse(&read(&root, "merge-conformance/manifest.json")).expect("merge manifest parses");
    let Some(JVal::Arr(entries)) = manifest.field("foldFixtures") else {
        panic!("merge manifest has no foldFixtures — the family this leg certifies is gone");
    };
    assert!(!entries.is_empty(), "foldFixtures is empty");

    let (mut derived, mut refusals, mut gated) = (0usize, 0usize, 0usize);
    // Every fixture runs and every failure is named in ONE report, so a corpus
    // addition this host does not yet meet reads as the whole list of what it
    // fails, never as the first of an unknown number (Phase 2706).
    let mut failures: Vec<String> = Vec::new();
    for fixture in entries {
        let id = str_field(fixture, "id").expect("id");
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            certify_fold_fixture(&root, fixture, &id)
        })) {
            Ok(FoldKind::Derive) => derived += 1,
            Ok(FoldKind::Refusal) => refusals += 1,
            Ok(FoldKind::Gated) => gated += 1,
            Err(payload) => failures.push(format!("{id}: {}", panic_text(payload.as_ref()))),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} foldFixtures failed:\n  {}",
        failures.len(),
        entries.len(),
        failures.join("\n  ")
    );
    for (ran, kind) in [
        (derived, "merge-fold-derive"),
        (refusals, "merge-fold-refusal"),
        (gated, "merge-fold-gated"),
    ] {
        assert_eq!(
            ran,
            manifest_count(&manifest, "foldFixtures", kind),
            "every {kind} ran"
        );
        assert!(ran > 0, "foldFixtures holds no {kind} fixture");
    }
    eprintln!(
        "fold corpus: {derived} derivations, {refusals} refusal envelopes, {gated} gated verdicts \
         — byte-identical + hash-verified in both orders (0 skipped)"
    );
}

/// The `foldFixtures` kind a fixture certified as.
enum FoldKind {
    Derive,
    Refusal,
    Gated,
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "a non-text panic".to_owned())
}

/// One `foldFixtures` entry, certified per its kind (the rules above). A
/// failure panics; the leg collects it beside every other.
fn certify_fold_fixture(root: &Path, fixture: &JVal, id: &str) -> FoldKind {
    let kind = str_field(fixture, "kind").unwrap_or_else(|| panic!("{id}: no kind"));
    let base = fold_base(root, fixture);
    let (a, b) = (
        fold_ops(root, fixture, "aOpsFile"),
        fold_ops(root, fixture, "bOpsFile"),
    );
    match kind.as_str() {
        "merge-fold-derive" => {
            let expected = fixture_file(root, fixture, "expectedFile");
            let hash = str_field(fixture, "outcomeHash").expect("outcomeHash");
            for (order, d1, d2) in [("a,b", &a, &b), ("b,a", &b, &a)] {
                let r = reconcile_fold(&base, d1, d2)
                    .unwrap_or_else(|f| panic!("{id} ({order}): refused: {f:?}"));
                let encoded = encode_node(&r.tree);
                assert_eq!(
                    encoded, expected,
                    "{id} ({order}): derived tree byte-identical"
                );
                assert_eq!(sha256_hex(&encoded), hash, "{id} ({order}): outcomeHash");
            }
            FoldKind::Derive
        }
        "merge-fold-refusal" => {
            let expected = fixture_file(root, fixture, "envelopeFile");
            let hash = str_field(fixture, "envelopeHash").expect("envelopeHash");
            let forward = refused(reconcile_fold(&base, &a, &b), id);
            let envelope = encode_fold_envelope(&forward);
            assert_eq!(envelope, expected, "{id}: envelope byte-identical");
            assert_eq!(sha256_hex(&envelope), hash, "{id}: envelopeHash");

            // Swap symmetry: the swapped reconcile's refusals, mirrored back
            // (left <-> right) and re-sorted, are the committed envelope.
            let mirrored: Vec<FoldRefusal> =
                refused(reconcile_fold(&base, &b, &a), &format!("{id} (swapped)"))
                    .into_iter()
                    .map(|r| FoldRefusal {
                        conflict: OpConflict {
                            left: r.conflict.right,
                            right: r.conflict.left,
                            ..r.conflict
                        },
                        class: r.class,
                    })
                    .collect();
            assert_eq!(
                encode_fold_envelope(&mirrored),
                expected,
                "{id}: the swapped envelope is the mirror"
            );
            FoldKind::Refusal
        }
        "merge-fold-gated" => {
            let expected = fixture_file(root, fixture, "verdictFile");
            let hash = str_field(fixture, "verdictHash").expect("verdictHash");
            for (order, d1, d2) in [("a,b", &a, &b), ("b,a", &b, &a)] {
                let defects = match reconcile_fold_gated(introduced_defects, &base, d1, d2) {
                    Err(GatedFoldFault::IntroducedDefects(d)) => d,
                    Err(GatedFoldFault::Fold(f)) => {
                        panic!("{id} ({order}): the fold refused: {f:?}")
                    }
                    Ok(_) => panic!("{id} ({order}): expected the gate to refuse"),
                };
                let verdict = encode_verdict(&defects);
                assert_eq!(verdict, expected, "{id} ({order}): verdict byte-identical");
                assert_eq!(sha256_hex(&verdict), hash, "{id} ({order}): verdictHash");
            }
            assert!(
                reconcile_fold(&base, &a, &b).is_ok(),
                "{id}: ungated, the gated fixture must derive"
            );
            FoldKind::Gated
        }
        other => panic!(
            "{id}: foldFixtures kind '{other}' is not certified by this host — a family must \
             be asserted or skipped BY NAME, never passed over silently"
        ),
    }
}

// ─── policy cases the foldFixtures corpus does not pin ──────────────────────

fn op(text: &str) -> TreeOp {
    decode_op(text).unwrap_or_else(|e| panic!("{text}: {e:?}"))
}

fn pane(id: &str) -> String {
    format!(
        r#"{{"id":"{id}","kind":{{"$type":"Markdown","text":{{"$type":"Literal","text":"x"}}}}}}"#
    )
}

/// A Box "root" holding the given panes.
fn box_of(panes: &[&str]) -> Node {
    let children: Vec<String> = panes.iter().map(|p| pane(p)).collect();
    decode_node(&format!(
        r#"{{"id":"root","kind":{{"$type":"Box","children":[{}],"layout":{{"$type":"Flex","direction":"Vertical","wrap":false}},"role":"Group"}}}}"#,
        children.join(",")
    ))
    .expect("box decodes")
}

fn insert(id: &str) -> TreeOp {
    op(&format!(
        r#"{{"$type":"InsertChild","child":{},"parentId":"root"}}"#,
        pane(id)
    ))
}

const BRAND_P: &str = r#"{"$type":"PatchStyle","style":{"tone":"Brand"},"target":"p"}"#;

/// A delta whose own op does not apply is refused before anything is judged,
/// naming the side and the index of the first failing op.
#[test]
fn a_delta_that_does_not_apply_names_its_side_and_index() {
    let base = box_of(&["p"]);
    let good = vec![op(BRAND_P)];
    let bad = vec![
        op(BRAND_P),
        op(r#"{"$type":"RemoveNode","target":"absent"}"#),
    ];
    for (d1, d2, want) in [(&good, &bad, FoldSide::B), (&bad, &good, FoldSide::A)] {
        match reconcile_fold(&base, d1, d2) {
            Err(FoldFault::DeltaDoesNotApply { side, index, .. }) => {
                assert_eq!((side, index), (want, 1));
            }
            other => panic!("want DeltaDoesNotApply, got {other:?}"),
        }
    }
}

/// Two empty deltas derive the base, with no script.
#[test]
fn two_empty_deltas_derive_the_base() {
    let base = box_of(&["p"]);
    let r = reconcile_fold(&base, &[], &[]).expect("derives");
    assert_eq!(encode_node(&r.tree), encode_node(&base));
    assert!(r.script.is_empty());
    assert_eq!((r.agreements, r.interleaves), (0, 0));
}

/// Two appends under one parent interleave by the child-order rule. The merge
/// records the rule's ReorderChildren whichever delta is named first — even
/// where the fold's order is already the rule's (fuaran#2645: the script, not
/// the checked fold order, fixes the children a replay reaches).
#[test]
fn an_interleave_records_its_script_and_is_symmetric() {
    let base = box_of(&["k"]);
    let (a, b) = (vec![insert("z")], vec![insert("m")]);
    let r = reconcile_fold(&base, &a, &b).expect("derives");
    assert_eq!((r.interleaves, r.script.len()), (1, 1));
    assert_eq!(
        encode_op(&r.script[0]),
        r#"{"$type":"ReorderChildren","newOrder":["k","m","z"],"parentId":"root"}"#
    );
    assert_eq!(encode_node(&r.tree), encode_node(&box_of(&["k", "m", "z"])));

    // Named the other way round, the fold's order is already the rule's, and
    // the script is recorded anyway, the same op.
    let swapped = reconcile_fold(&base, &b, &a).expect("swapped derives");
    assert_eq!(swapped.script.len(), 1);
    assert_eq!(encode_op(&swapped.script[0]), encode_op(&r.script[0]));
    assert_eq!(encode_node(&swapped.tree), encode_node(&r.tree));
}

/// A reorder beside an append to its parent is a REBASE (fuaran#2671): the
/// reorder is applied to the children the parent holds when the fold reaches
/// it, the append follows the reordered run, and both orders agree.
#[test]
fn a_reorder_beside_an_append_is_rebased() {
    let base = box_of(&["a", "b"]);
    let reorder = vec![op(
        r#"{"$type":"ReorderChildren","newOrder":["b","a"],"parentId":"root"}"#,
    )];
    let append = vec![insert("c")];
    for (d1, d2) in [(&reorder, &append), (&append, &reorder)] {
        let r = reconcile_fold(&base, d1, d2).expect("a rebase derives");
        assert_eq!(encode_node(&r.tree), encode_node(&box_of(&["b", "a", "c"])));
        assert_eq!(r.interleaves, 1);
    }
    // Two reorders of one parent that differ still refuse.
    let other = vec![op(
        r#"{"$type":"ReorderChildren","newOrder":["a","b"],"parentId":"root"}"#,
    )];
    assert!(matches!(
        reconcile_fold(&base, &reorder, &other),
        Err(FoldFault::Refused(_))
    ));
}

/// An agreement is counted once per distinct op, however many addresses the
/// pair collides on.
#[test]
fn an_agreement_counts_distinct_ops() {
    let base = box_of(&["p"]);
    let same = vec![op(r#"{"$type":"RemoveNode","target":"p"}"#)];
    let r = reconcile_fold(&base, &same, &same).expect("derives");
    assert_eq!(r.agreements, 1);
}

/// The envelope's entries are ordered canonically and exact duplicates are
/// removed, so the input order of the refusals does not reach the bytes.
#[test]
fn the_fold_envelope_is_canonical_and_deduplicated() {
    let (l, r) = (
        op(BRAND_P),
        op(r#"{"$type":"PatchStyle","style":{"tone":"Success"},"target":"p"}"#),
    );
    let refusal = |shape, slot: Option<&str>| FoldRefusal {
        conflict: OpConflict {
            left: l.clone(),
            right: r.clone(),
            address: "p".into(),
            shape,
            slot: slot.map(str::to_owned),
        },
        class: FoldRefusalClass::Conflict,
    };
    let slot = refusal(ConflictShape::SlotClash, Some("style.tone"));
    let whole = refusal(ConflictShape::ConcurrentUpdate, None);
    let x = encode_fold_envelope(&[slot.clone(), whole.clone(), slot.clone()]);
    let y = encode_fold_envelope(&[whole, slot]);
    assert_eq!(x, y, "the envelope depends on input order");
    let (left, right) = (encode_op(&l), encode_op(&r));
    assert_eq!(
        x,
        format!(
            r#"[{{"address":"p","class":"Conflict","left":{left},"right":{right},"shape":"ConcurrentUpdate"}},{{"address":"p","class":"Conflict","left":{left},"right":{right},"shape":"SlotClash","slot":"style.tone"}}]"#
        )
    );
}

// ─── the apply walk reaches every keyed position (Phase 2706) ───────────────

/// The reference's keyed walk holds a node in an envelope `fallback` as well as
/// in a slot argument, and the corpus draws only the slot arguments: an op
/// addressed to a node held only in a `fallback` applies, and a node held only
/// in a FragmentRef's slot argument (beside a value argument, which holds none)
/// takes an insert. Both fail on a lookup that walks the kind's arms alone.
#[test]
fn an_op_reaches_a_node_held_in_a_fallback_or_a_slot_argument() {
    use fuaran_rs::ops::apply;
    let tree = decode_node(
        r#"{"id":"root","kind":{"$type":"Box","children":[{"fallback":{"id":"fb","kind":{"$type":"Markdown","text":"Behind"}},"id":"m","kind":{"$type":"Markdown","text":"x"}},{"id":"ref","kind":{"$type":"FragmentRef","args":{"body":{"$type":"SlotArg","tree":{"id":"slot","kind":{"$type":"Box","children":[],"layout":{"$type":"Auto"},"role":"Dashboard"}}},"n":{"$type":"Int","value":1}},"name":"card"}}],"layout":{"$type":"Auto"},"role":"Dashboard"}}"#,
    )
    .expect("base decodes");
    let styled = apply(
        &tree,
        &op(r#"{"$type":"PatchStyle","style":{"tone":"Brand"},"target":"fb"}"#),
    )
    .expect("an op addressed inside a fallback applies");
    assert!(
        encode_node(&styled.new_tree).contains(r#""fallback":{"id":"fb","kind":{"$type":"Markdown","text":"Behind"},"style":{"tone":"Brand"}}"#),
        "the fallback node carries the patch"
    );
    let inserted = apply(
        &tree,
        &op(&format!(
            r#"{{"$type":"InsertChild","child":{},"parentId":"slot"}}"#,
            pane("new")
        )),
    )
    .expect("an insert into a slot argument's subtree applies");
    assert!(
        encode_node(&inserted.new_tree)
            .contains(r#""id":"slot","kind":{"$type":"Box","children":[{"id":"new""#),
        "the slot argument's subtree takes the child"
    );
}

// ─── an unchanged reorder writes nothing (fuaran#2696, Phase 2720) ──────────

const RESTATE_AB: &str = r#"{"$type":"ReorderChildren","newOrder":["a","b"],"parentId":"root"}"#;

/// A reorder whose list is the parent's child order where it applies reads the
/// parent's `kind` and child-list slots and writes neither; a reorder that
/// changes the order writes both.
#[test]
fn an_unchanged_reorder_reads_the_list_and_writes_nothing() {
    use fuaran_rs::dag::footprint_in;
    let base = box_of(&["a", "b"]);
    let restated = footprint_in(&base, &op(RESTATE_AB));
    let slot = |s: &str| ("root".to_owned(), s.to_owned());
    assert!(restated.slot_writes.is_empty(), "{restated:?}");
    assert!(restated.structure_writes.is_empty(), "{restated:?}");
    assert!(restated.slot_reads.contains(&slot("kind")));
    assert!(restated.slot_reads.contains(&slot("#children")));

    let real = footprint_in(
        &base,
        &op(r#"{"$type":"ReorderChildren","newOrder":["b","a"],"parentId":"root"}"#),
    );
    assert!(real.slot_writes.contains(&slot("kind")));
    assert!(real.slot_writes.contains(&slot("#children")));
}

/// The unchanged reorder is still seen by a removal of a child it lists: the
/// removal writes the parent's child list, which the reorder reads, so Core's
/// report still holds the pair.
#[test]
fn an_unchanged_reorder_still_conflicts_with_a_removal_of_a_listed_child() {
    use fuaran_rs::dag::conflicts_in;
    let base = box_of(&["a", "b"]);
    let (restate, remove) = (
        vec![op(RESTATE_AB)],
        vec![op(r#"{"$type":"RemoveNode","target":"b"}"#)],
    );
    for (d1, d2) in [(&restate, &remove), (&remove, &restate)] {
        let report = conflicts_in(&base, d1, d2);
        assert!(
            report.iter().any(|c| c.address == "root"),
            "the removal of a listed child meets the unchanged reorder: {report:?}"
        );
    }
}

/// Two reorders that each restate the order their own branch holds — one after
/// an append — are not reported against each other, and the fold derives the
/// append in both orders.
#[test]
fn two_restated_orders_derive() {
    let base = box_of(&["a", "b"]);
    let plain = vec![op(RESTATE_AB)];
    let appended = vec![
        insert("c"),
        op(r#"{"$type":"ReorderChildren","newOrder":["a","b","c"],"parentId":"root"}"#),
    ];
    for (d1, d2) in [(&plain, &appended), (&appended, &plain)] {
        let r = reconcile_fold(&base, d1, d2).expect("two restated orders derive");
        assert_eq!(encode_node(&r.tree), encode_node(&box_of(&["a", "b", "c"])));
    }
}
