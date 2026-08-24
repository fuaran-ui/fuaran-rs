//! Driver-semantics conformance: this host's bounded program loop, measured
//! against the program wire specification's scenario corpus.
//!
//! The corpus's codec families certify what a *document is*. This family
//! certifies what the *loop does*: a scenario is a tree, an ordered event
//! script, and the per-step trace a conformant loop produces from them. Each
//! scenario is driven through `fuaran_rs::bounded` and compared step by step —
//! the resolved tree **semantically** (decoded and re-encoded through this
//! host's own codec, so this host is measured against its own bytes), the
//! client effects **byte-for-byte** in their as-emitted envelope, the refusal
//! exactly, and the **denials** by decoding them into this host's own
//! vocabulary — and the **first** divergence is reported with its step index and
//! the member that differed.
//!
//! The bounded-path declaration this family presumes is in
//! `src/bounded/mod.rs`, which is where the claim belongs; this file is the
//! executable half of it.
//!
//! # How this leg is invoked, and why it is a local gate
//!
//! The scenario corpus is not a sibling this repository's public workflow
//! checks out, so this leg is **operator-invoked and locally scoped**:
//!
//! * set `FUARAN_PROGRAM_SPEC` to the specification's directory, **or** have it
//!   checked out beside this repository under `fuaran-program-spec/`;
//! * `cargo test --test driver_semantics`, or `pwsh ./run.ps1`, which passes it
//!   through.
//!
//! Where the corpus **is** claimed and cannot be read, this leg **fails** — it
//! never skips. A conformance check that passes without its oracle is worse than
//! no check, because it reports the same green as one that ran. Where no corpus
//! is claimed at all, the leg reports that it did not run and asserts nothing,
//! which is what keeps a checkout that has never seen the corpus honest rather
//! than red.
//!
//! # The harness's own obligations
//!
//! A green result means nothing without them, so both are asserted here:
//!
//! * **the number of scenarios run equals the number the manifest enumerates**
//!   — the manifest is the authoritative enumeration, never a directory listing
//!   and never a count written down in prose; and
//! * **a mutated trace makes this harness go red** — proved on every run against
//!   a perturbed copy of a real scenario, in the tree, in the effects, and in
//!   the denials, because a comparison that cannot fail certifies nothing. The
//!   denials probe is staged twice and deliberately: once as a host that
//!   *performs* what the scenario's policy declines, and once as a host that
//!   declines correctly and fails to *record* it. Only the first is what a
//!   security claim is about; only the second is what a regression looks like.

use std::path::{Path, PathBuf};

use fuaran_rs::bounded::{
    StepObservation, first_divergence, normalise_expectation, parse_events, parse_expectation,
    run_scenario,
};
use fuaran_rs::canonical::{JVal, parse};

/// The obligation every scenario in this family presumes. A scenario presuming
/// something else — a replay mode, an idempotency store — is a scenario this
/// host has not declared it implements, and it is refused by name rather than
/// silently run: the field exists so that a second obligation **enumerates**
/// rather than renumbers.
const DECLARED_OBLIGATION: &str = "bounded-loop";

const FAMILY: &str = "driver-semantics";

/// Where the corpus was found, and how — the two are reported together because
/// the remedy for a failure differs between them.
enum Corpus {
    /// An operator named it. Anything wrong with it from here is a hard failure.
    Declared(PathBuf),
    /// Found beside this repository.
    Discovered(PathBuf),
    /// Nothing claimed and nothing found.
    Absent,
}

impl Corpus {
    fn locate() -> Corpus {
        if let Ok(declared) = std::env::var("FUARAN_PROGRAM_SPEC") {
            if !declared.trim().is_empty() {
                return Corpus::Declared(PathBuf::from(declared).join("wire-fixtures"));
            }
        }
        let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
        loop {
            let root = dir.join("fuaran-program-spec").join("wire-fixtures");
            if root.join("manifest.json").is_file() {
                return Corpus::Discovered(root);
            }
            if !dir.pop() {
                return Corpus::Absent;
            }
        }
    }
}

/// One scenario, exactly as the manifest declares it.
struct ScenarioEntry {
    name: String,
    tree: String,
    events: String,
    expectation: String,
    steps: usize,
    /// The host-policy NAME the scenario presumes, where it names one. It lives
    /// in the manifest rather than in the three files because a denial is a fact
    /// about a policy, and a corpus carrying the policy as data would be
    /// specifying it — so the index names it and this host constructs what the
    /// name denotes.
    host_policy: Option<String>,
}

