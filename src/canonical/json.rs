//! Hand-rolled JSON value model + parser + canonical renderer.
//!
//! Rust's standard library has no JSON, and the wire contract needs byte-exact
//! canonical output anyway (`WIRE_FORMAT.md` §2), so the layer is hand-written —
//! the same "hand-write the hard part" stance every conformant host takes. The
//! parser is a recursive-descent port of the shared decoder shape: key-order
//! tolerant, sentinel-string number edges left to the typed decoder, structural
//! errors carrying the byte offset. The renderer emits the twelve §2 rules:
//! Ordinal-sorted object keys, the pinned number layout, the minimal escape set.

use super::float::format_finite_double;

/// A parsed JSON value. `Obj` preserves parse order (the decoder looks fields up
/// by name per §2 rule 2; the canonical renderer re-sorts on emit), keeping the
/// last occurrence of a duplicated key, mirroring the reference hosts' map
/// semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum JVal {
    Null,
    Bool(bool),
    /// JSON numbers parse as IEEE-754 doubles — the same numeric model every
    /// conformant host shares; integer slots truncate at the typed decoder.
    Num(f64),
    Str(String),
    Arr(Vec<JVal>),
    Obj(Vec<(String, JVal)>),
}

impl JVal {
    /// Field lookup by key (any key order; last duplicate wins by construction —
    /// the parser replaces on duplicate insert). Returns `None` on a non-object.
    pub fn field(&self, key: &str) -> Option<&JVal> {
        match self {
            JVal::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Structural parse failure carrying the byte offset where it was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
    /// True when this failure is a §21 resource-limit breach rather than a
    /// syntax error. §21.2 rule 2 forbids reporting a limit breach as
    /// `INVALID_JSON` — the input is well-formed and merely too large to walk,
    /// and calling it malformed sends the author to repair the wrong thing.
    /// `decode_node` reads this to choose between the two codes.
    pub limit: bool,
}

// ─── Parser ──────────────────────────────────────────────────────────────────

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    /// Current SYNTACTIC nesting depth (§21.1 MAX_JSON_DEPTH). Incremented on
    /// the way DOWN, before the recursion that would breach it (§21.2 rule 4).
    /// Without it `parse_value` / `parse_object` / `parse_array` are unbounded
    /// mutual recursion, and a Rust stack overflow ABORTS THE PROCESS — not a
    /// catchable condition, so no `Result` could ever be returned.
    depth: usize,
}

type PResult<T> = Result<T, ParseError>;

impl<'a> Parser<'a> {
    /// A §21 resource-limit refusal. Distinct from `fail` only in the flag,
    /// which is what stops the breach being reported as a syntax error above.
    fn fail_limit<T>(&self) -> PResult<T> {
        Err(ParseError {
            message: format!(
                "JSON nesting deeper than the wire limit MAX_JSON_DEPTH = {}",
                crate::limits::MAX_JSON_DEPTH
            ),
            offset: self.pos,
            limit: true,
        })
    }

    fn fail<T>(&self, message: impl Into<String>) -> PResult<T> {
        Err(ParseError {
            message: message.into(),
            offset: self.pos,
            limit: false,
        })
    }

    fn peek(&self) -> u8 {
        // Past-the-end reads as a space, mirroring the reference parser — every
        // consumer then fails with a structural message rather than panicking.
        if self.pos < self.bytes.len() {
            self.bytes[self.pos]
        } else {
            b' '
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn expect(&mut self, ch: u8) -> PResult<()> {
        if self.peek() == ch {
            self.pos += 1;
            Ok(())
        } else {
            self.fail(format!(
                "expected '{}' but found '{}'",
                ch as char,
                self.peek() as char
            ))
        }
    }

    fn parse_string_raw(&mut self) -> PResult<String> {
        self.expect(b'"')?;
        let mut out = String::new();
        // Pending high surrogate from a `\uD800`–`\uDBFF` escape, awaiting its
        // low half; a lone half lowers to U+FFFD (Rust strings cannot carry it).
        let mut pending_high: Option<u16> = None;
        loop {
            if self.pos >= self.bytes.len() {
                return self.fail("unterminated string");
            }
            let c = self.bytes[self.pos];
            if c != b'\\' && pending_high.take().is_some() {
                out.push('\u{FFFD}');
            }
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.bytes.len() {
                        return self.fail("unterminated escape");
                    }
                    let esc = self.bytes[self.pos];
                    self.pos += 1;
                    let simple = match esc {
                        b'"' => Some('"'),
                        b'\\' => Some('\\'),
                        b'/' => Some('/'),
                        b'b' => Some('\u{0008}'),
                        b'f' => Some('\u{000C}'),
                        b'n' => Some('\n'),
                        b'r' => Some('\r'),
                        b't' => Some('\t'),
                        b'u' => None,
                        other => {
                            return self.fail(format!("unknown escape '\\{}'", other as char));
                        }
                    };
                    match simple {
                        Some(ch) => {
                            if pending_high.take().is_some() {
                                out.push('\u{FFFD}');
                            }
                            out.push(ch);
                        }
                        None => {
                            let unit = self.parse_hex4()?;
                            match pending_high.take() {
                                Some(high) if (0xDC00..=0xDFFF).contains(&unit) => {
                                    let combined = 0x10000
                                        + ((u32::from(high) - 0xD800) << 10)
                                        + (u32::from(unit) - 0xDC00);
                                    out.push(
                                        char::from_u32(combined).expect("valid surrogate pair"),
                                    );
                                }
                                Some(_) => {
                                    out.push('\u{FFFD}');
                                    self.push_unit(&mut out, unit, &mut pending_high);
                                }
                                None => self.push_unit(&mut out, unit, &mut pending_high),
                            }
                        }
                    }
                }
                _ => {
                    // Consume one UTF-8 sequence verbatim (non-ASCII passes
                    // through literally per §2 rule 1).
                    let len = utf8_len(c);
                    let end = (self.pos + len).min(self.bytes.len());
                    match std::str::from_utf8(&self.bytes[self.pos..end]) {
                        Ok(s) if !s.is_empty() => {
                            out.push_str(s);
                            self.pos = end;
                        }
                        _ => {
                            out.push('\u{FFFD}');
                            self.pos += 1;
                        }
                    }
                }
            }
        }
    }

