//! Deterministic GFM markdown → HTML — the render-only cross-host contract
//! (`MARKDOWN.md`): one renderer shared byte-for-byte by every host, certified
//! against the shared markdown corpus. Coverage buckets: IN — CommonMark core
//! plus GFM tables / strikethrough / task lists / bare URLs; OUT — raw/inline
//! HTML escaped (no passthrough); DEFERRED — emoji, footnotes, anchors,
//! sub-sup, definition lists, the full named-entity table.
//!
//! Escapes by construction (no raw-HTML passthrough; URLs via the
//! [`super::sanitize`] scheme floor); the result still passes through
//! [`super::sanitize::sanitize_markdown_html`] as defence in depth.
//!
//! Host-parity primitives are explicit ASCII classes (not Unicode `is_*`
//! methods) so every host classifies identically at the edges.
//!
//! # Destination policy (`WIRE_FORMAT.md` §14.1)
//!
//! The scheme floor answers "is this URL safe to have"; it does not answer "is
//! this destination one the composition declared". [`to_html_with_egress`]
//! consults a [`super::egress::EgressPolicy`] for every link
//! ([`EgressClass::Hyperlink`]) and image ([`EgressClass::Media`]) destination,
//! and a refused one renders the inert `about:blank#fuaran-egress-refused`
//! href plus a `data-fuaran-egress-refused` marker naming the class and the
//! host — never the path or the query.
//!
//! **[`to_html`] SURVIVES AS THE PERMISSIVE CASE** — `to_html_with_egress`
//! under [`permissive_egress`], byte-for-byte. Three reasons, and they are the
//! same reasons every other posture inversion here is reached BY NAME: the
//! corpus is a cross-host byte-parity contract, so flipping the pure
//! function's default would rewrite existing fixtures in five hosts in one act
//! — a mass churn is exactly where a real divergence hides; `to_html` is
//! published surface on an Apache-2.0 crate, and a host author who wants the
//! pure function should reach it deliberately rather than meet a silent
//! behaviour swap; and keeping it named makes an unpolicied markdown render
//! greppable, which is the property the refusal shape exists to give.
//!
//! **The scheme floor's own answer is unchanged.** A URL the floor rejects
//! (`javascript:`, an unknown scheme, a protocol-relative reference) still
//! renders the bare `about:blank` it always has, with **no** marker — see
//! [`markdown_destination`] for why that is a decision rather than an
//! inconsistency.
//!
//! The policy is threaded as a borrowed parameter, never a global or a
//! thread-local: two renders under two different policies may run
//! concurrently, and an ambient policy is one a concurrent render can observe
//! halfway through being swapped.

use std::collections::HashMap;

use super::egress::{
    EGRESS_REFUSAL_URL, EgressClass, EgressPolicy, EgressVerdict, check_destination,
    egress_refusal_marker, permissive_egress,
};
use super::sanitize::sanitize_markdown_html;

// ─── Host-parity primitives ──────────────────────────────────────────────────

fn is_ws(c: char) -> bool {
    c == ' ' || ('\u{0009}'..='\u{000D}').contains(&c)
}

fn is_ascii_punct(c: char) -> bool {
    ('!'..='/').contains(&c)
        || (':'..='@').contains(&c)
        || ('['..='`').contains(&c)
        || ('{'..='~').contains(&c)
}

/// The cross-host escape set exactly: `&` `<` `>` `"` (and NOT `'`).
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

fn repeat_char(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}

fn trim_start_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_start_matches(|c| chars.contains(c))
}

fn trim_end_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_end_matches(|c| chars.contains(c))
}

fn trim_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    trim_end_chars(trim_start_chars(s, chars), chars)
}

// ─── Entity decoding (common subset; the rest is DEFERRED) ───────────────────

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{00A0}",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "deg" => "°",
        "plusmn" => "±",
        "times" => "×",
        "divide" => "÷",
        "frac12" => "½",
        "frac14" => "¼",
        "frac34" => "¾",
        "sup2" => "²",
        "sup3" => "³",
        "middot" => "·",
        "bull" => "•",
        "dagger" => "†",
        "euro" => "€",
        "pound" => "£",
        "cent" => "¢",
        "yen" => "¥",
        "sect" => "§",
        "para" => "¶",
        _ => return None,
    })
}

/// Decode an `&…;` entity at `i` (which points at `&`); returns the decoded
/// text + the index past the `;`.
fn try_decode_entity(text: &[char], i: usize) -> Option<(String, usize)> {
    let semi = (i + 1..text.len()).find(|&k| text[k] == ';')?;
    if semi == i + 1 {
        return None;
    }
    let body: String = text[i + 1..semi].iter().collect();
    if let Some(digits) = body.strip_prefix('#') {
        let (radix, digits) = if digits.starts_with('x') || digits.starts_with('X') {
            (16, &digits[1..])
        } else {
            (10, digits)
        };
        if digits.is_empty() {
            return None;
        }
        let code = u32::from_str_radix(digits, radix).ok()?;
        let cp = if code == 0 || code > 0x10FFFF {
            '\u{FFFD}'
        } else {
            char::from_u32(code).unwrap_or('\u{FFFD}')
        };
        return Some((cp.to_string(), semi + 1));
    }
    named_entity(&body).map(|s| (s.to_string(), semi + 1))
}

// ─── Inline AST ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Inline {
    Text(String),
    Raw(String),
    Emph(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Soft,
    Hard,
}

type Refs = HashMap<String, (String, Option<String>)>;

// ─── Destination policy at the link / image seam ─────────────────────────────

