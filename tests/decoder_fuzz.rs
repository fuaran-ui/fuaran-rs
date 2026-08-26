//! Decoder robustness fuzz — this host's leg.
//!
//! The threat model's load-bearing claim is that decoding is TOTAL: a malformed
//! or hostile input yields a structured, typed error, never a panic and never a
//! hang. Until a fuzz leg exists on a host, that claim rests there on a CURATED
//! reject corpus — inputs an author chose, which is evidence about the author's
//! imagination rather than about the decoder.
//!
//! ## Why this is a hand-written harness and not `cargo-fuzz`
//!
//! `cargo-fuzz` and `proptest` are the obvious reaches, and both were declined
//! for the same reason: this crate declares **no third-party dependencies**, and
//! `cargo-fuzz` additionally needs a nightly toolchain and a sanitizer runtime
//! the CI gate does not carry. A fuzz leg reachable only under a toolchain the
//! gate does not run is a leg the gate does not have. What is kept from those
//! tools is what actually matters — a deterministic, replayable generator over
//! the five named input families, and a minimiser — at the cost of losing
//! coverage-guided mutation, which is stated here rather than left to be
//! assumed. (The Go sibling's leg keeps coverage guidance, because its runtime
//! ships it in the standard toolchain; the two legs are complementary.)
//!
//! ## The invariants, per input
//!
//! 1. **Totality** — `decode_node` / `decode_op` return a `DecodeError` or a
//!    value. An escaping panic is a counterexample, caught by `catch_unwind`.
//! 2. **Termination** — it returns inside a time budget.
//! 3. **Bounded work** — allocation stays inside a budget proportional to the
//!    input. Measured for real: this file installs a counting global allocator,
//!    which is the one place in this workspace where the reference host's
//!    allocated-bytes invariant ports EXACTLY rather than by proxy.
//! 4. **Fixed point** — an accepted input's canonical form re-decodes and
//!    re-encodes to itself, fuzzed over the reachable accept-space rather than
//!    pinned by fixtures.
//!
//! ## Determinism
//!
//! SplitMix64, hand-rolled: replayability is the whole point of the seed, and
//! the standard library has no seedable PRNG at all.
//!
//! Long run, and the machine-readable evidence a scheduled job collects:
//!
//! ```text
//! FUARAN_FUZZ_LONG=1 FUARAN_FUZZ_ITERATIONS=250000 \
//!   FUARAN_FUZZ_EVIDENCE=<file> cargo test --test decoder_fuzz -- --nocapture
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use fuaran_rs::wire::{decode_node, decode_op, encode_node, encode_op};

// ─── The counting allocator ─────────────────────────────────────────────────
//
// A `#[global_allocator]` in an integration test binary governs that binary
// only, so this measures the harness's own process and nothing else.
//
// The counter is PER THREAD, and that is the whole design rather than a detail.
// A process-wide `AtomicUsize` was the first shape here and it was wrong in a way
// that looked exactly like a decoder defect: the test binary runs its tests in
// PARALLEL by default, so a sibling test allocating a 16 MiB string landed inside
// the fuzz run's measurement window and produced a run of `over-allocated`
// counterexamples on inputs the decoder handles in kilobytes. The reference
// host's invariant is `GetAllocatedBytesForCurrentThread` for the same reason,
// and a per-thread counter is what ports it faithfully.
//
// Two constraints make the implementation what it is. A `thread_local!` whose
// initialiser allocates would re-enter the allocator on first use and recurse
// forever, so the cell is `const`-initialised. And TLS is torn down before the
// last deallocations on a thread, so every access goes through `try_with` and
// treats an unavailable cell as "not counting" rather than panicking inside the
// allocator.

struct CountingAlloc;