    fn push_unit(&self, out: &mut String, unit: u16, pending_high: &mut Option<u16>) {
        if (0xD800..=0xDBFF).contains(&unit) {
            *pending_high = Some(unit);
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            out.push('\u{FFFD}');
        } else {
            out.push(char::from_u32(u32::from(unit)).expect("non-surrogate BMP unit"));
        }
    }

    fn parse_hex4(&mut self) -> PResult<u16> {
        if self.pos + 4 > self.bytes.len() {
            return self.fail("incomplete \\u escape");
        }
        let hex = &self.bytes[self.pos..self.pos + 4];
        let mut value: u16 = 0;
        for &b in hex {
            let digit = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => {
                    return self.fail(format!(
                        "invalid \\u escape '{}'",
                        String::from_utf8_lossy(hex)
                    ));
                }
            };
            value = value * 16 + u16::from(digit);
        }
        self.pos += 4;
        Ok(value)
    }

    fn parse_number(&mut self) -> PResult<f64> {
        let start = self.pos;
        while self.pos < self.bytes.len() && is_number_byte(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let slice = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        if slice.is_empty() {
            return self.fail(format!("invalid number '{slice}'"));
        }
        match slice.parse::<f64>() {
            Ok(n) if !n.is_nan() => Ok(n),
            _ => self.fail(format!("invalid number '{slice}'")),
        }
    }

    fn parse_value(&mut self) -> PResult<JVal> {
        self.skip_ws();
        match self.peek() {
            b'{' => {
                // §21.2 rule 4 — refuse BEFORE descending. A check after the
                // walk has already paid the cost it exists to refuse, and here
                // it would never run at all: the overflow is fatal.
                if self.depth >= crate::limits::MAX_JSON_DEPTH {
                    return self.fail_limit();
                }
                self.depth += 1;
                let r = self.parse_object();
                self.depth -= 1;
                r
            }
            b'[' => {
                if self.depth >= crate::limits::MAX_JSON_DEPTH {
                    return self.fail_limit();
                }
                self.depth += 1;
                let r = self.parse_array();
                self.depth -= 1;
                r
            }
            b'"' => self.parse_string_raw().map(JVal::Str),
            b't' => {
                if self.bytes[self.pos..].starts_with(b"true") {
                    self.pos += 4;
                    Ok(JVal::Bool(true))
                } else {
                    self.fail("expected 'true'")
                }
            }
            b'f' => {
                if self.bytes[self.pos..].starts_with(b"false") {
                    self.pos += 5;
                    Ok(JVal::Bool(false))
                } else {
                    self.fail("expected 'false'")
                }
            }
            b'n' => {
                if self.bytes[self.pos..].starts_with(b"null") {
                    self.pos += 4;
                    Ok(JVal::Null)
                } else {
                    self.fail("expected 'null'")
                }
            }
            _ => self.parse_number().map(JVal::Num),
        }
    }

    fn parse_object(&mut self) -> PResult<JVal> {
        self.expect(b'{')?;
        self.skip_ws();
        let mut fields: Vec<(String, JVal)> = Vec::new();
        if self.peek() == b'}' {
            self.pos += 1;
            return Ok(JVal::Obj(fields));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string_raw()?;
            self.skip_ws();
            self.expect(b':')?;
            let value = self.parse_value()?;
            match fields.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value, // duplicate key: last wins
                None => fields.push((key, value)),
            }
            self.skip_ws();
            match self.peek() {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Ok(JVal::Obj(fields));
                }
                other => {
                    return self.fail(format!(
                        "expected ',' or '}}' but found '{}'",
                        other as char
                    ));
                }
            }
        }
    }

    fn parse_array(&mut self) -> PResult<JVal> {
        self.expect(b'[')?;
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == b']' {
            self.pos += 1;
            return Ok(JVal::Arr(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Ok(JVal::Arr(items));
                }
                other => {
                    return self.fail(format!("expected ',' or ']' but found '{}'", other as char));
                }
            }
        }
    }
}