/// Everything a render pass carries that is not the markdown itself: the
/// link-reference table and the destination policy. Borrowed rather than owned
/// and passed rather than ambient — see the module header.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    refs: &'a Refs,
    policy: &'a EgressPolicy,
}

/// The `href` / `src` a markdown destination emits under `policy`, plus the
/// trailing attribute string that records a refusal in the document itself.
///
/// Three verdict groups, and the middle one is a deliberate decision rather
/// than an oversight:
///
/// - **Allowed** — the normalised URL, no marker. Identical to what the bare
///   scheme floor returned before this seam existed, so a permissive render is
///   byte-for-byte what it always was.
/// - **Unsafe (the SCHEME FLOOR refused)** — the bare `about:blank`, no marker.
///   The floor's answer is a different fact from a policy refusal: it says
///   "this URL is not safe to render at all", it has said it in that exact
///   spelling in every conformant host since the renderer was cut, and it is
///   pinned by the shared sanitization corpus. Re-spelling it here would churn
///   that corpus inside a change about EGRESS — mixing two decisions into one
///   set of bytes, which is where a genuine divergence hides.
/// - **Refused by policy** — the inert `about:blank#fuaran-egress-refused` plus
///   a marker naming the class and, where there is one, the host. Never the
///   path or the query: the query string of a refused exfiltration attempt is
///   the payload itself.
fn markdown_destination(policy: &EgressPolicy, class: EgressClass, url: &str) -> (String, String) {
    let verdict = check_destination(policy, class, url);
    match &verdict {
        EgressVerdict::Allowed(safe) => (safe.clone(), String::new()),
        EgressVerdict::UnsafeUrl => ("about:blank".to_string(), String::new()),
        refused => (EGRESS_REFUSAL_URL.to_string(), egress_attrs(refused)),
    }
}

/// Render a verdict's refusal marker as a trailing HTML attribute. Emitted LAST
/// on the element so an adopting host's diff against the pre-policy bytes is a
/// pure suffix — every attribute that was there is still where it was.
fn egress_attrs(verdict: &EgressVerdict) -> String {
    match egress_refusal_marker(verdict) {
        None => String::new(),
        Some((key, value)) => format!(" {key}=\"{}\"", escape_html(&value)),
    }
}

/// ASCII-only case fold — the cross-host reference-label contract.
fn ascii_lower(c: char) -> char {
    c.to_ascii_lowercase()
}

fn norm_label(s: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for ch in s.trim().chars() {
        if is_ws(ch) {
            in_ws = true;
        } else {
            if in_ws {
                out.push(' ');
            }
            in_ws = false;
            out.push(ascii_lower(ch));
        }
    }
    out
}

fn scan_code_span(text: &[char], i: usize) -> Option<(Inline, usize)> {
    let n = text.len();
    let mut j = i;
    while j < n && text[j] == '`' {
        j += 1;
    }
    let open_len = j - i;
    let mut k = j;
    let mut close_start: Option<usize> = None;
    while k < n && close_start.is_none() {
        if text[k] == '`' {
            let mut m = k;
            while m < n && text[m] == '`' {
                m += 1;
            }
            if m - k == open_len {
                close_start = Some(k);
            }
            k = m;
        } else {
            k += 1;
        }
    }
    let close_start = close_start?;
    let raw: String = text[j..close_start].iter().collect();
    let mut collapsed = raw.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let chars: Vec<char> = collapsed.chars().collect();
    if chars.len() >= 2
        && chars[0] == ' '
        && chars[chars.len() - 1] == ' '
        && !collapsed.trim().is_empty()
    {
        collapsed = chars[1..chars.len() - 1].iter().collect();
    }
    Some((
        Inline::Raw(format!("<code>{}</code>", escape_html(&collapsed))),
        close_start + open_len,
    ))
}

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-'
}

fn scan_autolink(policy: &EgressPolicy, text: &[char], i: usize) -> Option<(Inline, usize)> {
    let close = (i..text.len()).find(|&k| text[k] == '>')?;
    let body: String = text[i + 1..close].iter().collect();
    if body.is_empty() || body.contains(' ') || body.contains('<') {
        return None;
    }
    let colon = body.find(':');
    let looks_uri = colon.is_some_and(|c| {
        (2..=32).contains(&c)
            && body[..c].chars().all(is_scheme_char)
            && body.chars().next().is_some_and(|f| f.is_ascii_alphabetic())
    });
    let looks_email =
        !looks_uri && body.contains('@') && !body.contains(':') && !body.starts_with('@');
    if looks_uri {
        let (safe, attrs) = markdown_destination(policy, EgressClass::Hyperlink, &body);
        Some((
            Inline::Raw(format!(
                "<a href=\"{}\"{attrs}>{}</a>",
                escape_html(&safe),
                escape_html(&body)
            )),
            close + 1,
        ))
    } else if looks_email {
        // An email autolink has no URL of its own — the `mailto:` is the
        // renderer's, so the policy is asked about the destination the renderer
        // is about to emit. On acceptance the ORIGINAL bytes are emitted rather
        // than the normalised form, so a permissive render is unchanged to the
        // byte.
        let verdict = check_destination(policy, EgressClass::Hyperlink, &format!("mailto:{body}"));
        let html = match verdict {
            EgressVerdict::Allowed(_) => format!(
                "<a href=\"mailto:{}\">{}</a>",
                escape_html(&body),
                escape_html(&body)
            ),
            refused => format!(
                "<a href=\"{}\"{}>{}</a>",
                escape_html(EGRESS_REFUSAL_URL),
                egress_attrs(&refused),
                escape_html(&body)
            ),
        };
        Some((Inline::Raw(html), close + 1))
    } else {
        None
    }
}

