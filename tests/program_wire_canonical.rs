//! Canonical-encoding conformance against the program wire specification's
//! corpus — §2, and only §2.
//!
//! # What this leg claims, and what it deliberately does not
//!
//! This host is **not a logic host**. It implements none of the program wire's
//! §5.1 server-effect vocabulary, so it cannot decode a `RunQuery` or a
//! `HostCall` into typed values and re-encode them, and a leg claiming to
//! "round-trip the server-effect family" would be claiming something false.
//!
//! What this host *does* own is the **canonical encoding layer** — the same §2
//! discipline the tree wire format specifies and this crate already implements
//! in `canonical::render_canonical`. The program wire adopts that discipline by
//! reference (its §2 opens by saying so), so every round-trip vector in that
//! corpus is a document whose *bytes* this host is obliged to be able to
//! reproduce: parse it, render it canonically, get the same bytes back.
//!
//! That is a narrower claim than the reference logic host's, and it is stated
//! narrowly on purpose. It is also the claim that carries the whole content of
//! the member-ordering rule, which is what this leg exists for.
//!
//! # The rule this leg is really about
//!
//! §2's member ordering is **Ordinal** — numeric comparison of UTF-16 code
//! units. This crate implements it deliberately in `canonical::ordinal_cmp`,
//! whose comment has long said it is pinned "so a supplementary-plane key
//! cannot silently diverge across hosts". Until the corpus gained
//! `server-effect/notify-ordinal-divergence`, no fixture in any corpus this
//! host certifies against could check that claim: every other document's keys
//! are ASCII, where Ordinal ordering and this language's native `Ord` (UTF-8
//! byte order, equivalently code-point order) agree exactly.
//!
//! They disagree across one boundary and one only. A supplementary character
//! encodes in UTF-16 as a surrogate pair whose leading unit is in
//! U+D800–U+DBFF, so Ordinally it sorts **below** every character in
//! U+E000–U+FFFF — while by code point it sorts **above** all of them. The
//! divergence vector's payload keys straddle that boundary, so this host's
//! ordering is now measured rather than asserted.
//!
//! # How this leg is invoked, and why it is a local gate
//!
//! Exactly as `driver_semantics.rs` is, and for the same reason — the corpus is
//! not a sibling this repository's public workflow checks out:
//!
//! * set `FUARAN_PROGRAM_SPEC` to the specification's directory, **or** have it
//!   checked out beside this repository under `fuaran-program-spec/`;
//! * `cargo test --test program_wire_canonical`, or `pwsh ./run.ps1
//!   -DriverSemantics`, which passes it through.
//!
//! Where the corpus **is** claimed and cannot be read, this leg **fails** — it
//! never skips. A conformance check that passes without its oracle is worse
//! than no check, because it reports the same green as one that ran.
//!
//! # Absence is two different facts, and only one of them is a skip
//!
//! Until Phase 1653 an absent corpus simply returned, so "the oracle is not
//! here" and "the oracle is here and agrees" reported the same `ok`. That is
//! defensible in a standalone clone of this repository, which genuinely cannot
//! run the leg — and indefensible in the cross-host workspace, where an absent
//! corpus means the check has been switched off and nobody was told.
//!
//! So the two are separated, by the discriminator `tests/render.rs` already
//! uses for the reference host: **any sibling host present ⇒ hard failure**
//! naming what proved the shape; **nothing else present ⇒ the honest standalone
//! NOT RUN**, which is what the public workflow reports (it checks out this
//! repository and the wire corpus, and no sibling host). And a corpus that IS
//! located must SAY it is the program wire's — `manifest.json`'s
//! `specification` member is read as part of the acceptance predicate, so a
//! variable pointing one directory sideways is refused by name instead of being
//! accepted and then reported as a corpus missing every vector.

use fuaran_rs::canonical::{JVal, parse, render_canonical};
use std::path::PathBuf;

/// The family the program wire specifies **as emitted** rather than
/// canonically: a `kind` discriminator and members in declaration order, an
/// enumerated exception in its §2 because those bytes are a shipped wire that
/// predates the document.
///
/// It is excluded here because canonically re-rendering it would reorder
/// `Download` and `ReadFileBody` and re-spell their short escapes — that is,
/// this leg would go red for being *right* about §2 and wrong about the
/// exception. Excluding it is not a gap in coverage: the exception is pinned by
/// the specification's own corpus gate, and by this crate's
/// `bounded::effect` tests, which are where an as-emitted envelope belongs.
const AS_EMITTED_FAMILY: &str = "client-effect";

