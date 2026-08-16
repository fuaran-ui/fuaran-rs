//! The shared `sanitization/` corpus family, run against this host's URL floor.
//!
//! Unlike every other corpus family this one is **not** byte-parity: the markup a
//! host wraps around a URL differs legitimately between this renderer, a
//! static-HTML emitter and a React client, so comparing those bytes would pin
//! accidents rather than the contract. Each case states an **invariant** instead —
//! `reject` (refuse it) or `accept` (take it, and emit the normalised form) — plus
//! the reason the URL parser gives, which is what makes the case meaningful.
//!
//! The corpus verifies its own `reason` claims against a real WHATWG parser
//! (`sanitization/verify-against-url-parser.mjs`); this test verifies that *this*
//! host agrees with the resulting invariants.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::sanitize::{sanitize_url, sanitize_url_or_blank};

/// Walks up from the crate directory looking for the shared corpus (a sibling
/// checkout). `None` keeps the repo standalone-testable — the leg skips, not fails.
fn find_sanitization_manifest() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let candidate = dir
            .join("wire-format-fixtures")
            .join("sanitization")
            .join("manifest.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn str_field(fields: &JVal, key: &str) -> Option<String> {
    match fields.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

#[test]
fn sanitization_corpus_url_floor() {
    let Some(path) = find_sanitization_manifest() else {
        eprintln!("wire-format-fixtures/sanitization not found; skipping (standalone checkout)");
        return;
    };
    let raw = std::fs::read_to_string(&path).expect("read sanitization manifest");
    let doc = parse(&raw).expect("parse sanitization manifest");
    let Some(JVal::Arr(groups)) = doc.field("groups") else {
        panic!("sanitization manifest has no `groups` array");
    };

    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for group in groups {
        let Some(JVal::Arr(cases)) = group.field("cases") else {
            continue;
        };
        for case in cases {
            checked += 1;
            let id = str_field(case, "id").unwrap_or_else(|| "<unnamed>".into());
            let input = str_field(case, "input").unwrap_or_default();
            let invariant = str_field(case, "invariant").unwrap_or_default();
            let expected = str_field(case, "expected");

            match (invariant.as_str(), sanitize_url(&input)) {
                ("reject", Some(got)) => {
                    failures.push(format!("{id}: expected REJECT, got {got:?}"))
                }
                ("reject", None) => {
                    // §19 rule 6 — the or-blank variant substitutes about:blank.
                    let blank = sanitize_url_or_blank(&input);
                    if blank != "about:blank" {
                        failures.push(format!(
                            "{id}: rejected, but sanitize_url_or_blank gave {blank:?}"
                        ));
                    }
                }
                ("accept", None) => failures.push(format!("{id}: expected ACCEPT, was rejected")),
                ("accept", Some(got)) => {
                    if let Some(want) = expected
                        && want != got
                    {
                        failures.push(format!("{id}: expected {want:?}, got {got:?}"));
                    }
                }
                (other, _) => failures.push(format!("{id}: unknown invariant {other:?}")),
            }
        }
    }

    // Printed so the scanned count is visible under `--nocapture`: a loader that
    // silently parsed zero cases would otherwise read exactly as green as one that
    // ran them all.
    println!("sanitization/url-floor: {checked} cases");
    assert!(
        checked > 0,
        "sanitization manifest parsed but held no cases"
    );
    assert!(
        failures.is_empty(),
        "url-floor invariants violated:\n  {}",
        failures.join("\n  ")
    );
}