fn string_at(value: &JVal, key: &str) -> String {
    match value.field(key) {
        Some(JVal::Str(s)) => s.clone(),
        other => panic!("the manifest entry carries no string '{key}' (found {other:?})"),
    }
}

/// The manifest's enumeration of this family, in declared order.
///
/// Read from the manifest and **never** from a directory listing. A scenario
/// present on disk but absent from the manifest is not a harmless extra: it is
/// behaviour nobody is required to reproduce, while every host still reports
/// full conformance — and reading the directory would hide exactly that.
fn enumerated(fixtures: &Path) -> Vec<ScenarioEntry> {
    let manifest_path = fixtures.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "the scenario corpus is claimed at '{}' but its manifest could not be read: {e}. \
             This leg fails rather than skipping, deliberately: a conformance check that passes \
             when its oracle is missing is worse than no check.",
            manifest_path.display()
        )
    });
    let manifest = parse(&raw).expect("the manifest parses with this host's own JSON layer");

    match manifest.field("scenarioFamilies") {
        Some(JVal::Arr(families)) => assert!(
            families.iter().any(|f| f == &JVal::Str(FAMILY.into())),
            "the corpus does not enumerate a '{FAMILY}' scenario family"
        ),
        _ => panic!("the manifest declares no scenarioFamilies array"),
    }

    let Some(JVal::Arr(entries)) = manifest.field("scenarios") else {
        panic!("the manifest declares no scenarios array");
    };

    entries
        .iter()
        .filter(|e| string_at(e, "family") == FAMILY)
        .map(|e| {
            let name = string_at(e, "name");
            let requires = string_at(e, "requires");
            assert_eq!(
                requires, DECLARED_OBLIGATION,
                "scenario '{name}' presumes the '{requires}' obligation, which this host has not \
                 declared. That is a scope question, not a failure of the loop — declare the \
                 obligation, or say why it is out of scope, rather than running the scenario anyway."
            );
            let files = e.field("files").expect("the entry names its files");
            let steps = match e.field("steps") {
                Some(JVal::Num(n)) => *n as usize,
                _ => panic!("scenario '{name}' declares no step count"),
            };
            ScenarioEntry {
                name,
                tree: string_at(files, "tree"),
                events: string_at(files, "events"),
                expectation: string_at(files, "expectation"),
                steps,
                host_policy: match e.field("hostPolicy") {
                    None => None,
                    Some(JVal::Str(s)) => Some(s.clone()),
                    Some(other) => panic!("the entry carries a non-string hostPolicy ({other:?})"),
                },
            }
        })
        .collect()
}

fn read(fixtures: &Path, relative: &str) -> String {
    let path = fixtures.join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "the manifest enumerates '{relative}', which could not be read at '{}': {e}",
            path.display()
        )
    })
}

/// A loaded scenario, ready to run.
struct Scenario {
    name: String,
    tree_json: String,
    events: Vec<fuaran_rs::bounded::LiveEvent>,
    recorded: Vec<StepObservation>,
    host_policy: Option<String>,
}

fn load(fixtures: &Path, entry: &ScenarioEntry) -> Scenario {
    let events = parse_events(&read(fixtures, &entry.events))
        .unwrap_or_else(|e| panic!("{}: {e}", entry.name));
    let recorded = parse_expectation(&read(fixtures, &entry.expectation))
        .unwrap_or_else(|e| panic!("{}: {e}", entry.name));

    // The manifest is authoritative on the count, so the files are measured
    // against it rather than the other way round.
    assert_eq!(
        recorded.len(),
        entry.steps,
        "{}: the recorded trace carries {} steps; the manifest declares {}",
        entry.name,
        recorded.len(),
        entry.steps
    );
    // One entry per step, index 0 being the state before any event.
    assert_eq!(
        recorded.len(),
        events.len() + 1,
        "{}: a trace carries one entry per step, index 0 being the state before any event",
        entry.name
    );

    Scenario {
        name: entry.name.clone(),
        tree_json: read(fixtures, &entry.tree),
        events,
        recorded,
        host_policy: entry.host_policy.clone(),
    }
}

