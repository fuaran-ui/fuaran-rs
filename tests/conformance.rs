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
//! declared family is now covered; when the corpus is absent (standalone
//! checkout) every leg skips.

use std::path::{Path, PathBuf};

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::elicitation::{
    decode_answer_doc, decode_elicitation, decode_outcome, encode_elicitation, encode_outcome,
};
use fuaran_rs::envelope::{decode_envelope, encode_envelope};
use fuaran_rs::wire::{decode_node, decode_op, encode_node, encode_op};

/// Walks up from the crate directory looking for the shared corpus (a sibling
/// checkout). `None` keeps the repo standalone-testable — legs skip, not fail.
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

/// Phase 548 kind-set attestation: the emittable NodeKind vocabulary
/// (`CANONICAL_NODE_KINDS`) must equal the generated manifest `kinds` enumeration.
/// A vocabulary commit that skips this host fails here with a *named* missing kind
/// ("rust decoder lacks Drawing"), so the drift class dies at the host's next test
/// run rather than at a later audit.
#[test]
fn node_kind_set_matches_manifest() {
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let raw = std::fs::read_to_string(corpus.join("manifest.json")).expect("reading manifest");
    let manifest = parse(&raw).expect("manifest.json parses with the host's own JSON layer");
    let Some(JVal::Arr(entries)) = manifest.field("kinds") else {
        panic!(
            "manifest.json declares no 'kinds' array — regenerate the corpus with --emit-corpus"
        );
    };

    let manifest_kinds: std::collections::BTreeSet<String> = entries
        .iter()
        .filter_map(|e| match e {
            JVal::Str(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let decoder_kinds: std::collections::BTreeSet<String> = fuaran_rs::wire::CANONICAL_NODE_KINDS
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let missing: Vec<&String> = manifest_kinds.difference(&decoder_kinds).collect();
    let extra: Vec<&String> = decoder_kinds.difference(&manifest_kinds).collect();

    assert!(
        missing.is_empty(),
        "manifest kinds the rust decoder lacks (add the NodeKind variant + CANONICAL_NODE_KINDS entry): {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "rust decoder kinds the manifest omits (regenerate the corpus with --emit-corpus): {extra:?}"
    );
}

/// Phase 746 control-vocabulary attestation — the `node_kind_set_matches_manifest`
/// twin over `FormFieldKind`. The kind-set pin only ever covered NodeKind, so a
/// control-vocabulary commit that skipped this host stayed silent until a fixture
/// happened to exercise it; this leg names the missing case ("rust decoder lacks
/// DateRange") at the host's next test run instead.
#[test]
fn form_field_kind_set_matches_manifest() {
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let raw = std::fs::read_to_string(corpus.join("manifest.json")).expect("reading manifest");
    let manifest = parse(&raw).expect("manifest.json parses with the host's own JSON layer");
    let Some(JVal::Arr(entries)) = manifest.field("formFieldKinds") else {
        panic!(
            "manifest.json declares no 'formFieldKinds' array — regenerate the corpus with --emit-corpus"
        );
    };

    let manifest_kinds: std::collections::BTreeSet<String> = entries
        .iter()
        .filter_map(|e| match e {
            JVal::Str(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let decoder_kinds: std::collections::BTreeSet<String> =
        fuaran_rs::wire::CANONICAL_FORM_FIELD_KINDS
            .iter()
            .map(|s| (*s).to_string())
            .collect();

    let missing: Vec<&String> = manifest_kinds.difference(&decoder_kinds).collect();
    let extra: Vec<&String> = decoder_kinds.difference(&manifest_kinds).collect();

    assert!(
        missing.is_empty(),
        "manifest form-field kinds the rust decoder lacks (add the FormFieldKind variant + CANONICAL_FORM_FIELD_KINDS entry): {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "rust decoder form-field kinds the manifest omits (regenerate the corpus with --emit-corpus): {extra:?}"
    );
}

/// The FormFieldKind carrier rule (WIRE_FORMAT §11.2): a control discriminator is
/// reached through the PARENT node kind's `$type` — `"Form"` → `fields`,
/// `"Filters"` → `items` — never by property name. `DataGrid.columns[].kind.$type`
/// is a `CellKindErased` sharing spellings (`Text`, `Date`, `Checkbox`) with the
/// control vocabulary, so a property-name heuristic would attest the wrong family
/// and report green.
fn collect_control_kinds(raw: &JVal, controls: &mut std::collections::BTreeSet<String>) {
    let Some(kind) = raw.field("kind") else {
        return;
    };
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
    // Recurse the node-bearing positions a control carrier can nest under.
    for slot in ["child", "fallback", "default", "body"] {
        if let Some(inner) = kind.field(slot) {
            collect_control_kinds(inner, controls);
        }
    }
    if let Some(JVal::Arr(children)) = kind.field("children") {
        for c in children {
            collect_control_kinds(c, controls);
        }
    }
    if let Some(JVal::Arr(cases)) = kind.field("cases") {
        for c in cases {
            if let Some(inner) = c.field("child") {
                collect_control_kinds(inner, controls);
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
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
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
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
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
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
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
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
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
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
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
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
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
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
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
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
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

/// Names the corpus families this host does not yet run, so the skip is
/// explicit rather than silent (§15/§18 covered as of Phase 553;
/// lenient-accept covered above).
#[test]
fn corpus_families_beyond_the_floor_are_explicitly_skipped() {
    let Some(corpus) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let covered = [
        "node-round-trip",
        "op-round-trip",
        "reject",
        "envelope-round-trip",
        "envelope-reject",
        "elicitation-round-trip",
        "elicitation-reject",
        "elicitation-answer-accept",
        "elicitation-answer-reject",
        "lenient-accept",
    ];
    let mut skipped: std::collections::BTreeMap<String, usize> = Default::default();
    for fixture in load_manifest(&corpus) {
        if !covered.contains(&fixture.kind.as_str()) {
            *skipped.entry(fixture.kind.clone()).or_insert(0) += 1;
        }
    }
    for (kind, count) in &skipped {
        eprintln!("skipped family (beyond the codec floor): {kind} × {count}");
    }
}
