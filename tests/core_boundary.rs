//! The Core boundary (Phase 1863), held by a test rather than a convention.
//!
//! `src/core/` holds this host's twins of the Core reference subsystems — the
//! dataframe model, the Transform evaluator with list-param substitution, the
//! signature-searchable function registry and the canonical number form. The
//! public modules (`transform`, `function`, `wire`, `canonical`) re-export them,
//! so no published path moved. What makes the boundary worth having is that the
//! code inside it can be lifted out whole: it reaches nothing else in this crate.
//!
//! The crate declares no dependencies, a Rust parser included, so this is a
//! deliberately small source walk rather than a `syn` pass. It strips comments,
//! string literals and char literals, tokenises what is left, and resolves every
//! path that starts at `crate`, `self`, `super`, `$crate` or this crate's own
//! name to a module of this crate. The scanner's own edge cases are pinned by
//! the `scanner_*` tests below, each of which must see a violation it plants, so
//! a scanner that went blind would fail here instead of passing everything.

use std::fs;
use std::path::{Path, PathBuf};

/// The crate's own import name — the name `tests/` and `examples/` see it by.
const CRATE_NAME: &str = "fuaran_rs";

/// The module that IS the boundary.
const CORE: &str = "core";

/// The roots a `use` inside the boundary may start from: the standard library
/// crates and the in-crate path keywords. Everything else is either a domain
/// module of this host or a package the boundary must not depend on.
const ALLOWED_USE_ROOTS: &[&str] = &["std", "core", "alloc", "crate", "self", "super"];

// ─── Lexing ──────────────────────────────────────────────────────────────────

/// Replace every comment, string literal and char literal with spaces, keeping
/// newlines so line numbers survive. What remains is code.
fn strip(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';

    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();

        // Line comment (doc comments included).
        if c == '/' && next == Some('/') {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }

        // Block comment, nested.
        if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
            }
            continue;
        }

        // Raw string: r"…", r#"…"#, br#"…"# — `r` not ending a longer identifier.
        let prev_ident =
            i > 0 && is_ident(b[i - 1]) && !(b[i - 1] == 'b' && (i < 2 || !is_ident(b[i - 2])));
        if c == 'r' && !prev_ident {
            let mut j = i + 1;
            while j < b.len() && b[j] == '#' {
                j += 1;
            }
            if j < b.len() && b[j] == '"' {
                let hashes = j - (i + 1);
                out.push(' ');
                for _ in 0..hashes {
                    out.push(' ');
                }
                out.push(' ');
                i = j + 1;
                while i < b.len() {
                    if b[i] == '"' && (0..hashes).all(|k| b.get(i + 1 + k) == Some(&'#')) {
                        for _ in 0..=hashes {
                            out.push(' ');
                        }
                        i += 1 + hashes;
                        break;
                    }
                    out.push(blank(b[i]));
                    i += 1;
                }
                continue;
            }
        }

        // Ordinary (or byte) string.
        if c == '"' {
            out.push(' ');
            i += 1;
            while i < b.len() {
                if b[i] == '\\' {
                    out.push(' ');
                    if let Some(&e) = b.get(i + 1) {
                        out.push(blank(e));
                    }
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                }
                out.push(blank(b[i]));
                i += 1;
            }
            continue;
        }

        // Char literal — or a lifetime / label, which stays code.
        if c == '\'' {
            if next == Some('\\') {
                // An escaped char: skip the escaped char, then to the closing
                // quote, so an escaped quote does not close the literal.
                let mut j = i + 3;
                while j < b.len() && b[j] != '\'' && b[j] != '\n' {
                    j += 1;
                }
                for _ in i..=j.min(b.len() - 1) {
                    out.push(' ');
                }
                i = j + 1;
                continue;
            }
            if b.get(i + 2) == Some(&'\'') {
                out.push_str("   ");
                i += 3;
                continue;
            }
        }

        out.push(c);
        i += 1;
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    PathSep,
    Open,
    Close,
    Comma,
    Semi,
    Other,
}

