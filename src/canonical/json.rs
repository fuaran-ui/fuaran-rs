//! Hand-rolled JSON value model + parser + canonical renderer.
//!
//! Rust's standard library has no JSON, and the wire contract needs byte-exact
//! canonical output anyway (`WIRE_FORMAT.md` §2), so the layer is hand-written —
//! the same "hand-write the hard part" stance every conformant host takes. The
//! parser is a recursive-descent port of the shared decoder shape: key-order
//! tolerant, sentinel-string number edges left to the typed decoder, structural
//! errors carrying the byte offset. The renderer emits the twelve §2 rules:
//! Ordinal-sorted object keys, the pinned number layout, the minimal escape set.
//!
//! The parser is also where the §20 decode-determinism rules live, because
//! §20.1 binds each of them to an entry point and this is the one every reader
//! in this crate reaches: a repeated member, content after the root value, a
//! number outside the RFC 8259 grammar, a bare `NaN`, a raw C0 control
//! character and an unpaired surrogate are each refused here, on the way down.

use super::float::format_finite_double;

/// A parsed JSON value. `Obj` preserves parse order (the decoder looks fields up
/// by name per §2 rule 2; the canonical renderer re-sorts on emit). A duplicated
/// key never reaches this type at all — §20.2 row 1 refuses it at the parser,
/// because "which occurrence wins" was answered differently by different hosts
/// and the disagreement was silent.
#[derive(Debug, Clone, PartialEq)]
pub enum JVal {
    Null,
    Bool(bool),
    /// JSON numbers parse as IEEE-754 doubles.
    ///
    /// §2 rule 5 makes that conformant rather than a limitation, and the
    /// reasoning is worth keeping because the obvious remedy is unnecessary:
    /// integer identity on the wire stops at ±(2⁵³−1), and the bound was chosen
    /// precisely because the integer and float canonical layouts AGREE exactly
    /// over that range — a double holds every integer in it, and rule 5's
    /// fixed-point window (base-10 exponent ≤ 16) covers every one of them, so
    /// re-encoding produces the integer spelling with no integer type in play.
    /// Beyond the bound a conformant encoder must not emit an integer token at
    /// all, so there is nothing an `i64` arm could preserve that this host is
    /// required to preserve.
    Num(f64),
    Str(String),
    Arr(Vec<JVal>),
    Obj(Vec<(String, JVal)>),
}