thread_local! {
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

/// Process-wide, kept ONLY so the self-test can prove the hook is installed at
/// all. It is never used as a measurement: see the note above.
static TOTAL_ALLOC_EVENTS: AtomicUsize = AtomicUsize::new(0);

fn note_alloc(bytes: usize) {
    TOTAL_ALLOC_EVENTS.fetch_add(1, Ordering::Relaxed);
    let _ = ALLOCATED.try_with(|c| c.set(c.get().wrapping_add(bytes)));
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // A realloc that GROWS allocates the difference; one that shrinks does
        // not. Counting the whole new size would double-count the copy and make
        // every string-building path look super-linear.
        if new_size > layout.size() {
            note_alloc(new_size - layout.size());
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn allocated_bytes() -> usize {
    ALLOCATED.try_with(|c| c.get()).unwrap_or(0)
}

// ─── Deterministic PRNG ─────────────────────────────────────────────────────

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

struct Rng {
    s: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Rng {
            s: if seed == 0 { GOLDEN } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.s = self.s.wrapping_add(GOLDEN);
        let mut z = self.s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, n)`; `0` for a non-positive `n` so no caller has to guard.
    fn next(&mut self, n: usize) -> usize {
        if n <= 1 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    /// Uniform in `[lo, hi]`, inclusive.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo {
            lo
        } else {
            lo + self.next(hi - lo + 1)
        }
    }

    fn boolean(&mut self) -> bool {
        self.next_u64() % 2 == 1
    }

    fn pick<'a>(&mut self, xs: &'a [&'a str]) -> &'a str {
        xs[self.next(xs.len())]
    }

    fn pick_owned(&mut self, xs: &[String]) -> String {
        xs[self.next(xs.len())].clone()
    }
}

// ─── Corpus seeds + vocabulary ──────────────────────────────────────────────

/// Built-in seeds, so the harness is self-sufficient: the go-red self-test must
/// not depend on the shared corpus being checked out alongside this repo in
/// order to prove that the harness can fail.
const BUILTIN_SEEDS: &[&str] = &[
    r#"{"id":"a","kind":{"$type":"Heading","level":1,"text":"x","variant":"Standard"}}"#,
    r#"{"id":"b","kind":{"$type":"Box","children":[],"layout":{"$type":"Auto"},"role":"Group"}}"#,
    // Two hashes: the payload itself contains `"#`, which closes a single-hash
    // raw string mid-literal.
    r##"{"id":"c","kind":{"$type":"Markdown","source":"# hi"}}"##,
    r#"{"$type":"RemoveNode","path":["a"]}"#,
    r#"{"$type":"Batch","ops":[]}"#,
    "{}",
    "[]",
    "null",
    "",
];

/// Walks up from the crate directory looking for the shared corpus. `None` keeps
/// the repo standalone-testable: a corpus-less checkout gets a working harness
/// with a narrower seed pool, never a failure.
fn find_corpus() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let root = dir.join("wire-format-fixtures");
        if root.join("manifest.json").is_file() {
            return Some(root);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Every corpus payload the harness can find, as raw text. READ-ONLY by
/// construction: the fuzz never writes into the corpus. A REJECT fixture is the
/// most productive seed there is, since it already sits one edit away from the
/// refusal boundary the fuzz is probing.
fn load_seeds(corpus: Option<&Path>) -> Vec<String> {
    let mut seeds: Vec<String> = BUILTIN_SEEDS.iter().map(|s| (*s).to_string()).collect();
    let Some(corpus) = corpus else {
        return seeds;
    };
    for family in ["nodes", "ops", "reject", "lenient"] {
        let dir = corpus.join(family);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().is_some_and(|x| x == "json")
                    && !p.to_string_lossy().ends_with(".expected.json")
            })
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(raw) = std::fs::read_to_string(&path) {
                seeds.push(raw);
            }
        }
    }
    seeds
}

const FALLBACK_VOCAB: &[&str] = &[
    "Box", "Heading", "Markdown", "Metric", "Badge", "Form", "Button", "DataGrid", "Chart",
    "Custom",
];

/// The wire vocabulary the near-miss generators aim just beside, read from the
/// corpus MANIFEST so a newly-admitted kind is fuzzed the day it lands rather
/// than whenever someone remembers to extend a literal list here.
///
/// Parsed with the host's own JSON layer, which is the only one this crate has —
/// and a manifest this host cannot read is a finding in its own right, so the
/// fallback below is a narrower harness rather than a silent success.
fn load_vocabulary(corpus: Option<&Path>) -> Vec<String> {
    use fuaran_rs::canonical::{JVal, parse};
    if let Some(corpus) = corpus
        && let Ok(raw) = std::fs::read_to_string(corpus.join("manifest.json"))
        && let Ok(ast) = parse(&raw)
        && let Some(JVal::Arr(kinds)) = ast.field("kinds")
    {
        let names: Vec<String> = kinds
            .iter()
            .filter_map(|k| match k {
                JVal::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        if !names.is_empty() {
            return names;
        }
    }
    FALLBACK_VOCAB.iter().map(|s| (*s).to_string()).collect()
}

// ─── Alphabets ──────────────────────────────────────────────────────────────

/// Note what is ABSENT relative to the reference host's list: the lone UTF-16
/// surrogates. A Rust `&str` is guaranteed well-formed UTF-8, so this host
/// cannot be handed one through its public decode surface at all — the type
/// system removes the case rather than the harness declining to test it. That is
/// a genuine difference between the hosts, stated rather than smoothed over.
const HOSTILE_CHARS: &[&str] = &[
    "{", "}", "[", "]", "\"", ":", ",", "\\", "/", "-", "+", ".", "e", "E", "0", "9", "n", "t",
    "f", " ", "\t", "\n", "\r", "\u{0}", "\u{7f}", "\u{feff}", "\u{2028}", "é", "中", "\u{fffd}",
];

/// The JSON-ESCAPE entries are six-character TEXT (backslash, `u`, four hex
/// digits), not the code points they denote — a decoder's unescape path is what
/// they are aimed at, so writing them as the characters would test the wrong
/// thing.
const HOSTILE_TOKENS: &[&str] = &[
    "null",
    "true",
    "false",
    "{}",
    "[]",
    "\"\"",
    "-0",
    "1e999",
    "-1e999",
    "1E-999",
    "NaN",
    "Infinity",
    "-Infinity",
    "0x10",
    "00",
    "01",
    "1.2.3",
    "+1",
    ".5",
    "5.",
    r"\u0000",
    r"\uD800",
    r"\uFFFF",
    r"\x41",
    r"\",
    r#"\""#,
    r#""$type":"""#,
    r#""$type":null"#,
    r#""id":"""#,
    r#""id":null"#,
    r#""id":[]"#,
    r#""kind":"Heading""#,
    r#""children":"x""#,
    ",",
    ":",
    "[",
    "]",
    "{",
    "}",
    "\"",
    "'",
    "/*",
    "*/",
    "//",
    "\u{0}",
    "\u{feff}",
    "\r\n",
];

/// REAL wire keys, so a generated near-miss reaches deep into the typed decoders
/// instead of bouncing off the first `MISSING_FIELD`.
const WIRE_KEYS: &[&str] = &[
    "id",
    "kind",
    "$type",
    "children",
    "layout",
    "role",
    "text",
    "level",
    "variant",
    "source",
    "value",
    "label",
    "fields",
    "items",
    "columns",
    "rows",
    "onSubmit",
    "onClick",
    "required",
    "binding",
    "style",
    "props",
    "state",
    "ops",
    "path",
    "node",
    "index",
    "target",
    "name",
    "format",
    "unit",
    "min",
    "max",
    "options",
    "spec",
    "__proto__",
    "constructor",
    "",
    " ",
];

const SCALAR_LITERALS: &[&str] = &[
    "0",
    "-1",
    "1e308",
    "-1e308",
    "1e999",
    "3.141592653589793",
    "true",
    "false",
    "null",
    "\"\"",
    "\"x\"",
    "\"Standard\"",
    "\"Group\"",
    "9007199254740993",
    "-0.0",
];

/// A near-miss of a real vocabulary word: the class of input a model emitter
/// actually produces, and the class a curated reject corpus is worst at
/// covering, because a human writing fixtures reaches for obvious garbage.
///
/// Every index is a CHAR boundary, computed from `char_indices` rather than from
/// byte arithmetic: slicing a `String` mid-character panics, and a generator that
/// panicked would be reported as a decoder defect.
fn near_miss(rng: &mut Rng, word: &str) -> String {
    if word.is_empty() {
        return "x".to_string();
    }
    let boundaries: Vec<usize> = word.char_indices().map(|(i, _)| i).collect();
    match rng.next(8) {
        0 => word.to_lowercase(),
        1 => word.to_uppercase(),
        2 => format!("{word}s"),
        3 => word[..*boundaries.last().unwrap()].to_string(),
        4 => format!("{word} "),
        5 => format!(" {word}"),
        6 => {
            let i = boundaries[rng.next(boundaries.len())];
            let next = word[i..].chars().next().map(|c| i + c.len_utf8()).unwrap();
            format!("{}{}", &word[..i], &word[next..])
        }
        _ => {
            let i = boundaries[rng.next(boundaries.len())];
            format!("{}{}{}", &word[..i], rng.pick(HOSTILE_CHARS), &word[i..])
        }
    }
}

// ─── Mutators ───────────────────────────────────────────────────────────────
//
// Each corrupts a seed payload. Named individually so a reported counterexample
// records WHICH transformation produced it: a find whose provenance is only "the
// fuzzer did something" is markedly harder to act on.

const MUTATOR_NAMES: &[&str] = &[
    "flip-char",
    "delete-span",
    "insert-token",
    "duplicate-span",
    "truncate",
    "transpose",
    "repeat-structural",
    "retype-value",
    "near-miss-type",
    "delete-key",
    "duplicate-key",
    "escape-injection",
    "prefix-junk",
    "suffix-junk",
];

/// Char boundaries of `s`, plus its length — the index set every mutator draws
/// from, so no slice ever splits a character.
fn boundaries(s: &str) -> Vec<usize> {
    let mut b: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    b.push(s.len());
    b
}

/// The boundary at or after `from`, so a span cut never lands mid-character.
fn boundary_at_or_after(s: &str, from: usize) -> usize {
    let mut i = from.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn truncate_to(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let cut = {
        let mut i = max;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    s[..cut].to_string()
}

fn near_miss_type(rng: &mut Rng, vocab: &[String], s: &str) -> String {
    const MARKER: &str = r#""$type":""#;
    let positions: Vec<usize> = s.match_indices(MARKER).map(|(i, _)| i).collect();
    if positions.is_empty() {
        // No discriminator to corrupt — append one rather than returning the
        // input untouched. A silently no-op mutator quietly shrinks the effective
        // iteration count and nothing reports that it did.
        let word = rng.pick_owned(vocab);
        return format!("{s}{{\"$type\":\"{}\"}}", near_miss(rng, &word));
    }
    let start = positions[rng.next(positions.len())] + MARKER.len();
    let Some(rel) = s[start..].find('"') else {
        return s.to_string();
    };
    let close = start + rel;
    let replacement = if rng.boolean() {
        near_miss(rng, &s[start..close])
    } else {
        let word = rng.pick_owned(vocab);
        near_miss(rng, &word)
    };
    format!("{}{}{}", &s[..start], replacement, &s[close..])
}

/// Delete a whole `"key":value` pair, cutting from the key's opening quote to
/// just past the next comma.
fn delete_key(rng: &mut Rng, s: &str) -> String {
    let positions: Vec<usize> = s.match_indices("\":").map(|(i, _)| i).collect();
    if positions.is_empty() {
        return s.to_string();
    }
    let colon = positions[rng.next(positions.len())];
    let bytes = s.as_bytes();
    let mut close_quote = colon;
    while close_quote > 0 && bytes[close_quote] != b'"' {
        close_quote -= 1;
    }
    let mut open_quote = close_quote.saturating_sub(1);
    while open_quote > 0 && bytes[open_quote] != b'"' {
        open_quote -= 1;
    }
    let cut_from = open_quote;
    let cut_to = match s[colon..].find(',') {
        Some(rel) => colon + rel + 1,
        None => boundary_at_or_after(s, colon + 8),
    };
    format!("{}{}", &s[..cut_from], &s[cut_to..])
}

#[derive(Clone, Copy)]
struct Config {
    /// Names the stream, so a reported find's replay line reconstructs the exact
    /// configuration as well as the exact seed.
    name: &'static str,
    /// The bounded gate run keeps this small so the suite stays quick; the long
    /// run raises it past the §21 string bound so that bound is actually crossed.
    max_payload_chars: usize,
    /// One in this many inputs is a deliberately pathological (large) payload.
    heavy_every_n: usize,
}

const BOUNDED_CONFIG: Config = Config {
    name: "bounded",
    max_payload_chars: 48 * 1024,
    heavy_every_n: 120,
};

const LONG_CONFIG: Config = Config {
    name: "long",
    max_payload_chars: 2 * 1024 * 1024,
    heavy_every_n: 25,
};

fn mutate_once(rng: &mut Rng, vocab: &[String], cfg: Config, s: &str) -> (&'static str, String) {
    let name = MUTATOR_NAMES[rng.next(MUTATOR_NAMES.len())];
    let n = s.len();
    let b = boundaries(s);

    let result = match name {
        "flip-char" if n > 0 => {
            let i = b[rng.next(b.len() - 1)];
            let next = s[i..].chars().next().map(|c| i + c.len_utf8()).unwrap();
            format!("{}{}{}", &s[..i], rng.pick(HOSTILE_CHARS), &s[next..])
        }
        "delete-span" if n > 1 => {
            let i = b[rng.next(b.len() - 1)];
            let to = boundary_at_or_after(s, i + rng.range(1, 8));
            format!("{}{}", &s[..i], &s[to..])
        }
        "insert-token" => {
            let i = b[rng.next(b.len())];
            format!("{}{}{}", &s[..i], rng.pick(HOSTILE_TOKENS), &s[i..])
        }
        "duplicate-span" if n > 1 => {
            let i = b[rng.next(b.len() - 1)];
            let to = boundary_at_or_after(s, i + rng.range(1, 64));
            let at = b[rng.next(b.len())];
            format!("{}{}{}", &s[..at], &s[i..to], &s[at..])
        }
        "truncate" if n > 1 => s[..b[rng.next(b.len() - 1)]].to_string(),
        "transpose" if n > 2 => {
            let i = b[rng.next(b.len() - 2)];
            let mut it = s[i..].chars();
            let a = it.next().unwrap();
            let c = it.next().unwrap();
            let after = i + a.len_utf8() + c.len_utf8();
            format!("{}{}{}{}", &s[..i], c, a, &s[after..])
        }
        "repeat-structural" => {
            let ch = rng.pick(&["[", "{", "\"", "]", "}", ","]);
            let count = rng.range(2, 4096).min((cfg.max_payload_chars / 4).max(2));
            let at = b[rng.next(b.len())];
            format!("{}{}{}", &s[..at], ch.repeat(count), &s[at..])
        }
        "retype-value" if n > 0 => {
            let i = b[rng.next(b.len() - 1)];
            let to = boundary_at_or_after(s, i + rng.range(1, 12));
            format!("{}{}{}", &s[..i], rng.pick(SCALAR_LITERALS), &s[to..])
        }
        "near-miss-type" => near_miss_type(rng, vocab, s),
        "delete-key" => delete_key(rng, s),
        "duplicate-key" if n > 4 => {
            // A duplicated key is a real emitter defect and a classic cross-host
            // parser divergence (first-wins vs last-wins vs refuse) — §20 of the
            // wire specification records the measured matrix and PROPOSES a rule.
            // Fuzzing it for panics is in scope here; asserting which behaviour is
            // correct is not, until that rule is ratified.
            match (s.find('"'), s.find(',')) {
                (Some(i), Some(j)) if j > i => {
                    format!("{}{},{}", &s[..j + 1], &s[i..j], &s[j + 1..])
                }
                _ => s.to_string(),
            }
        }
        "escape-injection" if n > 0 => {
            let i = b[rng.next(b.len() - 1)];
            let esc = rng.pick(&[r"\u", r"\uD800", r"\u00", r"\", r"\/", r"\b\f"]);
            format!("{}{}{}", &s[..i], esc, &s[i..])
        }
        "prefix-junk" => {
            let mut junk = String::new();
            for _ in 0..rng.range(1, 16) {
                junk.push_str(rng.pick(HOSTILE_CHARS));
            }
            format!("{junk}{s}")
        }
        "suffix-junk" => {
            let mut junk = String::new();
            for _ in 0..rng.range(1, 16) {
                junk.push_str(rng.pick(HOSTILE_CHARS));
            }
            format!("{s}{junk}")
        }
        _ => format!("{}{}", s, rng.pick(HOSTILE_CHARS)),
    };

    (name, truncate_to(result, cfg.max_payload_chars))
}

// ─── Structure-aware generation ─────────────────────────────────────────────

fn gen_value(rng: &mut Rng, depth: usize, out: &mut String, vocab: &[String], cfg: Config) {
    if out.len() > cfg.max_payload_chars {
        out.push('0');
        return;
    }
    if depth == 0 {
        out.push_str(rng.pick(SCALAR_LITERALS));
        return;
    }
    let branch = rng.next(12);
    if branch <= 3 {
        out.push_str(rng.pick(SCALAR_LITERALS));
    } else if branch <= 7 {
        out.push('{');
        let n = rng.range(0, 5);
        for i in 0..n {
            if i > 0 {
                out.push(',');
            }
            out.push('"');
            out.push_str(rng.pick(WIRE_KEYS));
            out.push_str("\":");
            gen_value(rng, depth - 1, out, vocab, cfg);
        }
        out.push('}');
    } else if branch <= 10 {
        out.push('[');
        let n = rng.range(0, 5);
        for i in 0..n {
            if i > 0 {
                out.push(',');
            }
            gen_value(rng, depth - 1, out, vocab, cfg);
        }
        out.push(']');
    } else {
        // A plausible node shell around a wrong interior: the shape that gets
        // furthest into the typed decoders before it fails, and so the one most
        // likely to reach code a shallow syntax reject never does.
        out.push_str("{\"id\":\"g\",\"kind\":{\"$type\":\"");
        let word = rng.pick_owned(vocab);
        out.push_str(&near_miss(rng, &word));
        out.push_str("\",\"");
        out.push_str(rng.pick(WIRE_KEYS));
        out.push_str("\":");
        gen_value(rng, depth - 1, out, vocab, cfg);
        out.push_str("}}");
    }
}

/// Depth, width and string length taken past the §21 limits. Every payload is
/// assembled as TEXT: building one as a nested value would blow the harness's own
/// stack while CONSTRUCTING the input, which proves nothing about the decoder.
fn gen_pathological(rng: &mut Rng, cfg: Config) -> String {
    let cap = cfg.max_payload_chars;
    match rng.next(9) {
        0 => {
            let n = (cap / 2).min(rng.range(64, 200_000));
            format!("{}{}", "[".repeat(n), "]".repeat(n))
        }
        1 => {
            let n = (cap / 6).min(rng.range(64, 100_000));
            format!("{}1{}", "{\"a\":".repeat(n), "}".repeat(n))
        }
        // Unterminated as well as over-deep: the depth guard must fire on the way
        // DOWN, before truncation is ever reached.
        2 => "[".repeat((cap / 2).min(rng.range(64, 200_000))),
        3 => {
            // Deep NODE nesting rather than deep JSON — crosses the tree depth
            // bound while staying far inside the JSON one, isolating the tree limit.
            let mut acc =
                r#"{"id":"leaf","kind":{"$type":"Heading","level":1,"text":"x","variant":"Standard"}}"#
                    .to_string();
            let depth = rng.range(2, 400);
            for i in 1..=depth {
                if acc.len() >= cap {
                    break;
                }
                acc = format!(
                    "{{\"id\":\"n{i}\",\"kind\":{{\"$type\":\"Box\",\"children\":[{acc}],\"layout\":{{\"$type\":\"Auto\"}},\"role\":\"Group\"}}}}"
                );
            }
            acc
        }
        4 => {
            let n = (cap / 2).min(rng.range(1000, 200_000));
            let body = vec!["1"; n].join(",");
            format!("{{\"id\":\"a\",\"kind\":[{body}]}}")
        }
        5 => {
            let n = cap.min(rng.range(1000, 1_200_000));
            format!(
                "{{\"id\":\"a\",\"kind\":{{\"$type\":\"Heading\",\"level\":1,\"text\":\"{}\",\"variant\":\"Standard\"}}}}",
                "x".repeat(n)
            )
        }
        6 => {
            let mut acc = r#"{"$type":"Batch","ops":[]}"#.to_string();
            for _ in 0..rng.range(2, 300) {
                if acc.len() >= cap {
                    break;
                }
                acc = format!("{{\"$type\":\"Batch\",\"ops\":[{acc}]}}");
            }
            acc
        }
        7 => {
            // Escape-heavy: nearly every character an escape, so the unescape path
            // does the work rather than the structural walk.
            let n = (cap / 6).min(rng.range(500, 100_000));
            format!(
                "{{\"id\":\"a\",\"kind\":{{\"$type\":\"Markdown\",\"source\":\"{}\"}}}}",
                r"A".repeat(n)
            )
        }
        _ => {
            let n = (cap / 4).min(rng.range(500, 50_000));
            let body: Vec<String> = (0..n).map(|i| format!("\"k{i}\":1")).collect();
            format!("{{{}}}", body.join(","))
        }
    }
}

struct Generated {
    payload: String,
    origin: String,
}

/// Deterministic in `(seed, iteration, cfg)` — the replay contract. Every branch
/// draws from the same `Rng`, so ADDING a family renumbers the stream; that is
/// why a reported find carries its payload too and replay is the backstop rather
/// than the primary record.
fn generate(
    rng: &mut Rng,
    seeds: &[String],
    vocab: &[String],
    cfg: Config,
    iteration: usize,
) -> Generated {
    if iteration % cfg.heavy_every_n == 0 {
        return Generated {
            payload: gen_pathological(rng, cfg),
            origin: "pathological".to_string(),
        };
    }
    let branch = rng.next(10);
    if branch <= 1 {
        let mut out = String::new();
        let depth = rng.range(1, 6);
        gen_value(rng, depth, &mut out, vocab, cfg);
        return Generated {
            payload: out,
            origin: "structured-generation".to_string(),
        };
    }
    if branch == 2 {
        let mut out = String::new();
        for _ in 0..rng.range(0, 200) {
            out.push_str(rng.pick(HOSTILE_CHARS));
        }
        return Generated {
            payload: out,
            origin: "raw-junk".to_string(),
        };
    }
    if branch == 3 {
        // Crossover: prefix of one seed, suffix of another. Produces half-valid
        // documents no single-seed mutation reaches.
        let a = rng.pick_owned(seeds);
        let c = rng.pick_owned(seeds);
        let ab = boundaries(&a);
        let cb = boundaries(&c);
        let i = ab[rng.next(ab.len())];
        let j = cb[rng.next(cb.len())];
        return Generated {
            payload: format!("{}{}", &a[..i], &c[j..]),
            origin: "crossover".to_string(),
        };
    }
    let mut acc = rng.pick_owned(seeds);
    let mut names: Vec<&str> = Vec::new();
    for _ in 0..rng.range(1, 4) {
        let (name, next) = mutate_once(rng, vocab, cfg, &acc);
        acc = next;
        names.push(name);
    }
    Generated {
        payload: acc,
        origin: format!("mutation:{}", names.join("+")),
    }
}

// ─── Subjects + verdicts ────────────────────────────────────────────────────

/// What one decode entry point did with one input. Deliberately string-typed: the
/// harness compares canonical FORMS, so it needs no access to the tree types and
/// both entry points share one machinery.
#[derive(Default)]
struct SubjectResult {
    refused_code: Option<String>,
    canonical: String,
    re_decoded: Option<String>,
    re_decoded_code: Option<String>,
}

/// One decode entry point, or a deliberately-broken stand-in. `run` is allowed —
/// required, in the self-test's case — to panic: catching is the harness's job.
struct Subject {
    name: &'static str,
    run: Box<dyn Fn(&str) -> SubjectResult>,
}

fn node_subject() -> Subject {
    Subject {
        name: "decode_node",
        run: Box::new(|input| match decode_node(input) {
            Err(e) => SubjectResult {
                refused_code: Some(e.code.as_str().to_string()),
                ..Default::default()
            },
            Ok(tree) => {
                let canonical = encode_node(&tree);
                match decode_node(&canonical) {
                    Err(e) => SubjectResult {
                        canonical,
                        re_decoded_code: Some(e.code.as_str().to_string()),
                        ..Default::default()
                    },
                    Ok(again) => SubjectResult {
                        canonical,
                        re_decoded: Some(encode_node(&again)),
                        ..Default::default()
                    },
                }
            }
        }),
    }
}

fn op_subject() -> Subject {
    Subject {
        name: "decode_op",
        run: Box::new(|input| match decode_op(input) {
            Err(e) => SubjectResult {
                refused_code: Some(e.code.as_str().to_string()),
                ..Default::default()
            },
            Ok(op) => {
                let canonical = encode_op(&op);
                match decode_op(&canonical) {
                    Err(e) => SubjectResult {
                        canonical,
                        re_decoded_code: Some(e.code.as_str().to_string()),
                        ..Default::default()
                    },
                    Ok(again) => SubjectResult {
                        canonical,
                        re_decoded: Some(encode_op(&again)),
                        ..Default::default()
                    },
                }
            }
        }),
    }
}

/// BOTH public entry points, since the totality claim is made about the decoder,
/// not about one of its two doors.
fn real_subjects() -> Vec<Subject> {
    vec![node_subject(), op_subject()]
}

/// `kind` is the coarse class; `detail` is for the report. `rejected` and `clean`
/// are both PASSES — a fuzz harness that treated refusal as failure would be
/// asserting the opposite of the claim under test.
#[derive(Clone)]
struct Verdict {
    kind: String,
    detail: String,
}

impl Verdict {
    fn is_counterexample(&self) -> bool {
        self.kind != "rejected" && self.kind != "clean"
    }
}

/// The soft time budget, scaled to the profile the run is compiled under.
///
/// `cargo test` builds the DEBUG profile — no optimisation, and this crate's
/// hand-written JSON layer is exactly the sort of tight scalar code that costs an
/// order of magnitude there. The measured worst case on the bounded stream is
/// about a second unoptimised; a three-second budget (the figure the reference
/// host uses, in Release) would therefore sit within a factor of three of the
/// observed maximum on a developer machine and go red on a slower CI runner for
/// reasons having nothing to do with the decoder. A flaky budget is worse than a
/// loose one: it gets raised in a hurry by whoever it blocks, and nobody records
/// why. So the budget names the profile difference instead of absorbing it.
const fn soft_time_budget_ms() -> f64 {
    if cfg!(debug_assertions) {
        15_000.0
    } else {
        3_000.0
    }
}

#[derive(Clone, Copy)]
struct Budgets {
    soft_time_ms: f64,
    /// Allocation floor for an ORDINARY input: below this, no input is judged
    /// over-budget however small it was. Covers the fixed per-decode cost.
    alloc_floor_bytes: usize,
    /// Allowed allocation per input byte, above the floor.
    alloc_per_byte: usize,
}

const DEFAULT_BUDGETS: Budgets = Budgets {
    soft_time_ms: soft_time_budget_ms(),
    alloc_floor_bytes: 16 * 1024 * 1024,
    alloc_per_byte: 512,
};

struct Measured {
    verdict: Verdict,
    elapsed_ms: f64,
    allocated: usize,
}

/// Run one input through one subject and judge it against every invariant.
///
/// Every panic is caught HERE and nowhere else, which is what makes "no panic
/// escapes" a measured property rather than a hope. Note the one boundary: this
/// crate's RELEASE profile sets `panic = "abort"`, under which nothing is
/// catchable — so this harness is meaningful in the test profile it runs in, and
/// a release consumer's guarantee is that the decoder does not panic at all,
/// which is precisely what these runs are evidence for.
fn check(subject: &Subject, budgets: Budgets, input: &str) -> Measured {
    let before = allocated_bytes();
    let started = Instant::now();
    let outcome = catch_unwind(AssertUnwindSafe(|| (subject.run)(input)));
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let allocated = allocated_bytes().saturating_sub(before);

    let result = match outcome {
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_string());
            return Measured {
                verdict: Verdict {
                    kind: "escaped-panic".to_string(),
                    detail,
                },
                elapsed_ms,
                allocated,
            };
        }
        Ok(r) => r,
    };

    let at = |kind: &str, detail: String| Measured {
        verdict: Verdict {
            kind: kind.to_string(),
            detail,
        },
        elapsed_ms,
        allocated,
    };

    // Order matters: an input that both ran long AND over-allocated is reported
    // as the time breach, because that is the one an operator has to act on first.
    if elapsed_ms > budgets.soft_time_ms {
        return at(
            "timed-out",
            format!("decode returned only after {elapsed_ms:.0} ms"),
        );
    }
    let budget = budgets
        .alloc_floor_bytes
        .max(budgets.alloc_per_byte.saturating_mul(input.len()));
    if allocated > budget {
        return at(
            "over-allocated",
            format!("{allocated} bytes allocated, budget {budget}"),
        );
    }
    if let Some(code) = result.refused_code {
        return at("rejected", code);
    }
    if let Some(code) = result.re_decoded_code {
        return at(
            "canonical-refused",
            format!("the decoder's own output re-decodes as {code}"),
        );
    }
    match result.re_decoded {
        Some(second) if second == result.canonical => at("clean", String::new()),
        Some(second) => at(
            "fixed-point-broken",
            format!(
                "first canonical form {} chars, second {}",
                result.canonical.len(),
                second.len()
            ),
        ),
        None => at(
            "canonical-refused",
            "the decoder produced no re-decodable output".to_string(),
        ),
    }
}

// ─── The run ────────────────────────────────────────────────────────────────

struct Counterexample {
    subject: &'static str,
    iteration: usize,
    origin: String,
    verdict: Verdict,
    payload: String,
}

impl Counterexample {
    fn describe(&self, seed: u64, config_name: &str) -> String {
        let preview = if self.payload.len() > 300 {
            format!(
                "{} ...(truncated)",
                &self.payload[..boundary_before(&self.payload, 300)]
            )
        } else {
            self.payload.clone()
        };
        format!(
            "subject: {}\nseed: {seed}, iteration: {}, config: {config_name}\norigin: {}\n\
             verdict: {} — {}\nlength: {} bytes\ninput: {preview:?}\n\n\
             Counterexample policy: fix the decoder, then land the input as a permanent\n\
             reject fixture in the shared corpus, so every conformant host inherits the\n\
             case rather than only this one.",
            self.subject,
            self.iteration,
            self.origin,
            self.verdict.kind,
            self.verdict.detail,
            self.payload.len()
        )
    }
}

fn boundary_before(s: &str, max: usize) -> usize {
    let mut i = max.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[derive(Default)]
struct RunStats {
    iterations: usize,
    inputs: usize,
    corpus_seeds: usize,
    reject_codes: Vec<(String, usize)>,
    accepted: usize,
    max_decode_ms: f64,
    max_alloc_bytes: usize,
    max_alloc_ratio: f64,
    elapsed_seconds: f64,
    counterexamples: Vec<Counterexample>,
}

impl RunStats {
    fn bump_code(&mut self, code: &str) {
        if let Some(entry) = self.reject_codes.iter_mut().find(|(c, _)| c == code) {
            entry.1 += 1;
        } else {
            self.reject_codes.push((code.to_string(), 1));
        }
    }
}

/// `subjects` is a parameter precisely so the go-red self-test drives the
/// IDENTICAL machinery with a broken stand-in: a fuzz harness nobody has ever
/// seen fail is decoration.
fn run(
    subjects: &[Subject],
    budgets: Budgets,
    cfg: Config,
    seed: u64,
    iterations: usize,
    seeds: &[String],
    vocab: &[String],
) -> RunStats {
    let mut rng = Rng::new(seed);
    let started = Instant::now();
    let mut stats = RunStats {
        corpus_seeds: seeds.len(),
        ..Default::default()
    };

    for i in 1..=iterations {
        let g = generate(&mut rng, seeds, vocab, cfg, i);
        for subject in subjects {
            let m = check(subject, budgets, &g.payload);
            stats.inputs += 1;
            stats.max_decode_ms = stats.max_decode_ms.max(m.elapsed_ms);
            stats.max_alloc_bytes = stats.max_alloc_bytes.max(m.allocated);
            if !g.payload.is_empty() {
                stats.max_alloc_ratio = stats
                    .max_alloc_ratio
                    .max(m.allocated as f64 / g.payload.len() as f64);
            }
            match m.verdict.kind.as_str() {
                "rejected" => stats.bump_code(&m.verdict.detail),
                "clean" => stats.accepted += 1,
                _ => stats.counterexamples.push(Counterexample {
                    subject: subject.name,
                    iteration: i,
                    origin: g.origin.clone(),
                    verdict: m.verdict,
                    payload: g.payload.clone(),
                }),
            }
        }
        stats.iterations = i;
    }

    stats.elapsed_seconds = started.elapsed().as_secs_f64();
    stats
}

/// The one-line human summary, printed on every run, pass or fail: a harness
/// whose output is only visible when it fails cannot be checked for having
/// quietly stopped generating anything.
fn summarise(stats: &RunStats) -> String {
    let mut codes: Vec<String> = stats
        .reject_codes
        .iter()
        .map(|(c, n)| format!("{c}={n}"))
        .collect();
    codes.sort();
    let per_iteration = stats.inputs.checked_div(stats.iterations).unwrap_or(0);
    format!(
        "{} inputs ({} iterations x {per_iteration} entry points) in {:.1} s — accepted {}, \
         refused [{}], {} counterexamples; \
         max decode {:.0} ms; alloc peak {} bytes ({:.0} x)",
        stats.inputs,
        stats.iterations,
        stats.elapsed_seconds,
        stats.accepted,
        codes.join(" "),
        stats.counterexamples.len(),
        stats.max_decode_ms,
        stats.max_alloc_bytes,
        stats.max_alloc_ratio
    )
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The machine-readable result a scheduled job collects. Written by the run, so
/// the figures cannot drift from the methodology beside them.
fn evidence_json(stats: &RunStats, cfg: Config, seed: u64, corpus_present: bool) -> String {
    let mut codes: Vec<String> = stats
        .reject_codes
        .iter()
        .map(|(c, n)| format!("    \"{}\": {}", json_escape(c), n))
        .collect();
    codes.sort();
    format!(
        "{{\n  \"host\": \"fuaran-rs\",\n  \"entryPoints\": [\"decode_node\", \"decode_op\"],\n  \
         \"config\": \"{}\",\n  \"seed\": \"{seed}\",\n  \"iterations\": {},\n  \"inputs\": {},\n  \
         \"corpusSeeds\": {},\n  \"corpusPresent\": {corpus_present},\n  \"accepted\": {},\n  \
         \"rejectCodes\": {{\n{}\n  }},\n  \"counterexamples\": {},\n  \"maxDecodeMs\": {:.3},\n  \
         \"maxAllocBytes\": {},\n  \"maxAllocRatio\": {:.3},\n  \"elapsedSeconds\": {:.3}\n}}\n",
        cfg.name,
        stats.iterations,
        stats.inputs,
        stats.corpus_seeds,
        stats.accepted,
        codes.join(",\n"),
        stats.counterexamples.len(),
        stats.max_decode_ms,
        stats.max_alloc_bytes,
        stats.max_alloc_ratio,
        stats.elapsed_seconds
    )
}

// ─── The gate ───────────────────────────────────────────────────────────────

const SEED: u64 = 1023;

fn env_usize(name: &str, fallback: usize) -> usize {
    match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => raw
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or_else(|| panic!("{name}: {raw:?} is not a positive integer")),
        _ => fallback,
    }
}

#[test]
fn the_refusal_contract_holds_over_generated_hostile_input() {
    let corpus = find_corpus();
    let seeds = load_seeds(corpus.as_deref());
    let vocab = load_vocabulary(corpus.as_deref());

    let long = std::env::var("FUARAN_FUZZ_LONG").as_deref() == Ok("1");
    let cfg = if long { LONG_CONFIG } else { BOUNDED_CONFIG };
    let iterations = env_usize("FUARAN_FUZZ_ITERATIONS", if long { 250_000 } else { 4_000 });

    let subjects = real_subjects();
    let stats = run(
        &subjects,
        DEFAULT_BUDGETS,
        cfg,
        SEED,
        iterations,
        &seeds,
        &vocab,
    );
    println!("  [decoder-fuzz] {}", summarise(&stats));

    if let Ok(path) = std::env::var("FUARAN_FUZZ_EVIDENCE")
        && !path.trim().is_empty()
    {
        let path = PathBuf::from(path.trim());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("creating the evidence directory");
        }
        std::fs::write(&path, evidence_json(&stats, cfg, SEED, corpus.is_some()))
            .expect("writing the evidence record");
    }

    if !stats.counterexamples.is_empty() {
        let detail: Vec<String> = stats
            .counterexamples
            .iter()
            .take(5)
            .map(|c| c.describe(SEED, cfg.name))
            .collect();
        panic!(
            "{} counterexample(s) — the decoder's refusal contract does not hold over \
             generated hostile input.\n\n{}",
            stats.counterexamples.len(),
            detail.join("\n\n")
        );
    }

    // A run that generated nothing would report zero counterexamples and look
    // identical to a clean one. Pin the work actually done.
    assert_eq!(stats.iterations, iterations);
    assert_eq!(stats.inputs, iterations * subjects.len());
    // Both outcomes must occur. A stream that only ever refuses never reaches the
    // fixed-point invariant; one that only ever accepts is not hostile.
    assert!(
        stats.accepted > 0,
        "no generated input was ACCEPTED — the fixed-point invariant was never exercised"
    );
    assert!(
        !stats.reject_codes.is_empty(),
        "no generated input was REFUSED — the stream is not hostile"
    );
}

// ─── Go-red: the harness fails when the decoder is broken ───────────────────
//
// Permanent, not a one-off demonstration at authoring time. Each mutant breaks
// ONE invariant, and the inverse pin proves each is PARTIAL — a mutant that broke
// every input would make the harness look sensitive while testing nothing.

fn every_nth(
    n: usize,
    name: &'static str,
    broken: impl Fn() -> SubjectResult + 'static,
) -> Subject {
    Subject {
        name,
        run: Box::new(move |input: &str| {
            if input.len() % n == 0 {
                broken()
            } else {
                SubjectResult {
                    refused_code: Some("INVALID_JSON".to_string()),
                    ..Default::default()
                }
            }
        }),
    }
}

#[test]
fn the_harness_goes_red_on_a_broken_decoder() {
    let corpus = find_corpus();
    let seeds = load_seeds(corpus.as_deref());
    let vocab = load_vocabulary(corpus.as_deref());

    // The slow mutant is measured against a DELIBERATELY TIGHT budget rather
    // than the shipped three-second one. Sleeping past the real budget would cost
    // three seconds per firing — the sort of cost that gets a go-red test deleted
    // rather than fixed. What is under test is the harness's ability to see a
    // decode that returned past ITS budget, and that is exactly as true at 5 ms.
    let tight = Budgets {
        soft_time_ms: 5.0,
        ..DEFAULT_BUDGETS
    };
    let big = "x".repeat(DEFAULT_BUDGETS.alloc_floor_bytes + 1);

    let cases: Vec<(Subject, Budgets)> = vec![
        (
            every_nth(3, "mutant:panics", || {
                panic!("deliberate: the decoder let a panic escape")
            }),
            DEFAULT_BUDGETS,
        ),
        (
            every_nth(5, "mutant:slow", || {
                std::thread::sleep(std::time::Duration::from_millis(25));
                SubjectResult {
                    refused_code: Some("INVALID_JSON".to_string()),
                    ..Default::default()
                }
            }),
            tight,
        ),
        (
            every_nth(7, "mutant:allocates", {
                let big = big.clone();
                move || {
                    // Allocation, not a claim about it: the counting allocator sees
                    // this, so the invariant is measured on the mutant exactly as it
                    // is on the decoder.
                    let hog: Vec<String> = (0..8).map(|_| big.clone()).collect();
                    let canonical = hog[0][..2].to_string();
                    SubjectResult {
                        canonical: canonical.clone(),
                        re_decoded: Some(canonical),
                        ..Default::default()
                    }
                }
            }),
            DEFAULT_BUDGETS,
        ),
        (
            every_nth(11, "mutant:canonical-refused", || SubjectResult {
                canonical: "{}".to_string(),
                re_decoded_code: Some("INVALID_JSON".to_string()),
                ..Default::default()
            }),
            DEFAULT_BUDGETS,
        ),
        (
            every_nth(13, "mutant:fixed-point-broken", || SubjectResult {
                canonical: "{\"a\":1}".to_string(),
                re_decoded: Some("{\"a\":2}".to_string()),
                ..Default::default()
            }),
            DEFAULT_BUDGETS,
        ),
    ];

    // The panicking mutant fires ~60 times by design, and the default hook prints
    // a backtrace note for each — 60 alarming-looking blocks in the log of a
    // PASSING test, which is how a reader learns to skim exactly the output that
    // matters. Suppressed for the duration and restored afterwards, so a genuine
    // panic elsewhere still reports normally.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        for (subject, budgets) in cases {
            let name = subject.name;
            let stats = run(
                std::slice::from_ref(&subject),
                budgets,
                BOUNDED_CONFIG,
                SEED,
                200,
                &seeds,
                &vocab,
            );
            assert!(
                !stats.counterexamples.is_empty(),
                "{name} produced no counterexample — the harness cannot see this defect class"
            );
            // The inverse pin, in the same place as the claim it qualifies.
            assert!(
                stats.counterexamples.len() < stats.inputs,
                "{name} broke EVERY input — it proves nothing about the harness's discrimination"
            );
        }
    }));
    std::panic::set_hook(previous_hook);
    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn a_well_formed_node_is_neither_a_refusal_nor_a_counterexample() {
    // The floor under everything above: the machinery must call a GOOD input
    // good. A harness that reported every input as a counterexample would pass
    // every go-red test in this file.
    let good = r#"{"id":"a","kind":{"$type":"Heading","level":1,"text":"x","variant":"Standard"}}"#;
    let subject = node_subject();
    let m = check(&subject, DEFAULT_BUDGETS, good);
    assert!(
        !m.verdict.is_counterexample(),
        "a well-formed node decoded as {:?} ({})",
        m.verdict.kind,
        m.verdict.detail
    );
    assert_eq!(m.verdict.kind, "clean");
}

#[test]
fn the_allocation_counter_measures_something() {
    // A probe that always read zero would make the allocation invariant vacuous
    // while every other test in this file still passed. This is the one assertion
    // that the measurement itself works.
    let before = allocated_bytes();
    let hog: Vec<u8> = vec![0u8; 4 * 1024 * 1024];
    std::hint::black_box(&hog);
    let after = allocated_bytes();
    assert!(
        after - before >= 4 * 1024 * 1024,
        "the counting allocator saw {} bytes for a 4 MiB allocation",
        after - before
    );
    assert!(
        TOTAL_ALLOC_EVENTS.load(Ordering::Relaxed) > 0,
        "the global allocator hook was never reached — the measurement is not installed"
    );
}

#[test]
fn the_allocation_counter_is_per_thread() {
    // The pin on the fix, not a restatement of it. This test allocates 32 MiB on
    // a SIBLING thread while measuring on this one; a process-wide counter reads
    // the sibling's bytes and the assertion fails. The parallel-test pollution
    // that produced a run of phantom `over-allocated` counterexamples is exactly
    // this shape, and nothing else in the file would notice it coming back.
    let before = allocated_bytes();
    std::thread::spawn(|| {
        let hog: Vec<u8> = vec![0u8; 32 * 1024 * 1024];
        std::hint::black_box(&hog);
    })
    .join()
    .expect("the sibling thread");
    let after = allocated_bytes();
    assert!(
        after - before < 1024 * 1024,
        "this thread's counter moved by {} bytes for another thread's 32 MiB allocation",
        after - before
    );
}
