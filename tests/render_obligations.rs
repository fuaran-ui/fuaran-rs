//! Executable render-obligation conformance (`WIRE_FORMAT.md` §13) — this
//! host's adoption. The sibling of the reference host's own render-obligation
//! suite, ported rather than transpiled: same vocabulary, same three outcomes,
//! same report lines, idiomatic Rust.
//!
//! Codec conformance is byte-parity and strong. Render obligations were prose:
//! §3.6.2–§3.6.6 and §25.4 state, in sentences, that an accessible name is
//! always emitted, that `autoplay` never appears without `muted`, that an audio
//! transport has no autoplay pathway at all, that a refused source emits no
//! affordance. A host can pass every fixture in the corpus and silently fail
//! every one of those — none is a missing discriminator arm, so neither the
//! conformance corpus nor this crate's exhaustive `match` reaches them. This
//! host has the scar to prove it: the one media defect the compiler could not
//! catch was a `matches!` bool site, which is exactly this class.
//!
//! So the manifest carries them now, and this suite asserts FROM the manifest
//! rather than from a hand list beside it. Three consequences, which are the
//! whole point:
//!
//!   * The ENUMERATION is the corpus artefact's. A newly declared obligation on
//!     a kind this host renders arrives here as a claim with no checker and
//!     turns the suite RED — not as a paragraph a future reader may re-read.
//!
//!   * NOT CHECKED IS NOT PASSED. Every claim this host does not assert is
//!     printed by name with the section that states it, and fails the gate
//!     unless it carries a declared exemption. Silence is never an answer.
//!
//!   * The go-red property is PROVEN, not asserted. `status_of` is exercised
//!     against a claim no checker covers and must report it unchecked — the
//!     shape a new obligation takes on the day it lands — and the artefact path
//!     is overridable (see `artefact_path`) so the whole gate can be driven red
//!     against a perturbed scratch copy without touching the shared corpus.
//!
//! **The tier these obligations are stated over: the server-HTML emission of
//! `render::server`, which is this host's ONLY emission surface.** That is a
//! fact about this crate rather than a choice about scope — the headless
//! backend role serves that string directly, and the `wasm32` browser-client
//! role reaches the same walk through `ClientSession::render`, which calls
//! `render_to_html_with_egress`. So one suite covers both of this host's roles
//! by construction, and a second suite over the client arm would be asserting
//! against the same bytes.
//!
//! Every checker asserts in EMITTED OUTPUT through that render path. A checker
//! that inspected the decoded tree would be re-stating this crate's own type
//! system — the obligations are claims about output, and the type system is
//! precisely what does not reach them.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::egress::permissive_egress;
use fuaran_rs::render::{BindingSources, render_to_html, render_to_html_with_egress};
use fuaran_rs::wire::decode_node;

// ─── The artefact ────────────────────────────────────────────────────────────

