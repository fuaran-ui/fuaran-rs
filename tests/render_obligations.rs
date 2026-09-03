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

struct RenderFidelityManifest {
    obligation_vocabulary: Vec<VocabularyEntry>,
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

/// Every declared obligation, paired with the kind that owes it, in table
/// order.
fn all_obligations(manifest: &RenderFidelityManifest) -> Vec<(&str, &Obligation)> {
    manifest
        .kinds
        .iter()
        .flat_map(|row| row.obligations.iter().map(move |o| (row.kind.as_str(), o)))
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

/// Which (kind, claim) pairs this host asserts, and how.
///
/// Keyed by the claim's WIRE token, because the enumeration it is matched
/// against comes from the artefact. The value is a pointer to the `#[test]`
/// that asserts it, so a registry entry naming a claim nothing implements does
/// not compile.
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
        "Modal/aria-modal-only-when-blocking",
        owes_modal_aria_modal_only_when_blocking,
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
