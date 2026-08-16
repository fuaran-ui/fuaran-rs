//! Render-time injection-safety contract — the shared sanitisation floor every
//! string→markup seam routes through (the cross-host `SANITIZATION.md`
//! posture). Pure and dependency-free.
//!
//! Threat model: URL props block `javascript:` / `vbscript:` / `file:` / raw
//! `data:` schemes (http/https/mailto/tel/ftp/sftp + relative allowed);
//! markdown raw-HTML gets dangerous element blocks, tag-interior `on*=`
//! handlers, and dangerous protocols stripped as defence in depth on top of
//! the renderer's escape-by-construction; extra attributes hold the
//! `data-*` / `aria-*` allowlist.

use std::borrow::Cow;

const ALLOWED_URL_SCHEMES: &[&str] = &["http", "https", "mailto", "tel", "ftp", "sftp"];
const REJECTED_URL_SCHEMES: &[&str] = &["javascript", "vbscript", "file"];

/// Split a URL into its scheme candidate. A URL without a `:` before any `/`,
/// `?`, or `#` has no scheme (relative / fragment). ASCII whitespace + C0
/// controls are stripped from the candidate so `java\tscript:` classifies as
/// `javascript`.
fn extract_scheme(url: &str) -> Option<String> {
    let mut colon_idx = None;
    for (i, ch) in url.char_indices() {
        match ch {
            ':' => {
                colon_idx = Some(i);
                break;
            }
            '/' | '?' | '#' => break,
            _ => {}
        }
    }
    let colon = colon_idx?;
    let cleaned: String = url[..colon]
        .chars()
        .filter(|&c| (c as u32) > 0x20)
        .collect();
    Some(cleaned.trim().to_lowercase())
}

/// A protocol-relative URL: `//host/path` and the forms browsers fold into it.
/// WHATWG URL parsing treats `\` as `/` for special schemes, so `\\host`,
/// `/\host` and `\/host` all resolve exactly as `//host` does.
///
/// These carry no scheme, so the schemeless branch of [`sanitize_url`] would
/// otherwise admit them — but the browser resolves them against the CURRENT
/// page's scheme and lands on an OFF-ORIGIN host, defeating the same-origin
/// intent that makes a schemeless URL safe. On an `href` that is off-origin
/// navigation; on an image `src` it is an off-origin request that leaks the
/// Referer.
fn is_protocol_relative(url: &str) -> bool {
    let mut chars = url.chars();
    let is_sep = |c: Option<char>| matches!(c, Some('/') | Some('\\'));
    is_sep(chars.next()) && is_sep(chars.next())
}

/// §19 rule 1 — normalise exactly as the WHATWG URL Standard's basic URL parser
/// does before it parses anything, ASCII-exact, in this order:
///
/// 1. remove leading and trailing **C0 control or space** — all of U+0000–U+0020,
///    not merely the whitespace subset;
/// 2. remove every U+0009 / U+000A / U+000D from anywhere in what remains.
///
/// Deliberately **not** [`str::trim`]. A native trim answers a different question
/// in every language — Python's `strip` also removes U+001C–U+001F where Rust,
/// .NET, JS and Go do not; JS alone keeps U+0085 NEL where the other four drop it
/// — and all of them remove non-ASCII whitespace (U+00A0, U+2028, …) that the
/// parser keeps. The floor's whole purpose is that a tree vetted on one host is
/// safe on another, so the normalisation is defined by the parser that will
/// actually consume the string, not by the host's standard library.
///
/// Step 2 is those three code points **only**: the parser removes U+000B and
/// U+000C at the edges (step 1) and *keeps* them in the interior, so
/// `/<VT>/host/x` is an ordinary same-origin path and must stay one.
///
/// Returns a [`Cow`] so the common case — nothing to remove from the interior —
/// borrows a subslice rather than allocating.
fn normalize_url_for_floor(url: &str) -> Cow<'_, str> {
    let edges = url.trim_matches(|c: char| c <= '\u{20}');
    if edges.contains(['\t', '\n', '\r']) {
        Cow::Owned(edges.replace(['\t', '\n', '\r'], ""))
    } else {
        Cow::Borrowed(edges)
    }
}

