//! Cross-host function-registry conformance (Phase 558).
//!
//! Loads the shared `wire-format-fixtures/function-registry/goldens.json` — the
//! canonical registry + findBySignature (EXACT/SUBSUMES) queries + compose-path
//! queries with expected results, derived from the SHIPPED Python reference (the
//! twin of the F# `Fuaran.Core.FunctionRegistry` engine). This Rust host must
//! resolve every golden identically. The registry-shape pin is the 548-style
//! attestation guard: a shape drift fails here with the entry named. Skips
//! cleanly on a standalone checkout where the corpus is absent.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::function::{
    ComposeResult, ComposeStep, FunctionEntry, FunctionRegistry, HoleKind, MatchMode, SigEntry,
    SignatureQuery, Space,
};

fn find_goldens() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let g = dir
            .join("wire-format-fixtures")
            .join("function-registry")
            .join("goldens.json");
        if g.is_file() {
            return Some(g);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn str_field(j: &JVal, key: &str) -> String {
    match j.field(key) {
        Some(JVal::Str(s)) => s.clone(),
        other => panic!("expected string field '{key}', got {other:?}"),
    }
}

fn num_field(j: &JVal, key: &str) -> f64 {
    match j.field(key) {
        Some(JVal::Num(n)) => *n,
        other => panic!("expected number field '{key}', got {other:?}"),
    }
}

fn bool_field(j: &JVal, key: &str) -> bool {
    match j.field(key) {
        Some(JVal::Bool(b)) => *b,
        other => panic!("expected bool field '{key}', got {other:?}"),
    }
}

fn arr_field<'a>(j: &'a JVal, key: &str) -> &'a Vec<JVal> {
    match j.field(key) {
        Some(JVal::Arr(a)) => a,
        other => panic!("expected array field '{key}', got {other:?}"),
    }
}

fn as_str(j: &JVal) -> String {
    match j {
        JVal::Str(s) => s.clone(),
        other => panic!("expected string, got {other:?}"),
    }
}

fn parse_space(j: &JVal) -> Space {
    match str_field(j, "kind").as_str() {
        "intRange" => Space::IntRange {
            min: num_field(j, "min") as i64,
            max: num_field(j, "max") as i64,
        },
        "floatRange" => Space::FloatRange {
            min: num_field(j, "min"),
            max: num_field(j, "max"),
        },
        "stringLen" => Space::StringLen {
            min: num_field(j, "min") as i64,
            max: num_field(j, "max") as i64,
        },
        "enum" => Space::Enum {
            choices: arr_field(j, "choices").iter().map(as_str).collect(),
        },
        _ => Space::AnyString,
    }
}

fn parse_kind(s: &str) -> HoleKind {
    match s {
        "value" => HoleKind::Value,
        "slot" => HoleKind::Slot,
        "repeat" => HoleKind::Repeat,
        "action" => HoleKind::Action,
        other => panic!("unknown hole kind '{other}'"),
    }
}

fn parse_sig(j: &JVal) -> SigEntry {
    SigEntry {
        addr: str_field(j, "addr"),
        name: str_field(j, "name"),
        kind: parse_kind(&str_field(j, "kind")),
        space: match j.field("space") {
            Some(JVal::Null) | None => None,
            Some(s) => Some(parse_space(s)),
        },
        slot: match j.field("slot") {
            Some(JVal::Str(s)) => Some(s.clone()),
            _ => None,
        },
        required: bool_field(j, "required"),
    }
}

fn parse_sigs(j: &JVal, key: &str) -> Vec<SigEntry> {
    arr_field(j, key).iter().map(parse_sig).collect()
}

fn parse_entry(j: &JVal) -> FunctionEntry {
    FunctionEntry {
        id: str_field(j, "id"),
        result_type: str_field(j, "resultType"),
        holes: parse_sigs(j, "holes"),
    }
}

fn parse_expected(j: &JVal) -> ComposeResult {
    if bool_field(j, "ok") {
        let steps = arr_field(j, "steps")
            .iter()
            .map(|s| ComposeStep {
                function_id: str_field(s, "functionId"),
                fills_slot: match s.field("fillsSlot") {
                    Some(JVal::Str(x)) => Some(x.clone()),
                    _ => None,
                },
            })
            .collect();
        ComposeResult::ComposePath(steps)
    } else {
        ComposeResult::NoPath(str_field(j, "reason"))
    }
}

fn load() -> Option<(JVal, FunctionRegistry)> {
    let path = find_goldens()?;
    let raw = std::fs::read_to_string(&path).expect("reading goldens.json");
    let goldens = parse(&raw).expect("goldens.json parses with the host's own JSON layer");
    let mut reg = FunctionRegistry::new();
    for e in arr_field(&goldens, "registry") {
        reg.register(parse_entry(e))
            .expect("registering golden entry");
    }
    Some((goldens, reg))
}

#[test]
fn registry_shape_matches_goldens() {
    let Some((goldens, reg)) = load() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let got = reg.registry_signature_shape();
    let want: Vec<String> = arr_field(&goldens, "registryShape")
        .iter()
        .map(as_str)
        .collect();
    assert_eq!(got, want, "registry-shape drift (548-style attestation)");
}

#[test]
fn find_by_signature_matches_goldens() {
    let Some((goldens, reg)) = load() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    for f in arr_field(&goldens, "findBySignature") {
        let name = str_field(f, "name");
        let mode = if str_field(f, "mode") == "Subsumes" {
            MatchMode::Subsumes
        } else {
            MatchMode::Exact
        };
        let q = f.field("query").expect("query");
        let result_type = match q.field("resultType") {
            Some(JVal::Str(s)) => Some(s.clone()),
            _ => None,
        };
        let query = SignatureQuery {
            result_type,
            available: parse_sigs(q, "available"),
        };
        let got: Vec<String> = reg
            .find_by_signature(mode, &query)
            .iter()
            .map(|e| e.id.clone())
            .collect();
        let want: Vec<String> = arr_field(f, "expectedIds").iter().map(as_str).collect();
        assert_eq!(got, want, "findBySignature '{name}'");
    }
}

#[test]
fn compose_matches_goldens() {
    let Some((goldens, reg)) = load() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    for c in arr_field(&goldens, "compose") {
        let name = str_field(c, "name");
        let output = str_field(c, "output");
        let inputs = parse_sigs(c, "inputs");
        let want = parse_expected(c.field("expected").expect("expected"));
        let got = reg.compose(&output, &inputs, MatchMode::Subsumes, 4);
        assert_eq!(got, want, "compose '{name}'");
    }
}