/// Drive one scenario and compare it against its recorded trace, returning the
/// first divergence as a report.
fn check(scenario: &Scenario) -> Result<(), String> {
    let observed = run_scenario(
        &scenario.tree_json,
        &scenario.events,
        scenario.host_policy.as_deref(),
    )
    .map_err(|e| format!("{}: {e}", scenario.name))?;
    let expected = normalise_expectation(&scenario.name, &scenario.recorded)?;
    match first_divergence(&scenario.name, &expected, &observed) {
        None => Ok(()),
        Some(divergence) => Err(divergence.describe()),
    }
}

fn fixtures_root() -> Option<PathBuf> {
    match Corpus::locate() {
        Corpus::Declared(root) => {
            assert!(
                root.join("manifest.json").is_file(),
                "FUARAN_PROGRAM_SPEC names '{}', which carries no wire-fixtures/manifest.json. \
                 A claimed corpus that cannot be read is a failure, never a skip.",
                root.display()
            );
            Some(root)
        }
        Corpus::Discovered(root) => Some(root),
        Corpus::Absent => {
            eprintln!(
                "driver-semantics: NOT RUN — no scenario corpus was claimed or found, so this leg \
                 asserted nothing. Set FUARAN_PROGRAM_SPEC to the program wire specification's \
                 directory, or check it out beside this repository, to run it."
            );
            None
        }
    }
}

#[test]
fn the_loop_reproduces_every_scenario_the_corpus_enumerates() {
    let Some(fixtures) = fixtures_root() else {
        return;
    };
    let entries = enumerated(&fixtures);
    assert!(
        !entries.is_empty(),
        "the corpus enumerates no {FAMILY} scenario at '{}' — a family that silently found nothing \
         would report green while asserting nothing",
        fixtures.display()
    );

    let mut ran = Vec::new();
    let mut divergences = Vec::new();
    for entry in &entries {
        let scenario = load(&fixtures, entry);
        ran.push(scenario.name.clone());
        if let Err(report) = check(&scenario) {
            divergences.push(report);
        }
    }

    // Say what ran, by name. A count alone cannot be checked against the corpus
    // by whoever is reading the output, and this leg's whole value is that its
    // enumeration is the manifest's rather than its own.
    eprintln!(
        "driver-semantics: {} scenario(s) from '{}': {}",
        ran.len(),
        fixtures.display(),
        ran.join(", ")
    );

    // The harness's first obligation: it ran what the manifest enumerates.
    assert_eq!(
        ran.len(),
        entries.len(),
        "every scenario the manifest enumerates was loaded and run"
    );
    assert!(
        divergences.is_empty(),
        "{} of {} scenarios diverged:\n\n{}",
        divergences.len(),
        entries.len(),
        divergences.join("\n\n")
    );
}

#[test]
fn a_mutated_trace_makes_this_harness_go_red() {
    // The harness's second obligation. Run against a REAL scenario rather than a
    // synthetic pair, so what is proved is that this comparison — over these
    // bytes, through this normalisation — can fail.
    let Some(fixtures) = fixtures_root() else {
        return;
    };
    let entries = enumerated(&fixtures);
    let scenario = load(&fixtures, &entries[0]);
    check(&scenario).expect("the unperturbed scenario passes, or the probe below proves nothing");

    // (a) the tree. A change the decoder ACCEPTS — renaming a node rather than
    //     naming a case that does not exist. A perturbation that failed to
    //     decode would be caught by the decoder, which is a different check
    //     passing under this one's name.
    let mut perturbed_tree = Scenario {
        name: scenario.name.clone(),
        tree_json: scenario.tree_json.clone(),
        events: scenario.events.clone(),
        recorded: scenario.recorded.clone(),
        host_policy: scenario.host_policy.clone(),
    };
    perturbed_tree.recorded = perturbed_tree
        .recorded
        .iter()
        .map(|step| StepObservation {
            tree_json: step
                .tree_json
                .replace("\"id\":\"root\"", "\"id\":\"rooted\""),
            ..step.clone()
        })
        .collect();
    assert_ne!(
        perturbed_tree.recorded, scenario.recorded,
        "the perturbation actually changed the recorded trace"
    );
    let report = check(&perturbed_tree).expect_err("a mutated tree must be caught");
    assert!(
        report.contains("on tree"),
        "and named as a tree divergence: {report}"
    );

    // (b) the effects, whose bytes are pinned rather than normalised. Perturb
    //     the LAST step, so the failure cannot be the tree's.
    let mut perturbed_effects = Scenario {
        name: scenario.name.clone(),
        tree_json: scenario.tree_json.clone(),
        events: scenario.events.clone(),
        recorded: scenario.recorded.clone(),
        host_policy: scenario.host_policy.clone(),
    };
    let last = perturbed_effects.recorded.len() - 1;
    perturbed_effects.recorded[last]
        .effects
        .push("{\"kind\":\"Navigate\",\"route\":\"/invented\"}".into());
    let report = check(&perturbed_effects).expect_err("a mutated effect list must be caught");
    assert!(
        report.contains("on effects"),
        "and named as an effects divergence: {report}"
    );
}