/// The `(url "title")` tail of an inline link/image, starting just past `(`.
fn scan_inline_destination(text: &[char], start: usize) -> Option<(String, Option<String>, usize)> {
    let n = text.len();
    let mut i = start;
    while i < n && (text[i] == ' ' || text[i] == '\n' || text[i] == '\t') {
        i += 1;
    }
    let url: String;
    if i < n && text[i] == '<' {
        let close = (i..n).find(|&k| text[k] == '>')?;
        url = text[i + 1..close].iter().collect();
        i = close + 1;
    } else {
        let mut depth = 0;
        let mut buf = String::new();
        while i < n {
            let c = text[i];
            if c == ' ' || c == '\t' || c == '\n' {
                break;
            } else if c == '(' {
                depth += 1;
                buf.push(c);
                i += 1;
            } else if c == ')' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                buf.push(c);
                i += 1;
            } else if c == '\\' && i + 1 < n && is_ascii_punct(text[i + 1]) {
                buf.push(text[i + 1]);
                i += 2;
            } else {
                buf.push(c);
                i += 1;
            }
        }
        url = buf;
    }
    let mut title: Option<String> = None;
    let mut j = i;
    while j < n && (text[j] == ' ' || text[j] == '\t' || text[j] == '\n') {
        j += 1;
    }
    if j < n && (text[j] == '"' || text[j] == '\'') {
        let q = text[j];
        if let Some(t_close) = (j + 1..n).find(|&k| text[k] == q) {
            title = Some(text[j + 1..t_close].iter().collect());
            i = t_close + 1;
            while i < n && (text[i] == ' ' || text[i] == '\t' || text[i] == '\n') {
                i += 1;
            }
        }
    }
    if i < n && text[i] == ')' {
        Some((url, title, i + 1))
    } else {
        None
    }
}

/// The matching `]` for the `[` at `open0`, honouring escapes + code spans.
fn match_bracket(text: &[char], open0: usize) -> Option<usize> {
    let n = text.len();
    let mut i = open0 + 1;
    let mut depth = 1;
    while i < n {
        let c = text[i];
        if c == '\\' && i + 1 < n {
            i += 2;
        } else if c == '`' {
            i = match scan_code_span(text, i) {
                Some((_, next)) => next,
                None => i + 1,
            };
        } else if c == '[' {
            depth += 1;
            i += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    None
}

fn render_inlines(nodes: &[Inline]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Inline::Text(v) => out.push_str(&escape_html(v)),
            Inline::Raw(v) => out.push_str(v),
            Inline::Emph(c) => {
                out.push_str("<em>");
                out.push_str(&render_inlines(c));
                out.push_str("</em>");
            }
            Inline::Strong(c) => {
                out.push_str("<strong>");
                out.push_str(&render_inlines(c));
                out.push_str("</strong>");
            }
            Inline::Strike(c) => {
                out.push_str("<del>");
                out.push_str(&render_inlines(c));
                out.push_str("</del>");
            }
            Inline::Soft => out.push('\n'),
            Inline::Hard => out.push_str("<br />\n"),
        }
    }
    out
}

fn plain_text(nodes: &[Inline]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Inline::Text(v) => out.push_str(v),
            Inline::Emph(c) | Inline::Strong(c) | Inline::Strike(c) => {
                out.push_str(&plain_text(c));
            }
            Inline::Soft | Inline::Hard => out.push(' '),
            Inline::Raw(_) => {}
        }
    }
    out
}

fn scan_bare_autolink(policy: &EgressPolicy, text: &[char], i: usize) -> Option<(Inline, usize)> {
    let n = text.len();
    let starts = |p: &str| -> bool {
        let pc: Vec<char> = p.chars().collect();
        i + pc.len() <= n && text[i..i + pc.len()] == pc[..]
    };
    if !starts("https://") && !starts("http://") && !starts("www.") {
        return None;
    }
    let mut j = i;
    while j < n && !is_ws(text[j]) && text[j] != '<' {
        j += 1;
    }
    while j > i && ".,;:!?)\"'".contains(text[j - 1]) {
        j -= 1;
    }
    if j <= i + 4 {
        return None;
    }
    let raw: String = text[i..j].iter().collect();
    let href = if raw.starts_with("www.") {
        format!("http://{raw}")
    } else {
        raw.clone()
    };
    let (safe, attrs) = markdown_destination(policy, EgressClass::Hyperlink, &href);
    Some((
        Inline::Raw(format!(
            "<a href=\"{}\"{attrs}>{}</a>",
            escape_html(&safe),
            escape_html(&raw)
        )),
        j,
    ))
}

#[derive(Debug, Clone)]
enum Tok {
    Node(Inline),
    Delim {
        ch: char,
        count: usize,
        can_open: bool,
        can_close: bool,
    },
}

