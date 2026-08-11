//! A tiny string-HTML builder + the escaping floor. The server renderer emits
//! HTML *strings* — no DOM, no template engine — and this module is the single
//! seam where a string becomes HTML. Attribute *values* escape here; attribute
//! *keys* are renderer-controlled (the class vocabulary, `data-*` markers,
//! ARIA keys) — untrusted seams (URLs) are filtered upstream by
//! [`super::sanitize`] before they reach this module.

/// An attribute value: a string, a bare boolean attribute, or omitted.
#[derive(Debug, Clone)]
pub enum AttrVal {
    Str(String),
    /// `true` → the bare attribute (`disabled`); `false` → omitted entirely.
    Flag(bool),
}

impl From<&str> for AttrVal {
    fn from(v: &str) -> Self {
        AttrVal::Str(v.to_string())
    }
}

impl From<String> for AttrVal {
    fn from(v: String) -> Self {
        AttrVal::Str(v)
    }
}

impl From<bool> for AttrVal {
    fn from(v: bool) -> Self {
        AttrVal::Flag(v)
    }
}

impl From<i64> for AttrVal {
    fn from(v: i64) -> Self {
        AttrVal::Str(v.to_string())
    }
}

/// An attribute pair.
pub type Attr = (&'static str, AttrVal);

/// Escape a string for HTML *text* content.
pub fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape a string for an HTML double-quoted *attribute* value.
pub fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn attr_string(attrs: &[Attr]) -> String {
    let mut out = String::new();
    for (name, value) in attrs {
        match value {
            AttrVal::Flag(false) => {}
            AttrVal::Flag(true) => {
                out.push(' ');
                out.push_str(name);
            }
            AttrVal::Str(v) => {
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                out.push_str(&escape_attr(v));
                out.push('"');
            }
        }
    }
    out
}

/// Render a non-void element with pre-escaped / pre-rendered `inner` HTML.
pub fn el(tag: &str, attrs: &[Attr], inner: &str) -> String {
    format!("<{tag}{}>{inner}</{tag}>", attr_string(attrs))
}

/// Render a void element (`input` / `img` / …) — self-closing, no children.
pub fn void_el(tag: &str, attrs: &[Attr]) -> String {
    format!("<{tag}{} />", attr_string(attrs))
}

/// Render an element whose only child is escaped text content.
pub fn text_el(tag: &str, attrs: &[Attr], text: &str) -> String {
    el(tag, attrs, &escape_text(text))
}

/// Emit every UTF-16 code unit of the string as a decimal HTML entity
/// (`&#78;`) — the protected-link emission. Code-unit iteration (not code
/// points) matches the sibling hosts' per-char encode exactly, keeping the
/// emissions byte-identical across hosts.
pub fn entity_encode(value: &str) -> String {
    let mut out = String::new();
    for u in value.encode_utf16() {
        out.push_str("&#");
        out.push_str(&u.to_string());
        out.push(';');
    }
    out
}