#[test]
fn a_host_that_performs_the_denied_effect_fails_exactly_the_scenario_that_records_it() {
    // The go-red proof for the denials member, measured rather than asserted —
    // and staged as a HOST rather than as a fixture, because the claim under
    // test is about a host that sends the effect where the scenario's policy
    // forbids. Replacing the scenario's policy with none is exactly that host:
    // `run_scenario` then runs permissive, which performs what the recorded
    // trace says was declined.
    let Some(fixtures) = fixtures_root() else {
        return;
    };
    let entries = enumerated(&fixtures);
    let Some(entry) = entries.iter().find(|e| e.host_policy.is_some()) else {
        panic!(
            "no scenario declares a hostPolicy, so this probe stages nothing and the denials \
             member is unexercised by this host"
        );
    };
    let scenario = load(&fixtures, entry);
    check(&scenario).expect("the unperturbed scenario passes, or the probe below proves nothing");

    let permissive_host = Scenario {
        name: scenario.name.clone(),
        tree_json: scenario.tree_json.clone(),
        events: scenario.events.clone(),
        recorded: scenario.recorded.clone(),
        host_policy: None,
    };
    let observed_declining = run_scenario(
        &scenario.tree_json,
        &scenario.events,
        scenario.host_policy.as_deref(),
    )
    .expect("the declining host runs");
    let observed_performing =
        run_scenario(&permissive_host.tree_json, &permissive_host.events, None)
            .expect("the performing host runs");

    // The finding, made executable: the two hosts fold IDENTICALLY in every
    // other member. That is why this failure was invisible before the denials
    // member existed, and it is what makes the divergence below meaningful
    // rather than incidental.
    let strip = |steps: &[StepObservation]| -> Vec<StepObservation> {
        steps
            .iter()
            .map(|s| StepObservation {
                denials: None,
                ..s.clone()
            })
            .collect()
    };
    assert!(
        first_divergence(
            &scenario.name,
            &strip(&observed_declining),
            &strip(&observed_performing)
        )
        .is_none(),
        "the declining and performing hosts must agree on tree, effects and refusal — that \
         agreement is the finding this member exists to answer"
    );

    // And with the seam in view, they must not — on `denials` and nothing else.
    let observed_performing_seam: Vec<StepObservation> = observed_performing
        .iter()
        .map(|s| StepObservation {
            denials: Some(Vec::new()),
            ..s.clone()
        })
        .collect();
    let divergence = first_divergence(
        &scenario.name,
        &observed_declining,
        &observed_performing_seam,
    )
    .expect("a host that performed the denied effect must be caught");
    assert_eq!(
        divergence.member,
        "denials",
        "and named as a denials divergence: {}",
        divergence.describe()
    );

    // The other half of the pair: a host that DECLINES correctly but fails to
    // RECORD it. The recorded trace loses its denial while every other member
    // stays exactly right — the shape a regression really takes.
    let mut silent = scenario.recorded.clone();
    for step in &mut silent {
        step.denials = Some(Vec::new());
    }
    assert_ne!(
        silent, scenario.recorded,
        "the perturbation changed the trace"
    );
    let expected = normalise_expectation(&scenario.name, &silent).expect("it normalises");
    let report = first_divergence(&scenario.name, &expected, &observed_declining)
        .expect("a host that declined in silence must be caught")
        .describe();
    assert!(
        report.contains("on denials"),
        "and named as a denials divergence: {report}"
    );
}