#[allow(clippy::too_many_lines)]
fn tokenize(ctx: Ctx<'_>, text: &[char]) -> Vec<Tok> {
    let mut toks: Vec<Tok> = Vec::new();
    let n = text.len();
    let mut i = 0;
    let mut pending = String::new();

    macro_rules! flush {
        () => {
            if !pending.is_empty() {
                toks.push(Tok::Node(Inline::Text(std::mem::take(&mut pending))));
            }
        };
    }

    let prev_char = |i: usize| -> char { if i == 0 { ' ' } else { text[i - 1] } };

    while i < n {
        let c = text[i];
        if c == '\\' && i + 1 < n && text[i + 1] == '\n' {
            flush!();
            toks.push(Tok::Node(Inline::Hard));
            i += 2;
        } else if c == '\\' && i + 1 < n && is_ascii_punct(text[i + 1]) {
            pending.push(text[i + 1]);
            i += 2;
        } else if c == '`' {
            if let Some((node, next)) = scan_code_span(text, i) {
                flush!();
                toks.push(Tok::Node(node));
                i = next;
            } else {
                pending.push(c);
                i += 1;
            }
        } else if c == '&' {
            if let Some((decoded, next)) = try_decode_entity(text, i) {
                pending.push_str(&decoded);
                i = next;
            } else {
                pending.push(c);
                i += 1;
            }
        } else if c == '<' {
            if let Some((node, next)) = scan_autolink(ctx.policy, text, i) {
                flush!();
                toks.push(Tok::Node(node));
                i = next;
            } else {
                pending.push(c);
                i += 1;
            }
        } else if (c == '!' && i + 1 < n && text[i + 1] == '[') || c == '[' {
            let is_image = c == '!';
            let open = if is_image { i + 1 } else { i };
            match match_bracket(text, open) {
                None => {
                    pending.push(c);
                    i += 1;
                }
                Some(close) => {
                    let label_text: String = text[open + 1..close].iter().collect();
                    let mut resolved: Option<(String, Option<String>, usize)> = None;
                    if close + 1 < n && text[close + 1] == '(' {
                        resolved = scan_inline_destination(text, close + 2);
                    } else {
                        let mut ref_label = label_text.clone();
                        let mut consumed_to = close + 1;
                        if close + 1 < n && text[close + 1] == '[' {
                            if let Some(r2) = match_bracket(text, close + 1) {
                                let inner: String = text[close + 2..r2].iter().collect();
                                if !inner.trim().is_empty() {
                                    ref_label = inner;
                                }
                                consumed_to = r2 + 1;
                            }
                        }
                        if let Some((url, title)) = ctx.refs.get(&norm_label(&ref_label)) {
                            resolved = Some((url.clone(), title.clone(), consumed_to));
                        }
                    }
                    match resolved {
                        None => {
                            pending.push(c);
                            i += 1;
                        }
                        Some((url, title, next)) => {
                            flush!();
                            let label_chars: Vec<char> = label_text.chars().collect();
                            let class = if is_image {
                                EgressClass::Media
                            } else {
                                EgressClass::Hyperlink
                            };
                            let (safe, attrs) = markdown_destination(ctx.policy, class, &url);
                            let title_attr = title
                                .map(|t| format!(" title=\"{}\"", escape_html(&t)))
                                .unwrap_or_default();
                            let html = if is_image {
                                let alt = plain_text(&parse_inlines(ctx, &label_chars));
                                format!(
                                    "<img src=\"{}\" alt=\"{}\"{title_attr}{attrs} />",
                                    escape_html(&safe),
                                    escape_html(&alt),
                                )
                            } else {
                                let inner = render_inlines(&parse_inlines(ctx, &label_chars));
                                format!(
                                    "<a href=\"{}\"{title_attr}{attrs}>{inner}</a>",
                                    escape_html(&safe),
                                )
                            };
                            toks.push(Tok::Node(Inline::Raw(html)));
                            i = next;
                        }
                    }
                }
            }
        } else if c == '*' || c == '_' || c == '~' {
            let mut j = i;
            while j < n && text[j] == c {
                j += 1;
            }
            let run_len = j - i;
            let before = prev_char(i);
            let after = if j < n { text[j] } else { ' ' };
            let before_ws = is_ws(before);
            let after_ws = is_ws(after);
            let before_punct = is_ascii_punct(before);
            let after_punct = is_ascii_punct(after);
            let left_flank = !after_ws && (!after_punct || before_ws || before_punct);
            let right_flank = !before_ws && (!before_punct || after_ws || after_punct);
            let (can_open, can_close) = if c == '_' {
                (
                    left_flank && (!right_flank || before_punct),
                    right_flank && (!left_flank || after_punct),
                )
            } else {
                (left_flank, right_flank)
            };
            flush!();
            if c == '~' && run_len != 2 {
                toks.push(Tok::Node(Inline::Text(repeat_char(c, run_len))));
            } else {
                toks.push(Tok::Delim {
                    ch: c,
                    count: run_len,
                    can_open,
                    can_close,
                });
            }
            i = j;
        } else if c == '\n' {
            let trimmed_end = trim_end_chars(&pending, " ").to_string();
            let hard = pending.len() - trimmed_end.len() >= 2;
            pending = trimmed_end;
            flush!();
            toks.push(Tok::Node(if hard { Inline::Hard } else { Inline::Soft }));
            i += 1;
        } else if (c == 'h' || c == 'w')
            && (i == 0 || is_ws(prev_char(i)) || "(*_~".contains(prev_char(i)))
        {
            if let Some((node, next)) = scan_bare_autolink(ctx.policy, text, i) {
                flush!();
                toks.push(Tok::Node(node));
                i = next;
            } else {
                pending.push(c);
                i += 1;
            }
        } else {
            pending.push(c);
            i += 1;
        }
    }
    flush!();
    toks
}