/// Sibling hosts whose presence proves this is a cross-host workspace checkout
/// rather than a standalone clone. Deliberately excludes this host.
///
/// The same list, for the same purpose, as `tests/render.rs`'s
/// `OTHER_HOST_NAMES`: each integration test is its own crate, so the constant
/// cannot be shared without publishing it from the library, and publishing a
/// list of sibling repository names out of a crate is not a thing this host
/// should do to spare a duplication of seven strings.
const WORKSPACE_SIBLING_HOSTS: &[&str] = &[
    "fuaran-dotnet",
    "fuaran",
    "fuaran-ts",
    "fuaran-py",
    "fuaran-go",
    "fuaran-kt",
    "fuaran-swift",
];

/// The identity member every specification corpus in this estate carries. The
/// acceptance predicate READS it rather than trusting a directory name — a
/// manifest that merely CITES another specification satisfies a containment
/// probe, which is the recorded way this class of resolver goes wrong.
const PROGRAM_WIRE_IDENTITY: &str = "program-wire";

enum Corpus {
    /// An operator named it. Anything wrong with it from here is a hard failure.
    Declared(PathBuf),
    /// Found beside this repository.
    Discovered(PathBuf),
    /// Nothing claimed and nothing found, in a checkout that is plainly the
    /// cross-host workspace — so the oracle has been silently disabled rather
    /// than legitimately absent. Carries the sibling that proves the shape.
    MissingInWorkspace(String),
    /// Nothing claimed, nothing found, and nothing else here either — a genuine
    /// standalone clone of this repository.
    Absent,
}

impl Corpus {
    fn locate() -> Corpus {
        if let Ok(declared) = std::env::var("FUARAN_PROGRAM_SPEC") {
            if !declared.trim().is_empty() {
                return Corpus::Declared(PathBuf::from(declared).join("wire-fixtures"));
            }
        }
        let crate_dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
        let mut dir = crate_dir.clone();
        loop {
            let root = dir.join("fuaran-program-spec").join("wire-fixtures");
            if root.join("manifest.json").is_file() {
                return Corpus::Discovered(root);
            }
            if !dir.pop() {
                break;
            }
        }
        // Not found anywhere up the tree. Is this a standalone clone, or a
        // workspace checkout whose corpus is missing? The two look identical
        // from inside this function and are opposite facts: one is a repository
        // that legitimately cannot run this leg, the other is an oracle that has
        // been silently switched off. The discriminator is the one
        // `tests/render.rs` already uses for the reference host, and it is
        // chosen so the PUBLIC workflow — which checks out this repository and
        // the wire corpus and no sibling host — stays honestly NOT RUN.
        let mut dir = crate_dir;
        loop {
            for sibling in WORKSPACE_SIBLING_HOSTS {
                if dir.join(sibling).is_dir() {
                    return Corpus::MissingInWorkspace(format!(
                        "{}/ is present under {}",
                        sibling,
                        dir.display()
                    ));
                }
            }
            if !dir.pop() {
                break;
            }
        }
        Corpus::Absent
    }

