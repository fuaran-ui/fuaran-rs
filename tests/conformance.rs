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
//! # Absence is two different facts, and only one of them is a skip
//!
//! Until Phase 1664 an absent corpus made every leg here return, so "the oracle
//! is not here" and "the oracle is here and every family conforms" reported the
//! same `ok`. That is defensible in a standalone clone of this repository, which
//! genuinely cannot run the suite — and indefensible in the cross-host
//! workspace, where an absent corpus means the whole conformance claim has been
//! switched off and nobody was told. **A conformance check that passes without
//! its oracle is worse than no check**, because it reports the same green as one
//! that ran.
//!
//! So the two are separated, by the same discriminator `tests/render.rs` uses
//! for the reference host and `tests/program_wire_canonical.rs` uses for the
//! program corpus: **any sibling host present ⇒ hard failure** naming what
//! proved the shape; **nothing else present ⇒ the honest standalone NOT RUN**,
//! which is what the public workflow reports (it checks out this repository and
//! the wire corpus, and no sibling host). And a corpus that is CLAIMED —
//! `FUARAN_WIRE_FIXTURES` names one — is a hard failure from there on whatever
//! else is around it: an operator who named a path has said the oracle exists,
//! so a path that is wrong is a mistake to report rather than a state to
//! tolerate.
//!
//! [`classify_corpus`] is that decision as a pure function of a starting
//! directory, which is what makes it testable — see
//! `the_two_absences_are_told_apart`. It has to be, because this is the kind of
//! check that passes by doing nothing.

use std::path::{Path, PathBuf};

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::elicitation::{
    decode_answer_doc, decode_elicitation, decode_outcome, encode_elicitation, encode_outcome,
};
use fuaran_rs::envelope::{decode_envelope, encode_envelope};
use fuaran_rs::wire::{decode_node, decode_op, encode_node, encode_op};

/// The environment variable an operator names the corpus root with, matching the
/// estate's own spelling. A named path is CLAIMED: see [`Corpus::Declared`].
const CORPUS_ENV: &str = "FUARAN_WIRE_FIXTURES";

/// Sibling hosts whose presence proves this is a cross-host workspace checkout
/// rather than a standalone clone. Deliberately excludes this host.
///
/// The same list, for the same purpose, as `tests/render.rs`'s
/// `OTHER_HOST_NAMES` plus the reference host it names separately: each
/// integration test is its own crate, so the constant cannot be shared without
/// publishing it from the library, and publishing a list of sibling repository
/// names out of a crate is not a thing this host should do to spare a
/// duplication of seven strings.
const WORKSPACE_SIBLING_HOSTS: &[&str] = &[
    "fuaran-dotnet",
    "fuaran",
    "fuaran-ts",
    "fuaran-py",
    "fuaran-go",
    "fuaran-kt",
    "fuaran-swift",
];

/// What a corpus lookup found, with the two absences told apart.
#[derive(Debug, PartialEq, Eq)]
enum Corpus {
    /// An operator named it. Anything wrong with it from here is a hard failure.
    Declared(PathBuf),
    /// Found beside this repository.
    Discovered(PathBuf),
    /// Nothing claimed and nothing found, in a checkout that is plainly the
    /// cross-host workspace — so the oracle has been silently disabled rather
    /// than legitimately absent. Carries the sibling that proves the shape.
    MissingInWorkspace(String),
    /// Nothing claimed, nothing found, and nothing else here either — a genuine
    /// standalone clone of this repository.
    Absent,
}

/// The classification, as a pure function of where the walk starts. Split out
/// from [`locate_corpus`] so it can be exercised against directories built for
/// the purpose rather than only against whatever this machine happens to hold.
fn classify_corpus(start: &Path, declared: Option<&str>) -> Corpus {
    if let Some(declared) = declared
        && !declared.trim().is_empty()
    {
        return Corpus::Declared(PathBuf::from(declared));
    }
    let mut dir = start.to_path_buf();
    loop {
        let root = dir.join("wire-format-fixtures");
        if root.join("manifest.json").is_file() {
            return Corpus::Discovered(root);
        }
        if !dir.pop() {
            break;
        }
    }
    // Not found anywhere up the tree. Is this a standalone clone, or a workspace
    // checkout whose corpus is missing? The two look identical from inside this
    // function and are opposite facts: one is a repository that legitimately
    // cannot run these legs, the other is an oracle that has been silently
    // switched off. The discriminator is chosen so the PUBLIC workflow — which
    // checks out this repository and the wire corpus and no sibling host —
    // stays honestly NOT RUN.
    let mut dir = start.to_path_buf();
    loop {
        for sibling in WORKSPACE_SIBLING_HOSTS {
            if dir.join(sibling).is_dir() {
                return Corpus::MissingInWorkspace(format!(
                    "{}/ is present under {}",
                    sibling,
                    dir.display()
                ));
            }
        }
        if !dir.pop() {
            break;
        }
    }
    Corpus::Absent
}