#[allow(clippy::too_many_lines)]
fn process_emphasis(mut toks: Vec<Tok>) -> Vec<Inline> {
    let mut closer_idx = 0;
    while closer_idx < toks.len() {
        let (closer_ch, closer_count, closer_can_open, closer_can_close) = match &toks[closer_idx] {
            Tok::Delim {
                ch,
                count,
                can_open,
                can_close,
            } => (*ch, *count, *can_open, *can_close),
            Tok::Node(_) => {
                closer_idx += 1;
                continue;
            }
        };
        if !(closer_can_close && closer_count > 0) {
            closer_idx += 1;
            continue;
        }
        // Find the nearest eligible opener.
        let mut found: Option<usize> = None;
        let mut opener_idx = closer_idx as isize - 1;
        while opener_idx >= 0 && found.is_none() {
            if let Tok::Delim {
                ch,
                count,
                can_open,
                can_close,
            } = &toks[opener_idx as usize]
                && *ch == closer_ch
                && *can_open
                && *count > 0
            {
                let sum_ok = if (*can_close || closer_can_open) && closer_ch != '~' {
                    (*count + closer_count) % 3 != 0 || (*count % 3 == 0 && closer_count % 3 == 0)
                } else {
                    true
                };
                if sum_ok {
                    found = Some(opener_idx as usize);
                } else {
                    opener_idx -= 1;
                }
            } else {
                opener_idx -= 1;
            }
        }
        let Some(found) = found else {
            if !closer_can_open {
                toks[closer_idx] = Tok::Node(Inline::Text(repeat_char(closer_ch, closer_count)));
            }
            closer_idx += 1;
            continue;
        };
        let opener_count = match &toks[found] {
            Tok::Delim { count, .. } => *count,
            Tok::Node(_) => {
                closer_idx += 1;
                continue;
            }
        };
        let use_count = if closer_ch == '~' || (opener_count >= 2 && closer_count >= 2) {
            2
        } else {
            1
        };
        let mut inner: Vec<Inline> = Vec::new();
        for tok in toks.iter().take(closer_idx).skip(found + 1) {
            match tok {
                Tok::Node(node) => inner.push(node.clone()),
                Tok::Delim { ch, count, .. } if *count > 0 => {
                    inner.push(Inline::Text(repeat_char(*ch, *count)));
                }
                Tok::Delim { .. } => {}
            }
        }
        let wrapped = if closer_ch == '~' {
            Inline::Strike(inner)
        } else if use_count == 2 {
            Inline::Strong(inner)
        } else {
            Inline::Emph(inner)
        };
        let opener_left = opener_count.saturating_sub(use_count);
        let closer_left = closer_count.saturating_sub(use_count);
        let mut rebuilt: Vec<Tok> = Vec::with_capacity(toks.len());
        rebuilt.extend(toks.iter().take(found).cloned());
        if opener_left > 0 {
            rebuilt.push(Tok::Node(Inline::Text(repeat_char(closer_ch, opener_left))));
        }
        rebuilt.push(Tok::Node(wrapped));
        if closer_left > 0 {
            rebuilt.push(Tok::Node(Inline::Text(repeat_char(closer_ch, closer_left))));
        }
        rebuilt.extend(toks.iter().skip(closer_idx + 1).cloned());
        toks = rebuilt;
        closer_idx = found;
    }
    let mut result: Vec<Inline> = Vec::new();
    for tok in toks {
        match tok {
            Tok::Node(node) => result.push(node),
            Tok::Delim { ch, count, .. } if count > 0 => {
                result.push(Inline::Text(repeat_char(ch, count)));
            }
            Tok::Delim { .. } => {}
        }
    }
    result
}

fn parse_inlines(ctx: Ctx<'_>, text: &[char]) -> Vec<Inline> {
    process_emphasis(tokenize(ctx, text))
}

fn render_inline(ctx: Ctx<'_>, text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    render_inlines(&parse_inlines(ctx, &chars))
}

// ─── Block parsing ───────────────────────────────────────────────────────────

#[derive(Debug)]
struct ListItem {
    task: Option<bool>,
    blocks: Vec<Block>,
}

#[derive(Debug)]
enum Block {
    Heading {
        level: usize,
        text: String,
    },
    Paragraph {
        text: String,
    },
    Hr,
    Fenced {
        lang: String,
        content: String,
    },
    Indented {
        content: String,
    },
    Blockquote {
        blocks: Vec<Block>,
    },
    Bullet {
        tight: bool,
        items: Vec<ListItem>,
    },
    Ordered {
        start: i64,
        tight: bool,
        items: Vec<ListItem>,
    },
    Table {
        headers: Vec<String>,
        aligns: Vec<&'static str>,
        rows: Vec<Vec<String>>,
    },
}

fn leading_indent(s: &str) -> usize {
    let mut n = 0;
    for c in s.chars() {
        match c {
            ' ' => n += 1,
            '\t' => n += 4 - (n % 4),
            _ => break,
        }
    }
    n
}

fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

fn is_thematic_break(line: &str) -> bool {
    let t: String = line
        .trim()
        .chars()
        .filter(|&c| c != ' ' && c != '\t')
        .collect();
    t.chars().count() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

fn try_atx_heading(line: &str) -> Option<(usize, String)> {
    if leading_indent(line) >= 4 {
        return None;
    }
    let t = trim_start_chars(line, " ");
    let chars: Vec<char> = t.chars().collect();
    let mut lvl = 0;
    while lvl < chars.len() && lvl < 7 && chars[lvl] == '#' {
        lvl += 1;
    }
    if lvl == 0 || lvl > 6 {
        return None;
    }
    if lvl < chars.len() && chars[lvl] != ' ' && chars[lvl] != '\t' {
        return None;
    }
    let body: String = chars[lvl.min(chars.len())..]
        .iter()
        .collect::<String>()
        .trim()
        .to_string();
    let stripped = trim_end_chars(trim_end_chars(&body, "#"), " \t").to_string();
    let final_text = if !body.is_empty() && body.chars().all(|c| c == '#') {
        String::new()
    } else {
        stripped
    };
    Some((lvl, final_text))
}

fn parse_align_row(line: &str) -> Option<Vec<&'static str>> {
    let trimmed = line.trim();
    if !trimmed.contains('-') {
        return None;
    }
    let body = trim_chars(trimmed, "|");
    let cells: Vec<&str> = body.split('|').map(str::trim).collect();
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for core in cells {
        if core.is_empty() {
            return None;
        }
        let left = core.starts_with(':');
        let right = core.ends_with(':');
        let dashes = trim_chars(core, ":");
        if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => "center",
            (true, false) => "left",
            (false, true) => "right",
            (false, false) => "none",
        });
    }
    Some(aligns)
}

fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let body = t.strip_prefix('|').unwrap_or(t);
    let body2 = if body.ends_with('|') && !body.ends_with("\\|") {
        &body[..body.len() - 1]
    } else {
        body
    };
    let chars: Vec<char> = body2.chars().collect();
    let mut cells = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() && chars[i + 1] == '|' {
            buf.push('|');
            i += 2;
        } else if c == '|' {
            cells.push(buf.trim().to_string());
            buf = String::new();
            i += 1;
        } else {
            buf.push(c);
            i += 1;
        }
    }
    cells.push(buf.trim().to_string());
    cells
}

fn split_ws_once(s: &str) -> (&str, Option<&str>) {
    match s.find([' ', '\t']) {
        None => (s, None),
        Some(idx) => (&s[..idx], Some(&s[idx + 1..])),
    }
}

fn extract_ref_defs(lines: Vec<String>) -> (Refs, Vec<String>) {
    let mut refs: Refs = HashMap::new();
    let mut kept: Vec<String> = Vec::new();
    for line in lines {
        let t = line.trim();
        let mut handled = false;
        if t.starts_with('[')
            && leading_indent(&line) < 4
            && let Some(close) = t.find(']')
            && close > 1
            && t[close + 1..].starts_with(':')
        {
            let label = &t[1..close];
            let rest = t[close + 2..].trim();
            if !rest.is_empty() && !label.contains(']') {
                let (url, title_part) = split_ws_once(rest);
                let title_part = title_part.map(str::trim).unwrap_or("");
                let url_clean = url
                    .strip_prefix('<')
                    .and_then(|u| u.strip_suffix('>'))
                    .unwrap_or(url);
                let mut title: Option<String> = None;
                if title_part.len() >= 2
                    && (title_part.starts_with('"') || title_part.starts_with('\''))
                {
                    let q = title_part.chars().next().expect("non-empty");
                    if let Some(tc) = title_part[1..].find(q) {
                        title = Some(title_part[1..1 + tc].to_string());
                    }
                }
                refs.insert(norm_label(label), (url_clean.to_string(), title));
                handled = true;
            }
        }
        if !handled {
            kept.push(line);
        }
    }
    (refs, kept)
}

fn is_list_marker(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return false;
    }
    if "-*+".contains(chars[0]) && chars.len() >= 2 && (chars[1] == ' ' || chars[1] == '\t') {
        return true;
    }
    let mut k = 0;
    while k < chars.len() && k < 9 && chars[k].is_ascii_digit() {
        k += 1;
    }
    k > 0
        && k + 1 < chars.len()
        && (chars[k] == '.' || chars[k] == ')')
        && (chars[k + 1] == ' ' || chars[k + 1] == '\t')
}