    /// The acceptance half: a located corpus must SAY it is the program wire's,
    /// and an absence must not read as a pass.
    ///
    /// Reading the identity out of the manifest is what separates "this is the
    /// wrong corpus" from "this corpus is missing a vector", and the two have
    /// different remedies. Without it, an operator whose `FUARAN_PROGRAM_SPEC`
    /// points one directory sideways gets acceptance followed by a confident
    /// complaint about absent vectors — loud, and wrong about the cause.
    fn accept(self) -> Option<PathBuf> {
        let (fixtures, declared) = match self {
            Corpus::Declared(p) => (p, true),
            Corpus::Discovered(p) => (p, false),
            Corpus::MissingInWorkspace(evidence) => panic!(
                "the program wire corpus is neither claimed nor present, but this is a \
                 cross-host workspace checkout ({evidence}) — so this leg has been silently \
                 disabled rather than legitimately skipped, and a missing oracle reports the \
                 same green as a run. Clone `fuaran-program-spec` beside this repository, or \
                 name it with FUARAN_PROGRAM_SPEC. (A standalone clone of this repository \
                 alone still reports NOT RUN and asserts nothing.)"
            ),
            Corpus::Absent => {
                eprintln!(
                    "the program wire corpus is neither claimed nor present beside this \
                     repository, and no sibling host is present either; this leg asserted \
                     nothing. Set FUARAN_PROGRAM_SPEC to run it."
                );
                return None;
            }
        };

        let manifest_path = fixtures.join("manifest.json");
        let raw = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
            panic!(
                "the corpus is {} at '{}' but its manifest could not be read: {e}. \
                 A conformance check that passes without its oracle is worse than no check, \
                 so this is a failure rather than a skip.",
                if declared { "claimed" } else { "present" },
                manifest_path.display()
            )
        });
        let manifest = parse(&raw).expect("the manifest parses with this host's own JSON layer");
        match manifest.field("specification") {
            Some(JVal::Str(id)) if id == PROGRAM_WIRE_IDENTITY => Some(fixtures),
            other => panic!(
                "'{}' is not the program wire corpus: its manifest's `specification` member is \
                 {other:?}, not \"{PROGRAM_WIRE_IDENTITY}\". Refused at RESOLUTION rather than \
                 accepted and then reported as a corpus missing every vector, which names the \
                 wrong cause.",
                manifest_path.display()
            ),
        }
    }
}

fn string_field(entry: &JVal, key: &str) -> Option<String> {
    match entry.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

#[test]
fn every_round_trip_vector_re_renders_to_its_committed_bytes() {
    let Some(fixtures) = Corpus::locate().accept() else {
        return;
    };

    let manifest_path = fixtures.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).expect("the manifest was read at acceptance");
    let manifest = parse(&raw).expect("the manifest parses with this host's own JSON layer");

    let Some(JVal::Arr(vectors)) = manifest.field("vectors") else {
        panic!("the manifest declares no vectors array");
    };

    let mut checked = 0usize;
    let mut skipped_as_emitted = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut saw_divergence_vector = false;

    for entry in vectors {
        if string_field(entry, "kind").as_deref() != Some("round-trip") {
            continue;
        }
        let id = string_field(entry, "id").expect("a vector carries a string id");
        let file = string_field(entry, "file").expect("a vector carries a string file");

        if string_field(entry, "family").as_deref() == Some(AS_EMITTED_FAMILY) {
            skipped_as_emitted += 1;
            continue;
        }
        if id == "server-effect/notify-ordinal-divergence" {
            saw_divergence_vector = true;
        }

        let path = fixtures.join(&file);
        let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("{id}: the manifest enumerates '{file}', which could not be read: {e}")
        });

        let value = match parse(&committed) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!(
                    "{id}: this host's parser refused a round-trip vector: {e:?}"
                ));
                continue;
            }
        };

        let rendered = render_canonical(&value);
        checked += 1;

        if rendered != committed {
            failures.push(format!(
                "{id}\n     committed: {committed}\n     re-rendered: {rendered}"
            ));
        }
    }

    // The harness's own obligations, asserted rather than hoped for. A filter
    // that matched nothing would otherwise report the same green as a run.
    assert!(
        checked > 0,
        "no round-trip vectors were checked — the corpus was located but the filter matched nothing"
    );
    assert!(
        skipped_as_emitted > 0,
        "the as-emitted family was not found in the manifest; if it has been renamed this \
         exclusion is silently covering nothing, and the constant above needs updating"
    );

    // The point of the leg, pinned so it cannot quietly stop being the point.
    // Without this vector every remaining document has ASCII keys, and a host
    // sorting by its own native ordering would pass all of them.
    assert!(
        saw_divergence_vector,
        "the corpus no longer enumerates 'server-effect/notify-ordinal-divergence' as a round-trip \
         vector. That is the only vector here whose member order distinguishes Ordinal (UTF-16 \
         code-unit) ordering from this language's native ordering, so without it this leg cannot \
         detect the very divergence it exists for."
    );

    assert!(
        failures.is_empty(),
        "{} of {checked} round-trip vectors did not re-render to their committed bytes:\n  - {}",
        failures.len(),
        failures.join("\n  - ")
    );

    eprintln!(
        "program wire §2: {checked} round-trip vectors re-rendered byte-identically \
         ({skipped_as_emitted} as-emitted vectors excluded by design)"
    );
}