fn is_number_byte(b: u8) -> bool {
    matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Parse a JSON document. Empty / whitespace-only input is a structural error;
/// a single top-level value is parsed and trailing content is not inspected,
/// mirroring the reference hosts' parser.
pub fn parse(input: &str) -> Result<JVal, ParseError> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
        depth: 0,
    };
    p.skip_ws();
    if p.pos >= p.bytes.len() {
        return Err(ParseError {
            message: "input is empty".to_string(),
            offset: 0,
            limit: false,
        });
    }
    p.parse_value()
}

// ─── Canonical renderer (§2) ─────────────────────────────────────────────────

/// Quote + escape a string per §2 rule 6: only `"`, `\`, and the C0 control
/// characters escape (`\uXXXX`, lower-case hex); everything else — `/`
/// included — passes through literally.
pub fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The §2 rule-5 number form: finite doubles in the canonical layout, the IEEE
/// specials as quoted sentinel strings, negative zero collapsed to `0`.
pub fn format_number(n: f64) -> String {
    if n.is_nan() {
        "\"NaN\"".to_string()
    } else if n == f64::INFINITY {
        "\"Infinity\"".to_string()
    } else if n == f64::NEG_INFINITY {
        "\"-Infinity\"".to_string()
    } else {
        format_finite_double(n)
    }
}

/// Ordinal comparison on UTF-16 code units — `StringComparer.Ordinal`, the §2
/// rule-2 key order every host sorts by. Differs from Rust's default `str`
/// ordering (Unicode scalar values) only above the BMP; pinned here so a
/// supplementary-plane key cannot silently diverge across hosts.
pub fn ordinal_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Assemble an object from `(key, rendered-value)` pairs: Ordinal-sorted keys,
/// no whitespace. The building block every encoder arm uses.
pub fn render_object(fields: &mut [(String, String)]) -> String {
    fields.sort_by(|a, b| ordinal_cmp(&a.0, &b.0));
    let mut out = String::from("{");
    for (i, (k, v)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&escape_string(k));
        out.push(':');
        out.push_str(v);
    }
    out.push('}');
    out
}

/// Assemble an array from rendered item strings (source order, §2 rule 3).
pub fn render_array(items: &[String]) -> String {
    let mut out = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(item);
    }
    out.push(']');
    out
}

/// Re-render a parsed [`JVal`] to canonical wire bytes — Ordinal-sorted keys,
/// the rule-5 number layout, the rule-6 escapes. For input that was already
/// canonical, `render_canonical(&parse(x)?) == x`.
pub fn render_canonical(v: &JVal) -> String {
    match v {
        JVal::Null => "null".to_string(),
        JVal::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        JVal::Num(n) => format_number(*n),
        JVal::Str(s) => escape_string(s),
        JVal::Arr(items) => {
            let rendered: Vec<String> = items.iter().map(render_canonical).collect();
            render_array(&rendered)
        }
        JVal::Obj(fields) => {
            let mut rendered: Vec<(String, String)> = fields
                .iter()
                .map(|(k, v)| (k.clone(), render_canonical(v)))
                .collect();
            render_object(&mut rendered)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_empty_and_garbage() {
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
        assert!(parse("not json").is_err());
        assert!(parse("{\"a\":").is_err());
    }

    #[test]
    fn canonical_round_trip_is_byte_stable() {
        let cases = [
            "{}",
            "[]",
            "{\"a\":1,\"b\":[true,false,null],\"c\":\"x\"}",
            "{\"$type\":\"Static\",\"value\":\"<opaque>\"}",
            "{\"n\":0.30000000000000004,\"z\":1E+21}",
            "\"a\\\\b\\\"c\\u0001\"",
        ];
        for case in cases {
            let parsed = parse(case).expect(case);
            assert_eq!(render_canonical(&parsed), case, "round-trip of {case}");
        }
    }

    #[test]
    fn keys_sort_ordinal_on_render() {
        let parsed = parse("{\"b\":1,\"a\":2,\"$type\":\"X\"}").unwrap();
        assert_eq!(
            render_canonical(&parsed),
            "{\"$type\":\"X\",\"a\":2,\"b\":1}"
        );
    }

    #[test]
    fn number_edges_render_as_sentinels() {
        assert_eq!(format_number(f64::NAN), "\"NaN\"");
        assert_eq!(format_number(f64::INFINITY), "\"Infinity\"");
        assert_eq!(format_number(f64::NEG_INFINITY), "\"-Infinity\"");
        assert_eq!(format_number(-0.0), "0");
    }

    #[test]
    fn control_chars_escape_lowercase_hex() {
        assert_eq!(escape_string("\u{001F}"), "\"\\u001f\"");
        assert_eq!(escape_string("a/b"), "\"a/b\""); // '/' not escaped
        assert_eq!(escape_string("é✓"), "\"é✓\""); // non-ASCII literal
    }
}