#[allow(clippy::too_many_lines)]
fn parse_blocks(lines: &[String]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let n = lines.len();
    let mut i = 0;
    while i < n {
        let line = &lines[i];
        if is_blank(line) {
            i += 1;
            continue;
        }
        if is_thematic_break(line) && try_atx_heading(line).is_none() {
            blocks.push(Block::Hr);
            i += 1;
            continue;
        }
        if let Some((level, text)) = try_atx_heading(line) {
            blocks.push(Block::Heading { level, text });
            i += 1;
            continue;
        }
        let indent = leading_indent(line);
        let trimmed_start = trim_start_chars(line, " \t");
        if trimmed_start.starts_with("```") || trimmed_start.starts_with("~~~") {
            let fence = if trimmed_start.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            let info = trimmed_start[3..].trim();
            let lang = if info.is_empty() {
                String::new()
            } else {
                split_ws_once(info).0.to_string()
            };
            let mut content: Vec<&str> = Vec::new();
            let mut j = i + 1;
            let mut closed = false;
            while j < n && !closed {
                let ln = &lines[j];
                let fence_char = fence.chars().next().expect("non-empty fence");
                if trim_start_chars(ln, " \t").starts_with(fence)
                    && trim_end_chars(ln.trim(), &fence_char.to_string()).is_empty()
                {
                    closed = true;
                    j += 1;
                } else {
                    content.push(ln);
                    j += 1;
                }
            }
            blocks.push(Block::Fenced {
                lang,
                content: content.join("\n"),
            });
            i = j;
        } else if indent >= 4 {
            let mut content: Vec<String> = Vec::new();
            let mut j = i;
            while j < n && (leading_indent(&lines[j]) >= 4 || is_blank(&lines[j])) {
                let ln = &lines[j];
                content.push(if is_blank(ln) {
                    String::new()
                } else {
                    ln.chars().skip(4.min(ln.chars().count())).collect()
                });
                j += 1;
            }
            while content.last().is_some_and(String::is_empty) {
                content.pop();
            }
            blocks.push(Block::Indented {
                content: content.join("\n"),
            });
            i = j;
        } else if trimmed_start.starts_with('>') {
            let mut inner: Vec<String> = Vec::new();
            let mut j = i;
            while j < n && trim_start_chars(&lines[j], " \t").starts_with('>') {
                let ln = &trim_start_chars(&lines[j], " \t")[1..];
                inner.push(ln.strip_prefix(' ').unwrap_or(ln).to_string());
                j += 1;
            }
            blocks.push(Block::Blockquote {
                blocks: parse_blocks(&inner),
            });
            i = j;
        } else if line.contains('|')
            && i + 1 < n
            && parse_align_row(&lines[i + 1])
                .is_some_and(|aligns| aligns.len() == split_table_row(line).len())
        {
            let headers = split_table_row(line);
            let aligns = parse_align_row(&lines[i + 1]).expect("checked above");
            let mut rows: Vec<Vec<String>> = Vec::new();
            let mut j = i + 2;
            while j < n && !is_blank(&lines[j]) && lines[j].contains('|') {
                rows.push(split_table_row(&lines[j]));
                j += 1;
            }
            blocks.push(Block::Table {
                headers,
                aligns,
                rows,
            });
            i = j;
        } else if is_list_marker(trimmed_start) {
            let (list_block, nxt) = parse_list(lines, i);
            blocks.push(list_block);
            i = nxt;
        } else {
            let mut para: Vec<String> = Vec::new();
            let mut j = i;
            let mut setext = 0;
            while j < n {
                let ln = &lines[j];
                if is_blank(ln) {
                    break;
                }
                let t = ln.trim();
                if j > i
                    && leading_indent(ln) < 4
                    && !t.is_empty()
                    && (t.chars().all(|c| c == '=') || t.chars().all(|c| c == '-'))
                {
                    setext = if t.starts_with('=') { 1 } else { 2 };
                    j += 1;
                    break;
                }
                if j > i
                    && (is_thematic_break(ln)
                        || try_atx_heading(ln).is_some()
                        || trim_start_chars(ln, " ").starts_with('>')
                        || is_list_marker(trim_start_chars(ln, " \t")))
                {
                    break;
                }
                para.push(trim_start_chars(ln, " \t").to_string());
                j += 1;
            }
            let text = trim_end_chars(&para.join("\n"), " \t\n").to_string();
            if setext > 0 && !text.is_empty() {
                blocks.push(Block::Heading {
                    level: setext,
                    text,
                });
            } else if !text.is_empty() {
                blocks.push(Block::Paragraph { text });
            }
            i = j;
        }
    }
    blocks
}

#[allow(clippy::too_many_lines)]
fn parse_list(lines: &[String], start: usize) -> (Block, usize) {
    let n = lines.len();
    let first = trim_start_chars(&lines[start], " \t");
    let ordered = first.chars().next().is_some_and(|c| c.is_ascii_digit());
    let mut start_num: i64 = 1;
    if ordered {
        let digits: String = first.chars().take_while(char::is_ascii_digit).collect();
        start_num = digits.parse().unwrap_or(1);
    }
    let marker_width = |s: &str| -> usize {
        if !ordered {
            return 2;
        }
        s.chars().take_while(char::is_ascii_digit).count() + 2
    };
    let mut items: Vec<ListItem> = Vec::new();
    let mut i = start;
    let mut tight = true;
    let mut saw_blank_between = false;
    while i < n {
        let raw = &lines[i];
        let trimmed = trim_start_chars(raw, " \t");
        let base_indent = leading_indent(raw);
        if is_blank(raw) {
            saw_blank_between = true;
            i += 1;
        } else if is_list_marker(trimmed)
            && ordered == trimmed.chars().next().is_some_and(|c| c.is_ascii_digit())
            && base_indent < 4
        {
            if saw_blank_between && !items.is_empty() {
                tight = false;
            }
            saw_blank_between = false;
            let mw = marker_width(trimmed);
            let content_offset = base_indent + mw;
            let after_marker: String = trimmed.chars().skip(mw).collect();
            let (task, first_content) = if let Some(rest) = after_marker.strip_prefix("[ ]") {
                (Some(false), trim_start_chars(rest, " ").to_string())
            } else if let Some(rest) = after_marker
                .strip_prefix("[x]")
                .or_else(|| after_marker.strip_prefix("[X]"))
            {
                (Some(true), trim_start_chars(rest, " ").to_string())
            } else {
                (None, after_marker)
            };
            let mut item_lines: Vec<String> = vec![first_content];
            i += 1;
            while i < n {
                let ln = &lines[i];
                if is_blank(ln) {
                    item_lines.push(String::new());
                    i += 1;
                } else if leading_indent(ln) >= content_offset {
                    item_lines.push(
                        ln.chars()
                            .skip(content_offset.min(ln.chars().count()))
                            .collect(),
                    );
                    i += 1;
                } else if is_list_marker(trim_start_chars(ln, " \t")) && leading_indent(ln) < 4 {
                    break;
                } else if leading_indent(ln) > 0 && !is_list_marker(trim_start_chars(ln, " \t")) {
                    item_lines.push(ln.trim().to_string());
                    i += 1;
                } else {
                    break;
                }
            }
            while item_lines.last().is_some_and(String::is_empty) {
                item_lines.pop();
                saw_blank_between = true;
            }
            if item_lines.iter().any(String::is_empty) {
                tight = false;
            }
            items.push(ListItem {
                task,
                blocks: parse_blocks(&item_lines),
            });
        } else {
            break;
        }
    }
    let block = if ordered {
        Block::Ordered {
            start: start_num,
            tight,
            items,
        }
    } else {
        Block::Bullet { tight, items }
    };
    (block, i)
}

// ─── Block rendering ─────────────────────────────────────────────────────────

