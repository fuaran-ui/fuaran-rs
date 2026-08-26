//! Emits this host's refusal class for every reject fixture in the shared
//! wire-format corpus — this host's half of the cross-host identical-rejection
//! check.
//!
//! The corpus declares, per reject fixture, the code every conformant host must
//! answer with. Each host's own suite asserts its answer against that
//! declaration, which makes cross-host agreement true TRANSITIVELY — provided
//! every host's leg actually ran. That proviso is the gap: those legs return early
//! when the corpus is absent, and a conformance leg that silently asserts nothing
//! while the build stays green is a failure mode this repository has recorded
//! before.
//!
//! A cross-host runner collects one report per host and asserts the answers agree
//! with each other AND with the corpus declaration — one artefact, one place, and
//! a hard failure when a host is missing rather than a quiet omission.
//!
//! Per-host error TEXT stays free; the refusal CLASS must agree. The message rides
//! along so a reader can see what this host said, but the runner compares only the
//! code and the path prefix: pinning a message across five languages would be
//! pinning translation, not conformance.
//!
//! An EXAMPLE rather than a `[[bin]]`, deliberately: this is a harness that
//! exercises the library, not a program the crate ships to its consumers, and a
//! published crate should not grow an installable binary for the sake of a CI
//! step. `cargo clippy --all-targets` lints it either way.
//!
//! ```text
//! cargo run --example refusal_report -- [--corpus <dir>] [--out <file>]
//! ```
//!
//! Writes JSON to stdout by default. Exits non-zero only when the corpus cannot be
//! read: judging the answers is the runner's job, not this emitter's.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::wire::{decode_node, decode_op};

const HOST: &str = "fuaran-rs";

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

fn str_field(v: &JVal, key: &str) -> Option<String> {
    match v.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

/// `(refused, code, path, message)` for one payload under one decoder.
///
/// A panic is caught and reported as the answer it is: a host that panicked where
/// the contract says it returns has failed the totality claim, and saying so in
/// the report is more useful than dying mid-collection.
fn decode_one(decoder: &str, text: &str) -> (bool, String, String, String) {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if decoder == "op" {
            decode_op(text)
                .map(|_| ())
                .map_err(|e| (e.code.as_str().to_string(), e.path, e.message))
        } else {
            decode_node(text)
                .map(|_| ())
                .map_err(|e| (e.code.as_str().to_string(), e.path, e.message))
        }
    }));
    match outcome {
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_string());
            (true, "ESCAPED-PANIC".to_string(), "$".to_string(), detail)
        }
        Ok(Ok(())) => (false, String::new(), String::new(), String::new()),
        Ok(Err((code, path, message))) => (true, code, path, message),
    }
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let corpus = flag("--corpus").map(PathBuf::from).or_else(find_corpus);
    let Some(corpus) = corpus.filter(|c| c.join("manifest.json").is_file()) else {
        eprintln!(
            "{HOST}: the wire-format corpus was not found. Pass --corpus, or check the repo out beside the corpus."
        );
        std::process::exit(2);
    };

    let raw = match std::fs::read_to_string(corpus.join("manifest.json")) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("{HOST}: reading the corpus manifest: {e}");
            std::process::exit(2);
        }
    };
    let manifest = match parse(&raw) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{HOST}: parsing the corpus manifest: {e:?}");
            std::process::exit(2);
        }
    };
    let Some(JVal::Arr(fixtures)) = manifest.field("fixtures") else {
        eprintln!("{HOST}: the corpus manifest declares no fixtures array");
        std::process::exit(2);
    };

    let mut cases: Vec<String> = Vec::new();
    for fx in fixtures {
        if str_field(fx, "kind").as_deref() != Some("reject") {
            continue;
        }
        let id = str_field(fx, "id").unwrap_or_default();
        let decoder = str_field(fx, "decoder").unwrap_or_else(|| "node".to_string());
        if decoder != "node" && decoder != "op" {
            // Envelope / elicitation rejects run through their own decoders and are
            // NOT in scope here. Reported as skipped rather than silently dropped:
            // a runner that could not tell "not applicable" from "not present"
            // would read a shrinking corpus as agreement.
            cases.push(format!(
                "    {{\n      \"id\": \"{}\",\n      \"decoder\": \"{}\",\n      \"skipped\": \"decoder not in scope\"\n    }}",
                json_escape(&id),
                json_escape(&decoder)
            ));
            continue;
        }
        let input_file = str_field(fx, "inputFile").unwrap_or_default();
        let text = match std::fs::read_to_string(corpus.join(&input_file)) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{HOST}: reading {input_file}: {e}");
                std::process::exit(2);
            }
        };
        let (refused, code, path, message) = decode_one(&decoder, &text);
        cases.push(format!(
            "    {{\n      \"id\": \"{}\",\n      \"decoder\": \"{}\",\n      \"refused\": {},\n      \
             \"code\": \"{}\",\n      \"path\": \"{}\",\n      \"message\": \"{}\"\n    }}",
            json_escape(&id),
            json_escape(&decoder),
            refused,
            json_escape(&code),
            json_escape(&path),
            json_escape(&message)
        ));
    }

    let payload = format!(
        "{{\n  \"host\": \"{HOST}\",\n  \"corpus\": \"{}\",\n  \"cases\": [\n{}\n  ]\n}}\n",
        json_escape(&corpus.display().to_string()),
        cases.join(",\n")
    );

    match flag("--out") {
        Some(out) => {
            let out = PathBuf::from(out);
            if let Some(parent) = out.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                eprintln!("{HOST}: creating the output directory: {e}");
                std::process::exit(2);
            }
            if let Err(e) = std::fs::write(&out, payload) {
                eprintln!("{HOST}: writing the report: {e}");
                std::process::exit(2);
            }
        }
        None => print!("{payload}"),
    }
}
