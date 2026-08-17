//! The shared `sanitization/` corpus family, run against this host's render-time
//! safety floor (`WIRE_FORMAT.md` §22; §19 for the URL group).
//!
//! Unlike every other corpus family this one is **not** byte-parity: the markup a
//! host wraps around a payload differs legitimately between hosts, so comparing
//! those bytes would pin accidents rather than the contract. Each case states an
//! **invariant** instead — `reject`, `accept` or `inert` — and this test asserts
//! that *this* host satisfies it.
//!
//! The url-floor group's claims are verified by the corpus itself against a real
//! WHATWG parser (`sanitization/verify-against-url-parser.mjs`), so what is checked
//! here is agreement with an invariant established independently, rather than
//! agreement between two of our own assertions.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::markdown::to_html;
use fuaran_rs::render::sanitize::{
    is_allowed_extra_attribute_key, is_safe_extra_attribute_value, sanitize_url,
    sanitize_url_or_blank,
};

/// Groups whose seam does not exist on this host, with the reason. Declared rather
/// than omitted: a group silently skipped would read as covered in the family,
/// which is the shape §22.2 refuses. Empty here — this host has every seam.
const NOT_APPLICABLE: &[(&str, &str)] = &[];

fn find_manifest() -> Option<PathBuf> {
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

fn str_field(v: &JVal, key: &str) -> Option<String> {
    match v.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn str_list(v: &JVal, key: &str) -> Vec<String> {
    match v.field(key) {
        Some(JVal::Arr(items)) => items
            .iter()
            .filter_map(|i| match i {
                JVal::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Does `haystack` contain a LIVE tag carrying an `on*` attribute?
///
/// The corpus expresses this as the regex `<[^>]*\son[a-z]+\s*=`. This host is
/// stdlib-only and has no regex engine, so the pattern is implemented directly —
/// and `matches_forbidden` below PANICS on any pattern it does not recognise, so a
/// new corpus pattern cannot silently pass here while appearing to be checked.
fn has_live_handler(haystack: &str) -> bool {
    let lower = haystack.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Scan the tag interior, up to the closing `>` or end of input.
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != b'>' {
            // An `on*=` attribute must be preceded by whitespace (a separator),
            // which is what distinguishes it from a tag NAME beginning with "on".
            if bytes[j].is_ascii_whitespace() {
                let mut k = j + 1;
                if k + 1 < bytes.len() && &bytes[k..k + 2] == b"on" {
                    k += 2;
                    let start = k;
                    while k < bytes.len() && bytes[k].is_ascii_lowercase() {
                        k += 1;
                    }
                    if k > start {
                        let mut m = k;
                        while m < bytes.len() && bytes[m].is_ascii_whitespace() {
                            m += 1;
                        }
                        if m < bytes.len() && bytes[m] == b'=' {
                            return true;
                        }
                    }
                }
            }
            j += 1;
        }
        i = j.max(i + 1);
    }
    false
}

/// Does `haystack` contain a live `<a …href=javascript:…>`?
///
/// The corpus regex is `<a[^>]*href\s*=\s*["']?\s*javascript:`. Same reasoning as
/// `has_live_handler`.
fn has_live_javascript_href(haystack: &str) -> bool {
    let lower = haystack.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'<' || bytes[i + 1] != b'a' {
            i += 1;
            continue;
        }
        let mut j = i + 2;
        while j < bytes.len() && bytes[j] != b'>' {
            if lower[j..].starts_with("href") {
                let mut k = j + 4;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b'=' {
                    k += 1;
                    while k < bytes.len()
                        && (bytes[k].is_ascii_whitespace() || bytes[k] == b'"' || bytes[k] == b'\'')
                    {
                        k += 1;
                    }
                    if lower[k..].starts_with("javascript:") {
                        return true;
                    }
                }
            }
            j += 1;
        }
        i = j.max(i + 1);
    }
    false
}

/// Evaluate one corpus `forbiddenPattern` against rendered output.
///
/// Plain-literal patterns (`<script`, `<iframe`, …) are a case-insensitive
/// substring test. The two structural patterns have purpose-built matchers above.
/// Anything else PANICS — a corpus pattern this host cannot evaluate must fail
/// loudly rather than quietly report "no match", which would read as a pass.
fn matches_forbidden(rendered: &str, pattern: &str) -> bool {
    const LIVE_HANDLER: &str = r"<[^>]*\son[a-z]+\s*=";
    const LIVE_JS_HREF: &str = r#"<a[^>]*href\s*=\s*["']?\s*javascript:"#;

    match pattern {
        LIVE_HANDLER => has_live_handler(rendered),
        LIVE_JS_HREF => has_live_javascript_href(rendered),
        p if !p.contains(['[', '\\', '*', '?', '+', '(']) => rendered
            .to_ascii_lowercase()
            .contains(&p.to_ascii_lowercase()),
        p => panic!(
            "sanitization corpus carries forbidden pattern {p:?}, which this stdlib-only host has no \
             matcher for. Add one beside `has_live_handler` — do NOT let it fall through, because a \
             pattern that is never evaluated reads exactly like one that found nothing."
        ),
    }
}

fn assert_inert(rendered: &str, case: &JVal, id: &str, failures: &mut Vec<String>) {
    for p in str_list(case, "forbiddenPattern") {
        if matches_forbidden(rendered, &p) {
            let input = str_field(case, "input").unwrap_or_default();
            failures.push(format!(
                "{id}: output matches forbidden pattern {p:?} — payload {input:?} survived as live markup"
            ));
        }
    }
    // `required` is the other half of `inert`, catching a host that satisfies every
    // forbidden pattern by discarding the content entirely.
    for r in str_list(case, "required") {
        if !rendered.contains(&r) {
            failures.push(format!(
                "{id}: output is missing required {r:?} — the payload was stripped rather than escaped"
            ));
        }
    }
}

fn group<'a>(doc: &'a JVal, id: &str) -> Vec<&'a JVal> {
    let Some(JVal::Arr(groups)) = doc.field("groups") else {
        return Vec::new();
    };
    for g in groups {
        if str_field(g, "id").as_deref() == Some(id) {
            if let Some(JVal::Arr(cases)) = g.field("cases") {
                return cases.iter().collect();
            }
        }
    }
    Vec::new()
}

#[test]
fn sanitization_corpus() {
    let Some(path) = find_manifest() else {
        eprintln!("wire-format-fixtures/sanitization not found; skipping (standalone checkout)");
        return;
    };
    let raw = std::fs::read_to_string(&path).expect("read sanitization manifest");
    let doc = parse(&raw).expect("parse sanitization manifest");

    let mut failures: Vec<String> = Vec::new();

    // A group no leg runs would be silently untested here while reading as covered
    // in the family — the shape §22.2 refuses.
    let known = [
        "url-floor",
        "markdown-body",
        "text-source",
        "extra-attributes",
    ];
    if let Some(JVal::Arr(groups)) = doc.field("groups") {
        for g in groups {
            let id = str_field(g, "id").unwrap_or_default();
            let claimed =
                known.contains(&id.as_str()) || NOT_APPLICABLE.iter().any(|(n, _)| *n == id);
            if !claimed {
                failures.push(format!(
                    "the corpus carries group {id:?}, which this host neither runs nor declares not-applicable"
                ));
            }
        }
    }

    let url_cases = group(&doc, "url-floor");
    println!("sanitization/url-floor: {} cases", url_cases.len());
    for c in &url_cases {
        let id = str_field(c, "id").unwrap_or_default();
        let input = str_field(c, "input").unwrap_or_default();
        match (
            str_field(c, "invariant").unwrap_or_default().as_str(),
            sanitize_url(&input),
        ) {
            ("reject", Some(got)) => failures.push(format!("{id}: expected REJECT, got {got:?}")),
            ("reject", None) => {
                if sanitize_url_or_blank(&input) != "about:blank" {
                    failures.push(format!(
                        "{id}: rejected, but or-blank did not give about:blank"
                    ));
                }
            }
            ("accept", None) => failures.push(format!("{id}: expected ACCEPT, was rejected")),
            ("accept", Some(got)) => {
                if let Some(want) = str_field(c, "expected")
                    && want != got
                {
                    failures.push(format!("{id}: expected {want:?}, got {got:?}"));
                }
            }
            (other, _) => failures.push(format!("{id}: unknown invariant {other:?}")),
        }
    }

    // `to_html` applies both layers on this host — the deterministic GFM renderer,
    // which escapes by construction, then the defence-in-depth sweep — so it IS the
    // obligation's surface for both the markdown and text groups.
    let md_cases = group(&doc, "markdown-body");
    println!("sanitization/markdown-body: {} cases", md_cases.len());
    for c in &md_cases {
        let id = str_field(c, "id").unwrap_or_default();
        let input = str_field(c, "input").unwrap_or_default();
        assert_inert(&to_html(&input), c, &id, &mut failures);
    }

    let text_cases = group(&doc, "text-source");
    println!("sanitization/text-source: {} cases", text_cases.len());
    for c in &text_cases {
        let id = str_field(c, "id").unwrap_or_default();
        let input = str_field(c, "input").unwrap_or_default();
        assert_inert(&to_html(&input), c, &id, &mut failures);
    }

    let attr_cases = group(&doc, "extra-attributes");
    println!("sanitization/extra-attributes: {} cases", attr_cases.len());
    for c in &attr_cases {
        let id = str_field(c, "id").unwrap_or_default();
        let input = str_field(c, "input").unwrap_or_default();
        let admitted = match str_field(c, "target").as_deref() {
            Some("key") => is_allowed_extra_attribute_key(&input),
            Some("value") => is_safe_extra_attribute_value(&input),
            other => panic!("case {id} has unknown target {other:?}"),
        };
        let should_admit = str_field(c, "invariant").as_deref() == Some("accept");
        if admitted != should_admit {
            let verb = if should_admit { "ACCEPT" } else { "REJECT" };
            failures.push(format!("{id}: expected {verb}, payload {input:?}"));
        }
    }

    let total = url_cases.len() + md_cases.len() + text_cases.len() + attr_cases.len();
    assert!(total > 0, "sanitization manifest parsed but held no cases");
    assert!(
        failures.is_empty(),
        "sanitization invariants violated:\n  {}",
        failures.join("\n  ")
    );
}