fn locate_corpus() -> Corpus {
    classify_corpus(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        std::env::var(CORPUS_ENV).ok().as_deref(),
    )
}

/// The shared corpus root, or `None` for the one absence that is honestly a
/// skip. Every other absence PANICS here rather than returning.
///
/// A located corpus must also be READABLE as one: a `manifest.json` that is not
/// there separates "this is the wrong corpus" from "this corpus is missing a
/// fixture", and the two have different remedies. Without it, an operator whose
/// `FUARAN_WIRE_FIXTURES` points one directory sideways gets acceptance followed
/// by a confident complaint about absent fixtures — loud, and wrong about the
/// cause.
fn find_corpus() -> Option<PathBuf> {
    let (root, claimed) = match locate_corpus() {
        Corpus::Declared(p) => (p, true),
        Corpus::Discovered(p) => (p, false),
        Corpus::MissingInWorkspace(evidence) => panic!(
            "the wire-format corpus is neither claimed nor present, but this is a cross-host \
             workspace checkout ({evidence}) — so this conformance leg has been silently \
             disabled rather than legitimately skipped, and a missing oracle reports the same \
             green as a run. Clone `wire-format-fixtures` beside this repository, or name it \
             with {CORPUS_ENV}. (A standalone clone of this repository alone still reports NOT \
             RUN and asserts nothing.)"
        ),
        Corpus::Absent => {
            eprintln!(
                "the wire-format corpus is neither claimed nor present beside this repository, \
                 and no sibling host is present either; this leg asserted nothing. Set \
                 {CORPUS_ENV} to run it."
            );
            return None;
        }
    };
    assert!(
        root.join("manifest.json").is_file(),
        "the corpus is {} at '{}' but it holds no manifest.json, which is its authoritative \
         fixture enumeration. A conformance check that passes without its oracle is worse than \
         no check, so this is a failure rather than a skip.",
        if claimed { "claimed" } else { "present" },
        root.display()
    );
    Some(root)
}

/// The go-red for everything above. A guard whose whole job is to fail is
/// exactly the kind of code that quietly stops working, so each verdict is
/// produced here from a directory built to produce it — including the two that
/// cannot be produced on a machine holding a real corpus.
#[test]
fn the_two_absences_are_told_apart() {
    let scratch = std::env::temp_dir().join(format!(
        "fuaran-rs-corpus-classify-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let standalone = scratch.join("standalone").join("fuaran-rs");
    let workspace = scratch.join("workspace").join("fuaran-rs");
    std::fs::create_dir_all(&standalone).expect("scratch standalone tree");
    std::fs::create_dir_all(workspace.parent().unwrap().join("fuaran-ts"))
        .expect("scratch sibling host");
    std::fs::create_dir_all(&workspace).expect("scratch workspace tree");

    // Nothing named, nothing found, nothing else there: the honest NOT RUN.
    assert_eq!(classify_corpus(&standalone, None), Corpus::Absent);

    // Nothing named, nothing found, but a sibling host proves the shape.
    let missing = classify_corpus(&workspace, None);
    match &missing {
        Corpus::MissingInWorkspace(evidence) => {
            assert!(
                evidence.contains("fuaran-ts"),
                "the evidence names what proved the shape: {evidence}"
            );
        }
        other => panic!("a workspace checkout with no corpus must not read as a skip: {other:?}"),
    }

    // A claimed path is claimed whatever surrounds it — including a claim that
    // is wrong, which is the case an operator most needs told.
    assert_eq!(
        classify_corpus(&standalone, Some("/no/such/corpus")),
        Corpus::Declared(PathBuf::from("/no/such/corpus"))
    );
    // An empty or whitespace claim is not a claim.
    assert_eq!(classify_corpus(&standalone, Some("   ")), Corpus::Absent);

    // The discovery arm still discovers, so the change did not disable the
    // ordinary path in the act of hardening the absences.
    let discoverable = scratch.join("found").join("fuaran-rs");
    let corpus = scratch.join("found").join("wire-format-fixtures");
    std::fs::create_dir_all(&discoverable).expect("scratch discoverable tree");
    std::fs::create_dir_all(&corpus).expect("scratch corpus dir");
    std::fs::write(corpus.join("manifest.json"), b"{}").expect("scratch manifest");
    assert_eq!(
        classify_corpus(&discoverable, None),
        Corpus::Discovered(corpus)
    );

    let _ = std::fs::remove_dir_all(&scratch);
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
        // `find_corpus` PANICS on every absence but the standalone one, and
        // prints that one's account itself — so this arm is reached only where
        // asserting nothing is the honest answer.
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
