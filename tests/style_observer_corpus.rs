//! This host's certification against the shared `style-observer/` corpus family
//! (Phase 1752).
//!
//! `tests/theme.rs` beside this file carries the style-observer cases that Phase
//! 1724 PORTED from the Python host by hand. That port is what made the
//! byte-identical `StyleFlag` / `StyleObservation` encode a cross-host claim at
//! all — and it is also why the claim could not be checked: four hosts had each
//! written the same literals into their own test files, and four suites that
//! agree are not an oracle. A fifth host has nothing to certify against but the
//! other four's tests, and a regression introduced in all four at once — by the
//! port that created them — is invisible from every one of them.
//!
//! So the cases live in the corpus now, emitted by the reference host, and this
//! reads them. The literals in `tests/theme.rs` stay exactly where they are and
//! their role has changed rather than ended: written by hand, they are the
//! go-red partner of a family a generator writes.
//!
//! **Not checked is not passed.** A tier outside this host's vocabulary PANICS
//! by name with the vector id rather than being skipped, and the vacuity guard
//! refuses a run in which the family turned out to be empty — "every vector
//! matched" and "no vector was loaded" are the same green.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::theme::{
    FontRole, NodeArea, Rgba, StyleFlag, StyleInput, StyleObservation, StyleObserverOptions,
    decode, derive_style_flags, encode_style_flag, encode_style_observation, per_node_flags,
    to_style_observation, verify_usage_budgets,
};

/// Walk up from the crate dir to the shared corpus (mirrors conformance.rs).
fn find_corpus() -> Option<PathBuf> {
    if let Ok(declared) = std::env::var("FUARAN_WIRE_FIXTURES") {
        if !declared.is_empty() {
            let root = PathBuf::from(declared);
            assert!(
                root.join("manifest.json").is_file(),
                "FUARAN_WIRE_FIXTURES names '{}', which holds no manifest.json. It is refused rather \
                 than ignored: falling back to the walk would certify against a corpus nobody named.",
                root.display()
            );
            return Some(root);
        }
    }
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

fn field<'a>(v: &'a JVal, key: &str) -> &'a JVal {
    v.field(key)
        .unwrap_or_else(|| panic!("the vector has no `{key}` member"))
}

fn as_str(v: &JVal) -> &str {
    match v {
        JVal::Str(s) => s.as_str(),
        other => panic!("expected a string, found {other:?}"),
    }
}

fn as_num(v: &JVal) -> f64 {
    match v {
        JVal::Num(n) => *n,
        other => panic!("expected a number, found {other:?}"),
    }
}

fn as_arr(v: &JVal) -> &[JVal] {
    match v {
        JVal::Arr(items) => items.as_slice(),
        other => panic!("expected an array, found {other:?}"),
    }
}

/// `null` reads as absent; anything else must be a string. An absent fact never
/// fires a flag, so collapsing the two would silently change what a vector says.
fn opt_str(v: &JVal) -> Option<String> {
    match v {
        JVal::Null => None,
        JVal::Str(s) => Some(s.clone()),
        other => panic!("expected a string or null, found {other:?}"),
    }
}

fn rgba(v: &JVal) -> Rgba {
    Rgba {
        r: as_num(field(v, "r")),
        g: as_num(field(v, "g")),
        b: as_num(field(v, "b")),
        a: as_num(field(v, "a")),
    }
}

fn options(v: &JVal) -> StyleObserverOptions {
    StyleObserverOptions {
        contrast_aa_threshold: as_num(field(v, "contrastAaThreshold")),
        invisible_text_threshold: as_num(field(v, "invisibleTextThreshold")),
        accent_indistinct_threshold: as_num(field(v, "accentIndistinctThreshold")),
        ..StyleObserverOptions::default()
    }
}

fn style_input(v: &JVal) -> StyleInput {
    StyleInput {
        foreground: rgba(field(v, "foreground")),
        background_layers: as_arr(field(v, "backgroundLayers"))
            .iter()
            .map(rgba)
            .collect(),
        font_family: opt_str(field(v, "fontFamily")),
        emitted_tone: opt_str(field(v, "emittedTone")),
    }
}

fn font_role(wire: &str) -> FontRole {
    match wire {
        "Unknown" => FontRole::Unknown,
        "SansSerif" => FontRole::SansSerif,
        "Serif" => FontRole::Serif,
        "Monospace" => FontRole::Monospace,
        // Reported by name: a role this host cannot spell is a gap in the host,
        // and defaulting it to Unknown would hide exactly that.
        other => panic!("font role {other:?} is outside this host's vocabulary"),
    }
}

/// A snapshot the fixture spells out. The manifest-aware tiers take an
/// already-derived observation, so its flag list is deliberately empty — those
/// arms read only the tone, the effective background and the contrast ratio.
fn observation(v: &JVal) -> StyleObservation {
    StyleObservation {
        node_id: as_str(field(v, "nodeId")).to_string(),
        foreground: rgba(field(v, "foreground")),
        effective_background: rgba(field(v, "effectiveBackground")),
        font_role: font_role(as_str(field(v, "fontRole"))),
        emitted_tone: opt_str(field(v, "emittedTone")),
        contrast_ratio: as_num(field(v, "contrastRatio")),
        flags: Vec::new(),
    }
}