/// The sanitised URL, or `None` if the scheme is rejected. Empty string passes
/// through (a valid same-page href); unknown schemes reject conservatively, and
/// so do protocol-relative URLs despite carrying no scheme (see
/// [`is_protocol_relative`]).
///
/// The input is first normalised per §19 rule 1 (see [`normalize_url_for_floor`]),
/// and that normalised form is also what is **returned** on acceptance — so an
/// accepted URL carrying an interior tab loses it, which is what the browser would
/// have parsed anyway. That is why the return type is a [`Cow`] rather than a
/// borrow of the input.
pub fn sanitize_url(url: &str) -> Option<Cow<'_, str>> {
    let trimmed = normalize_url_for_floor(url);
    if trimmed.is_empty() {
        return Some(trimmed);
    }
    match extract_scheme(&trimmed) {
        None if is_protocol_relative(&trimmed) => None, // off-origin despite having no scheme
        None => Some(trimmed),                          // relative / fragment / same-origin
        Some(scheme) => {
            if REJECTED_URL_SCHEMES.contains(&scheme.as_str()) {
                None
            } else if ALLOWED_URL_SCHEMES.contains(&scheme.as_str()) {
                Some(trimmed)
            } else {
                None // unknown scheme — reject by default
            }
        }
    }
}

/// The URL itself if accepted, or `about:blank` — for call sites that must
/// emit *some* href to keep the element valid.
pub fn sanitize_url_or_blank(url: &str) -> String {
    sanitize_url(url)
        .map(Cow::into_owned)
        .unwrap_or_else(|| "about:blank".to_string())
}

const DANGEROUS_ELEMENTS: &[&str] = &[
    "script", "iframe", "object", "embed", "form", "link", "meta",
];
const DANGEROUS_PROTOCOLS: &[&str] = &["javascript:", "vbscript:"];