fn align_attr(a: &str) -> &'static str {
    match a {
        "left" => " align=\"left\"",
        "center" => " align=\"center\"",
        "right" => " align=\"right\"",
        _ => "",
    }
}

fn render_blocks(ctx: Ctx<'_>, blocks: &[Block]) -> String {
    blocks.iter().map(|b| render_block(ctx, b)).collect()
}

fn render_block(ctx: Ctx<'_>, b: &Block) -> String {
    match b {
        Block::Hr => "<hr />\n".to_string(),
        Block::Heading { level, text } => {
            format!("<h{level}>{}</h{level}>\n", render_inline(ctx, text))
        }
        Block::Paragraph { text } => format!("<p>{}</p>\n", render_inline(ctx, text)),
        Block::Fenced { lang, content } => {
            let cls = if lang.is_empty() {
                String::new()
            } else {
                format!(" class=\"language-{}\"", escape_html(lang))
            };
            format!("<pre><code{cls}>{}\n</code></pre>\n", escape_html(content))
        }
        Block::Indented { content } => {
            format!("<pre><code>{}\n</code></pre>\n", escape_html(content))
        }
        Block::Blockquote { blocks } => {
            format!(
                "<blockquote>\n{}</blockquote>\n",
                render_blocks(ctx, blocks)
            )
        }
        Block::Table {
            headers,
            aligns,
            rows,
        } => {
            let mut out = String::from("<table class=\"fuaran-table\"><thead><tr>");
            for (idx, h) in headers.iter().enumerate() {
                let a = aligns.get(idx).copied().unwrap_or("none");
                out.push_str(&format!(
                    "<th class=\"fuaran-table-header\"{}>{}</th>",
                    align_attr(a),
                    render_inline(ctx, h)
                ));
            }
            out.push_str("</tr></thead><tbody>");
            for row in rows {
                out.push_str("<tr class=\"fuaran-table-row\">");
                for idx in 0..headers.len() {
                    let cell = row.get(idx).map(String::as_str).unwrap_or("");
                    let a = aligns.get(idx).copied().unwrap_or("none");
                    out.push_str(&format!(
                        "<td class=\"fuaran-table-cell\"{}>{}</td>",
                        align_attr(a),
                        render_inline(ctx, cell)
                    ));
                }
                out.push_str("</tr>");
            }
            out.push_str("</tbody></table>\n");
            out
        }
        Block::Bullet { tight, items } => {
            format!("<ul>\n{}</ul>\n", render_items(ctx, *tight, items))
        }
        Block::Ordered {
            start,
            tight,
            items,
        } => {
            let start_attr = if *start == 1 {
                String::new()
            } else {
                format!(" start=\"{start}\"")
            };
            format!(
                "<ol{start_attr}>\n{}</ol>\n",
                render_items(ctx, *tight, items)
            )
        }
    }
}

fn render_items(ctx: Ctx<'_>, tight: bool, items: &[ListItem]) -> String {
    let mut out = String::new();
    for item in items {
        let checkbox = match item.task {
            None => "",
            Some(false) => {
                "<input class=\"fuaran-task-checkbox\" disabled=\"\" type=\"checkbox\" /> "
            }
            Some(true) => {
                "<input class=\"fuaran-task-checkbox\" checked=\"\" disabled=\"\" type=\"checkbox\" /> "
            }
        };
        let li_class = if item.task.is_some() {
            " class=\"fuaran-task-item\""
        } else {
            ""
        };
        if tight {
            let mut inner = String::new();
            for blk in &item.blocks {
                if let Block::Paragraph { text } = blk {
                    inner.push_str(&render_inline(ctx, text));
                } else {
                    inner.push('\n');
                    inner.push_str(&render_block(ctx, blk));
                }
            }
            out.push_str(&format!("<li{li_class}>{checkbox}{inner}</li>\n"));
        } else {
            out.push_str(&format!(
                "<li{li_class}>\n{checkbox}{}</li>\n",
                render_blocks(ctx, &item.blocks)
            ));
        }
    }
    out
}

// ─── Public entry points ─────────────────────────────────────────────────────

/// Render GFM markdown `source` to deterministic, cross-host HTML under a
/// destination policy (`WIRE_FORMAT.md` §14.1) — byte-identical to the sibling
/// hosts (verified against the shared markdown corpus). Every link and image
/// destination is checked against `policy`; a refused one renders the inert
/// refusal href plus a `data-fuaran-egress-refused` marker naming the class and
/// the host.
///
/// Escaped by construction; the result still passes through the markdown-HTML
/// sanitiser.
pub fn to_html_with_egress(policy: &EgressPolicy, source: &str) -> String {
    if source.is_empty() {
        return String::new();
    }
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let raw_lines: Vec<String> = normalized.split('\n').map(str::to_string).collect();
    let (refs, lines) = extract_ref_defs(raw_lines);
    let blocks = parse_blocks(&lines);
    let ctx = Ctx {
        refs: &refs,
        policy,
    };
    let html = render_blocks(ctx, &blocks);
    sanitize_markdown_html(&html)
}

/// Render GFM markdown `source` with **every destination permitted** — the pure
/// `source → html` function this renderer has always been, unchanged to the
/// byte.
///
/// This is [`to_html_with_egress`] under [`permissive_egress`], and the
/// equivalence is asserted by the corpus gate rather than merely intended. A
/// host rendering a **decoded (wire)** body wants
/// [`to_html_with_egress`] with a policy it constructed — an emission cannot
/// declare its own egress, so absent a host's declaration it should get none.
pub fn to_html(source: &str) -> String {
    to_html_with_egress(&permissive_egress(), source)
}
