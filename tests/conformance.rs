//! Certifies the `fuaran-rs` host against the shared `wire-format-fixtures`
//! corpus (the executable conformance suite of the Fuaran UI wire format).
//!
//! Per the corpus contract, `manifest.json` is the authoritative fixture
//! enumeration; per entry:
//! - `node-round-trip` / `op-round-trip` — decode `inputFile` with the named
//!   decoder, re-encode, assert **byte-equal** to `expectedFile`;
//! - `reject` — decode `inputFile`; assert the error's code equals
//!   `expectedErrorCode` and its path starts with `expectedPath`.
//!
//! The versioning-envelope (§15) and elicitation (§18) families are certified
//! here too (Phase 553): `envelope-round-trip` / `envelope-reject`,
//! `elicitation-round-trip` / `elicitation-reject`, and
//! `elicitation-answer-accept` / `elicitation-answer-reject`. The lenient-accept
//! family (§3.6 — bare-text shorthands, null/opaque statics, legacy upgrades,
//! the Phase 460 omit-when-default fields, and the enum/field-name aliases) is
//! certified via `lenient-accept` decode-then-canonical-re-encode. Every
//! declared family is now covered.
//!
//! # An absent corpus is a failure, never a skip
//!
//! Until Phase 1664 an absent corpus made every leg here return, so "the oracle
//! is not here" and "the oracle is here and every family conforms" reported the
//! same `ok`. Phase 1664 split the absence in two — a hard failure in the
//! cross-host workspace, an honest NOT RUN in a standalone clone. Phase 1938
//! retires the second half: **a conformance check that passes without its
//! oracle is worse than no check**, and the remedy for a clone without the
//! corpus is to provide it (clone `wire-format-fixtures` beside this repository,
//! or name it in `FUARAN_WIRE_FIXTURES` — the public workflow does the former),
//! not to report a leg that asserted nothing. A corpus that is CLAIMED and
//! unreadable is a failure too: an operator who named a path has said the
//! oracle exists, so a path that is wrong is a mistake to report.
//!
//! The lookup itself is the crate's one corpus locator, `common::corpus_root`
//! (Phase 2301); its verdicts are exercised in `tests/corpus_locator.rs`.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::elicitation::{
    decode_answer_doc, decode_elicitation, decode_outcome, encode_elicitation, encode_outcome,
};
use fuaran_rs::envelope::{decode_envelope, encode_envelope};
use fuaran_rs::wire::{decode_node, decode_op, encode_node, encode_op};

#[derive(Clone)]
struct Fixture {
    id: String,
    kind: String,
    decoder: String,
    input_file: String,
    expected_file: Option<String>,
    expected_error_code: Option<String>,
    expected_path: Option<String>,
}