impl JVal {
    /// Field lookup by key, in any key order. A duplicate cannot occur — the
    /// parser refuses one under §20.2 row 1 — so the first match is the only
    /// match. Returns `None` on a non-object.
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
        self.fail_limit_msg(format!(
            "JSON nesting deeper than the wire limit MAX_JSON_DEPTH = {}",
            crate::limits::MAX_JSON_DEPTH
        ))
    }

    fn fail_limit_msg<T>(&self, message: impl Into<String>) -> PResult<T> {
        Err(ParseError {
            message: message.into(),
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

    /// Parse one string literal.
    ///
    /// Three §20/§21 rules are enforced HERE rather than after the fact, and
    /// each of them has to be:
    ///
    /// * **§20.2 row 6 — an unpaired surrogate is `INVALID_JSON`.** This host
    ///   used to lower a lone half to U+FFFD, silently, so the same bytes meant
    ///   one thing here and another on a host that kept the code unit. The check
    ///   cannot be moved later: a Rust `String` holds Unicode scalar values, so
    ///   by the time the literal is assembled the evidence is gone — and even on
    ///   a UTF-16 host, an assembled string cannot tell a pair from two lone
    ///   halves. A high half must be followed IMMEDIATELY by a low half.
    /// * **§20.2 row 5 — a raw C0 control character is `INVALID_JSON`.** RFC
    ///   8259 requires them escaped and §2 rule 6 requires a conformant encoder
    ///   to escape them, so passing one through admits input this host's own
    ///   encoder cannot produce. The escaped spelling stays legal.
    /// * **§21.1 / §21.6 — the string bound, counted in CODE POINTS**, and
    ///   counted as the literal is built rather than measured afterwards. Both
    ///   halves matter: measuring afterwards has already paid the allocation the
    ///   bound exists to refuse, and counting bytes (`String::len`) would make
    ///   the allowance depend on the alphabet the author writes in — a CJK
    ///   document would get a third of the room a Latin one gets.
    fn parse_string_raw(&mut self) -> PResult<String> {
        self.expect(b'"')?;
        let mut out = String::new();
        let mut code_points: usize = 0;
        loop {
            if self.pos >= self.bytes.len() {
                return self.fail("unterminated string");
            }
            if code_points > crate::limits::MAX_STRING_LENGTH {
                return self.fail_limit_msg(format!(
                    "a string is longer than the wire limit MAX_STRING_LENGTH = {}",
                    crate::limits::MAX_STRING_LENGTH
                ));
            }
            let c = self.bytes[self.pos];
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
                            out.push(ch);
                            code_points += 1;
                        }
                        None => {
                            let unit = self.parse_hex4()?;
                            let scalar = self.resolve_escape_unit(unit)?;
                            out.push(scalar);
                            code_points += 1;
                        }
                    }
                }
                0x00..=0x1F => {
                    return self.fail(format!(
                        "a raw control character U+{c:04X} inside a string \
                         (WIRE_FORMAT.md §20.2 row 5): RFC 8259 requires it escaped, \
                         and a conformant encoder emits the escaped spelling"
                    ));
                }
                _ => {
                    // Consume one UTF-8 sequence verbatim (non-ASCII passes
                    // through literally per §2 rule 1).
                    let len = utf8_len(c);
                    let end = (self.pos + len).min(self.bytes.len());
                    match std::str::from_utf8(&self.bytes[self.pos..end]) {
                        Ok(s) if !s.is_empty() => {
                            out.push_str(s);
                            code_points += s.chars().count();
                            self.pos = end;
                        }
                        _ => {
                            out.push('\u{FFFD}');
                            code_points += 1;
                            self.pos += 1;
                        }
                    }
                }
            }
        }
    }

    /// Resolve one `\uXXXX` escape to the scalar value it denotes, consuming a
    /// following low half when this one is a high half (§20.2 row 6).
    ///
    /// Called with `self.pos` just past the four hex digits.
    fn resolve_escape_unit(&mut self, unit: u16) -> PResult<char> {
        if (0xD800..=0xDBFF).contains(&unit) {
            // "Immediately" is the whole content of the rule: a host that merely
            // counts surrogates, rather than requiring adjacency, reassembles a
            // scalar the author never wrote out of two halves that happened to
            // co-occur.
            let has_low = self.pos + 1 < self.bytes.len()
                && self.bytes[self.pos] == b'\\'
                && self.bytes[self.pos + 1] == b'u';
            if !has_low {
                return self.fail_surrogate("HIGH", unit,
                    "a \\uD800-\\uDBFF escape must be followed immediately by a \\uDC00-\\uDFFF escape");
            }
            self.pos += 2;
            let low = self.parse_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return self.fail_surrogate("HIGH", unit,
                    "a \\uD800-\\uDBFF escape must be followed immediately by a \\uDC00-\\uDFFF escape");
            }
            let combined =
                0x10000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
            return Ok(char::from_u32(combined).expect("a paired surrogate is a valid scalar"));
        }
        if (0xDC00..=0xDFFF).contains(&unit) {
            // A low half is only ever consumed above, as the second element of a
            // pair, so reaching it here means it stands alone.
            return self.fail_surrogate("LOW", unit,
                "a \\uDC00-\\uDFFF escape must be preceded immediately by a \\uD800-\\uDBFF escape");
        }
        Ok(char::from_u32(u32::from(unit)).expect("a non-surrogate BMP unit is a valid scalar"))
    }

    fn fail_surrogate<T>(&self, half: &str, unit: u16, why: &str) -> PResult<T> {
        self.fail(format!(
            "an unpaired {half} surrogate escape \\u{unit:04X} \
             (WIRE_FORMAT.md §20.2 row 6): {why}"
        ))
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

    /// Parse one number token, checking the **RFC 8259 grammar before the
    /// platform parser** (§20.2 row 3).
    ///
    /// The order is the fix, not an optimisation. Asking `str::parse::<f64>()`
    /// "is this a number" asks about Rust, not about this format: Rust's own
    /// float grammar accepts `+1`, `.5`, `5.` and `01`, none of which RFC 8259
    /// permits, and a different host's parser accepts a different subset. Row 4
    /// falls out of the same check — `NaN` and `inf` are Rust float literals and
    /// would otherwise be admitted as bare tokens, where §7's QUOTED sentinels
    /// are the specified representation.
    ///
    /// Row 7's `1e999` is deliberately untouched: it is a well-formed JSON
    /// number whose value is not representable, and IEEE-754 already specifies
    /// what a finite decimal that overflows becomes.
    fn parse_number(&mut self) -> PResult<f64> {
        let start = self.pos;
        while self.pos < self.bytes.len() && is_number_byte(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let slice = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        if !is_rfc8259_number(slice) {
            return self.fail(format!(
                "'{slice}' is not a number in the RFC 8259 grammar (WIRE_FORMAT.md §20.2 row 3)"
            ));
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
            // §20.2 row 1 — a repeated member is INVALID_JSON, not a
            // last-wins overwrite. This is one of the two rows that change
            // what a document MEANS rather than whether it is accepted: this
            // host kept the LAST occurrence and the reference host the FIRST,
            // so a vetting host and a rendering host read different trees
            // from identical bytes with no error raised anywhere.
            if fields.iter().any(|(k, _)| *k == key) {
                return self.fail(format!(
                    "the object member '{key}' appears more than once \
                     (WIRE_FORMAT.md §20.2 row 1): hosts disagreed on which \
                     occurrence wins, so the same bytes meant different trees"
                ));
            }
            fields.push((key, value));
            if fields.len() > crate::limits::MAX_ARRAY_LENGTH {
                return self.fail_limit_msg(format!(
                    "an object has more members than the wire limit MAX_ARRAY_LENGTH = {}",
                    crate::limits::MAX_ARRAY_LENGTH
                ));
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
            if items.len() > crate::limits::MAX_ARRAY_LENGTH {
                return self.fail_limit_msg(format!(
                    "an array is longer than the wire limit MAX_ARRAY_LENGTH = {}",
                    crate::limits::MAX_ARRAY_LENGTH
                ));
            }
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

/// The RFC 8259 number production, exactly:
///
/// ```text
/// number = [ "-" ] int [ frac ] [ exp ]
/// int    = "0" / ( digit1-9 *DIGIT )
/// frac   = "." 1*DIGIT
/// exp    = ("e" / "E") [ "+" / "-" ] 1*DIGIT
/// ```
///
/// Written out rather than delegated because delegating is the defect: every
/// platform's float parser accepts a different superset, so the accept set of a
/// host that reaches for one is a property of its runtime rather than of this
/// format. The five shapes the corpus pins — `+1`, `01`, `.5`, `1.`, `1e` — are
/// each accepted by at least one host's platform parser and by none of this.
fn is_rfc8259_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0usize;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    // int
    match b.get(i).copied() {
        Some(b'0') => i += 1,
        Some(d) if d.is_ascii_digit() => {
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
        _ => return false,
    }
    // frac
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    // exp
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == b.len() && !b.is_empty()
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Parse a JSON document: exactly ONE top-level value, and nothing after it.
///
/// Empty / whitespace-only input is a structural error. Content after the root
/// value is `INVALID_JSON` per §20.2 row 2 — §1 makes a wire artefact a single
/// JSON document, and stopping at the root value while ignoring the remainder
/// leaves a framing ambiguity rather than a tolerance: two hosts refused such an
/// input and three accepted it.
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
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        return p.fail("input carries content after the JSON document (WIRE_FORMAT.md §20.2 row 2)");
    }
    Ok(value)
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
    fn parse_requires_exactly_one_document() {
        // §20.2 row 2 — §1 makes a wire artefact a single JSON document, so
        // stopping at the root value and ignoring the remainder is a framing
        // ambiguity rather than a tolerance.
        assert!(parse("{} {}").is_err());
        assert!(parse("{}}").is_err());
        assert!(parse("1 2").is_err());
        // Trailing whitespace is not trailing content.
        assert!(parse("  {}  \n").is_ok());
    }

    #[test]
    fn parse_refuses_a_repeated_member() {
        // §20.2 row 1 — the row that changes what a document MEANS. Last-wins
        // here, first-wins on the reference host, and no error on either.
        assert!(parse("{\"a\":1,\"a\":2}").is_err());
        assert!(parse("{\"o\":{\"a\":1,\"a\":2}}").is_err());
        // The same key in SIBLING objects is the ordinary shape of every tree.
        assert!(parse("[{\"a\":1},{\"a\":2}]").is_ok());
    }

    #[test]
    fn parse_applies_the_rfc_8259_number_grammar() {
        // §20.2 row 3. Every one of these is accepted by Rust's own float
        // parser, which is why the grammar is checked before reaching it.
        for bad in ["+1", "01", ".5", "1.", "1e", "1e+", "0x10", "1.2.3"] {
            assert!(parse(bad).is_err(), "expected {bad} to be refused");
        }
        for good in ["0", "-0", "1", "-1", "1.5", "1e5", "1E-7", "1e+21"] {
            assert!(parse(good).is_ok(), "expected {good} to parse");
        }
    }

    #[test]
    fn parse_refuses_bare_non_finite_literals() {
        // §20.2 row 4. `NaN` and `inf` are Rust float literals, so they would
        // reach `str::parse::<f64>()` and be admitted without the grammar check.
        for bad in ["NaN", "Infinity", "-Infinity", "inf", "-inf", "nan"] {
            assert!(parse(bad).is_err(), "expected the bare {bad} to be refused");
        }
        // Row 7 is the one row that ratifies an ACCEPT, and it sits beside this
        // one refusing the same value written as a bare literal.
        assert_eq!(parse("1e999").unwrap(), JVal::Num(f64::INFINITY));
    }

    #[test]
    fn parse_refuses_a_raw_control_character_and_accepts_the_escape() {
        // §20.2 row 5 — a conformant encoder escapes them, so accepting the raw
        // byte admits input this host's own encoder cannot produce.
        assert!(parse("\"a\tb\"").is_err());
        assert!(parse("\"a\u{0000}b\"").is_err());
        assert!(parse("\"a\\tb\"").is_ok());
    }

    #[test]
    fn parse_refuses_unpaired_surrogates_and_keeps_the_pair() {
        // §20.2 row 6. This host lowered every one of the first four to U+FFFD,
        // silently, so the same bytes meant one thing here and another on a host
        // that kept the code unit.
        for bad in [
            "\"\\ud83d\"",
            "\"\\ude00\"",
            "\"\\ud83d x \\ude00\"",
            "\"\\ud83dA\"",
        ] {
            assert!(parse(bad).is_err(), "expected {bad} to be refused");
        }
        // The corrected twin: both halves, adjacent, denoting U+1F600. Refusing
        // every escape would otherwise look like a fix.
        assert_eq!(
            parse("\"\\ud83d\\ude00\"").unwrap(),
            JVal::Str("\u{1F600}".to_string())
        );
    }

    #[test]
    fn parse_bounds_a_string_in_code_points() {
        // §21.6 — the unit is the code point, so an astral character costs ONE.
        // Counting UTF-8 bytes (Rust's `len`) would charge it four, and it is
        // the at-the-limit astral case that then fails: the over-limit one
        // passes under every candidate unit, so a suite carrying only the
        // refusal never notices the unit is wrong.
        let at_limit = "\u{1D11E}".repeat(crate::limits::MAX_STRING_LENGTH);
        assert!(parse(&format!("\"{at_limit}\"")).is_ok());
        let over = "\u{1D11E}".repeat(crate::limits::MAX_STRING_LENGTH + 1);
        let e = parse(&format!("\"{over}\"")).unwrap_err();
        assert!(
            e.limit,
            "a limit breach must not be reported as a syntax error"
        );
    }

    #[test]
    fn control_chars_escape_lowercase_hex() {
        assert_eq!(escape_string("\u{001F}"), "\"\\u001f\"");
        assert_eq!(escape_string("a/b"), "\"a/b\""); // '/' not escaped
        assert_eq!(escape_string("é✓"), "\"é✓\""); // non-ASCII literal
    }
}
