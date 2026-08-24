//! Certifies the deterministic markdown renderer against the shared
//! cross-host corpus (`wire-format-fixtures/markdown/corpus.json`): every
//! fixture's render must equal its pinned `html` byte-for-byte.
//! Skips cleanly when the corpus is absent (standalone checkout).
//!
//! A fixture MAY carry an optional `policy` naming the destination policy the
//! render is performed under (`WIRE_FORMAT.md` §14.1). The corpus never carries
//! a policy as DATA — a policy an artefact can supply is one a hostile artefact
//! can widen — so the name is mapped to a policy this host CONSTRUCTS, and an
//! unrecognised name **fails** rather than falling back to permissive: a silent
//! fallback turns a fixture the host cannot evaluate into one it appears to
//! pass.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::egress::{
    EgressClass, EgressOrigin, EgressPolicy, deny_non_local_egress, permissive_egress,
};
use fuaran_rs::render::markdown::{to_html, to_html_with_egress};

fn find_corpus() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let file = dir
            .join("wire-format-fixtures")
            .join("markdown")
            .join("corpus.json");
        if file.is_file() {
            return Some(file);
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

/// `denyNonLocal`, plus exact host `cdn.example` scoped to **media** and host
/// suffix `docs.example` scoped to **hyperlink** (§14.1's conformance table).
/// This is what makes the gate falsifiable in both directions: a host that
/// refused every non-local destination unconditionally fails its allowed
/// fixtures, and one that ignored the policy fails the `denyNonLocal` ones.
fn declared_example() -> EgressPolicy {
    deny_non_local_egress()
        .allow_origin(
            EgressOrigin::ExactHost("cdn.example".to_string()),
            &[EgressClass::Media],
        )
        .allow_origin(
            EgressOrigin::HostSuffix("docs.example".to_string()),
            &[EgressClass::Hyperlink],
        )
}

/// The policy a fixture's `policy` name denotes, or `Err` naming one this host
/// does not construct. Never a fallback.
fn policy_for(name: Option<&str>) -> Result<EgressPolicy, String> {
    match name {
        None | Some("permissive") => Ok(permissive_egress()),
        Some("denyNonLocal") => Ok(deny_non_local_egress()),
        Some("declaredExample") => Ok(declared_example()),
        Some(other) => Err(format!(
            "unknown fixture policy {other:?} — this host constructs \
             permissive / denyNonLocal / declaredExample. Refusing to fall back \
             to permissive: that would report an unevaluated fixture as passing."
        )),
    }
}

/// The no-silent-fallback rule, pinned. A corpus that grows a policy name this
/// host does not construct must FAIL the gate; the failure mode this guards
/// against is a fixture that is never evaluated being reported as passing.
#[test]
fn an_unrecognised_policy_name_is_refused_not_defaulted() {
    assert!(policy_for(Some("someFuturePolicy")).is_err());
    assert!(policy_for(Some("PERMISSIVE")).is_err(), "names are exact");
    assert_eq!(policy_for(None).unwrap(), permissive_egress());
    assert_eq!(policy_for(Some("permissive")).unwrap(), permissive_egress());
    assert_eq!(
        policy_for(Some("denyNonLocal")).unwrap(),
        deny_non_local_egress()
    );
    assert_eq!(
        policy_for(Some("declaredExample")).unwrap(),
        declared_example()
    );
}

#[test]
fn markdown_corpus_renders_byte_identical() {
    let Some(path) = find_corpus() else {
        eprintln!("markdown corpus not found; skipping (standalone checkout)");
        return;
    };
    let raw = std::fs::read_to_string(&path).expect("reading markdown corpus");
    let corpus = parse(&raw).expect("markdown corpus parses");
    let Some(JVal::Arr(fixtures)) = corpus.field("fixtures") else {
        panic!("markdown corpus declares no fixtures array");
    };
    let mut failures: Vec<String> = Vec::new();
    let mut non_permissive = 0usize;
    for fixture in fixtures {
        let id = str_field(fixture, "id").expect("fixture id");
        let source = str_field(fixture, "source").expect("fixture source");
        let expected = str_field(fixture, "html").expect("fixture html");
        let named = str_field(fixture, "policy");
        let policy = match policy_for(named.as_deref()) {
            Ok(p) => p,
            Err(why) => {
                failures.push(format!("{id}: {why}"));
                continue;
            }
        };
        if named.as_deref().is_some_and(|n| n != "permissive") {
            non_permissive += 1;
        }
        let actual = to_html_with_egress(&policy, &source);
        if actual != expected {
            failures.push(format!(
                "{id} (policy {}):\n  source   {source:?}\n  expected {expected:?}\n  actual   {actual:?}",
                named.as_deref().unwrap_or("permissive")
            ));
            continue;
        }
        // The pure form IS the permissive case — asserted, not merely intended.
        if named.as_deref().is_none_or(|n| n == "permissive") {
            let pure = to_html(&source);
            if pure != expected {
                failures.push(format!(
                    "{id}: to_html diverged from to_html_with_egress(permissive)\n  \
                     expected {expected:?}\n  actual   {pure:?}"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} markdown fixtures diverged:\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n")
    );
    // Without this the whole gate could run on the permissive path and stay
    // green on a host that never implemented §14.1 at all.
    assert!(
        non_permissive > 0,
        "the markdown corpus carries no non-permissive `policy` fixture — the \
         destination-policy leg of this gate is vacuous"
    );
    eprintln!(
        "markdown corpus: {} fixtures byte-identical ({non_permissive} under a \
         non-permissive destination policy)",
        fixtures.len()
    );
}