fn str_field(fields: &JVal, key: &str) -> Option<String> {
    match fields.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn load_manifest(corpus: &Path) -> Vec<Fixture> {
    let raw = std::fs::read_to_string(corpus.join("manifest.json")).expect("reading manifest");
    let manifest = parse(&raw).expect("manifest.json parses with the host's own JSON layer");
    let Some(JVal::Arr(entries)) = manifest.field("fixtures") else {
        panic!("manifest.json declares no fixtures array");
    };
    entries
        .iter()
        .map(|e| Fixture {
            id: str_field(e, "id").expect("fixture id"),
            kind: str_field(e, "kind").expect("fixture kind"),
            decoder: str_field(e, "decoder").unwrap_or_default(),
            input_file: str_field(e, "inputFile").expect("fixture inputFile"),
            expected_file: str_field(e, "expectedFile"),
            expected_error_code: str_field(e, "expectedErrorCode"),
            expected_path: str_field(e, "expectedPath"),
        })
        .collect()
}

fn read_fixture(corpus: &Path, rel: &str) -> String {
    std::fs::read_to_string(corpus.join(rel))
        .unwrap_or_else(|e| panic!("reading fixture file '{rel}': {e}"))
}

/// The environment variable that turns [`generated_module_matches_the_corpus_idl`]
/// from the drift check into the regeneration: set, the test WRITES the module
/// rather than comparing it.
const REGENERATE_ENV: &str = "FUARAN_IDL_REGENERATE";

/// The corpus's `idl.json` bytes, read exactly as published — the generator's
/// one input and the recorded hash's subject.
fn corpus_idl(corpus: &Path) -> String {
    std::fs::read_to_string(corpus.join("idl.json")).unwrap_or_else(|e| {
        panic!(
            "the corpus at '{}' holds no readable idl.json ({e}); this host's structural \
             layer is generated from it, so its absence is a failure rather than a skip",
            corpus.display()
        )
    })
}

/// Phase 2298: the structural layer in `src/wire/generated.rs` is exactly what
/// the host's own generator writes from the corpus's `idl.json`. Red when the IDL
/// has moved and the module has not; with `FUARAN_IDL_REGENERATE=1` it is the
/// regeneration itself, which is the one step a vocabulary change asks of this
/// host's structure.
///
/// This replaces the two set pins that compared hand-kept kind and control lists
/// with the corpus manifest: those lists are now generated from the IDL the
/// manifest is emitted beside, and since Phase 2315 the `NodeKind` /
/// `FormFieldKind` enums themselves are generated from the same rows, so the
/// drift those pins caught cannot be written any more.
#[test]
fn generated_module_matches_the_corpus_idl() {
    let corpus = common::corpus_root();
    let expected = fuaran_rs::wire::idl_codegen::generate(&corpus_idl(&corpus))
        .unwrap_or_else(|e| panic!("the generator refused the corpus idl.json: {e}"));
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(fuaran_rs::wire::idl_codegen::GENERATED_PATH);
    if std::env::var(REGENERATE_ENV).is_ok_and(|v| !v.trim().is_empty()) {
        std::fs::write(&path, &expected).expect("writing the generated module");
        eprintln!("regenerated {} from {}", path.display(), corpus.display());
        return;
    }
    let committed = std::fs::read_to_string(&path).expect("reading the generated module");
    if committed != expected {
        let line = committed
            .lines()
            .zip(expected.lines())
            .position(|(a, b)| a != b)
            .map_or_else(
                || committed.lines().count().min(expected.lines().count()) + 1,
                |i| i + 1,
            );
        panic!(
            "src/wire/generated.rs is not what the generator writes from {}/idl.json \
             (first difference at line {line}). The IDL has moved: regenerate with \
             {REGENERATE_ENV}=1 cargo test --test conformance generated_module, then build — \
             the compiler names every hand-written site the change reaches.",
            corpus.display()
        );
    }
}

/// Phase 2298: the IDL hash the generated module records is the hash of the
/// corpus's `idl.json` as it stands. The regeneration check above already
/// implies it; this leg says WHICH input moved in one line, and it reads the
/// recorded constant the library compiled with rather than the file on disk.
#[test]
fn generated_module_records_the_corpus_idl_hash() {
    let corpus = common::corpus_root();
    let actual = fuaran_rs::opstream::sha256_hex(&corpus_idl(&corpus));
    assert_eq!(
        fuaran_rs::wire::IDL_SHA256,
        actual,
        "the generated module was produced from a different idl.json than the corpus at '{}' \
         publishes; regenerate with {REGENERATE_ENV}=1 cargo test --test conformance \
         generated_module",
        corpus.display()
    );
}

/// The go-red for the drift check: a generator that ignored its input, or a
/// comparison that could not fail, would leave the check above green on a moved
/// IDL. So the IDL is perturbed the three ways a vocabulary change perturbs it —
/// a kind, a field, an enum case — and each must change the generated module.
#[test]
fn the_generator_reacts_to_each_vocabulary_move() {
    let corpus = common::corpus_root();
    let idl = corpus_idl(&corpus);
    let generate = |text: &str| {
        fuaran_rs::wire::idl_codegen::generate(text)
            .unwrap_or_else(|e| panic!("the generator refused a perturbed idl.json: {e}"))
    };
    let base = generate(&idl);
    let moves = [
        (
            "a new kind",
            "\"tag\": \"Badge\"",
            "\"tag\": \"Badge\"}, {\"category\": \"Display\", \"fields\": [], \"tag\": \"Probe\"",
            "NodeKind::Probe(_)",
        ),
        (
            "a new enum case",
            "\"Neutral\",",
            "\"Neutral\", \"Probe\",",
            "Probe => \"Probe\"",
        ),
        (
            "a new field",
            "\"name\": \"label\",",
            "\"name\": \"probeField\", \"optionality\": {\"$type\": \"optional\"}, \"type\": {\"$type\": \"str\"}}, {\"name\": \"label\",",
            "pub probe_field: Option<String>,",
        ),
    ];
    for (what, from, to, witness) in moves {
        assert!(
            idl.contains(from),
            "the probe for {what} no longer finds its anchor in idl.json: {from}"
        );
        let moved = generate(&idl.replacen(from, to, 1));
        assert_ne!(
            moved, base,
            "{what} in idl.json left the generated module unchanged"
        );
        assert!(
            moved.contains(witness) && !base.contains(witness),
            "{what} did not surface as `{witness}` in the generated module"
        );
    }
}

/// Phase 2315: the dialect (`src/wire/dialect.rs`) is checked against the IDL at
/// generation, so it cannot decay into a mirror. Each of the three ways an entry
/// goes stale is provoked on a perturbed copy of the corpus's `idl.json` and must
/// be REFUSED, naming the entry: a departure the uniform rules now reproduce, an
/// entry naming a field the IDL no longer has, and a note naming nothing the
/// generator emits. The unperturbed copy must generate, so a refusal is the
/// perturbation's and not the rendering's.
#[test]
fn the_generator_refuses_a_stale_dialect_entry() {
    let corpus = common::corpus_root();
    let parsed = fuaran_rs::canonical::parse(&corpus_idl(&corpus))
        .unwrap_or_else(|e| panic!("the corpus idl.json does not parse: {e:?}"));
    let idl = fuaran_rs::canonical::render_canonical(&parsed);
    fuaran_rs::wire::idl_codegen::generate(&idl)
        .unwrap_or_else(|e| panic!("the canonical rendering of idl.json was refused: {e}"));
    let refusals = [
        (
            "a departure the rules now reproduce",
            r#""name":"multiple","optionality":{"$type":"optional"}"#,
            r#""name":"multiple","optionality":{"$type":"omitDefault","default":{"$type":"bool","value":false}}"#,
            "dialect entry Select.multiple is stale",
        ),
        (
            "an entry naming an absent field",
            r#""name":"stateKey""#,
            r#""name":"stateKeyProbe""#,
            "dialect entry Switch.stateKey names a field the IDL does not have",
        ),
        (
            "a note naming nothing emitted",
            r#""name":"trendPolarity""#,
            r#""name":"trendPolarityProbe""#,
            "dialect note 'MetricSpec.trend_polarity' names nothing",
        ),
    ];
    for (what, from, to, witness) in refusals {
        assert_eq!(
            idl.matches(from).count(),
            1,
            "the probe for {what} no longer finds exactly one anchor in idl.json: {from}"
        );
        match fuaran_rs::wire::idl_codegen::generate(&idl.replacen(from, to, 1)) {
            Ok(_) => panic!("{what}: the generator accepted a stale dialect"),
            Err(e) => assert!(
                e.contains(witness),
                "{what}: the generator refused, but not naming `{witness}`: {e}"
            ),
        }
    }
}

/// The FormFieldKind carrier rule (WIRE_FORMAT §11.2): a control discriminator is
/// reached through the PARENT node kind's `$type` — `"Form"` → `fields`,
/// `"Filters"` → `items` — never by property name. `DataGrid.columns[].kind.$type`
/// is a `CellKindErased` sharing spellings (`Text`, `Date`, `Checkbox`) with the
/// control vocabulary, so a property-name heuristic would attest the wrong family
/// and report green.
fn collect_control_kinds(raw: &JVal, controls: &mut std::collections::BTreeSet<String>) {
    if let Some(kind) = raw.field("kind") {
        if let Some(JVal::Str(tag)) = kind.field("$type") {
            let carrier = match tag.as_str() {
                "Form" => Some("fields"),
                "Filters" => Some("items"),
                _ => None,
            };
            if let Some(carrier) = carrier
                && let Some(JVal::Arr(items)) = kind.field(carrier)
            {
                for item in items {
                    if let Some(control) = item.field("kind")
                        && let Some(JVal::Str(control_tag)) = control.field("$type")
                    {
                        controls.insert(control_tag.clone());
                    }
                }
            }
        }
        descend_slots(kind, controls);
    }
    descend_slots(raw, controls);
}

/// Descend every node-bearing position of one wire object — the node envelope,
/// a kind, or a record — through the generated slot enumeration, so a slot the
/// vocabulary gains is walked without an edit here. A name the vocabulary uses
/// for a node in one owner and for a scalar in another (`default`) is harmless:
/// a scalar has no `kind` and no fields to descend.
fn descend_slots(obj: &JVal, controls: &mut std::collections::BTreeSet<String>) {
    let JVal::Obj(fields) = obj else {
        return;
    };
    for (key, value) in fields {
        let nodes = fuaran_rs::wire::NODE_SLOTS.contains(&key.as_str());
        let records = fuaran_rs::wire::NODE_RECORD_SLOTS.contains(&key.as_str());
        if !nodes && !records {
            continue;
        }
        let items = match value {
            JVal::Arr(items) => items.as_slice(),
            other => std::slice::from_ref(other),
        };
        for item in items {
            if nodes {
                collect_control_kinds(item, controls);
            } else {
                descend_slots(item, controls);
            }
        }
    }
}

/// The corpus-driven exhaustiveness guard for the control vocabulary: every
/// FormFieldKind discriminator a round-trip fixture carries must be a case the
/// decoder recognises. Rust's `enum`s make the five value-level matches
/// compiler-checked, but `decode_form_field_kind` is string-dispatch with an
/// `other =>` fallback — this leg is what covers that gap.
#[test]
fn corpus_control_kinds_are_all_recognised() {
    let corpus = common::corpus_root();
    let known: std::collections::BTreeSet<String> = fuaran_rs::wire::CANONICAL_FORM_FIELD_KINDS
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let mut seen = std::collections::BTreeSet::new();
    for fx in load_manifest(&corpus) {
        if fx.kind != "node-round-trip" {
            continue;
        }
        let text = read_fixture(&corpus, &fx.input_file);
        let parsed = parse(&text).expect("fixture parses");
        collect_control_kinds(&parsed, &mut seen);
    }

    let unknown: Vec<&String> = seen.difference(&known).collect();
    assert!(
        unknown.is_empty(),
        "corpus carries form-field kinds the decoder does not recognise — add the case (forward-coupling rule): {unknown:?}"
    );
    assert!(
        !seen.is_empty(),
        "control-vocabulary guard collected no discriminators — the sweep is not reaching the carriers"
    );
    eprintln!(
        "control-vocabulary guard: {} form-field kinds exercised by the corpus",
        seen.len()
    );
}

/// The round-trip legs: every node + op fixture must re-encode byte-identically.
#[test]
fn corpus_round_trips_byte_identical() {
    let corpus = common::corpus_root();
    let mut failures: Vec<String> = vec![];
    let mut ran = 0;
    for fixture in load_manifest(&corpus) {
        let is_node = fixture.kind == "node-round-trip";
        let is_op = fixture.kind == "op-round-trip";
        if !is_node && !is_op {
            continue;
        }
        ran += 1;
        let input = read_fixture(&corpus, &fixture.input_file);
        let expected_rel = fixture
            .expected_file
            .as_deref()
            .unwrap_or(&fixture.input_file);
        let expected = read_fixture(&corpus, expected_rel);
        let re_encoded = if is_node {
            decode_node(&input).map(|n| encode_node(&n))
        } else {
            decode_op(&input).map(|op| encode_op(&op))
        };
        match re_encoded {
            Err(e) => failures.push(format!(
                "{}: decode failed: {} at {}: {}",
                fixture.id,
                e.code.as_str(),
                e.path,
                e.message
            )),
            Ok(actual) if actual != expected => {
                let diff_at = actual
                    .bytes()
                    .zip(expected.bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or_else(|| actual.len().min(expected.len()));
                let lo = diff_at.saturating_sub(40);
                failures.push(format!(
                    "{}: re-encode diverges at byte {} —\n  expected …{}…\n  actual   …{}…",
                    fixture.id,
                    diff_at,
                    &expected[lo..(diff_at + 40).min(expected.len())],
                    &actual[lo..(diff_at + 40).min(actual.len())],
                ));
            }
            Ok(_) => {}
        }
    }
    assert!(ran > 0, "corpus declared no round-trip fixtures");
    assert!(
        failures.is_empty(),
        "{} of {} round-trip fixtures failed:\n{}",
        failures.len(),
        ran,
        failures.join("\n")
    );
    eprintln!("round-trip legs: {ran} fixtures byte-identical");
}

/// The reject leg: every malformed fixture fails decode with the canonical
/// code + a `$`-rooted path carrying the expected prefix.
#[test]
fn corpus_rejects_surface_canonical_code_and_path() {
    let corpus = common::corpus_root();
    let mut failures: Vec<String> = vec![];
    let mut ran = 0;
    for fixture in load_manifest(&corpus) {
        if fixture.kind != "reject" {
            continue;
        }
        ran += 1;
        let input = read_fixture(&corpus, &fixture.input_file);
        let error = match fixture.decoder.as_str() {
            "node" => decode_node(&input).map(|_| ()).err(),
            "op" => decode_op(&input).map(|_| ()).err(),
            other => {
                failures.push(format!("{}: unknown decoder '{other}'", fixture.id));
                continue;
            }
        };
        let expected_code = fixture.expected_error_code.as_deref().unwrap_or("");
        let expected_path = fixture.expected_path.as_deref().unwrap_or("");
        match error {
            None => failures.push(format!(
                "{}: decode ACCEPTED a malformed input (expected {expected_code} at {expected_path})",
                fixture.id
            )),
            Some(e) => {
                if e.code.as_str() != expected_code {
                    failures.push(format!(
                        "{}: wrong code — expected {expected_code}, got {} at {}: {}",
                        fixture.id,
                        e.code.as_str(),
                        e.path,
                        e.message
                    ));
                } else if !e.path.starts_with(expected_path) {
                    failures.push(format!(
                        "{}: wrong path — expected prefix {expected_path}, got {}",
                        fixture.id, e.path
                    ));
                } else if !expected_path.ends_with(".$type") && e.path.ends_with(".$type") {
                    // Phase 1073 — the ruled bare-enum reject-path spelling, pinned.
                    //
                    // The prefix check above cannot catch a spurious `.$type`: this host
                    // reported `$.style.tone.$type` where the corpus says `$.style.tone`
                    // for the corpus's whole life and passed every time. Prefix matching
                    // stays (six fixtures name a position legitimately deeper than the
                    // corpus's stated slot), so this is the guard that makes the ruling
                    // enforceable.
                    //
                    // WIRE_FORMAT.md §6: `$type` appears in a path only when the
                    // DISCRIMINATOR is at fault. A bare enum carries none on the wire, so
                    // the suffix named a JSON member the document does not contain. Use
                    // `unknown_enum_case`, not `unknown_du_case`.
                    failures.push(format!(
                        "{}: spurious `.$type` — corpus expects {expected_path} (a bare-enum \
                         position, no discriminator on the wire), got {}",
                        fixture.id, e.path
                    ));
                }
            }
        }
    }
    assert!(ran > 0, "corpus declared no reject fixtures");
    assert!(
        failures.is_empty(),
        "{} of {} reject fixtures failed:\n{}",
        failures.len(),
        ran,
        failures.join("\n")
    );
    eprintln!("reject leg: {ran} fixtures surface the canonical code + path");
}

/// The §15 versioning-envelope round-trip leg: every `envelope-round-trip`
/// fixture negotiates + decodes + re-encodes byte-identically (Current decodes
/// fully; Behind preserves an unknown kind verbatim).
#[test]
fn corpus_envelope_round_trips_byte_identical() {
    let corpus = common::corpus_root();
    let mut failures: Vec<String> = vec![];
    let mut ran = 0;
    for fixture in load_manifest(&corpus) {
        if fixture.kind != "envelope-round-trip" {
            continue;
        }
        ran += 1;
        let input = read_fixture(&corpus, &fixture.input_file);
        let expected = read_fixture(
            &corpus,
            fixture
                .expected_file
                .as_deref()
                .unwrap_or(&fixture.input_file),
        );
        match decode_envelope(&input) {
            Err(e) => failures.push(format!(
                "{}: decode failed: {} at {}: {}",
                fixture.id,
                e.code.as_str(),
                e.path,
                e.message
            )),
            Ok(env) => {
                let actual = encode_envelope(&env);
                if actual != expected {
                    failures.push(format!(
                        "{}: re-encode diverges —\n  expected {expected}\n  actual   {actual}",
                        fixture.id
                    ));
                }
            }
        }
    }
    assert!(ran > 0, "corpus declared no envelope-round-trip fixtures");
    assert!(
        failures.is_empty(),
        "{} of {} envelope round-trip fixtures failed:\n{}",
        failures.len(),
        ran,
        failures.join("\n")
    );
    eprintln!("envelope round-trip leg: {ran} fixtures byte-identical");
}

/// The §15 envelope reject leg: a Foreign profile is refused with
/// `FOREIGN_PROFILE` at `$.$profile`.
#[test]
fn corpus_envelope_rejects_surface_canonical_code_and_path() {
    let corpus = common::corpus_root();
    let mut failures: Vec<String> = vec![];
    let mut ran = 0;
    for fixture in load_manifest(&corpus) {
        if fixture.kind != "envelope-reject" {
            continue;
        }
        ran += 1;
        let input = read_fixture(&corpus, &fixture.input_file);
        let expected_code = fixture.expected_error_code.as_deref().unwrap_or("");
        let expected_path = fixture.expected_path.as_deref().unwrap_or("");
        match decode_envelope(&input) {
            Ok(_) => failures.push(format!(
                "{}: decode ACCEPTED a malformed envelope (expected {expected_code} at {expected_path})",
                fixture.id
            )),
            Err(e) => {
                if e.code.as_str() != expected_code {
                    failures.push(format!(
                        "{}: wrong code — expected {expected_code}, got {} at {}",
                        fixture.id,
                        e.code.as_str(),
                        e.path
                    ));
                } else if !e.path.starts_with(expected_path) {
                    failures.push(format!(
                        "{}: wrong path — expected prefix {expected_path}, got {}",
                        fixture.id, e.path
                    ));
                }
            }
        }
    }
    assert!(ran > 0, "corpus declared no envelope-reject fixtures");
    assert!(
        failures.is_empty(),
        "{} of {} envelope reject fixtures failed:\n{}",
        failures.len(),
        ran,
        failures.join("\n")
    );
    eprintln!("envelope reject leg: {ran} fixtures surface the canonical code + path");
}

/// Re-encode an elicitation fixture through the decoder named by the fixture
/// (`elicitation` → the envelope codec; `elicitation-outcome` → the outcome
/// codec), returning either the re-encoded bytes or a `(code, path, message)`.
fn elicitation_round_trip(decoder: &str, input: &str) -> Result<String, (String, String, String)> {
    match decoder {
        "elicitation" => decode_elicitation(input)
            .map(|e| encode_elicitation(&e))
            .map_err(|e| (e.code.as_str().to_string(), e.path, e.message)),
        "elicitation-outcome" => decode_outcome(input)
            .map(|o| encode_outcome(&o))
            .map_err(|e| (e.code.as_str().to_string(), e.path, e.message)),
        other => Err((
            "UNKNOWN_DECODER".to_string(),
            other.to_string(),
            String::new(),
        )),
    }
}

/// The §18 elicitation round-trip leg: every `elicitation-round-trip` fixture
/// (envelope + outcome decoders) re-encodes byte-identically.
#[test]
fn corpus_elicitation_round_trips_byte_identical() {
    let corpus = common::corpus_root();
    let mut failures: Vec<String> = vec![];
    let mut ran = 0;
    for fixture in load_manifest(&corpus) {
        if fixture.kind != "elicitation-round-trip" {
            continue;
        }
        ran += 1;
        let input = read_fixture(&corpus, &fixture.input_file);
        let expected = read_fixture(
            &corpus,
            fixture
                .expected_file
                .as_deref()
                .unwrap_or(&fixture.input_file),
        );
        match elicitation_round_trip(&fixture.decoder, &input) {
            Err((code, path, message)) => failures.push(format!(
                "{}: decode failed: {code} at {path}: {message}",
                fixture.id
            )),
            Ok(actual) if actual != expected => failures.push(format!(
                "{}: re-encode diverges —\n  expected {expected}\n  actual   {actual}",
                fixture.id
            )),
            Ok(_) => {}
        }
    }
    assert!(
        ran > 0,
        "corpus declared no elicitation-round-trip fixtures"
    );
    assert!(
        failures.is_empty(),
        "{} of {} elicitation round-trip fixtures failed:\n{}",
        failures.len(),
        ran,
        failures.join("\n")
    );
    eprintln!("elicitation round-trip leg: {ran} fixtures byte-identical");
}

/// Decode an elicitation reject fixture through its named decoder, returning the
/// structured error (or `None` if it wrongly accepted).
fn elicitation_reject(decoder: &str, input: &str) -> Option<(String, String)> {
    match decoder {
        "elicitation" => decode_elicitation(input)
            .err()
            .map(|e| (e.code.as_str().to_string(), e.path)),
        "elicitation-outcome" => decode_outcome(input)
            .err()
            .map(|e| (e.code.as_str().to_string(), e.path)),
        "elicitation-answer" => decode_answer_doc(input)
            .err()
            .map(|e| (e.code.as_str().to_string(), e.path)),
        _ => Some(("UNKNOWN_DECODER".to_string(), decoder.to_string())),
    }
}

/// The §18 elicitation reject + answer-accept/reject legs: reject fixtures
/// surface the expected code + `$`-rooted path prefix; answer-accept fixtures
/// validate clean.
#[test]
fn corpus_elicitation_rejects_and_answers_conform() {
    let corpus = common::corpus_root();
    let mut failures: Vec<String> = vec![];
    let mut ran = 0;
    for fixture in load_manifest(&corpus) {
        match fixture.kind.as_str() {
            "elicitation-reject" | "elicitation-answer-reject" => {
                ran += 1;
                let input = read_fixture(&corpus, &fixture.input_file);
                let expected_code = fixture.expected_error_code.as_deref().unwrap_or("");
                let expected_path = fixture.expected_path.as_deref().unwrap_or("");
                match elicitation_reject(&fixture.decoder, &input) {
                    None => failures.push(format!(
                        "{}: decode ACCEPTED a malformed input (expected {expected_code} at {expected_path})",
                        fixture.id
                    )),
                    Some((code, path)) => {
                        if code != expected_code {
                            failures.push(format!(
                                "{}: wrong code — expected {expected_code}, got {code} at {path}",
                                fixture.id
                            ));
                        } else if !path.starts_with(expected_path) {
                            failures.push(format!(
                                "{}: wrong path — expected prefix {expected_path}, got {path}",
                                fixture.id
                            ));
                        }
                    }
                }
            }
            "elicitation-answer-accept" => {
                ran += 1;
                let input = read_fixture(&corpus, &fixture.input_file);
                if let Err(e) = decode_answer_doc(&input) {
                    failures.push(format!(
                        "{}: answer-accept fixture was REJECTED: {} at {}: {}",
                        fixture.id,
                        e.code.as_str(),
                        e.path,
                        e.message
                    ));
                }
            }
            _ => {}
        }
    }
    assert!(
        ran > 0,
        "corpus declared no elicitation reject/answer fixtures"
    );
    assert!(
        failures.is_empty(),
        "{} of {} elicitation reject/answer fixtures failed:\n{}",
        failures.len(),
        ran,
        failures.join("\n")
    );
    eprintln!("elicitation reject + answer legs: {ran} fixtures conform");
}

/// The lenient-accept leg (WIRE_FORMAT.md §3.6): every `lenient-accept` fixture
/// decodes its `inputFile` with the named decoder, re-encodes, and asserts
/// byte-equality against `expectedFile`. The inputs carry the decode-only
/// lenient forms — bare-text shorthands, null/opaque statics, legacy container
/// upgrades, the Phase 460 omit-when-default / explicit-default stylistic
/// fields, and the enum-value / field-name aliases — and the expected files are
/// the canonical normalisation (aliases never survive a re-encode).
#[test]
fn corpus_lenient_accept_round_trips_byte_identical() {
    let corpus = common::corpus_root();
    let mut failures: Vec<String> = vec![];
    let mut ran = 0;
    for fixture in load_manifest(&corpus) {
        if fixture.kind != "lenient-accept" {
            continue;
        }
        ran += 1;
        let input = read_fixture(&corpus, &fixture.input_file);
        let expected_rel = fixture
            .expected_file
            .as_deref()
            .unwrap_or(&fixture.input_file);
        let expected = read_fixture(&corpus, expected_rel);
        let re_encoded = match fixture.decoder.as_str() {
            "node" => decode_node(&input).map(|n| encode_node(&n)),
            "op" => decode_op(&input).map(|op| encode_op(&op)),
            other => {
                failures.push(format!("{}: unknown decoder '{other}'", fixture.id));
                continue;
            }
        };
        match re_encoded {
            Err(e) => failures.push(format!(
                "{}: decode failed: {} at {}: {}",
                fixture.id,
                e.code.as_str(),
                e.path,
                e.message
            )),
            Ok(actual) if actual != expected => {
                let diff_at = actual
                    .bytes()
                    .zip(expected.bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or_else(|| actual.len().min(expected.len()));
                let lo = diff_at.saturating_sub(40);
                failures.push(format!(
                    "{}: re-encode diverges at byte {} —\n  expected …{}…\n  actual   …{}…",
                    fixture.id,
                    diff_at,
                    &expected[lo..(diff_at + 40).min(expected.len())],
                    &actual[lo..(diff_at + 40).min(actual.len())],
                ));
            }
            Ok(_) => {}
        }
    }
    assert!(ran > 0, "corpus declared no lenient-accept fixtures");
    assert!(
        failures.is_empty(),
        "{} of {} lenient-accept fixtures failed:\n{}",
        failures.len(),
        ran,
        failures.join("\n")
    );
    eprintln!("lenient-accept leg: {ran} fixtures normalise byte-identical");
}

// ── Rule-marked twins — the validator declaration, checked against behaviour ──
//
// Phase 1836. Most of this corpus certifies the CODEC, and a round-trip fixture
// says nothing about a validator: `filters-dependson-undeclared` round-trips
// byte-identically on a host with no FUARAN075 at all, which is exactly how this
// host passed it silently until Phase 1800 measured it. So the fixtures that
// exist to exercise a validator rule are MARKED here, each negative with its
// control, and `validator-coverage.json` is held to what the validator does on
// them: `implemented` must refuse the negative (with the reader's id and the
// offending name) and pass the control; `abstained` must be true — a validator
// that raises the code while the file says it does not is a lie in the other
// direction. A marked code in NEITHER set is a failure, not a default.

/// `(code, negative fixture id, reader node id, offending name, control fixture id)`.
const RULE_MARKED_TWINS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "FUARAN075",
        "filters-dependson-undeclared",
        "scoped-metric",
        "genre",
        "filters-dependson-declared",
    ),
    (
        "FUARAN075",
        "filters-param-source-undeclared",
        "scoped-grid",
        "genre",
        "filters-param-source-declared",
    ),
];

#[derive(Debug, PartialEq, Eq)]
enum Declared {
    Implemented,
    Abstained,
}

/// What this host's `validator-coverage.json` says about `code` — and a panic
/// when it says both, or nothing.
fn declared_stance(code: &str) -> Declared {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("validator-coverage.json");
    let raw = std::fs::read_to_string(&path).expect("reading validator-coverage.json");
    let decl = parse(&raw).expect("validator-coverage.json parses");
    let implemented = matches!(
        decl.field("implemented"),
        Some(JVal::Arr(codes)) if codes.iter().any(|c| matches!(c, JVal::Str(s) if s == code))
    );
    let abstained = matches!(
        decl.field("abstained"),
        Some(JVal::Obj(entries)) if entries.iter().any(|(k, _)| k == code)
    );
    match (implemented, abstained) {
        (true, false) => Declared::Implemented,
        (false, true) => Declared::Abstained,
        (true, true) => panic!(
            "validator-coverage.json names {code} in BOTH `implemented` and `abstained` — one \
             of them is false"
        ),
        (false, false) => panic!(
            "validator-coverage.json names {code} in neither `implemented` nor `abstained`, but \
             the corpus carries fixtures marked for it. The abstention default is not a \
             decision; record one."
        ),
    }
}

#[test]
fn every_rule_marked_code_is_declared_exactly_once() {
    for (code, ..) in RULE_MARKED_TWINS {
        declared_stance(code);
    }
}

#[test]
fn corpus_rule_marked_twins_decide_as_declared() {
    let corpus = common::corpus_root();
    let manifest = load_manifest(&corpus);
    let findings_on = |id: &str| {
        let fixture = manifest
            .iter()
            .find(|f| f.id == id)
            .unwrap_or_else(|| panic!("the corpus has no fixture '{id}' — a marked twin moved"));
        let tree = decode_node(&read_fixture(&corpus, &fixture.input_file))
            .unwrap_or_else(|e| panic!("marked fixture '{id}' does not decode: {e:?}"));
        fuaran_rs::validator::validate(&tree)
    };

    for (code, negative, reader, name, control) in RULE_MARKED_TWINS {
        let on_negative: Vec<_> = findings_on(negative)
            .into_iter()
            .filter(|f| f.code == *code)
            .collect();
        let on_control: Vec<_> = findings_on(control)
            .into_iter()
            .filter(|f| f.code == *code)
            .collect();
        match declared_stance(code) {
            Declared::Implemented => {
                assert_eq!(
                    on_negative.len(),
                    1,
                    "{code} is declared implemented, so '{negative}' must be refused exactly \
                     once; got {on_negative:?}"
                );
                let f = &on_negative[0];
                assert_eq!(
                    f.node_id, *reader,
                    "'{negative}': {code} anchors to the edge's reader"
                );
                assert!(
                    f.message.contains(&format!("'{name}'")),
                    "'{negative}': {code}'s message names '{name}': {}",
                    f.message
                );
                assert!(
                    on_control.is_empty(),
                    "'{control}' is the declared twin of '{negative}' and must pass: {on_control:?}"
                );
            }
            Declared::Abstained => {
                assert!(
                    on_negative.is_empty() && on_control.is_empty(),
                    "{code} is declared ABSTAINED but the validator raises it — the declaration \
                     is false"
                );
                eprintln!(
                    "{code}: '{negative}' is ABSTAINED on this host, not passed — the silent \
                     no-op is declared, and nothing here certifies it"
                );
            }
        }
    }
}

// ── The corpus families this host runs (WIRE_FORMAT §27.7, Phase 1938) ───────
//
// `HOST_CORPUS_FAMILIES` replaced a printer that logged the uncovered root
// fixture kinds and asserted nothing: it could not see a self-enumerated
// family. The declaration is a host STATEMENT — no introspection can see which
// families a test suite runs — so the gate below is what keeps it true: a
// family the corpus has and the table does not name, or one the table names
// and the corpus no longer has, fails. `host-capability-manifest.json` at the
// repository root publishes it, generated here; regenerate it with
// `FUARAN_MANIFEST_REGENERATE=1 cargo test --test conformance host_capability_manifest_is_current`.
// Its token claim (§27.3, Phase 2332) is the generated layer's
// `CONSTRUCT_TOKENS`, read back against the corpus below ("The token claim").

const NO_CARD_CODEC: &str =
    "this host has no contract-card codec (§25) for the card fixtures to run against yet";
const TELEPORT_UNRUN: &str = "the teleport module is unit-tested by hand; this suite does not yet run the corpus teleport/ fixtures";
const LAW_UNRUN: &str = "this host's suite does not yet run this law family's vectors";
const HOSTED_OPAQUE: &str = "this host's generated type model carries each hosted slot opaquely, so it holds no enumeration of the discriminators inside one (§27.5) and this host makes no claim in this family";

/// One host answer for a corpus family (§27.7).
#[derive(Clone, Copy)]
enum FamilyAnswer {
    Covered,
    Pending(&'static str),
    NotApplicable(&'static str),
}

/// Every corpus family and this host's answer, sorted by family name.
const HOST_CORPUS_FAMILIES: &[(&str, FamilyAnswer)] = &[
    ("apply/duplicateIdsApply", FamilyAnswer::Covered),
    ("apply/limitsApply", FamilyAnswer::Covered),
    (
        "apply/skeletonApply",
        FamilyAnswer::Pending(
            "the apply engine's suite does not yet run the skeleton-apply vectors",
        ),
    ),
    ("apply/styleNormaliseApply", FamilyAnswer::Covered),
    ("contract-card-reject", FamilyAnswer::Pending(NO_CARD_CODEC)),
    (
        "contract-card-round-trip",
        FamilyAnswer::Pending(NO_CARD_CODEC),
    ),
    ("dag", FamilyAnswer::Covered),
    (
        "decode-policy",
        FamilyAnswer::NotApplicable("this host's decoder offers no §23 kind admission policy"),
    ),
    (
        "design-seam-reject",
        FamilyAnswer::Pending("this host has no design-provider seam codec yet (Phase 2295)"),
    ),
    (
        "design-seam-round-trip",
        FamilyAnswer::Pending("this host has no design-provider seam codec yet (Phase 2295)"),
    ),
    (
        "devtools-relay",
        FamilyAnswer::NotApplicable("this host ships no devtools relay endpoint"),
    ),
    ("diff/treeDiff", FamilyAnswer::Covered),
    ("elicitation-answer-accept", FamilyAnswer::Covered),
    ("elicitation-answer-reject", FamilyAnswer::Covered),
    ("elicitation-reject", FamilyAnswer::Covered),
    ("elicitation-round-trip", FamilyAnswer::Covered),
    ("envelope-reject", FamilyAnswer::Covered),
    ("envelope-round-trip", FamilyAnswer::Covered),
    (
        "formatting",
        FamilyAnswer::Pending(
            "the host formatter is not yet certified against the golden; carried by fuaran#2328",
        ),
    ),
    ("laws/capabilityLaws", FamilyAnswer::Pending(LAW_UNRUN)),
    ("laws/dataframeParity", FamilyAnswer::Covered),
    ("laws/decimal", FamilyAnswer::Covered),
    ("laws/fieldLaws", FamilyAnswer::Pending(LAW_UNRUN)),
    ("laws/transformLaws", FamilyAnswer::Covered),
    ("lenient-accept", FamilyAnswer::Covered),
    ("lowers-to", FamilyAnswer::Covered),
    ("merge-conformance", FamilyAnswer::Covered),
    ("node-round-trip", FamilyAnswer::Covered),
    ("op-round-trip", FamilyAnswer::Covered),
    ("reject", FamilyAnswer::Covered),
    (
        "repair",
        FamilyAnswer::Pending("this host has no §28 repair function yet"),
    ),
    (
        "row-encode",
        FamilyAnswer::NotApplicable(
            "a row cell's only number is an f64 and the wire model holds no date value, so neither §5 encode refusal can arise here",
        ),
    ),
    ("sanitization", FamilyAnswer::Covered),
    (
        "stored-emissions",
        FamilyAnswer::Pending("this host's suite does not yet run the stored-emission fixtures"),
    ),
    ("style-observer", FamilyAnswer::Covered),
    ("teleport-decode", FamilyAnswer::Pending(TELEPORT_UNRUN)),
    ("teleport-reject", FamilyAnswer::Pending(TELEPORT_UNRUN)),
];

/// The corpus family set, derived from the corpus tree (§27.7): each root
/// manifest fixture kind, and each top-level directory carrying its own
/// `manifest.json` — one family named by the directory, or one family
/// `<directory>/<id>` per entry of that manifest's `families` array of objects.
fn corpus_families(root: &Path) -> std::collections::BTreeSet<String> {
    let mut found: std::collections::BTreeSet<String> =
        load_manifest(root).into_iter().map(|f| f.kind).collect();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .expect("listing the corpus")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let Ok(raw) = std::fs::read_to_string(dir.join("manifest.json")) else {
            continue;
        };
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let own = parse(&raw).unwrap_or_else(|e| panic!("{name}/manifest.json: {e:?}"));
        match own.field("families") {
            Some(JVal::Arr(members))
                if !members.is_empty() && members.iter().all(|m| matches!(m, JVal::Obj(_))) =>
            {
                for member in members {
                    let id = str_field(member, "id").unwrap_or_else(|| {
                        panic!("{name}/manifest.json: a families entry carries no id")
                    });
                    found.insert(format!("{name}/{id}"));
                }
            }
            _ => {
                found.insert(name);
            }
        }
    }
    found
}

#[test]
fn the_manifest_names_every_corpus_family_and_no_other() {
    let corpus = common::corpus_root();
    let families = corpus_families(&corpus);
    let declared: std::collections::BTreeSet<String> = HOST_CORPUS_FAMILIES
        .iter()
        .map(|(f, _)| (*f).to_string())
        .collect();
    let missing: Vec<&String> = families.difference(&declared).collect();
    let stale: Vec<&String> = declared.difference(&families).collect();
    assert!(
        missing.is_empty(),
        "corpus families this host's manifest does not name: {missing:?}"
    );
    assert!(
        stale.is_empty(),
        "families the manifest names that the corpus no longer has: {stale:?}"
    );
}

/// The falsifier: a family that exists only as a directory with its own
/// manifest — the shape the printer could not see — joins the derived set, and
/// so does each `families` entry.
#[test]
fn the_family_derivation_sees_a_self_enumerated_family() {
    let dir = std::env::temp_dir().join(format!(
        "fuaran-rs-corpus-families-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let write = |rel: &str, body: &str| {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).expect("scratch dir");
        std::fs::write(p, body).expect("scratch file");
    };
    write(
        "manifest.json",
        r#"{"fixtures":[{"id":"a","kind":"node-round-trip","inputFile":"nodes/a.json"}]}"#,
    );
    write("newfamily/manifest.json", r#"{"cases":[]}"#);
    write(
        "split/manifest.json",
        r#"{"families":[{"id":"a"},{"id":"b"}]}"#,
    );
    write("goldens/goldens.json", "{}");
    let got = corpus_families(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let want: std::collections::BTreeSet<String> =
        ["node-round-trip", "newfamily", "split/a", "split/b"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
    assert_eq!(got, want);
}

fn json_string(s: &str) -> String {
    fuaran_rs::canonical::render_canonical(&JVal::Str(s.to_string()))
}

/// The published artefact: the members in the §27.3 order, maps sorted by
/// key, two-space indent, trailing newline.
fn host_capability_manifest() -> String {
    let token_families = [
        "caseFields",
        "hostedCases",
        "kindFields",
        "kinds",
        "recordFields",
        "unionCases",
    ];
    let mut out = String::from("{\n");
    out.push_str("  \"$manifest\": \"fuaran.host-capability/1\",\n");
    out.push_str("  \"host\": \"fuaran-rs\",\n");
    out.push_str(&format!(
        "  \"hostVersion\": {},\n",
        json_string(env!("CARGO_PKG_VERSION"))
    ));
    out.push_str("  \"corpusAuthority\": null,\n");
    out.push_str(&format!(
        "  \"generator\": {},\n",
        json_string(
            "tests/conformance.rs (FUARAN_MANIFEST_REGENERATE=1 cargo test --test conformance host_capability_manifest_is_current)"
        )
    ));
    out.push_str("  \"families\": {\n");
    for (i, f) in token_families.iter().enumerate() {
        let claim = if DERIVED_FAMILIES.contains(f) {
            "\"covered\": true".to_string()
        } else {
            format!(
                "\"covered\": false,\n      \"reason\": {}",
                json_string(HOSTED_OPAQUE)
            )
        };
        out.push_str(&format!(
            "    \"{f}\": {{\n      {claim}\n    }}{}\n",
            if i + 1 == token_families.len() {
                ""
            } else {
                ","
            }
        ));
    }
    out.push_str("  },\n");
    out.push_str("  \"corpusFamilies\": {\n");
    for (i, (family, answer)) in HOST_CORPUS_FAMILIES.iter().enumerate() {
        let value = match answer {
            FamilyAnswer::Covered => "\"covered\"".to_string(),
            FamilyAnswer::Pending(why) => {
                format!("{{\n      \"pending\": {}\n    }}", json_string(why))
            }
            FamilyAnswer::NotApplicable(why) => {
                format!("{{\n      \"notApplicable\": {}\n    }}", json_string(why))
            }
        };
        out.push_str(&format!(
            "    {}: {value}{}\n",
            json_string(family),
            if i + 1 == HOST_CORPUS_FAMILIES.len() {
                ""
            } else {
                ","
            }
        ));
    }
    out.push_str("  },\n");
    let tokens: Vec<String> = fuaran_rs::wire::CONSTRUCT_TOKENS
        .iter()
        .map(|t| format!("    {}", json_string(t)))
        .collect();
    if tokens.is_empty() {
        out.push_str("  \"tokens\": []\n}\n");
    } else {
        out.push_str(&format!(
            "  \"tokens\": [\n{}\n  ]\n}}\n",
            tokens.join(",\n")
        ));
    }
    out
}

#[test]
fn host_capability_manifest_is_current() {
    let sorted = HOST_CORPUS_FAMILIES.windows(2).all(|w| w[0].0 < w[1].0);
    assert!(
        sorted,
        "HOST_CORPUS_FAMILIES must stay sorted by family name"
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("host-capability-manifest.json");
    let want = host_capability_manifest();
    parse(&want).expect("the rendered manifest is JSON");
    if std::env::var_os("FUARAN_MANIFEST_REGENERATE").is_some() {
        std::fs::write(&path, &want).expect("writing host-capability-manifest.json");
        return;
    }
    let got = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading the published manifest: {e}"));
    assert!(
        got == want,
        "host-capability-manifest.json is not what this host's declaration renders — regenerate it \
         with FUARAN_MANIFEST_REGENERATE=1 cargo test --test conformance host_capability_manifest_is_current"
    );
}

// ── The token claim (WIRE_FORMAT §27.1–27.4, Phase 2332) ─────────────────────
//
// The claim is `fuaran_rs::wire::CONSTRUCT_TOKENS`, which the host's generator
// writes into the generated layer beside the wire types it projects from
// `idl.json` — never a list written here. `hostedCases` stays not covered: the
// generated layer types each hosted slot opaquely, so it holds no enumeration
// of the discriminators inside one (§27.5).

/// The token families `CONSTRUCT_TOKENS` carries; the sixth, `hostedCases`, is
/// declared not covered.
const DERIVED_FAMILIES: &[&str] = &[
    "caseFields",
    "kindFields",
    "kinds",
    "recordFields",
    "unionCases",
];

/// The §27.3 token family a token belongs to, read from the §27.2 grammar alone.
fn token_family(token: &str) -> &'static str {
    let segments: Vec<&str> = token.split('.').collect();
    match (segments[0], segments.len()) {
        ("Kind", 2) => "kinds",
        ("Kind", _) => "kindFields",
        (_, 3) => "caseFields",
        _ if segments[1].starts_with(|c: char| c.is_ascii_uppercase()) => "unionCases",
        _ => "recordFields",
    }
}

#[test]
fn every_claimed_token_lies_in_a_covered_family() {
    let outside: Vec<&str> = fuaran_rs::wire::CONSTRUCT_TOKENS
        .iter()
        .copied()
        .filter(|t| !DERIVED_FAMILIES.contains(&token_family(t)))
        .collect();
    assert!(
        outside.is_empty(),
        "tokens outside the covered families: {outside:?}"
    );
}

/// The tokens a module generated from `idl` would claim, less those of the
/// unmodified corpus IDL: what removing a construct from the model removes.
fn removed_by(idl: &str, edit: impl Fn(&mut JVal)) -> Vec<String> {
    let before = fuaran_rs::wire::idl_codegen::construct_tokens(idl).expect("the corpus IDL reads");
    let mut doc = parse(idl).expect("idl.json parses");
    edit(&mut doc);
    let edited = fuaran_rs::canonical::render_canonical(&doc);
    let after =
        fuaran_rs::wire::idl_codegen::construct_tokens(&edited).expect("the edited IDL reads");
    before.into_iter().filter(|t| !after.contains(t)).collect()
}

fn rows_mut<'a>(doc: &'a mut JVal, key: &str) -> &'a mut Vec<JVal> {
    let JVal::Obj(members) = doc else {
        panic!("idl.json is an object")
    };
    match members.iter_mut().find(|(k, _)| k == key) {
        Some((_, JVal::Arr(rows))) => rows,
        _ => panic!("idl.json has no {key} array"),
    }
}

#[test]
fn removing_a_kind_removes_exactly_its_tokens() {
    let idl = corpus_idl(&common::corpus_root());
    let removed = removed_by(&idl, |doc| {
        rows_mut(doc, "kinds").retain(|k| str_field(k, "tag").as_deref() != Some("Badge"));
    });
    assert_eq!(
        removed,
        ["Kind.Badge", "Kind.Badge.label", "Kind.Badge.variant"]
    );
}

#[test]
fn removing_a_union_case_removes_exactly_its_tokens() {
    let idl = corpus_idl(&common::corpus_root());
    let removed = removed_by(&idl, |doc| {
        for union in rows_mut(doc, "unions").iter_mut() {
            if str_field(union, "name").as_deref() == Some("Binding") {
                rows_mut(union, "cases")
                    .retain(|c| str_field(c, "tag").as_deref() != Some("Static"));
            }
        }
    });
    assert_eq!(removed, ["Binding.Static", "Binding.Static.value"]);
}

#[test]
fn a_host_only_row_is_no_token() {
    let claimed = fuaran_rs::wire::CONSTRUCT_TOKENS;
    assert!(claimed.contains(&"Action.Dispatch"));
    assert!(
        !claimed.contains(&"Action.Dispatch.msg"),
        "a hostOnly row never reaches the wire"
    );
}

// ── Reading the claim back against the corpus ───────────────────────────────
//
// Every fixture of the round-trip families is walked against the corpus's OWN
// idl.json — never this host's generated layer, so the claim is checked against
// the oracle rather than against itself — for the construct tokens it
// exercises (§27.4). A token the corpus exercises and this host does not claim
// fails by name; so does a claimed token carried by a fixture this host does
// not round-trip.

/// The families whose canonical documents are wire trees a host decodes and
/// re-encodes.
const EXERCISING_FAMILIES: &[&str] = &["node-round-trip", "op-round-trip", "lenient-accept"];

/// The corpus IDL, indexed for the walk.
struct Vocabulary {
    kinds: BTreeMap<String, JVal>,
    ops: BTreeMap<String, JVal>,
    records: BTreeMap<String, JVal>,
    unions: BTreeMap<String, BTreeMap<String, JVal>>,
    transparent: BTreeMap<String, String>,
    node_fields: JVal,
}

fn arr(v: Option<&JVal>) -> &[JVal] {
    match v {
        Some(JVal::Arr(items)) => items,
        _ => &[],
    }
}

fn fields_of(row: &JVal) -> JVal {
    row.field("fields").cloned().unwrap_or(JVal::Arr(vec![]))
}

impl Vocabulary {
    fn read(idl: &str) -> Self {
        let doc = parse(idl).expect("idl.json parses");
        let by = |key: &str, id: &str| -> BTreeMap<String, JVal> {
            arr(doc.field(key))
                .iter()
                .map(|r| (str_field(r, id).expect("a row id"), fields_of(r)))
                .collect()
        };
        let unions = arr(doc.field("unions"))
            .iter()
            .map(|u| {
                let cases = arr(u.field("cases"))
                    .iter()
                    .map(|c| (str_field(c, "tag").expect("a case tag"), fields_of(c)))
                    .collect();
                (str_field(u, "name").expect("a union name"), cases)
            })
            .collect();
        let transparent = arr(doc
            .field("harden")
            .and_then(|h| h.field("transparentUnions")))
        .iter()
        .map(|t| {
            (
                str_field(t, "union").expect("union"),
                str_field(t, "case").expect("case"),
            )
        })
        .collect();
        Vocabulary {
            kinds: by("kinds", "tag"),
            ops: by("ops", "tag"),
            records: by("records", "name"),
            unions,
            transparent,
            node_fields: doc
                .field("nodeFields")
                .cloned()
                .unwrap_or(JVal::Arr(vec![])),
        }
    }

    fn members(
        &self,
        v: &JVal,
        fields: &JVal,
        owner: Option<&str>,
        arg: Option<&JVal>,
        out: &mut BTreeSet<String>,
    ) {
        if !matches!(v, JVal::Obj(_)) {
            return;
        }
        for f in arr(Some(fields)) {
            let name = str_field(f, "name").expect("a field name");
            let Some(value) = v.field(&name) else {
                continue;
            };
            if let Some(owner) = owner {
                out.insert(format!("{owner}.{name}"));
            }
            let ty = f.field("type").expect("a field type");
            let ty = match (str_field(ty, "$type").as_deref(), arg) {
                (Some("var"), Some(bound)) => bound,
                _ => ty,
            };
            self.walk(value, ty, out);
        }
    }

    fn walk(&self, v: &JVal, ty: &JVal, out: &mut BTreeSet<String>) {
        let tag = str_field(v, "$type");
        match str_field(ty, "$type").as_deref() {
            Some("node") => {
                if let Some(kind) = v.field("kind") {
                    self.walk(
                        kind,
                        &JVal::Obj(vec![("$type".into(), JVal::Str("kind".into()))]),
                        out,
                    );
                }
                self.members(v, &self.node_fields, None, None, out);
            }
            Some("kind") => {
                if let Some(tag) = tag {
                    let owner = format!("Kind.{tag}");
                    out.insert(owner.clone());
                    if let Some(fields) = self.kinds.get(&tag) {
                        self.members(v, fields, Some(&owner), None, out);
                    }
                }
            }
            Some("op") => {
                if let Some(fields) = tag.and_then(|t| self.ops.get(&t)) {
                    self.members(v, fields, None, None, out);
                }
            }
            Some("union") => {
                let name = str_field(ty, "name").expect("a union type names its union");
                if let Some(tag) = tag {
                    let owner = format!("{name}.{tag}");
                    out.insert(owner.clone());
                    let arg = arr(ty.field("args")).first();
                    if let Some(fields) = self.unions.get(&name).and_then(|cases| cases.get(&tag)) {
                        self.members(v, fields, Some(&owner), arg, out);
                    }
                } else if let Some(case) = self.transparent.get(&name) {
                    out.insert(format!("{name}.{case}"));
                }
            }
            Some("record") => {
                let name = str_field(ty, "name").expect("a record type names its record");
                if let Some(fields) = self.records.get(&name) {
                    self.members(v, fields, Some(&name), None, out);
                }
            }
            // A hosted slot whose wire type the IDL declares is walked through
            // it; one carried as bare JSON is opaque here, as it is to the IDL.
            Some("hosted") => {
                if let Some(wire @ JVal::Obj(_)) = ty.field("wire") {
                    self.walk(v, wire, out);
                }
            }
            Some("list") => {
                if let JVal::Arr(items) = v {
                    for (i, item) in items.iter().enumerate() {
                        let of = ty.field("of").or_else(|| arr(ty.field("items")).get(i));
                        if let Some(of) = of {
                            self.walk(item, of, out);
                        }
                    }
                }
            }
            Some("map") => {
                if let (JVal::Obj(entries), Some(values)) = (v, ty.field("values")) {
                    for (_, item) in entries {
                        self.walk(item, values, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Each token the corpus exercises, with the ids of the round-trip fixtures
/// that carry it.
fn exercised_tokens(corpus: &Path) -> BTreeMap<String, Vec<Fixture>> {
    let vocabulary = Vocabulary::read(&corpus_idl(corpus));
    let mut by_token: BTreeMap<String, Vec<Fixture>> = BTreeMap::new();
    for fixture in load_manifest(corpus) {
        if !EXERCISING_FAMILIES.contains(&fixture.kind.as_str()) {
            continue;
        }
        let expected = fixture
            .expected_file
            .as_deref()
            .unwrap_or(&fixture.input_file);
        let doc = parse(&read_fixture(corpus, expected)).expect("a canonical document parses");
        let mut found = BTreeSet::new();
        let root = JVal::Obj(vec![("$type".into(), JVal::Str(fixture.decoder.clone()))]);
        vocabulary.walk(&doc, &root, &mut found);
        for token in found {
            by_token.entry(token).or_default().push(fixture.clone());
        }
    }
    by_token
}

/// This host decodes a fixture's canonical document and re-encodes it
/// byte-identically.
fn round_trips(corpus: &Path) -> impl Fn(&Fixture) -> bool + '_ {
    move |fixture| {
        let expected = read_fixture(
            corpus,
            fixture
                .expected_file
                .as_deref()
                .unwrap_or(&fixture.input_file),
        );
        let got = if fixture.decoder == "op" {
            decode_op(&expected).map(|op| encode_op(&op))
        } else {
            decode_node(&expected).map(|n| encode_node(&n))
        };
        got.is_ok_and(|g| g == expected)
    }
}

/// Every way the claim and the corpus disagree, one finding per token, in token
/// order.
fn claim_findings(
    claim: &[&str],
    by_token: &BTreeMap<String, Vec<Fixture>>,
    run: impl Fn(&Fixture) -> bool,
) -> Vec<String> {
    let mut verdicts: BTreeMap<String, bool> = BTreeMap::new();
    let mut findings = vec![];
    for (token, fixtures) in by_token {
        if !claim.contains(&token.as_str()) {
            findings.push(format!(
                "{token}: the corpus exercises it ({}) and this host does not claim it",
                fixtures[0].id
            ));
            continue;
        }
        let red: Vec<&str> = fixtures
            .iter()
            .filter(|f| !*verdicts.entry(f.id.clone()).or_insert_with(|| run(f)))
            .map(|f| f.id.as_str())
            .collect();
        if !red.is_empty() {
            findings.push(format!(
                "{token}: claimed, and this host does not round-trip {}",
                red.join(", ")
            ));
        }
    }
    findings
}

#[test]
fn the_claim_reads_back_against_the_corpus() {
    let corpus = common::corpus_root();
    let by_token = exercised_tokens(&corpus);
    let reached: BTreeSet<&str> = by_token.keys().map(|t| token_family(t)).collect();
    assert_eq!(
        reached.into_iter().collect::<Vec<_>>(),
        DERIVED_FAMILIES,
        "the corpus walk must reach every derived family, or the read-back checks nothing there"
    );
    let findings = claim_findings(
        fuaran_rs::wire::CONSTRUCT_TOKENS,
        &by_token,
        round_trips(&corpus),
    );
    assert!(
        findings.is_empty(),
        "the claim and the corpus disagree:\n{}",
        findings.join("\n")
    );
}

#[test]
fn an_omitted_token_the_corpus_exercises_fails_by_name() {
    let corpus = common::corpus_root();
    let claim: Vec<&str> = fuaran_rs::wire::CONSTRUCT_TOKENS
        .iter()
        .copied()
        .filter(|t| *t != "Kind.Badge")
        .collect();
    let findings = claim_findings(&claim, &exercised_tokens(&corpus), round_trips(&corpus));
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0].starts_with("Kind.Badge: the corpus exercises it ("),
        "{findings:?}"
    );
}

#[test]
fn a_claimed_token_this_host_does_not_round_trip_fails_by_name() {
    let corpus = common::corpus_root();
    let by_token = exercised_tokens(&corpus);
    let victim = by_token
        .get("Kind.Badge")
        .and_then(|fs| fs.first())
        .map(|f| f.id.clone())
        .expect("a fixture exercises Kind.Badge, or this falsifier tests nothing");
    let findings = claim_findings(fuaran_rs::wire::CONSTRUCT_TOKENS, &by_token, |f| {
        f.id != victim
    });
    assert!(!findings.is_empty());
    assert!(findings.iter().all(|f| f.contains(&victim)), "{findings:?}");
    assert!(
        findings
            .iter()
            .any(|f| f.starts_with("Kind.Badge: claimed")),
        "{findings:?}"
    );
}

// ── laws/dataframeParity — the evaluator against the reference's answers ────
//
// Phase 1938 closes this evaluator's "no oracle" (1712): the corpus law family
// `dataframeParity` carries the reference evaluator's answers over authored
// (source, pipeline) pairs, with a recorded producer commit. This host has no
// public table codec, so each vector is carried on a DataGrid node whose source
// is a `Transform` binding — the wire position the pair lives in — and read back
// through the node codec.

fn grid_json(source: &str, pipeline: &str) -> String {
    format!(
        r#"{{"id":"parity","kind":{{"$type":"DataGrid","columns":[],"editable":false,"rowKey":"<closure>","source":{{"$type":"Transform","pipeline":{pipeline},"source":{source}}}}}}}"#
    )
}

/// The canonical bytes at `kind.source.<member>` of an encoded parity grid.
fn grid_member(encoded: &str, member: &str) -> String {
    let doc = parse(encoded).expect("the encoded grid parses");
    let value = doc
        .field("kind")
        .and_then(|k| k.field("source"))
        .and_then(|s| s.field(member))
        .unwrap_or_else(|| panic!("the encoded grid carries no kind.source.{member}"));
    fuaran_rs::canonical::render_canonical(value)
}

#[test]
fn the_evaluator_matches_the_reference_on_every_dataframe_parity_vector() {
    use fuaran_rs::transform::{Table, eval_pipeline};
    use fuaran_rs::wire::{Binding, DataSource, NodeKind, TransformSource};

    let laws = common::corpus_root().join("laws");
    let manifest = parse(&read_fixture(&laws, "manifest.json")).expect("laws/manifest.json parses");
    let Some(JVal::Arr(families)) = manifest.field("families") else {
        panic!("laws/manifest.json declares no families");
    };
    let file = families
        .iter()
        .find(|f| str_field(f, "id").as_deref() == Some("dataframeParity"))
        .and_then(|f| str_field(f, "file"))
        .expect("laws/manifest.json declares the dataframeParity family");
    let doc = parse(&read_fixture(&laws, &file)).expect("the parity vectors parse");
    let Some(JVal::Arr(vectors)) = doc.field("vectors") else {
        panic!("laws/{file} carries no vectors");
    };
    assert!(!vectors.is_empty(), "laws/{file} carries no vectors");

    let mut failures: Vec<String> = vec![];
    for v in vectors {
        let id = str_field(v, "id").expect("vector id");
        let input = v.field("input").expect("vector input");
        let expected = v.field("expected").expect("vector expected");
        let (source, pipeline) = (
            str_field(input, "source").expect("input.source"),
            str_field(input, "pipeline").expect("input.pipeline"),
        );
        let node = match decode_node(&grid_json(&source, &pipeline)) {
            Ok(n) => n,
            Err(e) => {
                failures.push(format!("{id}: the parity grid did not decode: {e:?}"));
                continue;
            }
        };
        // Legs 1 and 2 — both inputs round-trip byte-exact through this host's codec.
        let encoded = encode_node(&node);
        for (member, input) in [("source", &source), ("pipeline", &pipeline)] {
            let got = grid_member(&encoded, member);
            if &got != input {
                failures.push(format!(
                    "{id}: {member} round-trip\n  got  {got}\n  want {input}"
                ));
            }
        }
        // Leg 3 — evaluation agrees with the reference's answer.
        let NodeKind::DataGrid(spec) = &node.kind else {
            panic!("{id}: the parity grid decoded to another kind");
        };
        let Binding::Transform {
            pipeline: steps,
            source: TransformSource::Data(DataSource::Embedded { schema, columns }),
            ..
        } = &spec.source
        else {
            panic!("{id}: the parity grid's source is not an embedded Transform");
        };
        let table = Table {
            schema: schema.clone(),
            columns: columns.clone(),
        };
        let verdict = str_field(expected, "verdict").expect("expected.verdict");
        match (eval_pipeline(steps, &table), verdict.as_str()) {
            (Err(_), "ok") => {
                failures.push(format!("{id}: the reference evaluated; this host refused"))
            }
            (Ok(_), "error") => {
                failures.push(format!("{id}: the reference refused; this host evaluated"))
            }
            (Err(_), _) => {}
            (Ok(out), _) => {
                let mut carrier = node.clone();
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
                let got = grid_member(&encode_node(&carrier), "source");
                let want = str_field(expected, "table").expect("expected.table");
                if got != want {
                    failures.push(format!(
                        "{id}: evaluator ≠ reference\n  got  {got}\n  want {want}"
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} dataframeParity vectors failed:\n{}",
        failures.len(),
        vectors.len(),
        failures.join("\n")
    );
    eprintln!(
        "dataframeParity leg: {} vectors agree with the reference",
        vectors.len()
    );
}