/// Resolve the render-fidelity manifest.
///
/// `FUARAN_RENDER_FIDELITY` names the artefact explicitly; otherwise the
/// corpus is located by walking up for a `wire-format-fixtures/` checkout, the
/// same way every other corpus-reading suite in this crate does.
///
/// **The override exists so the go-red property can be PROVEN.** Perturbing the
/// enumeration is the only way to demonstrate that a newly declared obligation
/// turns this gate red, and the shared corpus is not ours to perturb — it is a
/// separate repository seven hosts certify against. Pointing this variable at a
/// scratch copy carrying one injected claim drives the gate red without writing
/// a byte to the artefact everyone else reads.
///
/// A CLAIMED artefact that cannot be read FAILS rather than skipping, matching
/// this crate's driver-semantics posture: a conformance gate that goes green
/// without its oracle is worse than no gate at all.
fn artefact_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("FUARAN_RENDER_FIDELITY") {
        let claimed = PathBuf::from(&explicit);
        assert!(
            claimed.is_file(),
            "FUARAN_RENDER_FIDELITY names '{explicit}', which is not a readable file. \
             A claimed artefact that cannot be read fails rather than skipping."
        );
        return Some(claimed);
    }
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let artefact = dir
            .join("wire-format-fixtures")
            .join("render-fidelity.json");
        if artefact.is_file() {
            return Some(artefact);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// One entry of the closed claim vocabulary the artefact enumerates.
struct VocabularyEntry {
    id: String,
    meaning: String,
}

/// One checkable claim a kind owes, bound to the section that states it.
struct Obligation {
    id: String,
    statement: String,
    section: String,
}

/// One `kinds` row, reduced to what obligation coverage needs.
struct KindRow {
    kind: String,
    obligations: Vec<Obligation>,
}

/// One `traits` row (WIRE_FORMAT.md §13, Phase 1696): a node-level member whose
/// claims ride the ENVELOPE rather than any one kind, so every kind this host
/// renders owes them and none of them owns them.
///
/// `trait_id` is the wire path of the member it governs, which is why it shares
/// one registry with the kind names: a dotted path can never collide with a
/// `kind.$type`. `scope` is the tagged `appliesTo.scope` - `allKinds`, or
/// `namedKinds` with the list beside it - because "every kind" must not be
/// spellable as an empty array, which reads as the opposite claim.
struct TraitRow {
    trait_id: String,
    scope: String,
    scope_kinds: Vec<String>,
    obligations: Vec<Obligation>,
}

struct RenderFidelityManifest {
    obligation_vocabulary: Vec<VocabularyEntry>,
    traits: Vec<TraitRow>,
    kinds: Vec<KindRow>,
}

fn str_field(j: &JVal, key: &str) -> String {
    match j.field(key) {
        Some(JVal::Str(s)) => s.clone(),
        other => panic!("expected string field '{key}', got {other:?}"),
    }
}

fn parse_manifest(text: &str) -> RenderFidelityManifest {
    let root = parse(text).expect("the render-fidelity artefact is well-formed JSON");
    let vocabulary = match root.field("obligationVocabulary") {
        Some(JVal::Arr(items)) => items
            .iter()
            .map(|v| VocabularyEntry {
                id: str_field(v, "id"),
                meaning: str_field(v, "meaning"),
            })
            .collect(),
        // Absent is a legal shape for an artefact that predates §13's
        // obligation block; the gate's own non-zero guard is what refuses it,
        // with a message saying which of the two possible causes to look at.
        _ => Vec::new(),
    };
    // Absent is a legal shape for an artefact predating §13's trait block, on
    // exactly the terms the vocabulary above is: the gate's own non-zero guard
    // is what refuses an artefact that declares nothing at all.
    let traits = match root.field("traits") {
        Some(JVal::Arr(rows)) => rows
            .iter()
            .map(|row| {
                let applies_to = row.field("appliesTo");
                let scope = match applies_to.and_then(|a| a.field("scope")) {
                    Some(JVal::Str(v)) => v.clone(),
                    other => panic!("a trait row carries no appliesTo.scope: {other:?}"),
                };
                let scope_kinds = match applies_to.and_then(|a| a.field("kinds")) {
                    Some(JVal::Arr(ks)) => ks
                        .iter()
                        .map(|k| match k {
                            JVal::Str(v) => v.clone(),
                            other => panic!("appliesTo.kinds holds a non-string: {other:?}"),
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                TraitRow {
                    trait_id: str_field(row, "trait"),
                    scope,
                    scope_kinds,
                    obligations: match row.field("obligations") {
                        Some(JVal::Arr(items)) => items
                            .iter()
                            .map(|o| Obligation {
                                id: str_field(o, "id"),
                                statement: str_field(o, "statement"),
                                section: str_field(o, "section"),
                            })
                            .collect(),
                        _ => Vec::new(),
                    },
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    let kinds = match root.field("kinds") {
        Some(JVal::Arr(rows)) => rows
            .iter()
            .map(|row| KindRow {
                kind: str_field(row, "kind"),
                obligations: match row.field("obligations") {
                    Some(JVal::Arr(items)) => items
                        .iter()
                        .map(|o| Obligation {
                            id: str_field(o, "id"),
                            statement: str_field(o, "statement"),
                            section: str_field(o, "section"),
                        })
                        .collect(),
                    _ => Vec::new(),
                },
            })
            .collect(),
        other => panic!("the artefact carries no 'kinds' enumeration: {other:?}"),
    };
    RenderFidelityManifest {
        obligation_vocabulary: vocabulary,
        traits,
        kinds,
    }
}

/// Load the manifest, or report its absence and leave the caller to return.
///
/// A standalone checkout has no corpus beside it, so this reports rather than
/// certifying — but it REPORTS, on stderr, naming what it looked for. "Nothing
/// to certify" must never read as "everything certified".
fn load() -> Option<RenderFidelityManifest> {
    let Some(path) = artefact_path() else {
        eprintln!(
            "render-obligation conformance NOT RUN: no wire-format-fixtures/render-fidelity.json \
             found above {} (standalone checkout). Set FUARAN_RENDER_FIDELITY to name one.",
            env!("CARGO_MANIFEST_DIR")
        );
        return None;
    };
    let text = std::fs::read_to_string(&path).expect("reading the render-fidelity artefact");
    Some(parse_manifest(&text))
}

// ─── The reporting surface (WIRE_FORMAT.md §13) ──────────────────────────────
//
// The shape every adopting host uses, so the hosts answer the same question in
// the same words rather than each inventing a way to say "we did not check
// that". The Rust port of the reference host's coverage surface.

/// A host's answer for one declared obligation.
///
/// `Unchecked` is the case the whole mechanism exists for: a host that renders
/// a kind and has no checker for one of its claims must say so, WITH a reason —
/// not checked is not passed, and an obligation that quietly falls out of a
/// host's suite is exactly the silent failure the closed vocabulary replaces.
/// `NotRendered` is distinct: nothing is owed, rather than owed and unpaid.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ObligationOutcome {
    Asserted,
    Unchecked { reason: String },
    NotRendered { reason: String },
}

/// One line of a host's obligation report.
struct ObligationReport {
    kind: String,
    claim_id: String,
    statement: String,
    section: String,
    outcome: ObligationOutcome,
}

/// Every declared obligation, paired with the SUBJECT that owes it, in table
/// order: the kind rows first, then the trait rows.
///
/// Both arrays, deliberately. The whole mechanism is that the ENUMERATION is
/// the artefact's, so a trait declared tomorrow must reach this host's report
/// without this host changing anything but its answer - and a reader iterating
/// `kinds` alone would hold a green gate over an unowed claim.
fn all_obligations(manifest: &RenderFidelityManifest) -> Vec<(&str, &Obligation)> {
    manifest
        .kinds
        .iter()
        .flat_map(|row| row.obligations.iter().map(move |o| (row.kind.as_str(), o)))
        .chain(manifest.traits.iter().flat_map(|row| {
            row.obligations
                .iter()
                .map(move |o| (row.trait_id.as_str(), o))
        }))
        .collect()
}

/// Project the manifest through this host's own answer, one line per declared
/// obligation.
///
/// The ENUMERATION is the manifest's, never the host's — so a newly declared
/// obligation appears in the report the moment it lands rather than when
/// someone remembers it.
fn report_obligations(
    manifest: &RenderFidelityManifest,
    status_of: impl Fn(&str, &str) -> ObligationOutcome,
) -> Vec<ObligationReport> {
    all_obligations(manifest)
        .into_iter()
        .map(|(kind, o)| ObligationReport {
            kind: kind.to_string(),
            claim_id: o.id.clone(),
            statement: o.statement.clone(),
            section: o.section.clone(),
            outcome: status_of(kind, &o.id),
        })
        .collect()
}

/// The report lines a host must SURFACE: everything it did not assert.
///
/// Empty is the only silent result — anything else is printed, so an unchecked
/// obligation is visible in the run rather than inferable from its absence.
fn unasserted_obligations(report: &[ObligationReport]) -> Vec<&ObligationReport> {
    report
        .iter()
        .filter(|line| line.outcome != ObligationOutcome::Asserted)
        .collect()
}

/// The one-line rendering of a report line, so the same sentence appears in
/// every host's output.
fn describe_obligation_report(line: &ObligationReport) -> String {
    let outcome = match &line.outcome {
        ObligationOutcome::Asserted => "asserted".to_string(),
        ObligationOutcome::Unchecked { reason } => format!("UNCHECKED ({reason})"),
        ObligationOutcome::NotRendered { reason } => format!("not rendered ({reason})"),
    };
    format!(
        "{}/{} [{}]: {outcome}",
        line.kind, line.claim_id, line.section
    )
}

// ─── Render helpers ──────────────────────────────────────────────────────────

fn node(json: &str) -> fuaran_rs::wire::Node {
    decode_node(json).expect("the obligation fixture decodes")
}

/// The ambient posture a decoded tree gets: deny-non-local egress. This is the
/// policy the two "refused" obligations are stated under.
fn render(json: &str) -> String {
    render_to_html(&node(json), &BindingSources::default())
}

/// The destination policy widened BY NAME, for the allow twins.
fn render_permissive(json: &str) -> String {
    render_to_html_with_egress(
        &permissive_egress(),
        &node(json),
        &BindingSources::default(),
    )
}

/// The open tag of the first `<tag …>` in an emission.
fn open_tag<'a>(html: &'a str, tag: &str) -> &'a str {
    let from = &html[html.find(&format!("<{tag}")).expect("the element")..];
    &from[..from.find('>').expect("an open tag") + 1]
}

/// A destination that is safe by the scheme floor and entirely undeclared, so
/// the ambient egress policy refuses it. This is the input the three "refused"
/// obligations are about.
const REFUSED: &str = "https://collector.example/asset.jpg";
/// The marked refusal a refused destination renders as.
const REFUSAL_URL: &str = "about:blank#fuaran-egress-refused";

// ─── The checkers ────────────────────────────────────────────────────────────
//
// One per (kind, claim), and each is its own `#[test]` so a failing obligation
// names the claim it broke rather than surfacing as one opaque red test. The
// registry below holds a pointer to each, which is what stops a registry entry
// from naming a claim no test implements.
//
// Each pins BOTH directions where the obligation has two: an emission test
// alone cannot tell a renderer that honours a conditional from one that emits
// unconditionally, and a renderer that dropped EVERY poster would satisfy a
// refusal-only assertion while being a worse bug than the one it guards.

/// §3.6.6 — an `aria-label` carrying the resolved label is ALWAYS emitted.
#[test]
fn owes_media_accessible_name_always() {
    // BOTH variants, because the label is mandatory for the KIND and not for
    // one arm of it. A renderer emitting it only on `<video>` passes a
    // video-only test.
    let video = render(
        r#"{"id":"mv","kind":{"$type":"Media","kind":{"$type":"Video"},"label":"Studio walkthrough","src":{"$type":"Static","value":"/walkthrough.mp4"}}}"#,
    );
    let audio = render(
        r#"{"id":"ma","kind":{"$type":"Media","kind":{"$type":"Audio"},"label":"Curator commentary","src":{"$type":"Static","value":"/commentary.mp3"}}}"#,
    );

    assert!(
        video.contains(r#"aria-label="Studio walkthrough""#),
        "a video emits the resolved label as aria-label: {video}"
    );
    assert!(
        audio.contains(r#"aria-label="Curator commentary""#),
        "an audio emits the resolved label as aria-label: {audio}"
    );
}

/// §3.6.6 — `autoplay` is emitted ONLY together with `muted`, and `muted` rides
/// `autoplay`.
#[test]
fn owes_media_autoplay_muted_pairing() {
    let autoplaying = render(
        r#"{"id":"mva","kind":{"$type":"Media","kind":{"$type":"Video","autoplay":true},"label":"Ambient loop","src":{"$type":"Static","value":"/ambient.mp4"}}}"#,
    );
    let tag = open_tag(&autoplaying, "video");
    assert!(
        tag.contains(" autoplay"),
        "a declared autoplay is emitted: {tag}"
    );
    assert!(
        tag.contains(" muted"),
        "and never without muted — an unmuted autoplay is blocked and means nothing: {tag}"
    );

    // The pairing runs one way, and this is the half a one-sided assertion
    // misses: `muted` unasked silences a video the reader started themselves.
    let plain = render(
        r#"{"id":"mv","kind":{"$type":"Media","kind":{"$type":"Video"},"label":"Studio walkthrough","src":{"$type":"Static","value":"/walkthrough.mp4"}}}"#,
    );
    let tag = open_tag(&plain, "video");
    assert!(
        !tag.contains(" autoplay"),
        "autoplay is not declared, so it must not be emitted: {tag}"
    );
    assert!(
        !tag.contains(" muted"),
        "muted rides autoplay; unasked it is a behaviour change: {tag}"
    );
}

/// §3.6.6 — the Audio variant has NO autoplay pathway at all.
#[test]
fn owes_media_no_autoplay_pathway() {
    // Stated ON THE WIRE, which is the sharper pin: the case declares no such
    // slot, so a document asking for it has nowhere to land the request. A
    // renderer that merely defaults it off would pass a plain-audio assertion.
    let audio = render(
        r#"{"id":"ma","kind":{"$type":"Media","kind":{"$type":"Audio","autoplay":true},"label":"Curator commentary","src":{"$type":"Static","value":"/commentary.mp3"}}}"#,
    );

    assert!(
        !audio.contains("autoplay"),
        "an <audio> must never carry an autoplay attribute: {audio}"
    );
    assert!(
        !audio.contains("muted"),
        "an <audio> has no autoplay, so it has nothing to mute: {audio}"
    );
}

/// §3.6.6 — a `poster` the URL-scheme + egress floor refuses is DROPPED rather
/// than emitted at the refusal URL.
#[test]
fn owes_media_refused_source_dropped() {
    let refused = format!(
        r#"{{"id":"mvp","kind":{{"$type":"Media","kind":{{"$type":"Video","poster":{{"$type":"Static","value":"{REFUSED}"}}}},"label":"Studio walkthrough","src":{{"$type":"Static","value":"/walkthrough.mp4"}}}}}}"#
    );
    let refused = render(&refused);

    assert!(
        !refused.contains("collector.example"),
        "a refused poster's destination is never emitted: {refused}"
    );
    assert!(
        !refused.contains("poster="),
        "a refused poster is DROPPED, not emitted at the refusal URL — a poster at the refusal \
         URL is a broken image over the player, where no poster shows the first frame: {refused}"
    );

    // The allow twin. Without it a renderer that dropped EVERY poster would
    // pass the refusal assertion and this obligation would guard nothing.
    let allowed = render(
        r#"{"id":"mvp2","kind":{"$type":"Media","kind":{"$type":"Video","poster":{"$type":"Static","value":"/walkthrough-poster.jpg"}},"label":"Studio walkthrough","src":{"$type":"Static","value":"/walkthrough.mp4"}}}"#,
    );
    assert!(
        allowed.contains(r#"poster="/walkthrough-poster.jpg""#),
        "a local poster still renders: {allowed}"
    );
}

/// §3.6.2 — `alt` is emitted on every image, the empty string included.
#[test]
fn owes_image_alt_always_emitted() {
    let named = render(
        r#"{"id":"img","kind":{"$type":"Image","alt":"Fishing boats moored at first light","src":{"$type":"Static","value":"/harbour.jpg"},"variant":"Default"}}"#,
    );
    assert!(
        named.contains(r#"alt="Fishing boats moored at first light""#),
        "the alt text is emitted: {named}"
    );

    // The decorative case is the one that matters. An omitted `alt` and an
    // empty one are different claims to assistive technology: omitted means
    // "nobody said", empty means "this is decorative, skip it".
    let decorative = render(
        r#"{"id":"imgd","kind":{"$type":"Image","alt":"","src":{"$type":"Static","value":"/rule.png"},"variant":"Default"}}"#,
    );
    assert!(
        decorative.contains(r#"alt="""#),
        "a decorative image emits an EMPTY alt, never no alt at all: {decorative}"
    );
}

/// §3.6.5 — a declared expansion emits a real working anchor to the full-size
/// asset, honoured with no script at all.
#[test]
fn owes_image_anchor_affordance_on_expandable() {
    let html = render(
        r#"{"id":"imge","kind":{"$type":"Image","alt":"Harbour","expandable":true,"src":{"$type":"Static","value":"/harbour.jpg"},"variant":"Default"}}"#,
    );

    // The ELEMENT is pinned, not only the class: the whole no-JS claim is that
    // this is an `<a href>`, and a `<span class="fuaran-image-expand">` carrying
    // the data attribute would pass a class-only assertion while giving a
    // scriptless reader nothing.
    let anchor = open_tag(&html, "a");
    assert!(
        anchor.contains(r#"class="fuaran-image-expand""#),
        "{anchor}"
    );
    assert!(
        anchor.contains(r#"href="/harbour.jpg""#),
        "a WORKING link to the asset the image already names: {anchor}"
    );
    assert!(anchor.contains("data-fuaran-expandable"), "{anchor}");
    assert!(
        html.find("<a ").expect("the anchor") < html.find("<img").expect("the image"),
        "the anchor WRAPS the image: {html}"
    );
    assert!(
        !html.contains("onclick"),
        "honoured with no script at all: {html}"
    );

    // The other direction: an undeclared expansion emits no anchor, so the
    // assertion above is about the declaration and not about this host always
    // wrapping images.
    let not_expandable = render(
        r#"{"id":"imgp","kind":{"$type":"Image","alt":"Harbour","src":{"$type":"Static","value":"/harbour.jpg"},"variant":"Default"}}"#,
    );
    assert!(
        !not_expandable.contains("fuaran-image-expand"),
        "an undeclared expansion emits no anchor: {not_expandable}"
    );
}

/// §3.6.5 — a source the egress floor refused emits no affordance.
#[test]
fn owes_image_refused_src_no_affordance() {
    let html = render(&format!(
        r#"{{"id":"imgr","kind":{{"$type":"Image","alt":"Harbour","expandable":true,"src":{{"$type":"Static","value":"{REFUSED}"}},"variant":"Default"}}}}"#
    ));

    assert!(
        !html.contains("fuaran-image-expand"),
        "a src the egress floor refused emits NO expand anchor — an affordance that cannot be \
         honoured is worse than none: {html}"
    );

    // The image itself still renders, at the refusal URL. Without this leg a
    // renderer that dropped the whole node would pass the assertion above, and
    // this obligation would be satisfied by a worse bug than the one it guards.
    assert!(
        html.contains(REFUSAL_URL),
        "the img is still emitted, with the marked refusal URL as its src: {html}"
    );
    assert!(
        !html.contains(r#"href="https://collector.example"#),
        "and the refused destination never becomes a navigable href: {html}"
    );
}

/// §3.6.3 + §3.6.5 — the caption sits outside the expansion anchor.
#[test]
fn owes_image_figure_caption_outside_link() {
    let html = render(
        r#"{"id":"imgef","kind":{"$type":"Image","alt":"Harbour","caption":"The harbour at dawn","expandable":true,"src":{"$type":"Static","value":"/harbour.jpg"},"variant":"Default"}}"#,
    );

    // Asserting the two opening tags IN ORDER is what catches the inversion
    // (anchor outside figure), which would carry every one of the same classes.
    let figure = html.find("<figure").expect("the figure");
    let anchor = html.find("<a ").expect("the anchor");
    let img = html.find("<img").expect("the image");
    let anchor_end = html.find("</a>").expect("the anchor close");
    let caption = html.find("<figcaption").expect("the caption");

    assert!(
        figure < anchor && anchor < img,
        "the figure wraps the anchor, not the other way round: {html}"
    );
    assert!(
        anchor_end < caption,
        "the figcaption is the anchor's SIBLING — the caption is prose a reader quotes, not a \
         second click surface: {html}"
    );
    assert!(
        html.contains(
            r#"<figcaption class="fuaran-image-figure-caption">The harbour at dawn</figcaption>"#
        ),
        "{html}"
    );
}

/// §3.6.4 — responsive candidates are emitted in ascending width order, and a
/// refused candidate is dropped rather than emitted.
#[test]
fn owes_image_srcset_ascending_by_width() {
    // Authored DESCENDING, so the assertion pins the renderer's SORT and not
    // merely its spelling: a renderer emitting authored order would produce a
    // srcset containing all the same URLs and fail here.
    let html = render(
        r#"{"id":"imgs","kind":{"$type":"Image","alt":"Harbour","src":{"$type":"Static","value":"/harbour.jpg"},"srcSet":[{"src":{"$type":"Static","value":"/harbour-1600.jpg"},"width":1600},{"src":{"$type":"Static","value":"/harbour-800.jpg"},"width":800},{"src":{"$type":"Static","value":"/harbour-400.jpg"},"width":400}],"variant":"Default"}}"#,
    );
    let tag = open_tag(&html, "img");
    assert!(
        tag.contains(
            r#"srcset="/harbour-400.jpg 400w, /harbour-800.jpg 800w, /harbour-1600.jpg 1600w""#
        ),
        "candidates are emitted ascending by width: {tag}"
    );

    // The second half of the same obligation: a refused candidate is DROPPED,
    // so the primary src remains the fallback rather than the list carrying a
    // destination the floor refused.
    let with_refused = render(&format!(
        r#"{{"id":"imgs2","kind":{{"$type":"Image","alt":"Harbour","src":{{"$type":"Static","value":"/harbour.jpg"}},"srcSet":[{{"src":{{"$type":"Static","value":"/harbour-400.jpg"}},"width":400}},{{"src":{{"$type":"Static","value":"{REFUSED}"}},"width":1600}}],"variant":"Default"}}}}"#
    ));
    let tag = open_tag(&with_refused, "img");
    assert!(
        !tag.contains("collector.example"),
        "a refused candidate's destination is never emitted: {tag}"
    );
    assert!(
        !tag.contains("about:blank"),
        "…nor emitted in neutered form at the refusal URL: {tag}"
    );
    assert!(
        tag.contains("/harbour-400.jpg 400w"),
        "…while the candidates that pass the floor still are: {tag}"
    );

    // A permitted remote candidate is served, so the refusal assertion above is
    // about the FLOOR and not about this host dropping every remote candidate.
    let permitted = render_permissive(
        r#"{"id":"imgs3","kind":{"$type":"Image","alt":"Harbour","src":{"$type":"Static","value":"/harbour.jpg"},"srcSet":[{"src":{"$type":"Static","value":"https://cdn.example/harbour-1600.jpg"},"width":1600}],"variant":"Default"}}"#,
    );
    assert!(
        open_tag(&permitted, "img").contains("https://cdn.example/harbour-1600.jpg 1600w"),
        "{permitted}"
    );
}

/// §25.4 — an unregistered custom node renders a labelled placeholder, never a
/// blank and never a guess.
///
/// **This host asserts the UNCARDED path only, and that is the conformant
/// answer here rather than a partial one.** The claim is conditional on a
/// contract card being AVAILABLE for the node's identity; this host ships no
/// card reader at all, so no card is ever available for any identity and the
/// carded branches — described / hash-mismatch / malformed-prop-bag — are
/// unreachable by construction. What the obligation requires of a host in that
/// position is exactly what the identity-only placeholder does: name the
/// component, emit no prop VALUE, and invent no description it does not have.
///
/// **This host does NOT thereby claim §25 adoption.** That is a separate bar
/// with its own §11.0 table, and reading a card is what it asks for. Building a
/// card reader is a phase, not a line in a test file.
#[test]
fn owes_custom_unregistered_custom_labelled() {
    let html = render(
        r#"{"id":"cust","kind":{"$type":"Custom","componentId":"sparkline","moduleId":"analytics","props":{"series":{"points":[1,2,3]}}}}"#,
    );

    assert!(
        html.contains("Custom analytics.sparkline"),
        "the identity-only placeholder names the component: {html}"
    );
    // Never a prop VALUE: this host was not asked to interpret the node's
    // props, and a placeholder that leaked one would be rendering data it
    // cannot claim to understand.
    assert!(
        !html.contains("points"),
        "no prop value reaches the placeholder: {html}"
    );
    assert!(
        html.contains("props: series"),
        "the declared prop NAMES are what a reader is owed: {html}"
    );
    // A host with no card claims nothing about a card, and invents no
    // description. A blank would be the other failure; this is neither.
    assert!(
        !html.contains("data-fuaran-custom-card"),
        "a host with no card reader claims no card verdict: {html}"
    );
    assert!(!html.trim().is_empty(), "never a blank: {html}");
}

// ─── The registry ────────────────────────────────────────────────────────────

// ─── Phase 1110 — Media text tracks and the transcript (§3.6.6) ──────────────

/// §3.6.6 obligation 2 — `<track>` children are emitted in the AUTHORED order,
/// never re-sorted.
///
/// The fixture is authored in an order NO sort produces (`gd`, then two `en`),
/// which is what makes this separately testable from §3.6.4's `srcSet` rule: a
/// renderer that sorted by `srclang` would emit `en, en, gd`, and one that
/// sorted by `label` would emit them differently again. Both pass an
/// emission-only check and fail here.
#[test]
fn owes_media_authored_child_order() {
    let html = render(
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Video"},"label":"Harbour restoration","src":{"$type":"Static","value":"/restoration-2.mp4"},"tracks":[{"kind":"Subtitles","label":"Gaidhlig","src":{"$type":"Static","value":"/a.vtt"},"srcLang":"gd"},{"kind":"Captions","label":"English captions","src":{"$type":"Static","value":"/b.vtt"},"srcLang":"en"},{"kind":"Captions","label":"English captions (verbose)","src":{"$type":"Static","value":"/c.vtt"},"srcLang":"en"}]}}"#,
    );
    let at = |needle: &str| {
        html.find(needle)
            .unwrap_or_else(|| panic!("missing {needle} in {html}"))
    };
    let (a, b, c) = (
        at(r#"src="/a.vtt""#),
        at(r#"src="/b.vtt""#),
        at(r#"src="/c.vtt""#),
    );
    assert!(
        a < b && b < c,
        "tracks are emitted in the AUTHORED order, never re-sorted: {html}"
    );
}

/// §3.6.6 obligation 3 — at most one `<track>` of a given kind carries
/// `default`, and the FIRST election of a kind wins.
///
/// Both directions, and the second is the one that matters: the losing track is
/// STILL EMITTED — only its claim on the menu is dropped — and the election is
/// PER KIND, so a captions default and a subtitles default coexist. A renderer
/// that kept one default across the whole list would pass a naive count and
/// fail the coexistence leg.
#[test]
fn owes_media_single_default_per_kind() {
    let html = render(
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Video"},"label":"Harbour restoration","src":{"$type":"Static","value":"/restoration-2.mp4"},"tracks":[{"default":true,"kind":"Captions","label":"English captions","src":{"$type":"Static","value":"/first.vtt"},"srcLang":"en"},{"default":true,"kind":"Captions","label":"English captions (verbose)","src":{"$type":"Static","value":"/second.vtt"},"srcLang":"en"},{"default":true,"kind":"Subtitles","label":"Gaidhlig","src":{"$type":"Static","value":"/third.vtt"},"srcLang":"gd"}]}}"#,
    );
    let track = |src: &str| {
        let start = html
            .find(&format!(r#"src="{src}""#))
            .unwrap_or_else(|| panic!("missing {src} in {html}"));
        let open = html[..start].rfind("<track").expect("track element");
        let close = html[open..].find('>').expect("track close") + open;
        html[open..=close].to_string()
    };
    assert!(
        track("/first.vtt").contains("default"),
        "the FIRST election of a kind is honoured: {html}"
    );
    assert!(
        !track("/second.vtt").contains("default"),
        "a later election of the SAME kind is emitted WITHOUT the attribute: {html}"
    );
    // The track is still emitted — only its claim on the menu is dropped.
    assert!(
        html.contains(r#"src="/second.vtt""#),
        "the losing track is still emitted: {html}"
    );
    // Per KIND, not per element: a subtitles default coexists with a captions one.
    assert!(
        track("/third.vtt").contains("default"),
        "the election is PER KIND — a subtitles default coexists with a captions \
         default: {html}"
    );
}

/// §3.6.6 — a declared `transcript` renders as a `<details>` disclosure BESIDE
/// the transport, carrying the MEDIA's resolved label as its accessible name.
///
/// Both directions. `<video>` and `<audio>` admit only source-ish children, so a
/// transcript placed INSIDE would be fallback content a browser never shows —
/// hence the position assertion, not merely a presence one. And absent, the
/// emission is the bare element: the wrapper appears ONLY here, so a renderer
/// that always wrapped would change the markup of every media node.
#[test]
fn owes_media_transcript_disclosure_named() {
    let html = render(
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Audio"},"label":"Curator's commentary","src":{"$type":"Static","value":"/commentary.mp3"},"transcript":"The harbour was rebuilt twice."}}"#,
    );
    let audio = html
        .find("<audio")
        .unwrap_or_else(|| panic!("no <audio>: {html}"));
    let details = html
        .find("<details")
        .unwrap_or_else(|| panic!("no transcript disclosure: {html}"));
    assert!(
        audio < details,
        "the transcript renders BESIDE the transport and AFTER it, never inside: {html}"
    );
    assert!(
        html.contains(r#"<div class="fuaran-media-group""#),
        "a present transcript gains the group wrapper: {html}"
    );
    assert!(
        html.contains(r#"class="fuaran-media-transcript" aria-label="Curator&#x27;s commentary""#),
        "the disclosure carries the MEDIA's resolved label as its own accessible \
         name, so a reader meeting it out of context is told which recording it \
         transcribes: {html}"
    );

    // The absent twin. Without it a renderer that ALWAYS emitted the wrapper
    // would pass every assertion above and change every media node's markup.
    let bare = render(
        r#"{"id":"m2","kind":{"$type":"Media","kind":{"$type":"Audio"},"label":"Curator's commentary","src":{"$type":"Static","value":"/commentary.mp3"}}}"#,
    );
    assert!(
        !bare.contains("fuaran-media-group") && !bare.contains("<details"),
        "absent, the emission is the bare element it would otherwise be: {bare}"
    );
}

// ─── Phase 1111 — the sandboxed third-party embed (§3.6.8) ───────────────────

/// §3.6.8 — a `title` carrying the resolved title is ALWAYS emitted.
///
/// Including on a REFUSED embed, which is the leg worth having: a frame with no
/// accessible name is announced as "frame" and nothing else, and a renderer that
/// built the attribute list only on the success path would strip the name from
/// exactly the frames a reader most needs described.
#[test]
fn owes_embed_accessible_name_always() {
    let allowed = render_permissive(
        r#"{"id":"e","kind":{"$type":"Embed","src":{"$type":"Static","value":"https://player.example/embed/harbour"},"title":"Harbour restoration, part two"}}"#,
    );
    assert!(
        allowed.contains(r#"title="Harbour restoration, part two""#),
        "the resolved title is emitted: {allowed}"
    );
    // Refused by the default deny-non-local policy — the name survives.
    let refused = render(
        r#"{"id":"e","kind":{"$type":"Embed","src":{"$type":"Static","value":"https://player.example/embed/harbour"},"title":"Harbour restoration, part two"}}"#,
    );
    assert!(
        refused.contains(r#"title="Harbour restoration, part two""#),
        "the title is emitted on a REFUSED embed too — a frame with no accessible \
         name is announced as \"frame\" and nothing else: {refused}"
    );
}

/// §3.6.8 obligations 1, 2 and 3 — `sandbox` on EVERY embed, EMPTY when nothing
/// is granted; tokens in DECLARATION order and de-duplicated; `AllowFullscreen`
/// is NOT a sandbox token; `loading` and `referrerpolicy` unconditional.
///
/// The permissionless leg is the one a naive renderer fails: omitting the
/// attribute when the list is empty produces the same markup as an UNSANDBOXED
/// frame, which is the opposite of what the empty list declares.
#[test]
fn owes_embed_sandbox_always_exactly_declared() {
    let bare = render_permissive(
        r#"{"id":"e","kind":{"$type":"Embed","src":{"$type":"Static","value":"https://player.example/embed/harbour"},"title":"Harbour"}}"#,
    );
    assert!(
        bare.contains(r#"sandbox="""#),
        "the sandbox declaration is emitted on EVERY embed and is EMPTY when \
         nothing is granted — omitting it would produce the same markup as an \
         UNSANDBOXED frame: {bare}"
    );
    assert!(
        bare.contains(r#"loading="lazy""#)
            && bare.contains(r#"referrerpolicy="strict-origin-when-cross-origin""#),
        "both are unconditional, with no wire slot for either: {bare}"
    );
    assert!(
        !bare.contains("allow="),
        "an empty `allow` is not the same statement as an absent one: {bare}"
    );

    // Declaration order, de-duplication, and the token that is NOT one.
    let granted = render_permissive(
        r#"{"id":"e","kind":{"$type":"Embed","permissions":["AllowSameOrigin","AllowScripts","AllowScripts","AllowFullscreen"],"src":{"$type":"Static","value":"https://player.example/embed/harbour"},"title":"Harbour"}}"#,
    );
    assert!(
        granted.contains(r#"sandbox="allow-scripts allow-same-origin""#),
        "tokens are emitted in the VOCABULARY's declaration order and \
         de-duplicated, whatever order the document authored: {granted}"
    );
    assert!(
        !granted.contains("allow-forms"),
        "an undeclared relaxation is never emitted: {granted}"
    );
    assert!(
        granted.contains(r#"allow="fullscreen""#),
        "`AllowFullscreen` is a permissions-policy directive, NOT a sandbox \
         token — a host that mapped the whole enum onto sandbox tokens passes \
         every other fixture and fails here: {granted}"
    );
    assert!(
        !granted.contains("allow-fullscreen"),
        "`AllowFullscreen` must not also appear as a sandbox token: {granted}"
    );
}

/// §19.1 rule 4 — a `src` the `embed` egress class refuses OMITS the attribute
/// entirely.
///
/// This is the one place a refusal does not take §19 rule 6's
/// substitute-`about:blank` route, and the assertions pin both halves of why: an
/// `<iframe>` pointed at the refusal URL would RENDER that page, so neither the
/// destination NOR the refusal URL may appear — while the refusal is still
/// RECORDED, so "nothing was declared" and "this was refused" stay different
/// facts.
///
/// The `http` leg pins the stricter scheme floor: §19's ordinary accept set
/// admits it and this class does not.
#[test]
fn owes_embed_refused_embed_source_omitted() {
    let refused = render(
        r#"{"id":"e","kind":{"$type":"Embed","src":{"$type":"Static","value":"https://collector.example/frame"},"title":"Harbour"}}"#,
    );
    // The refusal MARKER names the host, by design — that is the record, and it
    // never carries the URL, because the query string of a refused exfiltration
    // attempt is the payload itself. What must not appear is the DESTINATION.
    assert!(
        !refused.contains("https://collector.example") && !refused.contains("/frame"),
        "a refused destination is never emitted: {refused}"
    );
    assert!(
        refused.contains(r#"data-fuaran-egress-refused="embed:collector.example""#),
        "the refusal is recorded under the embed's OWN class, never `media:` — a          composition that declared an origin for image egress has said nothing          about which DOCUMENTS it is willing to run: {refused}"
    );
    assert!(
        !refused.contains("src="),
        "the source attribute is OMITTED entirely — an <iframe> at the refusal \
         URL RENDERS that page, where one with no source is a well-defined empty \
         browsing context that fetches nothing: {refused}"
    );
    assert!(
        !refused.contains(REFUSAL_URL),
        "and NOT substituted with the refusal URL: {refused}"
    );
    assert!(
        refused.contains("data-fuaran-egress-refused"),
        "the refusal is still recorded, so \"nothing was declared\" and \"this was \
         refused\" stay different facts: {refused}"
    );

    // The stricter floor: `http` is refused where §19's ordinary accept set
    // admits it, because an intermediary that can rewrite the channel is an
    // intermediary's script running in a frame this page created.
    let insecure = render_permissive(
        r#"{"id":"e","kind":{"$type":"Embed","src":{"$type":"Static","value":"http://player.example/embed/harbour"},"title":"Harbour"}}"#,
    );
    assert!(
        !insecure.contains("src="),
        "the embed class accepts `https` and nothing else: {insecure}"
    );
    // A schemeless reference names a same-origin document, which is exactly the
    // shape a guest granted AllowSameOrigin + AllowScripts can reach out of.
    let relative = render_permissive(
        r#"{"id":"e","kind":{"$type":"Embed","src":{"$type":"Static","value":"/local/frame.html"},"title":"Harbour"}}"#,
    );
    assert!(
        !relative.contains("src="),
        "a schemeless reference is refused by this class: {relative}"
    );

    // The allow twin. Without it a renderer that emitted NO embed source ever
    // would pass every assertion above and this obligation would guard nothing.
    let allowed = render_permissive(
        r#"{"id":"e","kind":{"$type":"Embed","src":{"$type":"Static","value":"https://player.example/embed/harbour"},"title":"Harbour"}}"#,
    );
    assert!(
        allowed.contains(r#"src="https://player.example/embed/harbour""#),
        "a permitted https embed still renders its source: {allowed}"
    );
}

// ─── Phase 1120 — Tree (§3.6.12) ─────────────────────────────────────────────

/// §3.6.12 obligation 5 — every row carries a STATED `aria-label` equal to its
/// visible label.
///
/// A `treeitem` OWNS its child group, so a name computed from contents reads the
/// whole branch out as the row's own name: a parent row whose accessible name
/// came from its subtree would announce "Goods Cocoa Yarn". Both the parent and
/// a leaf are asserted, because a renderer that stated the name only where it
/// had no children would leave exactly the rows that need it computing theirs.
#[test]
fn owes_tree_accessible_name_always() {
    let html = render(
        r#"{"id":"t","kind":{"$type":"Tree","items":[{"children":[{"id":"cocoa","label":"Cocoa"}],"id":"goods","label":"Goods"},{"id":"ledger","label":"Ledger"}]}}"#,
    );
    for (id, label) in [("goods", "Goods"), ("cocoa", "Cocoa"), ("ledger", "Ledger")] {
        assert!(
            html.contains(&format!(r#"aria-label="{label}""#)),
            "row '{id}' states its own visible label as its accessible name: {html}"
        );
    }
    // The name is STATED, not computed: the parent's own label, not its branch.
    assert!(
        !html.contains(r#"aria-label="Goods Cocoa""#),
        "a name computed from contents would read the whole branch out: {html}"
    );
}

// ─── Phase 1115 — FileUpload ingress routes (§3.6.10) ────────────────────────

/// §3.6.10 obligations 1 and 5 — the `<input type="file">` and its label are
/// emitted WHATEVER gestures the document declares.
///
/// All four flag combinations, because a declared route is ADDITIONAL and never
/// a replacement: a host that swapped the picker for a drop zone would ship a
/// pointer-only control, and there is no keyboard equivalent of a drag. The
/// declaration twin is asserted too, so a renderer that ignored both members
/// outright does not pass by emitting the picker and nothing else.
#[test]
fn owes_file_upload_picker_always_present() {
    for (drop, paste) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut members = String::new();
        if paste {
            members.push_str(r#""acceptPaste":true,"#);
        }
        if drop {
            members.push_str(r#""dropTarget":true,"#);
        }
        let html = render(&format!(
            r#"{{"id":"u","kind":{{"$type":"FileUpload","accept":[".csv"],{members}"label":"Drop a spreadsheet","multiple":false,"onSelect":"<closure>"}}}}"#
        ));
        assert!(
            html.contains(r#"type="file""#),
            "the picker is emitted whatever the document declares \
             (drop={drop}, paste={paste}): {html}"
        );
        assert!(
            html.contains(r#"class="fuaran-file-upload-label""#)
                && html.contains("Drop a spreadsheet"),
            "and so is its label (drop={drop}, paste={paste}): {html}"
        );
        assert_eq!(
            html.contains("data-fuaran-upload-drop"),
            drop,
            "the drop route is declared exactly when the document declares it: {html}"
        );
        assert_eq!(
            html.contains("data-fuaran-upload-paste"),
            paste,
            "the paste route is declared exactly when the document declares it: {html}"
        );
    }
}

// ─── Phase 1548 — FileUpload declared ceilings (§3.6.23) ─────────────────────

/// §3.6.23 obligation 4 — each declared ceiling is recorded as READ, and never
/// by VALUE.
///
/// TWO claims in one, and the second is what a marker-emission test alone would
/// miss: the marker records only THAT a ceiling was declared, so a renderer that
/// put the NUMBER in the markup would pass an emission assertion while telling a
/// reader this tier enforces a bound it cannot enforce at all. Nothing on this
/// path can act on the number — HTML has no attribute for a byte ceiling, and
/// `multiple` is a boolean rather than a count. Both directions are asserted, so
/// a renderer emitting either marker unconditionally does not pass.
#[test]
fn owes_file_upload_ceiling_recorded_never_enforced() {
    for (bytes, files) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut members = String::new();
        if bytes {
            members.push_str(r#""maxBytes":5242880,"#);
        }
        if files {
            members.push_str(r#""maxFiles":3,"#);
        }
        let html = render(&format!(
            r#"{{"id":"u","kind":{{"$type":"FileUpload","accept":["application/pdf"],"label":"Attach a scan",{members}"multiple":true,"onSelect":"<closure>"}}}}"#
        ));
        assert_eq!(
            html.contains("data-fuaran-upload-max-bytes"),
            bytes,
            "the byte ceiling is recorded exactly when the document declares it \n             (bytes={bytes}, files={files}): {html}"
        );
        assert_eq!(
            html.contains("data-fuaran-upload-max-files"),
            files,
            "the count ceiling is recorded exactly when the document declares it \n             (bytes={bytes}, files={files}): {html}"
        );
        assert!(
            !html.contains("5242880"),
            "the VALUE is never emitted — carrying it would claim an enforcement \n             that is not there: {html}"
        );
        assert!(
            html.contains(r#"type="file""#),
            "and a declared ceiling changes nothing about the control itself: {html}"
        );
    }
}

// ─── §3.6.11 — Modal / Popover modality ──────────────────────────────────────

/// §3.6.11 — the `aria-modal` inertness claim is emitted for the BLOCKING
/// modality alone.
///
/// A popover does not make the rest of the page inert, so claiming it would tell
/// assistive technology to ignore content the reader can still reach. Never
/// emitted as `"false"` either: the attribute's ABSENCE is the other statement,
/// and `aria-modal="false"` is a third thing neither modality means.
#[test]
fn owes_modal_aria_modal_only_when_blocking() {
    let blocking = render(
        r#"{"id":"m","kind":{"$type":"Modal","children":[],"dismissable":true,"open":{"$type":"Static","value":true}}}"#,
    );
    assert!(
        blocking.contains(r#"aria-modal="true""#),
        "the blocking modality claims inertness: {blocking}"
    );
    assert!(
        blocking.contains(r#"role="dialog""#),
        "and is a dialog: {blocking}"
    );

    let popover = render(
        r#"{"id":"m","kind":{"$type":"Modal","children":[],"dismissable":true,"modality":"Popover","open":{"$type":"Static","value":true}}}"#,
    );
    assert!(
        !popover.contains("aria-modal"),
        "a popover does not make the page inert, so it claims nothing — and the \
         absence IS the statement, never `aria-modal=\"false\"`: {popover}"
    );
    assert!(
        popover.contains(r#"role="dialog""#),
        "a popover is still a dialog: {popover}"
    );
}

// ─── Phase 1696 — the `style.direction` trait (§3.1) ─────────────────────────
//
// A trait rides the node ENVELOPE, so these five checkers are written against a
// kind chosen for being uninteresting: the claims are about the wrapper, and a
// checker leaning on some kind's own markup would be asserting that kind.
//
// The emission itself has been correct here since Phase 1472. What is new is
// that the claim is ENUMERABLE: the roster declares it, so a regression is
// reported by name rather than noticed by whoever next reads §3.1.
//
// Two of the five are COMPARISONS rather than emission assertions, and that is
// what makes them checkable at all. Rule 4 says `auto` is the absence of a
// declaration, and the honest test is that the two emissions are byte-identical:
// the reference host emits `dir="auto"` for a bidi-isolated display leaf under a
// heuristic this host has deliberately not adopted, so "emits nothing" would be
// a claim that means different things on different hosts. Rule 5 says nothing
// else is derived, and the test is that a declared emission differs from the
// undeclared one by the direction and its isolation ALONE - a subtraction no
// single-node assertion can express.

/// One leaf whose `style.direction` is as given; `None` omits the member.
fn direction_leaf(direction: Option<&str>, text: &str) -> String {
    let style = match direction {
        Some(d) => format!(r#","style":{{"direction":"{d}"}}"#),
        None => String::new(),
    };
    render(&format!(
        r#"{{"id":"d","kind":{{"$type":"Badge","label":"{text}","variant":"Neutral"}}{style}}}"#
    ))
}

/// An `rtl` container holding one child, so the two claims a single leaf cannot
/// carry - inheritance and descendant emission - have a tree to act on.
fn direction_block(child_direction: Option<&str>) -> String {
    let child_style = match child_direction {
        Some(d) => format!(r#","style":{{"direction":"{d}"}}"#),
        None => String::new(),
    };
    render(&format!(
        r#"{{"id":"block","kind":{{"$type":"Box","children":[{{"id":"child","kind":{{"$type":"Badge","label":"RR123456789IL","variant":"Neutral"}}{child_style}}}],"layout":{{"$type":"Flex","direction":"Vertical","wrap":false}},"role":"Group"}},"style":{{"direction":"rtl"}}}}"#
    ))
}

/// §3.1 rule 1 — the declared direction is EMITTED on the element carrying the
/// node's own run.
#[test]
fn owes_style_direction_declared_direction_emitted() {
    let ltr = direction_leaf(Some("ltr"), "RR123456789IL");
    let rtl = direction_leaf(Some("rtl"), "\u{5e9}\u{5dc}\u{5d5}\u{5dd}");
    assert!(
        ltr.contains(r#" dir="ltr""#),
        "a declared ltr direction is emitted on the node's own wrapper: {ltr}"
    );
    assert!(
        rtl.contains(r#" dir="rtl""#),
        "and so is a declared rtl one: {rtl}"
    );

    // The twin. Without it a renderer emitting `dir="ltr"` on every node would
    // pass both assertions above while saying nothing true.
    let undeclared = direction_leaf(None, "plain");
    assert!(
        !undeclared.contains(" dir="),
        "an undeclared node must not carry a direction it never declared: {undeclared}"
    );
}

/// §3.1 rule 2 — the declared run is ISOLATED from the surrounding
/// bidirectional context.
///
/// The isolation is the class, whose reference-stylesheet rule is
/// `unicode-bidi: isolate`. `dir` alone states a direction and leaves the text
/// AROUND the run reordered, which is the half that is invisible when you look
/// only at the value itself.
#[test]
fn owes_style_direction_declared_run_isolated() {
    let ltr = direction_leaf(Some("ltr"), "RR123456789IL");
    let rtl = direction_leaf(Some("rtl"), "\u{5e9}\u{5dc}\u{5d5}\u{5dd}");
    assert!(
        ltr.contains("fuaran-dir-ltr"),
        "a declared ltr run carries the isolating class: {ltr}"
    );
    assert!(
        rtl.contains("fuaran-dir-rtl"),
        "and so does a declared rtl one: {rtl}"
    );

    let undeclared = direction_leaf(None, "plain");
    assert!(
        !undeclared.contains("fuaran-dir-"),
        "an undeclared node is isolated by nothing, because it declared nothing: {undeclared}"
    );
}

/// §3.1 rule 3 — the DECLARATION wins over any direction the host would
/// otherwise infer, an inherited one included.
#[test]
fn owes_style_direction_declaration_wins_over_inference() {
    // An `ltr` reference INSIDE an `rtl` block - the case the member exists for.
    let html = direction_block(Some("ltr"));
    assert!(
        html.contains(r#" dir="rtl""#),
        "the declaring container keeps its own direction: {html}"
    );
    assert!(
        html.contains(r#" dir="ltr""#),
        "the nested declaration did not win over the inherited direction: {html}"
    );
}

/// §3.1 rule 4 — `auto` is the ABSENCE of a declaration, as a byte comparison.
#[test]
fn owes_style_direction_auto_is_no_declaration() {
    let explicit = direction_leaf(Some("auto"), "plain");
    let omitted = direction_leaf(None, "plain");
    assert_eq!(
        explicit, omitted,
        "a node declaring `auto` must render identically to the same node omitting the member - \
         `auto` IS the absence of a declaration"
    );
}

/// §3.1 rule 5 — NOTHING else is derived from the declaration.
#[test]
fn owes_style_direction_no_derived_direction_behaviour() {
    // The SUBTRACTION: a renderer that also flipped an alignment, swapped a
    // layout side or pushed a direction onto descendants fails here and passes
    // every assertion above.
    let declared = direction_leaf(Some("rtl"), "RR123456789IL");
    let undeclared = direction_leaf(None, "RR123456789IL");
    let stripped = declared
        .replacen(r#" dir="rtl""#, "", 1)
        .replacen(" fuaran-dir-rtl", "", 1);
    assert_eq!(
        stripped, undeclared,
        "a declared direction changed something other than the direction and its isolation - no \
         layout side, locale, alignment or descendant direction may be derived from it"
    );

    // ...and the descendant half, stated separately because a single leaf
    // cannot carry it: an undeclared child inside a declaring parent emits no
    // direction of its own. Inheritance is the receiving surface's, not a
    // second emission.
    let html = direction_block(None);
    assert_eq!(
        html.matches(" dir=").count(),
        1,
        "exactly one element declared a direction, so exactly one may carry it - a direction \
         pushed onto descendants is a derived behaviour rule 5 forbids: {html}"
    );
}

/// Which (kind, claim) pairs this host asserts, and how.
///
/// Keyed by the claim's WIRE token, because the enumeration it is matched
/// against comes from the artefact. The value is a pointer to the `#[test]`
/// that asserts it, so a registry entry naming a claim nothing implements does
/// not compile.

// ─── Phase 1704 — Sparkline float-sequence resolution (§24.7) ────────────────
//
// The claims are about a HOST-FED series, so these two checkers are the only
// ones in this file that render against a non-empty store. That is structural
// rather than convenient: a float-sequence slot TYPES its elements at decode, so
// `[1,"3.5",3]` is a `WRONG_TYPE` and no document can carry the case. The store
// is the only place a foreign element exists, which is why §24.7 is a render
// obligation and not a codec family.
//
// The observable is the emitted `<polyline points="…">`: the lowering yields one
// point per series element, so counting points counts readings. An assertion on
// the em-dash alone could not tell a host that read every element from one that
// read the first two and gave up.
//
// This host already resolved the seam the way §24.7 states (Phase 1673 made
// `resolve_float_seq` propagate the sentinel rather than drop the element), so
// nothing in `src/` moves here. That is not a reason to skip the checkers: the
// declaration's whole mechanism is that every adopting host answers the claim
// from the artefact's enumeration, and a host that merely happens to conform
// today is exactly the host a later refactor breaks silently.
//
// The document is the corpus's own bound-source sparkline —
// `nodes/state-absent-default.json`'s `absent-default-sparkline`, reproduced here
// as one node so the checker renders the subject rather than digging it out of a
// six-node composite.
const BOUND_SPARKLINE: &str = r#"{"id":"absent-default-sparkline","kind":{"$type":"Sparkline","source":{"$type":"State","key":"series"}}}"#;

/// Render the bound sparkline with `series` fed from the store.
fn render_series(series: JVal) -> String {
    let mut sources = BindingSources::default();
    sources.state.insert("series".to_string(), series);
    render_to_html(&node(BOUND_SPARKLINE), &sources)
}

/// How many readings the emission shows: one `x,y` pair per element.
fn point_count(html: &str) -> usize {
    let marker = "points=\"";
    let Some(start) = html.find(marker) else {
        return 0;
    };
    let rest = &html[start + marker.len()..];
    let Some(end) = rest.find('"') else {
        return 0;
    };
    rest[..end].split_whitespace().count()
}

fn num_series(values: &[f64]) -> JVal {
    JVal::Arr(values.iter().copied().map(JVal::Num).collect())
}

/// §24.7 — ONE READING PER ELEMENT, whatever the elements are.
#[test]
fn owes_sparkline_float_seq_reads_element_wise() {
    let finite = render_series(num_series(&[1.0, 2.0, 3.0, 4.0]));
    assert_eq!(
        point_count(&finite),
        4,
        "a four-element series must draw four readings: {finite}"
    );

    // The element the rule is about: one the host cannot read as a number, among
    // readable neighbours. Several shapes, because a host special-casing strings
    // and one special-casing foreign types are different defects.
    for foreign in [
        JVal::Str("banana".to_string()),
        JVal::Bool(true),
        JVal::Null,
        JVal::Arr(vec![JVal::Num(1.0)]),
    ] {
        let html = render_series(JVal::Arr(vec![
            JVal::Num(1.0),
            foreign.clone(),
            JVal::Num(3.0),
            JVal::Num(4.0),
        ]));
        assert!(
            !html.contains("fuaran-sparkline-empty"),
            "one unreadable element ({foreign:?}) suppressed the whole series - the em-dash is the \
             UNRESOLVED case, not the partly-readable one; discarding the readable points tells the \
             reader nothing at all: {html}"
        );
        assert_eq!(
            point_count(&html),
            4,
            "an unreadable element ({foreign:?}) changed the series LENGTH - a series index is a \
             position, so a dropped reading slides every later one one place left: {html}"
        );
    }
}

/// §24.7 — the element accept set is §7's and CLOSED.
#[test]
fn owes_sparkline_float_seq_accept_set_closed() {
    // The twin FIRST, so the comparison below is against a real render rather
    // than two em-dashes agreeing about nothing.
    let genuine = render_series(num_series(&[0.0, 3.5, 7.0]));
    assert_eq!(
        point_count(&genuine),
        3,
        "the genuine number must be read - the closed set admits JSON numbers: {genuine}"
    );

    // The comparison IS the claim, and it is the one formulation that reads the
    // same on every host: a host that coerced "3.5" emits byte-identical markup
    // for the two, whatever its geometry. Asserting the characters `3.5` are
    // absent would pass on a host that coerced and then scaled the coordinate.
    for spelling in ["3.5", "+3.5", " 3.5 ", "0x1p-2", "inf", "nan"] {
        let coerced = render_series(JVal::Arr(vec![
            JVal::Num(0.0),
            JVal::Str(spelling.to_string()),
            JVal::Num(7.0),
        ]));
        assert_ne!(
            coerced, genuine,
            "the string {spelling:?} resolved to the number it spells - the accept set at this slot \
             is §7's and closed, and this host's own decoder refuses exactly this spelling"
        );
    }

    // …and the three the set DOES admit, in the same shape. Without them the
    // claim above would be satisfied by a host that read no string at all,
    // including the sentinels the format exists to spell.
    for sentinel in ["NaN", "Infinity", "-Infinity"] {
        let html = render_series(JVal::Arr(vec![
            JVal::Num(1.0),
            JVal::Str(sentinel.to_string()),
            JVal::Num(3.0),
        ]));
        assert_eq!(
            point_count(&html),
            3,
            "the sentinel {sentinel:?} is IN the accept set and must read as its non-finite value: {html}"
        );
    }
}

const CHECKERS: &[(&str, fn())] = &[
    (
        "Media/accessible-name-always",
        owes_media_accessible_name_always,
    ),
    (
        "Media/autoplay-muted-pairing",
        owes_media_autoplay_muted_pairing,
    ),
    ("Media/no-autoplay-pathway", owes_media_no_autoplay_pathway),
    (
        "Media/refused-source-dropped",
        owes_media_refused_source_dropped,
    ),
    ("Image/alt-always-emitted", owes_image_alt_always_emitted),
    (
        "Image/anchor-affordance-on-expandable",
        owes_image_anchor_affordance_on_expandable,
    ),
    (
        "Image/refused-src-no-affordance",
        owes_image_refused_src_no_affordance,
    ),
    (
        "Image/figure-caption-outside-link",
        owes_image_figure_caption_outside_link,
    ),
    (
        "Image/srcset-ascending-by-width",
        owes_image_srcset_ascending_by_width,
    ),
    (
        "Custom/unregistered-custom-labelled",
        owes_custom_unregistered_custom_labelled,
    ),
    // Phase 1128 — the platform-baseline wave's obligations, adopted here.
    (
        "Media/authored-child-order",
        owes_media_authored_child_order,
    ),
    (
        "Media/single-default-per-kind",
        owes_media_single_default_per_kind,
    ),
    (
        "Media/transcript-disclosure-named",
        owes_media_transcript_disclosure_named,
    ),
    (
        "Embed/accessible-name-always",
        owes_embed_accessible_name_always,
    ),
    (
        "Embed/sandbox-always-exactly-declared",
        owes_embed_sandbox_always_exactly_declared,
    ),
    (
        "Embed/refused-embed-source-omitted",
        owes_embed_refused_embed_source_omitted,
    ),
    (
        "Tree/accessible-name-always",
        owes_tree_accessible_name_always,
    ),
    (
        "FileUpload/picker-always-present",
        owes_file_upload_picker_always_present,
    ),
    (
        "FileUpload/ceiling-recorded-never-enforced",
        owes_file_upload_ceiling_recorded_never_enforced,
    ),
    (
        "Modal/aria-modal-only-when-blocking",
        owes_modal_aria_modal_only_when_blocking,
    ),
    // Phase 1696 - the node-level trait, keyed by its id rather than a kind.
    (
        "style.direction/declared-direction-emitted",
        owes_style_direction_declared_direction_emitted,
    ),
    (
        "style.direction/declared-run-isolated",
        owes_style_direction_declared_run_isolated,
    ),
    (
        "style.direction/declaration-wins-over-inference",
        owes_style_direction_declaration_wins_over_inference,
    ),
    (
        "style.direction/auto-is-no-declaration",
        owes_style_direction_auto_is_no_declaration,
    ),
    (
        "style.direction/no-derived-direction-behaviour",
        owes_style_direction_no_derived_direction_behaviour,
    ),
    // Phase 1704 — the two float-sequence resolution claims (§24.7).
    (
        "Sparkline/float-seq-reads-element-wise",
        owes_sparkline_float_seq_reads_element_wise,
    ),
    (
        "Sparkline/float-seq-accept-set-closed",
        owes_sparkline_float_seq_accept_set_closed,
    ),
];

/// Obligations this host declares it does NOT check, each with a reason.
///
/// EMPTY is the correct state for this host: its server walk is exhaustive over
/// `NodeKind` with no catch-all arm, so it renders every canonical kind and
/// every declared obligation is one it owes. The table exists because the
/// alternative — an unchecked obligation silently absent from the registry — is
/// precisely the failure the manifest replaces. A host that genuinely cannot
/// check a claim records it here in a full sentence and its report says so out
/// loud.
const DECLARED_EXEMPTIONS: &[(&str, &str)] = &[];

fn status_of(kind: &str, claim_id: &str) -> ObligationOutcome {
    let key = format!("{kind}/{claim_id}");
    if CHECKERS.iter().any(|(k, _)| *k == key) {
        return ObligationOutcome::Asserted;
    }
    if let Some((_, reason)) = DECLARED_EXEMPTIONS.iter().find(|(k, _)| *k == key) {
        return ObligationOutcome::Unchecked {
            reason: (*reason).to_string(),
        };
    }
    ObligationOutcome::Unchecked {
        reason: "no checker registered in render_obligations.rs and no declared exemption — \
                 add one, or declare why this host cannot check it"
            .to_string(),
    }
}

fn is_exempt(line: &ObligationReport) -> bool {
    let key = format!("{}/{}", line.kind, line.claim_id);
    DECLARED_EXEMPTIONS.iter().any(|(k, _)| *k == key)
}

// ─── The gate ────────────────────────────────────────────────────────────────

#[test]
fn asserts_every_obligation_the_manifest_declares() {
    let Some(manifest) = load() else { return };
    let report = report_obligations(&manifest, status_of);

    assert!(
        !report.is_empty(),
        "the manifest declares no obligations at all — either the artefact is stale or this suite \
         is reading the wrong file, and either way it is asserting nothing"
    );

    // NOT CHECKED IS NOT PASSED. Everything this host did not assert is printed
    // by name and section BEFORE the gate decides, so an exempted claim is
    // visible in the run rather than inferable from its absence.
    let unmet = unasserted_obligations(&report);
    for line in &unmet {
        println!(
            "  render obligation not asserted: {}",
            describe_obligation_report(line)
        );
        // The normative statement, beneath the shared one-liner rather than
        // inside it: `describe_obligation_report` is the sentence every host
        // prints and must stay identical across them, while the claim itself is
        // what whoever adds the missing checker actually needs — and needing it
        // is the only situation in which these lines are printed at all.
        if !line.statement.is_empty() {
            println!("      claim: {}", line.statement);
        }
    }

    let undeclared: Vec<String> = unmet
        .iter()
        .filter(|line| !is_exempt(line))
        .map(|line| format!("{}/{} [{}]", line.kind, line.claim_id, line.section))
        .collect();

    assert!(
        undeclared.is_empty(),
        "a render obligation this host owes has no checker: assert it, or add a declared \
         exemption saying why this host cannot. Unmet: {undeclared:?}"
    );
}

#[test]
fn reports_an_obligation_with_no_checker_as_unchecked() {
    // The go-red proof, in the small. This is the shape a NEWLY-DECLARED
    // obligation takes on the day it lands: a kind/claim pair the registry does
    // not cover. Without this probe the gate above could be green because the
    // classification never reports anything, which is the completeness check
    // that cannot fail.
    let outcome = status_of("Markdown", "accessible-name-always");
    match &outcome {
        ObligationOutcome::Unchecked { reason } => assert!(
            reason.contains("no checker registered"),
            "in words a reader can act on: {reason}"
        ),
        other => panic!("an unregistered (kind, claim) must be reported UNCHECKED, got {other:?}"),
    }

    // …and the gate's own filter must classify it as unasserted, which is what
    // turns the suite red.
    let probe = ObligationReport {
        kind: "Markdown".to_string(),
        claim_id: "accessible-name-always".to_string(),
        statement: String::new(),
        section: "probe".to_string(),
        outcome,
    };
    assert_eq!(
        unasserted_obligations(std::slice::from_ref(&probe)).len(),
        1
    );
    assert!(describe_obligation_report(&probe).contains("UNCHECKED"));

    // The third outcome is pinned here too, because nothing this host renders
    // produces it: `NotRendered` is what a host owes when it does not render a
    // kind at all, and its wording must match the other hosts' for the day one
    // of them uses it.
    let not_rendered = ObligationReport {
        kind: "Media".to_string(),
        claim_id: "accessible-name-always".to_string(),
        statement: String::new(),
        section: "probe".to_string(),
        outcome: ObligationOutcome::NotRendered {
            reason: "this host does not render the kind".to_string(),
        },
    };
    assert_eq!(
        describe_obligation_report(&not_rendered),
        "Media/accessible-name-always [probe]: not rendered (this host does not render the kind)"
    );
    assert_eq!(
        unasserted_obligations(std::slice::from_ref(&not_rendered)).len(),
        1,
        "nothing is owed, but it is still SURFACED"
    );
}

#[test]
fn resolves_every_declared_claim_id_against_the_closed_vocabulary() {
    // A row naming a claim the vocabulary omits is unresolvable: a host keying
    // its registry off the vocabulary could never report it, and a host must
    // never accept a claim it cannot name.
    let Some(manifest) = load() else { return };

    assert!(
        !manifest.obligation_vocabulary.is_empty(),
        "the artefact carries no obligation vocabulary"
    );
    for entry in &manifest.obligation_vocabulary {
        assert!(
            !entry.meaning.is_empty(),
            "{}: a closed vocabulary whose entries say nothing is not a vocabulary",
            entry.id
        );
    }

    let unresolvable: Vec<String> = all_obligations(&manifest)
        .into_iter()
        .filter(|(_, o)| !manifest.obligation_vocabulary.iter().any(|v| v.id == o.id))
        .map(|(kind, o)| format!("{kind}/{}", o.id))
        .collect();

    assert!(
        unresolvable.is_empty(),
        "a kind declares an obligation the closed vocabulary does not carry: {unresolvable:?}"
    );

    // Every claim carries a section and a statement. An obligation with no
    // section is an assertion about a host's habits, not about the
    // specification, and is not admissible.
    for (kind, o) in all_obligations(&manifest) {
        assert!(
            o.section.contains("WIRE_FORMAT.md"),
            "{kind}/{}: no spec section",
            o.id
        );
        assert!(
            !o.statement.is_empty(),
            "{kind}/{}: no normative statement",
            o.id
        );
    }
}

/// A trait's declared SCOPE is one this host can act on.
///
/// `appliesTo` decides whether a trait claim is OWED here at all: a trait riding
/// only kinds this host does not render owes nothing, and one riding the
/// envelope is owed by everything. A scope this host cannot interpret is
/// therefore not a cosmetic defect — it is an unanswerable question about
/// whether the gate should be red.
///
/// The two arms are asserted in BOTH directions, because the tagged shape exists
/// precisely so that "every kind" is not spellable as an empty array: an
/// `allKinds` carrying a list, or a `namedKinds` carrying none, would each read
/// as the opposite of what it says.
#[test]
fn every_declared_trait_scope_is_actionable() {
    let Some(manifest) = load() else { return };

    for row in &manifest.traits {
        assert!(
            row.trait_id.contains('.'),
            "a trait id is the wire path of the member it governs, never a bare kind name: {}",
            row.trait_id
        );
        assert!(
            !manifest.kinds.iter().any(|k| k.kind == row.trait_id),
            "{} collides with a kind name; one registry keys both populations",
            row.trait_id
        );
        match row.scope.as_str() {
            "allKinds" => assert!(
                row.scope_kinds.is_empty(),
                "{}: an allKinds scope names no kinds - a list would be a narrower claim than the                  scope itself: {:?}",
                row.trait_id,
                row.scope_kinds
            ),
            "namedKinds" => assert!(
                !row.scope_kinds.is_empty(),
                "{}: a namedKinds scope with an empty list rides NOTHING, which is satisfiable by                  rendering nothing at all",
                row.trait_id
            ),
            other => panic!(
                "{}: this host cannot interpret the scope {other:?}, so it cannot say whether the                  trait's claims are owed here",
                row.trait_id
            ),
        }
    }
}

#[test]
fn registers_no_checker_for_an_obligation_the_manifest_does_not_declare() {
    // A checker for a claim no row declares is a stale assertion: it passes
    // forever and guards a contract that has moved, which is exactly the drift
    // the generated artefact exists to remove.
    let Some(manifest) = load() else { return };
    let declared: Vec<String> = all_obligations(&manifest)
        .into_iter()
        .map(|(kind, o)| format!("{kind}/{}", o.id))
        .collect();

    let orphans: Vec<&str> = CHECKERS
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| !declared.iter().any(|d| d == k))
        .collect();

    assert!(
        orphans.is_empty(),
        "a checker asserts an obligation no manifest row declares — either the row was removed or \
         the checker was never declared: {orphans:?}"
    );
}