/// Tokenise stripped code into `(token, line)` pairs. Only the shapes a path
/// check needs are distinguished; everything else is `Other`.
fn tokenize(code: &str) -> Vec<(Tok, usize)> {
    let b: Vec<char> = code.chars().collect();
    let mut toks = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            let word: String = b[start..i].iter().collect();
            // `$crate` resolves exactly like `crate`.
            let word = if word == "$crate" {
                "crate".to_string()
            } else {
                word
            };
            toks.push((Tok::Ident(word), line));
        } else if c == ':' && b.get(i + 1) == Some(&':') {
            toks.push((Tok::PathSep, line));
            i += 2;
        } else {
            toks.push((
                match c {
                    '{' => Tok::Open,
                    '}' => Tok::Close,
                    ',' => Tok::Comma,
                    ';' => Tok::Semi,
                    _ => Tok::Other,
                },
                line,
            ));
            i += 1;
        }
    }
    toks
}

// ─── Resolution ──────────────────────────────────────────────────────────────

/// One place the boundary reaches outside itself.
#[derive(Debug, Clone, PartialEq)]
struct Breach {
    line: usize,
    /// The module path the reference resolves to, from the crate root.
    target: String,
    /// Whether the reference sits in a `use` declaration.
    in_use: bool,
}

fn ident(t: &(Tok, usize)) -> Option<&str> {
    match &t.0 {
        Tok::Ident(s) => Some(s),
        _ => None,
    }
}

/// Resolve a path's leading keyword run to a crate-root module path, or `None`
/// when the path does not start at this crate (a std path, a local item).
/// Returns the resolved prefix and the index of the first unconsumed token.
fn resolve_root(toks: &[(Tok, usize)], i: usize, here: &[String]) -> Option<(Vec<String>, usize)> {
    let first = ident(&toks[i])?;
    let (mut resolved, mut j) = match first {
        "crate" => (Vec::new(), i + 1),
        n if n == CRATE_NAME => (Vec::new(), i + 1),
        "self" => (here.to_vec(), i + 1),
        "super" => {
            let mut at = here.to_vec();
            let mut j = i;
            loop {
                // `super` above the crate root does not compile; treat it as
                // the root so it is still reported rather than ignored.
                at.pop();
                j += 1;
                let chained = toks.get(j).map(|t| &t.0) == Some(&Tok::PathSep)
                    && toks.get(j + 1).and_then(ident) == Some("super");
                if !chained {
                    break;
                }
                j += 1;
            }
            (at, j)
        }
        _ => return None,
    };
    // Consume `:: ident` segments.
    while toks.get(j).map(|t| &t.0) == Some(&Tok::PathSep) {
        match toks.get(j + 1).and_then(ident) {
            Some(seg) => {
                resolved.push(seg.to_string());
                j += 2;
            }
            None => break,
        }
    }
    Some((resolved, j))
}