fn encoded(flags: &[StyleFlag]) -> Vec<String> {
    flags.iter().map(encode_style_flag).collect()
}

fn expected(v: &JVal, key: &str) -> Vec<String> {
    as_arr(field(v, key))
        .iter()
        .map(|item| as_str(item).to_string())
        .collect()
}

/// Every `style-observer` vector the corpus manifest lists, parsed.
fn vectors() -> Option<Vec<(String, JVal)>> {
    let corpus = find_corpus()?;
    let manifest = parse(
        &std::fs::read_to_string(corpus.join("manifest.json")).expect("reading manifest.json"),
    )
    .expect("parsing manifest.json");

    let mut out = Vec::new();
    for fixture in as_arr(field(&manifest, "fixtures")) {
        if fixture.field("kind").map(as_str) != Some("style-observer") {
            continue;
        }
        let id = as_str(field(fixture, "id")).to_string();
        let input_file = as_str(field(fixture, "inputFile"));
        let text = std::fs::read_to_string(corpus.join(input_file))
            .unwrap_or_else(|e| panic!("{id}: manifest.json lists {input_file}, unreadable: {e}"));
        out.push((
            id.clone(),
            parse(&text).unwrap_or_else(|e| panic!("{id}: {e:?}")),
        ));
    }

    // The measurement must not be vacuous: an iteration over an empty list
    // reports total success having compared nothing at all.
    assert!(
        !out.is_empty(),
        "manifest.json at {} lists no style-observer fixtures — this suite would have certified \
         nothing while reporting success",
        corpus.display()
    );
    Some(out)
}

#[test]
fn every_style_observer_vector_matches_this_host_byte_for_byte() {
    let Some(cases) = vectors() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };

    for (id, case) in &cases {
        match as_str(field(case, "tier")) {
            "observation" => {
                let opts = options(field(case, "options"));
                let input = style_input(field(case, "input"));

                assert_eq!(
                    encoded(&derive_style_flags(&opts, &input)),
                    expected(case, "expectedFlags"),
                    "{id}: the derived flags do not encode to the family's bytes"
                );
                assert_eq!(
                    encode_style_observation(&to_style_observation(
                        &opts,
                        as_str(field(case, "nodeId")),
                        &input
                    )),
                    as_str(field(case, "expectedObservation")),
                    "{id}: the encoded observation is not byte-identical to the family's"
                );
            }
            "per-node-manifest" => {
                let manifest = decode(as_str(field(case, "manifest")))
                    .unwrap_or_else(|| panic!("{id}: the vector's manifest does not decode here"));
                assert_eq!(
                    encoded(&per_node_flags(
                        &manifest,
                        &observation(field(case, "observation"))
                    )),
                    expected(case, "expectedManifestFlags"),
                    "{id}: the manifest-aware per-node flags do not encode to the family's bytes"
                );
            }
            "usage-budget" => {
                let manifest = decode(as_str(field(case, "manifest")))
                    .unwrap_or_else(|| panic!("{id}: the vector's manifest does not decode here"));
                let nodes: Vec<NodeArea> = as_arr(field(case, "nodeAreas"))
                    .iter()
                    .map(|entry| NodeArea {
                        obs: observation(field(entry, "observation")),
                        area: as_num(field(entry, "area")),
                    })
                    .collect();
                assert_eq!(
                    encoded(&verify_usage_budgets(&manifest, &nodes)),
                    expected(case, "expectedBudgetFlags"),
                    "{id}: the usage-budget flags do not encode to the family's bytes"
                );
            }
            other => panic!("{id}: tier {other:?} is outside this host's vocabulary"),
        }
    }
}

/// The proof that the comparison above can fail. A byte comparison falls
/// silently into certifying nothing — an absent corpus, an empty enumeration, a
/// skipped list — and every one of those looks exactly like a pass.
#[test]
fn a_single_flipped_expected_byte_is_caught() {
    let Some(cases) = vectors() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };

    let (id, case) = cases
        .iter()
        .find(|(_, c)| as_str(field(c, "tier")) == "observation")
        .expect("no observation-tier vector to perturb — the probe measured nothing");

    let genuine = as_str(field(case, "expectedObservation"));
    let produced = encode_style_observation(&to_style_observation(
        &options(field(case, "options")),
        as_str(field(case, "nodeId")),
        &style_input(field(case, "input")),
    ));
    assert_eq!(
        produced, genuine,
        "{id}: the unperturbed vector should still pass"
    );

    let perturbed = format!("{}X}}", &genuine[..genuine.len() - 2]);
    assert_ne!(
        perturbed, genuine,
        "the perturbation changed nothing, so this test proves nothing — the probe, not the \
         subject, is what failed"
    );
    assert_ne!(
        produced, perturbed,
        "a flipped expectation byte compared equal to what this host produces"
    );
}