/// ASCII-case-insensitive substring search (chars, so indices stay aligned
/// with the source string across hosts).
fn index_of_ci(haystack: &[char], needle: &str, from: usize) -> Option<usize> {
    let needle: Vec<char> = needle.chars().map(|c| c.to_ascii_lowercase()).collect();
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| {
        haystack[i..i + needle.len()]
            .iter()
            .zip(&needle)
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

fn index_of_any(haystack: &[char], chars: &[char], from: usize) -> Option<usize> {
    (from..haystack.len()).find(|&i| chars.contains(&haystack[i]))
}

/// Strip dangerous element blocks, tag-interior `on*=` handlers, and dangerous
/// protocols from a chunk of HTML. Approximate by design — the render path
/// constrains the input to the deterministic markdown renderer's output, so
/// the substring sweep is defence in depth, not the primary gate.
pub fn sanitize_markdown_html(html: &str) -> String {
    if html.is_empty() {
        return String::new();
    }
    let mut result: Vec<char> = html.chars().collect();

    // Remove balanced dangerous element blocks; iterate for nested / siblings.
    for tag in DANGEROUS_ELEMENTS {
        let open_tag = format!("<{tag}");
        let close_tag = format!("</{tag}>");
        while let Some(i) = index_of_ci(&result, &open_tag, 0) {
            if let Some(j) = index_of_ci(&result, &close_tag, i) {
                result.drain(i..j + close_tag.chars().count());
            } else if let Some(end) = index_of_any(&result, &['>'], i) {
                result.drain(i..=end);
            } else {
                result.truncate(i);
                break;
            }
        }
    }

    result = strip_event_handlers(result);

    for proto in DANGEROUS_PROTOCOLS {
        while let Some(i) = index_of_ci(&result, proto, 0) {
            let replacement: Vec<char> = "about:blank".chars().collect();
            result.splice(i..i + proto.len(), replacement);
        }
    }

    result.into_iter().collect()
}

/// Strip inline `on*="…"` event-handler attributes, anchored to tag interiors
/// (the `<…>` restriction keeps prose words like "one" / "only" intact — the
/// renderer escapes raw HTML, so a real handler can only sit inside a tag the
/// renderer itself emitted).
fn strip_event_handlers(input: Vec<char>) -> Vec<char> {
    let mut s = input;
    loop {
        let mut found: Option<usize> = None;
        let mut inside_tag = false;
        let n = s.len();
        for i in 0..n.saturating_sub(3) {
            let c0 = s[i].to_ascii_lowercase();
            if c0 == '<' {
                inside_tag = true;
            } else if c0 == '>' {
                inside_tag = false;
            } else if inside_tag
                && (c0 == ' ' || c0 == '\t' || c0 == '\n')
                && s[i + 1].eq_ignore_ascii_case(&'o')
                && s[i + 2].eq_ignore_ascii_case(&'n')
                && s[i + 3].is_ascii_alphabetic()
            {
                found = Some(i);
                break;
            }
        }
        let Some(found) = found else {
            return s;
        };
        let eq = index_of_any(&s, &['='], found);
        let next_space = index_of_any(&s, &[' ', '\t', '\n', '>'], found + 1);
        let is_boolean_attr = match (eq, next_space) {
            (None, _) => true,
            (Some(e), Some(sp)) => sp < e,
            (Some(_), None) => false,
        };
        if is_boolean_attr {
            // Boolean attribute like `onload` with no `=` — strip the name only.
            let stop_at = next_space.unwrap_or(s.len());
            s.drain(found..stop_at);
        } else {
            let eq = eq.expect("non-boolean branch has an '='");
            let mut v = eq + 1;
            while v < s.len() && (s[v] == ' ' || s[v] == '\t') {
                v += 1;
            }
            let stop_at = if v < s.len() && (s[v] == '\'' || s[v] == '"') {
                let quote = s[v];
                index_of_any(&s, &[quote], v + 1)
                    .map(|close| close + 1)
                    .unwrap_or(s.len())
            } else {
                index_of_any(&s, &[' ', '\t', '\n', '>'], v).unwrap_or(s.len())
            };
            s.drain(found..stop_at);
        }
    }
}

/// Allowlist predicate for an extra-attribute key: `data-*` / `aria-*` only,
/// with explicit rejection of `on*` handlers and `style`.
pub fn is_allowed_extra_attribute_key(key: &str) -> bool {
    let trimmed = key.trim();
    if trimmed.is_empty()
        || trimmed.to_lowercase().starts_with("on")
        || trimmed.to_lowercase() == "style"
    {
        return false;
    }
    trimmed.starts_with("data-") || trimmed.starts_with("aria-")
}

/// Reject values carrying C0 control bytes (except tab) or angle brackets —
/// attribute-injection vectors under a verbatim-emission contract.
pub fn is_safe_extra_attribute_value(value: &str) -> bool {
    value
        .chars()
        .all(|ch| !(((ch as u32) < 0x20 && ch != '\t') || ch == '<' || ch == '>'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_schemes() {
        assert_eq!(
            sanitize_url("https://x.dev/a").as_deref(),
            Some("https://x.dev/a")
        );
        assert_eq!(
            sanitize_url("/relative#frag").as_deref(),
            Some("/relative#frag")
        );
        assert_eq!(sanitize_url("javascript:alert(1)"), None);
        assert_eq!(sanitize_url("JAVAscript:alert(1)"), None);
        assert_eq!(sanitize_url("java\tscript:alert(1)"), None);
        assert_eq!(sanitize_url("data:text/html,x"), None);
        assert_eq!(sanitize_url("").as_deref(), Some(""));
        assert_eq!(sanitize_url_or_blank("vbscript:x"), "about:blank");
    }

    #[test]
    fn protocol_relative_urls_are_rejected() {
        // No scheme, so the schemeless branch would admit these — but the browser
        // resolves them against the current page's scheme and lands OFF-ORIGIN.
        // `\` is WHATWG's lenient normalisation of `/` for special schemes, so all
        // four two-separator forms resolve identically.
        for url in [
            "//evil.example/x",
            r"/\evil.example/x",
            r"\\evil.example/x",
            r"\/evil.example/x",
            "//",
            "  //evil.example/x", // rejection survives whitespace trimming
        ] {
            assert_eq!(
                sanitize_url(url).as_deref(),
                None,
                "expected rejection for {url:?}"
            );
            assert_eq!(sanitize_url_or_blank(url), "about:blank");
        }
    }

    /// §19 rule 1 — the WHATWG basic URL parser's own pre-parse normalisation.
    ///
    /// Control characters are written as escapes throughout: a raw C0 byte in
    /// source is invisible in review and does not survive a copy-paste, which is
    /// the wrong property for the payloads a security pin is made of.
    #[test]
    fn url_floor_normalises_as_the_url_parser_does() {
        // V1 — an interior TAB / LF / CR BETWEEN the two slash-ish characters.
        // Before rule 1 normalised, `/<TAB>/host/x` had first two characters `/`
        // and TAB, so `is_protocol_relative` read an ordinary relative reference
        // and accepted, while the browser removed the tab by the URL Standard's
        // step 2 and resolved `//host/x` OFF-ORIGIN. Verified against the WHATWG
        // parser: all twelve spellings resolve to `https://evil.example/x`.
        for c in ['\t', '\n', '\r'] {
            for a in ['/', '\\'] {
                for b in ['/', '\\'] {
                    let url = format!("{a}{c}{b}evil.example/x");
                    assert_eq!(sanitize_url(&url).as_deref(), None, "V1 {url:?}");
                }
            }
        }
        assert_eq!(sanitize_url("/\t\r/\nevil.example/x").as_deref(), None);

        // V2 — a LEADING C0 control that is not whitespace. No native trim removes
        // U+0001 or NUL, so the two slashes sat at positions 1 and 2 and
        // `is_protocol_relative` never saw them; the parser removes them by step 1
        // and resolves off-origin.
        for c in ['\u{1}', '\u{0}', '\u{1f}'] {
            let url = format!("{c}//evil.example/x");
            assert_eq!(sanitize_url(&url).as_deref(), None, "V2 {url:?}");
        }

        // Step 1 is the whole C0-or-space range, at both ends; and rule 1's output
        // is what gets RETURNED — an accepted URL loses its interior tab.
        assert_eq!(
            sanitize_url("https://good.example/x\u{1}").as_deref(),
            Some("https://good.example/x")
        );
        assert_eq!(
            sanitize_url("https://good.ex\tample/x").as_deref(),
            Some("https://good.example/x")
        );

        // U+000B and U+000C are removed at the EDGES by step 1 and KEPT in the
        // interior — the parser treats `/<VT>/host/x` as a same-origin path, and so
        // must the floor. Pinned because widening step 2 to "all C0" would silently
        // over-reject here.
        for c in ['\u{b}', '\u{c}'] {
            let url = format!("/{c}/evil.example/x");
            assert_eq!(sanitize_url(&url).as_deref(), Some(url.as_str()), "{url:?}");
        }

        // ASCII-exact LOOSENS these, correctly: the parser keeps them and resolves
        // an ordinary same-origin path, where `str::trim` removed them and the floor
        // then saw `//` and rejected. U+0085 is where JS diverged from Rust, .NET,
        // Python and Go; ASCII-exact ends the divergence in both directions.
        for c in ['\u{a0}', '\u{85}'] {
            let url = format!("{c}//evil.example/x");
            assert_eq!(sanitize_url(&url).as_deref(), Some(url.as_str()), "{url:?}");
        }

        // Rule 2 is UNCHANGED and still stricter than the browser, which is why V1
        // and V2 are off-origin navigation rather than script execution.
        assert_eq!(sanitize_url("java\tscript:alert(1)").as_deref(), None);
        assert_eq!(sanitize_url("java\u{b}script:alert(1)").as_deref(), None);
    }

    #[test]
    fn single_slash_relative_paths_still_pass() {
        for url in ["/", "/a", "/foo//bar", "./rel", "page", "#frag", "foo/bar"] {
            assert_eq!(
                sanitize_url(url).as_deref(),
                Some(url),
                "expected pass-through for {url:?}"
            );
        }
        // An absolute URL whose authority legitimately uses `//` is unaffected.
        assert_eq!(
            sanitize_url("https://ok.example/x").as_deref(),
            Some("https://ok.example/x")
        );
    }

    #[test]
    fn markdown_sweep_is_index_aligned_under_case_folding() {
        // This host already folds ASCII-only, so the sweep's search copy stays
        // index-aligned with the original. Pinned so a future switch to a
        // Unicode-aware fold (which is not length-preserving — U+0130 folds to two
        // chars) cannot silently reintroduce the sibling hosts' misalignment.
        assert_eq!(sanitize_markdown_html("İ<script>alert(1)</script>"), "İ");
        assert_eq!(
            sanitize_markdown_html("<p>İİ</p><SCRIPT>x</SCRIPT><p>b</p>"),
            "<p>İİ</p><p>b</p>"
        );
        assert_eq!(sanitize_markdown_html("İ<iframe src='x'></iframe>b"), "İb");
    }

    #[test]
    fn markdown_html_strips_dangerous_blocks() {
        assert_eq!(
            sanitize_markdown_html("<p>a</p><script>alert(1)</script><p>b</p>"),
            "<p>a</p><p>b</p>"
        );
        assert_eq!(
            sanitize_markdown_html("<a href=\"x\" onclick=\"evil()\">t</a>"),
            "<a href=\"x\">t</a>"
        );
        assert_eq!(
            sanitize_markdown_html("<a href=\"javascript:go()\">t</a>"),
            "<a href=\"about:blankgo()\">t</a>"
        );
        // Prose containing "on<letter>" after whitespace survives (tag-interior anchor).
        assert_eq!(
            sanitize_markdown_html("<p>the only one</p>"),
            "<p>the only one</p>"
        );
    }
}