/// Every reference in one boundary file that resolves outside `core`.
///
/// `file_mods` is the file's module path below the crate root (`["core"]` for
/// `src/core/mod.rs`, `["core", "transform"]` for `src/core/transform.rs`).
fn breaches(src: &str, file_mods: &[String]) -> (Vec<Breach>, usize) {
    let toks = tokenize(&strip(src));
    let mut out = Vec::new();
    let mut paths_seen = 0;

    // Inline `mod name { … }` nesting: (brace depth it opened at, name).
    let mut inline: Vec<(usize, String)> = Vec::new();
    let mut depth = 0usize;
    // Brace depth at which the current `use` declaration started, if any.
    let mut in_use = false;

    let mut i = 0;
    while i < toks.len() {
        match &toks[i].0 {
            Tok::Open => {
                depth += 1;
                i += 1;
                continue;
            }
            Tok::Close => {
                depth = depth.saturating_sub(1);
                if inline.last().is_some_and(|(d, _)| *d == depth) {
                    inline.pop();
                }
                i += 1;
                continue;
            }
            Tok::Semi => {
                in_use = false;
                i += 1;
                continue;
            }
            _ => {}
        }

        let prev_sep = i > 0 && toks[i - 1].0 == Tok::PathSep;
        let word = ident(&toks[i]);

        if word == Some("mod") {
            if let (Some(name), Some((Tok::Open, _))) =
                (toks.get(i + 1).and_then(ident), toks.get(i + 2))
            {
                inline.push((depth, name.to_string()));
            }
        }
        if word == Some("use") && !prev_sep {
            in_use = true;
        }

        if !prev_sep {
            let mut here: Vec<String> = file_mods.to_vec();
            here.extend(inline.iter().map(|(_, n)| n.clone()));
            if let Some((prefix, j)) = resolve_root(&toks, i, &here) {
                // A keyword alone (`pub(crate)`, `pub(super)`) is not a path.
                let is_path = j > i + 1 || toks.get(j).map(|t| &t.0) == Some(&Tok::PathSep);
                if is_path {
                    paths_seen += 1;
                    let line = toks[i].1;
                    let group = toks.get(j).map(|t| &t.0) == Some(&Tok::PathSep)
                        && toks.get(j + 1).map(|t| &t.0) == Some(&Tok::Open);
                    if group && prefix.is_empty() {
                        // `crate::{a, b::c}` — each entry's first segment decides.
                        let mut k = j + 2;
                        let mut level = 1;
                        let mut entry_start = true;
                        while k < toks.len() && level > 0 {
                            match &toks[k].0 {
                                Tok::Open => level += 1,
                                Tok::Close => level -= 1,
                                Tok::Comma if level == 1 => {
                                    entry_start = true;
                                    k += 1;
                                    continue;
                                }
                                Tok::Ident(s) if level == 1 && entry_start && s != CORE => {
                                    out.push(Breach {
                                        line: toks[k].1,
                                        target: if s == "self" {
                                            "crate".into()
                                        } else {
                                            s.clone()
                                        },
                                        in_use,
                                    });
                                }
                                _ => {}
                            }
                            entry_start = false;
                            k += 1;
                        }
                    } else if prefix.first().map(String::as_str) != Some(CORE) {
                        out.push(Breach {
                            line,
                            target: if prefix.is_empty() {
                                "crate".into()
                            } else {
                                prefix.join("::")
                            },
                            in_use,
                        });
                    }
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    (out, paths_seen)
}

/// The names a file declares itself — a `use` may root at one of these (`use
/// Enum::*`, a macro re-exported by path) without leaving the file.
fn local_items(src: &str) -> Vec<String> {
    let toks = tokenize(&strip(src));
    let kinds = [
        "mod",
        "enum",
        "struct",
        "trait",
        "type",
        "fn",
        "const",
        "static",
        "macro_rules",
    ];
    let mut names = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if ident(t).is_some_and(|w| kinds.contains(&w)) {
            // `macro_rules! name` has the `!` between.
            let at = if ident(t) == Some("macro_rules") {
                i + 2
            } else {
                i + 1
            };
            if let Some(name) = toks.get(at).and_then(ident) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// The root of every `use` declaration in a file, with its line.
fn use_roots(src: &str) -> Vec<(String, usize)> {
    let toks = tokenize(&strip(src));
    let mut roots = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let prev_sep = i > 0 && toks[i - 1].0 == Tok::PathSep;
        if ident(t) == Some("use") && !prev_sep {
            // `use ::name` and `use name` both root at `name`.
            let mut j = i + 1;
            if toks.get(j).map(|t| &t.0) == Some(&Tok::PathSep) {
                j += 1;
            }
            if let Some(root) = toks.get(j).and_then(ident) {
                roots.push((root.to_string(), toks[j].1));
            }
        }
        if ident(t) == Some("extern") && toks.get(i + 1).and_then(ident) == Some("crate") {
            if let Some(name) = toks.get(i + 2).and_then(ident) {
                roots.push((format!("extern crate {name}"), t.1));
            }
        }
    }
    roots
}

// ─── The tree ────────────────────────────────────────────────────────────────

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `src/core/`, with its module path from the crate root.
fn core_files() -> Vec<(PathBuf, Vec<String>)> {
    fn walk(dir: &Path, mods: &[String], out: &mut Vec<(PathBuf, Vec<String>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("read src/core")
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            if p.is_dir() {
                let mut m = mods.to_vec();
                m.push(stem);
                walk(&p, &m, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let mut m = mods.to_vec();
                if stem != "mod" {
                    m.push(stem);
                }
                out.push((p, m));
            }
        }
    }
    let mut out = Vec::new();
    walk(&src_dir().join(CORE), &[CORE.to_string()], &mut out);
    out
}

/// The crate's top-level modules other than `core` — the host's domain
/// packages — read from `lib.rs`, so a module added later is covered unasked.
fn domain_modules() -> Vec<String> {
    let lib = fs::read_to_string(src_dir().join("lib.rs")).expect("read lib.rs");
    let toks = tokenize(&strip(&lib));
    let mut mods = Vec::new();
    for w in toks.windows(3) {
        if ident(&w[0]) == Some("mod") && w[2].0 == Tok::Semi {
            if let Some(name) = ident(&w[1]) {
                if name != CORE {
                    mods.push(name.to_string());
                }
            }
        }
    }
    mods
}

fn rel(p: &Path) -> String {
    p.strip_prefix(env!("CARGO_MANIFEST_DIR"))
        .unwrap_or(p)
        .display()
        .to_string()
}

// ─── The boundary ────────────────────────────────────────────────────────────

#[test]
fn core_references_no_module_of_this_crate_outside_the_boundary() {
    let files = core_files();
    let names: Vec<String> = files
        .iter()
        .map(|(p, _)| rel(p).replace('\\', "/"))
        .collect();
    // Non-vacuity: the walk found the twins it exists to hold.
    for expected in [
        "src/core/mod.rs",
        "src/core/dataframe.rs",
        "src/core/transform.rs",
        "src/core/function.rs",
        "src/core/number.rs",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "the walk did not find {expected}: {names:?}"
        );
    }

    let mut found = Vec::new();
    let mut paths_seen = 0;
    for (path, mods) in &files {
        let (b, seen) = breaches(&fs::read_to_string(path).unwrap(), mods);
        paths_seen += seen;
        found.extend(
            b.into_iter()
                .map(|b| format!("{}:{} -> crate::{}", rel(path), b.line, b.target)),
        );
    }
    // Non-vacuity: the boundary's own intra-core paths were resolved.
    assert!(
        paths_seen > 0,
        "no in-crate path was resolved at all — the scanner is blind"
    );
    assert!(
        found.is_empty(),
        "the Core boundary (src/core/) references modules outside it; the twins must reach \
         nothing but `core` and the standard library:\n  {}",
        found.join("\n  ")
    );
}

#[test]
fn nothing_inside_the_boundary_imports_from_the_domain_packages() {
    let domain = domain_modules();
    // Non-vacuity: lib.rs was read and its domain modules found.
    for m in ["wire", "canonical", "render", "transform", "function"] {
        assert!(
            domain.iter().any(|d| d == m),
            "lib.rs scan missed `{m}`: {domain:?}"
        );
    }

    let mut found = Vec::new();
    for (path, mods) in core_files() {
        let src = fs::read_to_string(&path).unwrap();
        let local = local_items(&src);
        for (root, line) in use_roots(&src) {
            if !ALLOWED_USE_ROOTS.contains(&root.as_str()) && !local.contains(&root) {
                found.push(format!(
                    "{}:{line} -> `use {root}…` (not std or the boundary itself)",
                    rel(&path)
                ));
            }
        }
        for b in breaches(&src, &mods).0.into_iter().filter(|b| b.in_use) {
            let top = b.target.split("::").next().unwrap_or_default().to_string();
            let what = if domain.contains(&top) {
                "domain package"
            } else {
                "crate root"
            };
            found.push(format!(
                "{}:{} -> `use` of {what} `crate::{}`",
                rel(&path),
                b.line,
                b.target
            ));
        }
    }
    assert!(
        found.is_empty(),
        "the Core boundary (src/core/) imports from outside it:\n  {}",
        found.join("\n  ")
    );
}

#[test]
fn the_public_twin_modules_only_re_export_the_boundary() {
    // `transform` and `function` are the published faces of the twins. They
    // hold no implementation of their own, so every Core twin is behind the
    // boundary and a later change to one is an edit in `src/core/` only.
    for facade in ["transform/mod.rs", "function/mod.rs"] {
        let src = fs::read_to_string(src_dir().join(facade)).unwrap();
        let toks = tokenize(&strip(&src));
        let items: Vec<String> = toks
            .iter()
            .filter_map(ident)
            .filter(|w| {
                [
                    "fn",
                    "struct",
                    "enum",
                    "impl",
                    "trait",
                    "type",
                    "const",
                    "static",
                    "macro_rules",
                    "mod",
                ]
                .contains(w)
            })
            .map(str::to_string)
            .collect();
        assert!(
            items.is_empty(),
            "src/{facade} defines items of its own ({items:?}); it must only re-export src/core/"
        );
        assert!(
            src.contains("pub use crate::core::"),
            "src/{facade} does not re-export the boundary"
        );
    }
}

// ─── The scanner, made to fail ───────────────────────────────────────────────

fn m(path: &[&str]) -> Vec<String> {
    path.iter().map(|s| s.to_string()).collect()
}

#[test]
fn scanner_sees_a_planted_domain_import() {
    let (b, _) = breaches("use crate::wire::Node;\n", &m(&["core", "transform"]));
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(b[0].target, "wire::Node");
    assert!(b[0].in_use);
    assert_eq!(
        use_roots("use crate::wire::Node;"),
        vec![("crate".to_string(), 1)]
    );
}

#[test]
fn scanner_sees_expression_paths_groups_super_escapes_and_the_crate_name() {
    let here = m(&["core", "transform"]);
    let cases = [
        ("fn f() { crate::render::go(); }", "render::go"),
        ("use crate::{core::number, wire::Cell};", "wire"),
        ("use super::super::render;", "render"),
        ("use fuaran_rs::ops::apply;", "ops::apply"),
        (
            "fn f() -> char { let q = '\"'; crate::wire::x(); q }",
            "wire::x",
        ),
        (
            "macro_rules! m { () => { $crate::wire::Cell::Null } }",
            "wire::Cell::Null",
        ),
    ];
    for (src, target) in cases {
        let (b, _) = breaches(src, &here);
        assert_eq!(b.len(), 1, "{src}: {b:?}");
        assert_eq!(b[0].target, target, "{src}");
    }
    // From `src/core/mod.rs` a single `super` is already the crate root.
    let (b, _) = breaches("use super::wire;", &m(&["core"]));
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(use_roots("extern crate serde;")[0].0, "extern crate serde");
    let local = local_items("macro_rules! m { () => {} } pub(crate) use m; enum E { A } use E::*;");
    assert_eq!(local, vec!["m".to_string(), "E".to_string()]);
}

#[test]
fn scanner_ignores_comments_strings_and_in_boundary_paths() {
    let here = m(&["core", "transform"]);
    let clean = [
        "// use crate::wire::Node;\n",
        "/* crate::wire::Node /* nested */ crate::render */",
        "const S: &str = \"crate::wire::Node\";",
        "const R: &str = r#\"crate::wire \"quoted\" \"#;",
        "use crate::core::number::format_number;",
        "use super::dataframe::Cell;",
        "use self::inner::x;",
        "pub(crate) fn f() {} pub(super) fn g() {}",
        "fn f<'a>(x: &'a str) -> &'a str { x }",
        // `tests` → `transform` → `core`: two `super`s stay inside.
        "mod tests { use super::super::number; }",
    ];
    for src in clean {
        let (b, _) = breaches(src, &here);
        assert!(b.is_empty(), "{src}: {b:?}");
    }
    // …and that the inline module is popped: the same path after it escapes.
    let (b, _) = breaches("mod tests { } use super::super::render;", &here);
    assert_eq!(b.len(), 1, "{b:?}");
}
