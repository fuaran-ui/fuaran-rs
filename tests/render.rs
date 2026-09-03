//! Server-HTML renderer behaviour: the parity-locked class vocabulary, inert
//! interactivity, sanitiser posture, binding resolution, the islands
//! partial-hydration laws, and the reference-CSS byte-copy.

use std::collections::HashMap;
use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::client::ClientSession;
use fuaran_rs::render::class_names::trend_sentiment;
use fuaran_rs::render::egress::permissive_egress;
use fuaran_rs::render::server::{hydrate_script_id, island_script_id};
use fuaran_rs::render::{
    BindingSources, render_hydratable, render_to_html, render_to_html_with_egress,
    render_with_islands,
};
use fuaran_rs::wire::{Node, NodeKind, TrendPolarity, decode_node, encode_node};

fn node(json: &str) -> Node {
    decode_node(json).expect("test tree decodes")
}

fn render(json: &str) -> String {
    render_to_html(&node(json), &BindingSources::default())
}

/// Render with the destination policy widened BY NAME.
///
/// The convenience entry point above is ambiently deny-non-local (Phase 1037),
/// which is the posture a decoded tree gets. A test whose subject is something
/// else — the scheme floor, the protected-email emission, a11y placement —
/// reaches this one so the thing under test is the only thing that can fail.
fn render_permissive(json: &str) -> String {
    render_to_html_with_egress(
        &permissive_egress(),
        &node(json),
        &BindingSources::default(),
    )
}

#[test]
fn heading_renders_with_reference_classes() {
    let html = render(
        r#"{"id":"h1","kind":{"$type":"Heading","level":2,"text":{"$type":"Literal","text":"Revenue <Q3>"},"variant":"Eyebrow"}}"#,
    );
    assert!(html.contains("data-fuaran-node-id=\"h1\""));
    assert!(html.contains("fuaran-kind-heading"));
    assert!(
        html.contains(
            "fuaran-node fuaran-tone-default fuaran-weight-standard fuaran-emphasis-normal"
        )
    );
    assert!(html.contains("<h2 class=\"fuaran-heading fuaran-heading-eyebrow\">"));
    // Text content is escaped.
    assert!(html.contains("Revenue &lt;Q3&gt;"));
}

#[test]
fn button_renders_inert_with_unwired_hint() {
    let html = render(
        r#"{"id":"b","kind":{"$type":"Button","label":{"$type":"Literal","text":"Go"},"onClick":{"$type":"Navigate","route":"/x"},"variant":"Primary"}}"#,
    );
    assert!(html.contains("fuaran-button fuaran-button-primary fuaran-button-unwired"));
    assert!(html.contains("title=")); // the unwired tooltip
    assert!(!html.contains("onclick")); // no event handlers, ever
}

#[test]
fn link_href_is_sanitised() {
    // Rendered PERMISSIVE, so the scheme floor is the only thing that can be
    // refusing. Its rejection renders as a refusal rather than the bare
    // `about:blank` this pinned before Phase 1037: once a call site consults the
    // policy, "this destination was refused" is one fact with one rendering, and
    // splitting it by which gate refused would make the `javascript:` URL — the
    // more dangerous case — the one that renders as an ordinary blank.
    let html = render_permissive(
        r#"{"id":"l","kind":{"$type":"Link","download":false,"href":{"$type":"Static","value":"javascript:alert(1)"},"label":{"$type":"Literal","text":"Docs"}}}"#,
    );
    assert!(html.contains("href=\"about:blank#fuaran-egress-refused\""));
    assert!(html.contains("data-fuaran-egress-refused=\"unsafe-url\""));
    assert!(!html.contains("javascript:"));
    let ok = render_permissive(
        r#"{"id":"l","kind":{"$type":"Link","download":false,"href":{"$type":"Static","value":"https://x.dev/docs"},"label":{"$type":"Literal","text":"Docs"}}}"#,
    );
    assert!(ok.contains("href=\"https://x.dev/docs\""));
    assert!(!ok.contains("fuaran-egress-refused"));
}

#[test]
fn link_protected_email_emits_no_plaintext_address() {
    // The protected emission: wrapper span + protected anchor with every
    // href/label character a decimal entity; no plaintext address or scheme
    // anywhere in the output.
    //
    // PERMISSIVE, because `mailto:` is a hostless scheme and the ambient default
    // refuses those (Phase 1037: an egress channel with no host for a rule to
    // name can only be permitted wholesale). Under the default this arm is not
    // reached at all — the href is the refusal URL, which does not start
    // `mailto:` — and that behaviour is pinned by the ambient test below.
    let html = render_permissive(
        r#"{"id":"plk","kind":{"$type":"Link","download":false,"href":{"$type":"Static","value":"mailto:contact@example.com"},"label":{"$type":"Literal","text":"Email us"},"protection":"email"}}"#,
    );
    assert!(html.contains("<span class=\"fuaran-link-protected-wrap\">"));
    assert!(html.contains(
        "<a class=\"fuaran-link fuaran-link-protected\" href=\"&#109;&#97;&#105;&#108;&#116;&#111;&#58;"
    ));
    for banned in ["mailto:", "contact@example.com", "@example", "Email us"] {
        assert!(
            !html.contains(banned),
            "protected output leaks plaintext {banned:?}: {html}"
        );
    }
}

#[test]
fn box_corners_render_their_retired_kind_hooks() {
    let card = render(
        r#"{"id":"c","kind":{"$type":"Box","children":[],"heading":{"$type":"Literal","text":"Totals"},"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Card"}}"#,
    );
    assert!(card.contains("fuaran-kind-card"));
    assert!(card.contains("<section class=\"fuaran-layout-card\">"));
    assert!(card.contains("fuaran-card-heading"));

    let dash = render(
        r#"{"id":"d","kind":{"$type":"Box","children":[],"layout":{"$type":"Auto"},"role":"Dashboard"}}"#,
    );
    assert!(dash.contains("fuaran-kind-dashboard"));
    assert!(dash.contains("fuaran-layout-dashboard"));

    let grid = render(
        r#"{"id":"g","kind":{"$type":"Box","children":[],"layout":{"$type":"Grid","cols":3},"role":"Group"}}"#,
    );
    assert!(grid.contains("fuaran-kind-grid-layout"));
    assert!(grid.contains("grid-template-columns:repeat(3, 1fr)"));

    let sep = render(
        r#"{"id":"s","kind":{"$type":"Box","children":[],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Separator"}}"#,
    );
    assert!(sep.contains("fuaran-kind-divider"));
    assert!(sep.contains("<hr class=\"fuaran-layout-separator\" />"));
}

#[test]
fn metric_resolves_state_binding_and_loading_slot() {
    let tree = node(
        r#"{"id":"m","kind":{"$type":"Metric","emphasis":"Normal","format":{"$type":"None"},"label":{"$type":"Literal","text":"Revenue"},"value":{"$type":"Query","accessor":"<closure>","name":"revenue"},"tone":"Default","weight":"Standard"},"state":{"onLoading":{"id":"skel","kind":{"$type":"Skeleton","rows":1}}}}"#,
    );
    // No source registered → the loading slot renders.
    let loading = render_to_html(&tree, &BindingSources::default());
    assert!(loading.contains("fuaran-skeleton"));

    // Registered → the value renders.
    let sources = BindingSources {
        query_results: HashMap::from([("revenue".to_string(), JVal::Num(42.5))]),
        ..Default::default()
    };
    let resolved = render_to_html(&tree, &sources);
    assert!(resolved.contains("fuaran-metric-value\">42.5<"));
}

// ─── Phase 867: trend sentiment (WIRE_FORMAT.md §3.6.1) ──────────────────────
//
// Sentiment = sign(trend) × polarity, rendered on the TREND ELEMENT alone. The
// pair these assertions exist for is one falling number read two ways: the same
// −0.0734 is a regression under the default polarity and an improvement under
// `LowerIsBetter`, and NEITHER reading touches `tone` or the printed sign.

/// Walks up from the crate directory looking for the shared corpus, matching
/// `tests/conformance.rs`'s locator. `None` keeps the repo standalone-testable.
fn corpus_root() -> Option<PathBuf> {
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

/// One `Metric` with a resolvable static trend. `polarity` is spliced verbatim
/// so a test can pass the reserved spelling and watch the decode refuse it.
fn metric_with_trend(polarity_member: &str) -> String {
    format!(
        r#"{{"id":"m","kind":{{"$type":"Metric","format":{{"$type":"Duration","style":"Compact","unit":"Minutes"}},"label":"Avg wait","tone":"Warning","trend":{{"$type":"Static","value":-0.0734}},"trendFormat":{{"$type":"Percent","decimals":2}}{polarity_member},"value":{{"$type":"Static","value":80}}}}}}"#
    )
}

#[test]
fn the_sentiment_composition_is_sign_times_polarity() {
    // The §3.6.1 table, in full — the reason the slot is not `tone` and not a
    // sign test: BOTH inputs matter, and neither alone determines the answer.
    for (polarity, trend, want) in [
        (TrendPolarity::HigherIsBetter, 1.5, "improving"),
        (TrendPolarity::HigherIsBetter, -1.5, "regressing"),
        (TrendPolarity::HigherIsBetter, 0.0, "unchanged"),
        (TrendPolarity::LowerIsBetter, 1.5, "regressing"),
        (TrendPolarity::LowerIsBetter, -1.5, "improving"),
        (TrendPolarity::LowerIsBetter, 0.0, "unchanged"),
    ] {
        let (sentiment, _glyph) = trend_sentiment(polarity, trend);
        assert_eq!(sentiment, want, "{polarity:?} × {trend}");
    }
    // Every sentiment carries a DISTINCT glyph — the non-colour channel is only
    // a channel if the three cases are distinguishable without colour.
    let glyphs: Vec<&str> = [
        (TrendPolarity::HigherIsBetter, 1.0),
        (TrendPolarity::HigherIsBetter, -1.0),
        (TrendPolarity::HigherIsBetter, 0.0),
    ]
    .into_iter()
    .map(|(p, t)| trend_sentiment(p, t).1)
    .collect();
    assert_eq!(glyphs, vec!["\u{25B2}", "\u{25BC}", "\u{2192}"]);
}

#[test]
fn metric_trend_without_polarity_reads_a_fall_as_a_regression() {
    let html = render(&metric_with_trend(""));
    assert!(
        html.contains(
            r#"<div class="fuaran-metric-trend fuaran-metric-trend-regressing"><span class="fuaran-metric-trend-glyph" role="img" aria-label="regressing">\u{25BC}</span>-7.34%</div>"#
                .replace("\\u{25BC}", "\u{25BC}")
                .as_str()
        ),
        "absent polarity means HigherIsBetter, so a fall is a regression: {html}"
    );
    // The defect this phase closes: the trend used to carry ONE class, painted
    // success unconditionally, so a falling number read as good on every host.
    assert!(
        !html.contains(r#"class="fuaran-metric-trend""#),
        "a resolved trend must never emit the bare unconditional class: {html}"
    );
}

#[test]
fn metric_trend_polarity_inverts_the_sentiment_without_touching_sign_or_tone() {
    let html = render(&metric_with_trend(r#","trendPolarity":"LowerIsBetter""#));
    assert!(
        html.contains(r#"fuaran-metric-trend fuaran-metric-trend-improving"#),
        "sign(−) × LowerIsBetter(−1) is positive ⇒ improving: {html}"
    );
    // Clause 3 — polarity changes how the number READS, never what it SAYS.
    assert!(
        html.contains("-7.34%"),
        "the numeric text keeps its minus sign: {html}"
    );
    // Clause 2 / the composition rule — `tone` is never written to. The tile
    // stays Warning even though the trend now reads as an improvement, which is
    // the pair a single `tone` slot could never have expressed.
    assert!(
        html.contains("fuaran-metric fuaran-metric-warning"),
        "tone still colours the tile and is untouched by polarity: {html}"
    );
    // The glyph deliberately DISAGREES with the sign — that disagreement is the
    // visible evidence the declaration was honoured.
    assert!(
        html.contains(&format!(
            r#"<span class="fuaran-metric-trend-glyph" role="img" aria-label="improving">{}</span>"#,
            '\u{25B2}'
        )),
        "the up-triangle carries the sentiment on a non-colour channel: {html}"
    );
}

#[test]
fn metric_trend_of_zero_is_unchanged_under_either_polarity() {
    for member in ["", r#","trendPolarity":"LowerIsBetter""#] {
        let html = render(&metric_with_trend(member).replace(r#""value":-0.0734"#, r#""value":0"#));
        assert!(
            html.contains(&format!(
                r#"fuaran-metric-trend-unchanged"><span class="fuaran-metric-trend-glyph" role="img" aria-label="unchanged">{}</span>"#,
                '\u{2192}'
            )),
            "a zero trend is neither an improvement nor a regression ({member}): {html}"
        );
    }
}

#[test]
fn metric_trend_that_cannot_resolve_keeps_its_bare_div() {
    // No sentiment is computable, so none is claimed — emitting `unchanged`
    // would assert a fact about a number the renderer does not have.
    let html = render(
        r#"{"id":"m","kind":{"$type":"Metric","label":"Avg wait","trend":{"$type":"Query","accessor":"<closure>","name":"missing"},"trendPolarity":"LowerIsBetter","value":{"$type":"Static","value":80}}}"#,
    );
    assert!(
        html.contains(r#"<div class="fuaran-metric-trend"></div>"#),
        "an unresolved trend keeps its bare div byte-for-byte: {html}"
    );
    assert!(
        !html.contains("fuaran-metric-trend-glyph"),
        "no glyph without a resolved number: {html}"
    );
}

#[test]
fn metric_trend_polarity_is_inert_without_a_trend() {
    // §3.6.1 clause 4 — legal, and says nothing.
    let html = render(
        r#"{"id":"m","kind":{"$type":"Metric","label":"Avg wait","trendPolarity":"LowerIsBetter","value":{"$type":"Static","value":80}}}"#,
    );
    assert!(!html.contains("fuaran-metric-trend"), "{html}");
    assert!(html.contains("fuaran-metric-value"), "{html}");
}

#[test]
fn metric_trend_sentiment_markup_matches_the_corpus_fixture() {
    // The fixture the wave added to gate exactly this: a `"tone":"Warning"` tile
    // whose falling −7.34% reads as an IMPROVEMENT. Asserted as one whole
    // string, because the parity claim is about the emitted bytes rather than
    // about a set of substrings that happen to be present.
    let Some(root) = corpus_root() else {
        eprintln!("corpus absent; skipping the fixture-parity leg (standalone checkout)");
        return;
    };
    let fixture = root.join("nodes").join("metric-inverted-polarity.json");
    let json = std::fs::read_to_string(&fixture)
        .unwrap_or_else(|e| panic!("corpus located but {fixture:?} unreadable: {e}"));
    let html = render(&json);
    let expected = format!(
        concat!(
            r#"<div class="fuaran-metric-trend fuaran-metric-trend-improving">"#,
            r#"<span class="fuaran-metric-trend-glyph" role="img" aria-label="improving">{}</span>"#,
            r#"-7.34%</div>"#
        ),
        '\u{25B2}'
    );
    assert!(
        html.contains(&expected),
        "corpus-fixture trend markup diverged.\n  expected: {expected}\n  in: {html}"
    );
    eprintln!(
        "trend-sentiment corpus parity EXECUTED against {}",
        fixture.display()
    );
}

#[test]
fn markdown_node_renders_through_the_deterministic_renderer() {
    let html = render(
        r##"{"id":"md","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"# Hi\n\n**bold**"}}}"##,
    );
    assert!(html.contains(
        "<div class=\"fuaran-markdown\"><h1>Hi</h1>\n<p><strong>bold</strong></p>\n</div>"
    ));
}

#[test]
fn static_grid_renders_the_semantic_table_leg() {
    let html = render(
        r#"{"id":"t","kind":{"$type":"DataGrid","columns":[],"editable":false,"source":{"$type":"Static","value":"<opaque>"},"staticRows":{"headers":[{"$type":"Literal","text":"Term"}],"rows":[[{"$type":"Literal","text":"MVU"}]]}}}"#,
    );
    assert!(html.contains("<table class=\"fuaran-table\">"));
    assert!(html.contains("fuaran-table-header\">Term<"));
    assert!(html.contains("fuaran-table-cell\">MVU<"));
}

#[test]
fn chart_lowers_in_host_to_inline_svg() {
    // Phase 551 — fuaran-rs LOWER-IN-HOST posture: a raw Chart at the render
    // boundary is lowered to a canonical Drawing and rendered as first-party inline
    // SVG (so the WASM client reaches Chart-as-data parity through this renderer),
    // never a placeholder and never a silent empty region.
    let tree = node(
        r#"{"id":"c","kind":{"$type":"Chart","kind":"Bar","source":{"$type":"Query","accessor":"<closure>","name":"rows"},"stacked":false,"title":{"$type":"Literal","text":"Revenue by quarter"},"xField":"quarter","yFields":["revenue"]}}"#,
    );
    let rows = parse(
        r#"[{"quarter":"Q1","revenue":120},{"quarter":"Q2","revenue":150},{"quarter":"Q3","revenue":90},{"quarter":"Q4","revenue":175}]"#,
    )
    .unwrap();
    let sources = BindingSources {
        query_results: HashMap::from([("rows".to_string(), rows)]),
        ..Default::default()
    };
    let html = render_to_html(&tree, &sources);
    // Lowered to inline SVG through the shared Drawing renderer.
    assert!(
        html.contains("<svg class=\"fuaran-drawing\""),
        "chart did not lower to inline SVG:\n{html}"
    );
    assert!(html.contains("class=\"fuaran-chart\""));
    // The lowered geometry is present (bar rects + the visible title label).
    assert!(html.contains("fuaran-drawing-rect"));
    assert!(html.contains("Revenue by quarter"));
    // NOT the old placeholder passthrough (that is the headless fuaran-go posture).
    assert!(
        !html.contains("fuaran-chart-placeholder"),
        "must not emit the require-pre-lowered placeholder:\n{html}"
    );
    assert!(!html.contains("Wire a chart adapter"));
}

#[test]
fn data_grid_projects_declarative_fields() {
    let tree = node(
        r#"{"id":"g","kind":{"$type":"DataGrid","columns":[
            {"field":"name","format":{"$type":"None"},"kind":{"$type":"Text"},"label":"Name","width":{"$type":"Auto"}},
            {"field":"total","format":{"$type":"Currency","code":"GBP"},"kind":{"$type":"Numeric"},"label":"Total","width":{"$type":"Auto"}}
        ],"editable":false,"source":{"$type":"Query","accessor":"<closure>","name":"rows"}}}"#,
    );
    let rows = parse(r#"[{"name":"Widgets","total":1234.5}]"#).unwrap();
    let sources = BindingSources {
        query_results: HashMap::from([("rows".to_string(), rows)]),
        ..Default::default()
    };
    let html = render_to_html(&tree, &sources);
    assert!(html.contains(">Widgets<"));
    assert!(html.contains(">GBP 1234.50<"));
}

#[test]
fn select_drops_the_opaque_placeholder_option() {
    // An opaque options source renders no concrete <option> (§5 render contract).
    let html = render(
        r#"{"id":"s","kind":{"$type":"Select","label":{"$type":"Literal","text":"Pick"},"source":{"$type":"Static","value":"<opaque>"},"value":{"$type":"Static","value":null}}}"#,
    );
    assert!(!html.contains("&lt;opaque&gt;"));
    assert!(html.contains("fuaran-select-control"));
}

#[test]
fn modal_stays_in_dom_hidden_when_closed() {
    let closed = render(
        r#"{"id":"m","kind":{"$type":"Modal","children":[],"dismissable":true,"open":{"$type":"Static","value":false}}}"#,
    );
    assert!(closed.contains("fuaran-modal-overlay\" hidden"));
    assert!(closed.contains("role=\"dialog\""));
    assert!(closed.contains("aria-modal=\"true\""));
    let open = render(
        r#"{"id":"m","kind":{"$type":"Modal","children":[],"dismissable":true,"open":{"$type":"Static","value":true}}}"#,
    );
    assert!(!open.contains("hidden"));
}

/// A kind whose body is NOT the node's semantic element keeps the whole
/// projection on the wrapper — which is every kind except `Link` / `Button` /
/// `Image`. See `accessibility_projects_onto_the_semantic_element` below for
/// the other half of the rule.
#[test]
fn accessibility_projects_onto_the_wrapper_for_a_non_forwarding_kind() {
    let html = render(
        r#"{"id":"a","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"x"}},"accessibility":{"label":{"$type":"Static","value":"Summary"},"role":"region","liveRegion":"polite"}}"#,
    );
    assert!(html.contains("aria-label=\"Summary\""));
    assert!(html.contains("role=\"region\""));
    assert!(html.contains("aria-live=\"polite\""));
}

// ─── WHERE the projection lands ──────────────────────────────────────────────
//
// A node's accessibility projection is emitted on the node's SEMANTIC ELEMENT —
// the single element the kind body renders, when that element rather than the
// wrapper carries the node's semantics: Link (<a>), Button (<button>), Image
// (<img>). Assistive technology does not associate a role on a non-interactive
// container with the interactive element inside it, so placement is the whole
// point, and these assertions are placement-sensitive: they split the emitted
// HTML at each element's own open tag rather than searching the whole string,
// which is what every other check in this file does.

/// The wrapper's own open tag — everything up to its first `>`.
fn wrapper_tag(html: &str) -> &str {
    &html[..html.find('>').expect("a wrapper open tag") + 1]
}

/// The open tag of the first `<tag …>` in the markup.
fn open_tag<'a>(html: &'a str, tag: &str) -> &'a str {
    let from = &html[html.find(&format!("<{tag}")).expect("the element")..];
    &from[..from.find('>').expect("an open tag") + 1]
}

const A11Y: &str = r#""accessibility":{"label":{"$type":"Static","value":"Home"},"role":"link"}"#;

#[test]
fn accessibility_projects_onto_the_semantic_element() {
    let link = render(&format!(
        r#"{{"id":"lk","kind":{{"$type":"Link","download":false,"href":{{"$type":"Static","value":"/home"}},"label":{{"$type":"Literal","text":"Home"}}}},{A11Y}}}"#
    ));
    assert!(!wrapper_tag(&link).contains("role="));
    assert!(!wrapper_tag(&link).contains("aria-label"));
    assert!(wrapper_tag(&link).contains(r#"data-fuaran-node-id="lk""#));
    assert!(open_tag(&link, "a").contains(r#"role="link""#));
    assert!(open_tag(&link, "a").contains(r#"aria-label="Home""#));

    let button = render(&format!(
        r#"{{"id":"btn","kind":{{"$type":"Button","label":{{"$type":"Literal","text":"Go"}},"onClick":{{"$type":"Navigate","route":"/x"}},"variant":"Primary"}},{A11Y}}}"#
    ));
    assert!(!wrapper_tag(&button).contains("aria-label"));
    assert!(open_tag(&button, "button").contains(r#"aria-label="Home""#));

    let image = render(&format!(
        r#"{{"id":"img","kind":{{"$type":"Image","alt":{{"$type":"Literal","text":"Alt"}},"src":{{"$type":"Static","value":"/a.png"}},"variant":"Default"}},{A11Y}}}"#
    ));
    assert!(!wrapper_tag(&image).contains("aria-label"));
    assert!(open_tag(&image, "img").contains(r#"aria-label="Home""#));
}

/// The protected-email Link builds its anchor as an entity-encoded opaque
/// string, so the projection lands on the wrap `<span>` — the only element that
/// arm owns in every tier. A stated limit, pinned so it stays deliberate.
#[test]
fn protected_email_link_projects_onto_the_wrap_span() {
    // Permissive for the same reason as the emission test above: the subject is
    // WHERE the projection lands, not whether `mailto:` is permitted.
    let html = render_permissive(&format!(
        r#"{{"id":"plk","kind":{{"$type":"Link","download":false,"href":{{"$type":"Static","value":"mailto:u@e.com"}},"label":{{"$type":"Literal","text":"u@e.com"}},"protection":"email"}},{A11Y}}}"#
    ));
    assert!(!wrapper_tag(&html).contains("aria-label"));
    assert!(open_tag(&html, "span").contains(r#"aria-label="Home""#));
}

#[test]
fn switch_renders_the_state_matched_case() {
    let tree = node(
        r#"{"id":"sw","kind":{"$type":"Switch","cases":[
            {"child":{"id":"a","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"Case A"}}},"match":"a"}
        ],"default":{"id":"d","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"Default"}}},"stateKey":"mode"}}"#,
    );
    let default_render = render_to_html(&tree, &BindingSources::default());
    assert!(default_render.contains("Default"));
    let sources = BindingSources {
        state: HashMap::from([("mode".to_string(), JVal::Str("a".to_string()))]),
        ..Default::default()
    };
    let matched = render_to_html(&tree, &sources);
    assert!(matched.contains("Case A"));
    assert!(!matched.contains("Default"));
}

#[test]
fn fragment_ref_expands_namespaced() {
    let html = render(
        r#"{"id":"root","kind":{"$type":"Box","children":[
            {"id":"decl","kind":{"$type":"FragmentDecl","body":{"id":"frag-body","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"tile"}}},"name":"tile"}},
            {"id":"use-1","kind":{"$type":"FragmentRef","name":"tile"}}
        ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#,
    );
    // The decl is zero-paint; the ref expands with namespaced ids.
    assert!(html.contains("data-fuaran-node-id=\"use-1.frag-body\""));
    assert!(html.contains(">tile<") || html.contains("<p>tile</p>"));
}

// ─── Islands laws ────────────────────────────────────────────────────────────

fn islands_page() -> Node {
    node(
        r#"{"id":"page","kind":{"$type":"Box","children":[
            {"id":"static-md","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"inert prose"}}},
            {"id":"widget","kind":{"$type":"Button","label":{"$type":"Literal","text":"Refresh"},"onClick":{"$type":"Call","endpoint":"api/refresh","into":{"$type":"State","key":"r"}},"variant":"Primary"}}
        ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#,
    )
}

#[test]
fn zero_islands_is_byte_identical_to_plain_render() {
    let tree = islands_page();
    let sources = BindingSources::default();
    assert_eq!(
        render_with_islands(&tree, &sources, &[]),
        render_to_html(&tree, &sources)
    );
}

#[test]
fn islands_emit_boundary_wrapper_and_scoped_payload() {
    let tree = islands_page();
    let sources = BindingSources::default();
    let html = render_with_islands(&tree, &sources, &["widget"]);

    // One boundary wrapper + one scoped payload script.
    assert_eq!(html.matches("data-fuaran-island=\"widget\"").count(), 1);
    let script_id = island_script_id("widget");
    assert!(html.contains(&format!("id=\"{script_id}\"")));
    assert!(html.contains("data-fuaran-island-payload=\"widget\""));

    // The payload is the island subtree's canonical wire JSON (script-escaped).
    let island = match &tree.kind {
        fuaran_rs::wire::NodeKind::Box(spec) => spec.children[1].clone(),
        _ => unreachable!(),
    };
    let payload = encode_node(&island)
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    assert!(html.contains(&payload));

    // The boundary wrapper's children equal the island's plain static render.
    let plain_island = render_to_html(&island, &sources);
    assert!(html.contains(&format!(
        "<div data-fuaran-island=\"widget\">{plain_island}</div>"
    )));

    // The static remainder is byte-identical: the islands page before its
    // hydrate scripts equals the plain render with only the island's own
    // render lifted into the boundary wrapper.
    let plain_page = render_to_html(&tree, &sources);
    let script_start = html.find("<script").expect("hydrate script present");
    let expected = plain_page.replace(
        &plain_island,
        &format!("<div data-fuaran-island=\"widget\">{plain_island}</div>"),
    );
    assert_eq!(&html[..script_start], expected);
}

#[test]
fn hydratable_render_embeds_the_whole_tree() {
    let tree = islands_page();
    let sources = BindingSources::default();
    let html = render_hydratable(&tree, &sources);
    assert!(html.starts_with(&render_to_html(&tree, &sources)));
    let script_id = hydrate_script_id("page");
    assert!(html.contains(&format!("id=\"{script_id}\"")));
    assert!(html.contains("data-fuaran-hydrate-root=\"page\""));
    // No raw `<` survives inside the payload (script-breakout safety).
    let payload_start = html.find("<script").unwrap();
    let payload = &html[payload_start..];
    let inner_start = payload.find('>').unwrap() + 1;
    let inner_end = payload.rfind("</script>").unwrap();
    assert!(!payload[inner_start..inner_end].contains('<'));
}

// ─── Reference-host resolution (the parity oracles' shared seam) ─────────────

/// The spellings the F# reference host has shipped under. It was renamed once
/// (`fuaran` → `fuaran-dotnet`) and this file's path was not updated, so the
/// byte-parity oracle below silently returned early for as long as the rename was
/// old — a gate reporting success while checking nothing. Accepting both means a
/// rename in either direction cannot disable it again.
const REFERENCE_HOST_NAMES: &[&str] = &["fuaran-dotnet", "fuaran"];

/// Sibling hosts whose presence proves this is a cross-host checkout (the shape
/// the conformance gate builds) rather than a standalone clone. Excludes this
/// host and the reference host.
const OTHER_HOST_NAMES: &[&str] = &[
    "fuaran-ts",
    "fuaran-py",
    "fuaran-go",
    "fuaran-kt",
    "fuaran-swift",
];

/// Locates the F# reference host by walking up from the crate directory.
///
/// `None` means "genuinely standalone — skip", and that case is correct: it is
/// why the guard exists. What is NOT correct is skipping in a cross-host
/// checkout, where a missing reference host means the oracle has been silently
/// disabled — so that case **panics** instead, naming what was tried.
fn reference_host_root() -> Option<PathBuf> {
    let crate_dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    let mut dir = crate_dir.clone();
    loop {
        for name in REFERENCE_HOST_NAMES {
            if dir.join(name).join("src").is_dir() {
                return Some(dir.join(name));
            }
        }
        if !dir.pop() {
            break;
        }
    }
    // Not found anywhere up the tree. Is this a standalone clone, or a
    // cross-host checkout whose reference host moved?
    let mut dir = crate_dir.clone();
    loop {
        for sibling in OTHER_HOST_NAMES {
            if dir.join(sibling).is_dir() {
                panic!(
                    "cross-host checkout detected ({sibling}/ is present under {}) but the F# reference host is at \
                     none of {REFERENCE_HOST_NAMES:?} — the render-parity oracles cannot run. This is the failure \
                     mode this check exists for: if the sibling was renamed again, add the new spelling to \
                     REFERENCE_HOST_NAMES rather than letting the oracle skip.",
                    dir.display()
                );
            }
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

#[test]
fn reference_css_byte_copy_matches_the_reference_artefact() {
    let crate_dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    let local = crate_dir.join("css").join("fuaran.css");
    let local_bytes = std::fs::read(&local).expect("css/fuaran.css ships with the crate");
    let Some(root) = reference_host_root() else {
        eprintln!("reference stylesheet not found; skipping byte-parity (standalone checkout)");
        return;
    };
    let reference = root
        .join("src")
        .join("Fuaran.UI.Renderer")
        .join("content")
        .join("fuaran-reference.css");
    // NOT an early return — the host was located, so a missing stylesheet is a
    // moved artefact, not a standalone clone.
    let reference_bytes = std::fs::read(&reference).unwrap_or_else(|e| {
        panic!("canonical stylesheet missing inside the located reference host {reference:?}: {e}")
    });
    assert_eq!(
        local_bytes, reference_bytes,
        "css/fuaran.css must stay a byte-copy of the reference stylesheet"
    );
    eprintln!(
        "reference-CSS byte parity EXECUTED against {} ({} bytes)",
        reference.display(),
        reference_bytes.len()
    );
}

// ─── Render-time Transform resolution (Phase 649) — corpus parity ─────────────
//
// The server-HTML render carries the compute values a `Binding.Transform`
// yields at render time: the Phase 632 scalar path in scalar slots (Badge /
// Callout / Fact text), row-frame resolution in DataGrid / Chart row contexts,
// and Phase 629 Selection.defaultValue seeding of the pipeline params. These
// assertions fail loudly on divergence. The `wasm32` client renders through the
// very same `render_to_html`, so this certifies both consumption legs.

/// Locate `wire-format-fixtures/nodes/` by walking up from the crate dir;
/// `None` on a standalone checkout (the corpus lives in the sibling tree).
fn corpus_nodes_dir() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let root = dir.join("wire-format-fixtures");
        if root.join("manifest.json").is_file() {
            return Some(root.join("nodes"));
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn load_fixture(id: &str) -> Option<Node> {
    let path = corpus_nodes_dir()?.join(format!("{id}.json"));
    let raw = std::fs::read_to_string(path).expect("fixture file reads");
    Some(decode_node(&raw).expect("fixture decodes with the host codec"))
}

/// Find a descendant node by id (recursing through `Box` children — the shape
/// of these dashboard fixtures), so an assertion can scope to one sub-tree.
fn find_node<'a>(node: &'a Node, id: &str) -> Option<&'a Node> {
    if node.id == id {
        return Some(node);
    }
    if let NodeKind::Box(spec) = &node.kind {
        for child in &spec.children {
            if let Some(found) = find_node(child, id) {
                return Some(found);
            }
        }
    }
    None
}

fn render_sub(tree: &Node, id: &str) -> String {
    let node = find_node(tree, id).unwrap_or_else(|| panic!("fixture is missing node '{id}'"));
    render_to_html(node, &BindingSources::default())
}

#[test]
fn transform_scalar_slots_resolve_their_compute_values() {
    let Some(tree) = load_fixture("scalar-transform-composition") else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };

    // Badge label — a global-aggregate scalar Transform (filter severity ==
    // critical → groupBy keys [] count): the 1×1 result cell is the count 2.
    let badge = render_sub(&tree, "critical-count-badge");
    assert!(
        badge.contains("fuaran-badge-critical"),
        "badge tone class present: {badge}"
    );
    assert!(
        badge.contains(">2<"),
        "Badge label must resolve the critical count 2 (not empty / rows): {badge}"
    );

    // Callout body — a param-defaulted row-field lookup (Selection.defaultValue
    // 'TCK-2041' → filter id == param → project alert → limit 1).
    let callout = render_sub(&tree, "sla-warning");
    assert!(
        callout.contains("TCK-2041 breaches SLA in 2 hours"),
        "Callout body must resolve the defaulted row's alert text: {callout}"
    );

    // The DataGrid source (an embedded Transform, empty pipeline) resolves to
    // its rows in the row context.
    let grid = render_sub(&tree, "scalar-ticket-grid");
    for expected in ["TCK-2041", "TCK-2042", "TCK-2043", "critical", "high"] {
        assert!(
            grid.contains(expected),
            "grid must carry the resolved Transform row value '{expected}': {grid}"
        );
    }
}

#[test]
fn selection_default_seeds_master_detail() {
    let Some(tree) = load_fixture("master-detail-preselected") else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };

    // The detail Fact reads Selection.defaultValue (field 'id') before any real
    // selection — Phase 629.
    let detail = render_sub(&tree, "detail-ticket");
    assert!(
        detail.contains("TCK-2041"),
        "Fact must resolve the preselected Selection.defaultValue: {detail}"
    );

    // The related grid's Transform param is seeded from the same default, so it
    // filters to exactly the preselected ticket.
    let related = render_sub(&tree, "related-grid");
    assert!(
        related.contains("TCK-2041"),
        "related grid must carry the preselected ticket row: {related}"
    );
    assert!(
        !related.contains("TCK-2042"),
        "related grid must filter to the preselected ticket only (Selection-seeded param): {related}"
    );
}

/// The second-row fixture exists to make PRUNE-vs-SEED observable, and until
/// this test it did not: this host decoded and re-encoded it byte-identically,
/// which a host that *pruned* rather than *seeded* would also do. `render.rs`
/// loads its fixtures BY NAME and this one was not among them, so the fixture
/// was decorative here — carried by the corpus, asserted by nothing.
///
/// The discriminator is the SECOND row. Its `Selection.defaultValue` is
/// `TCK-2042`, so a host that seeds from the default resolves to the second
/// row's values, while a host that prunes the unwritten selection resolves to
/// all three rows (or none) — and the first-row fixture beside it cannot tell
/// those apart, because row one is what you get either way.
#[test]
fn selection_default_seeds_master_detail_second_row() {
    let Some(tree) = load_fixture("master-detail-preselected-second-row") else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };

    // The detail Fact reads Selection.defaultValue (field 'id') — the SECOND
    // row's ticket, not the first.
    let detail = render_sub(&tree, "detail-ticket");
    assert!(
        detail.contains("TCK-2042"),
        "Fact must resolve the second row's Selection.defaultValue: {detail}"
    );
    assert!(
        !detail.contains("TCK-2041"),
        "Fact must NOT fall back to the first row — that is the prune behaviour this fixture discriminates: {detail}"
    );

    // The Callout body is a Transform seeded from the same default: filter on
    // the param, project the note column, limit 1. Its resolved cell is the
    // second row's note.
    let note = render_sub(&tree, "detail-note");
    assert!(
        note.contains("Search index stale"),
        "Callout body must resolve the second row's note: {note}"
    );

    // The related grid's Transform param is seeded from the same default, so it
    // filters to EXACTLY the preselected ticket — one row. Counted rather than
    // substring-checked: "contains TCK-2042" would also pass on an unpruned
    // three-row grid, which is the very confusion this fixture was authored to
    // resolve.
    //
    // The class is `fuaran-grid-row`, the BOUND-grid row: `fuaran-table-row` is
    // the `staticRows` leg and matches zero times here, so reaching for it
    // would have made this assertion fail for the wrong reason.
    let related = render_sub(&tree, "related-grid");
    assert_eq!(
        related.matches("fuaran-grid-row").count(),
        1,
        "related grid must render exactly one row (the Selection-seeded param): {related}"
    );
    assert!(
        related.contains("TCK-2042")
            && !related.contains("TCK-2041")
            && !related.contains("TCK-2043"),
        "the single related row must be the preselected ticket: {related}"
    );
}

#[test]
fn unset_filter_params_prune_and_resolve_all_rows() {
    let Some(tree) = load_fixture("filterable-static-dashboard") else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };

    // No filter is written, so both Filter-sourced params are unbound; the
    // filter steps referencing them are pruned ("unset filter ⇒ no
    // constraint"), leaving every row — both retention values render.
    let grid = render_sub(&tree, "episode-grid");
    assert!(
        grid.contains("0.62") && grid.contains("0.55"),
        "unset filters must prune to all rows (both retention values): {grid}"
    );

    // The Chart source resolves the same way and lowers to inline SVG.
    let chart = render_sub(&tree, "retention-chart");
    assert!(
        chart.contains("fuaran-chart") && chart.contains("<svg"),
        "chart must resolve its Transform rows and lower to inline SVG: {chart}"
    );
}

// ─── Resolved projection (Phase 650) ─────────────────────────────────────────
//
// The session-level resolved projection folds scalar-slot Transform resolution
// into the wire tree itself, so a decode-only consumer renders resolved compute
// values without an evaluator. These tests assert the projection STRUCTURALLY
// (the scalar Transform slots become literals / Static numbers) and that the
// existing `tree_json` entry point is byte-unchanged (the additive contract).

fn load_fixture_json(id: &str) -> Option<String> {
    let path = corpus_nodes_dir()?.join(format!("{id}.json"));
    Some(std::fs::read_to_string(path).expect("fixture file reads"))
}

fn projected_node(session: &ClientSession, id: &str) -> Node {
    let tree = decode_node(&session.project_resolved()).expect("resolved projection re-decodes");
    find_node(&tree, id)
        .unwrap_or_else(|| panic!("projection is missing node '{id}'"))
        .clone()
}

#[test]
fn resolved_projection_folds_scalar_transforms_to_literals() {
    let Some(raw) = load_fixture_json("scalar-transform-composition") else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let session = ClientSession::new(&raw).expect("fixture decodes to a session");

    // The additive contract: tree_json is byte-identical to a straight
    // decode→encode round-trip — the projection is a SEPARATE entry point.
    assert_eq!(
        session.tree_json(),
        encode_node(&node(&raw)),
        "project_resolved must not perturb the raw tree_json round-trip"
    );

    // Badge label — a global-aggregate scalar Transform → the 1×1 count cell 2.
    match &projected_node(&session, "critical-count-badge").kind {
        NodeKind::Badge(spec) => assert_eq!(
            spec.label,
            fuaran_rs::wire::TextSource::Literal("2".to_string()),
            "Badge label Transform must fold to the literal count 2"
        ),
        other => panic!("expected a Badge, got {}", other.type_name()),
    }

    // Callout body — a param-defaulted row-field lookup → the defaulted alert.
    match &projected_node(&session, "sla-warning").kind {
        NodeKind::Callout(spec) => assert_eq!(
            spec.body,
            fuaran_rs::wire::TextSource::Literal("TCK-2041 breaches SLA in 2 hours".to_string()),
            "Callout body Transform must fold to the defaulted row's alert text"
        ),
        other => panic!("expected a Callout, got {}", other.type_name()),
    }

    // The DataGrid source is a ROW-context Transform (a collection) — it is left
    // as a Transform, never folded to a scalar literal.
    match &projected_node(&session, "scalar-ticket-grid").kind {
        NodeKind::DataGrid(spec) => assert!(
            matches!(spec.source, fuaran_rs::wire::Binding::Transform { .. }),
            "a row-context Transform source stays a Transform in the projection"
        ),
        other => panic!("expected a DataGrid, got {}", other.type_name()),
    }
}

#[test]
fn resolved_projection_preserves_selection_defaults() {
    let Some(raw) = load_fixture_json("master-detail-preselected") else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let session = ClientSession::new(&raw).expect("fixture decodes to a session");

    // The detail Fact value is a Selection(defaultValue 'TCK-2041') — NOT a
    // Transform — so the projection leaves it byte-identical; the decode-only
    // consumer resolves the seeded Selection default itself. The projected tree
    // still renders TCK-2041.
    match &projected_node(&session, "detail-ticket").kind {
        NodeKind::Fact(spec) => match &spec.value {
            fuaran_rs::wire::TextSource::Bound(b) => assert!(
                matches!(**b, fuaran_rs::wire::Binding::Selection { .. }),
                "the Selection-bound Fact value stays a Selection binding"
            ),
            other => panic!("expected a Bound Selection Fact value, got {other:?}"),
        },
        other => panic!("expected a Fact, got {}", other.type_name()),
    }

    // The related grid's Transform param is seeded from the same Selection
    // default; its source stays a (row-context) Transform in the projection.
    let projected = decode_node(&session.project_resolved()).expect("projection re-decodes");
    assert!(
        find_node(&projected, "related-grid").is_some(),
        "the related grid survives the projection"
    );
}

#[test]
fn date_range_renders_two_variant_typed_inputs_over_the_pair() {
    // 0.7.0 — the single-control date range. Mirrors the reference SERVER
    // renderer: a `fuaran-field-range` span over two variant-typed inputs and a
    // separator, where only the FROM end carries `data-fuaran-field` (the pair's
    // addressable slot) and both ends share the min/max/step constraints.
    let html = render(
        r#"{"id":"f1","kind":{"$type":"Form","fields":[{"id":"stay","kind":{"$type":"DateRange","max":"2026-12-31","min":"2026-01-01","value":{"from":"2026-03-01","to":"2026-03-08"},"variant":"Date"},"label":"Stay","required":true}],"onSubmit":{"$type":"Dispatch"},"submitLabel":"Book"}}"#,
    );
    assert!(
        html.contains("<span class=\"fuaran-field-range\">"),
        "{html}"
    );
    assert!(
        html.contains("fuaran-form-field-control fuaran-field-range-min"),
        "{html}"
    );
    assert!(
        html.contains("fuaran-form-field-control fuaran-field-range-max"),
        "{html}"
    );
    assert!(html.contains("fuaran-field-range-sep"), "{html}");
    // The resolved pair reaches both ends, in order.
    assert!(html.contains("value=\"2026-03-01\""), "{html}");
    assert!(html.contains("value=\"2026-03-08\""), "{html}");
    // Only the FROM end is addressable.
    assert_eq!(
        html.matches("data-fuaran-field=\"stay\"").count(),
        1,
        "{html}"
    );
    // Shared constraints ride BOTH ends.
    assert_eq!(html.matches("min=\"2026-01-01\"").count(), 2, "{html}");
    assert_eq!(html.matches("max=\"2026-12-31\"").count(), 2, "{html}");
    // Variant drives the native input type.
    assert_eq!(html.matches("type=\"date\"").count(), 2, "{html}");
}

#[test]
fn date_range_variants_select_their_native_input_type() {
    for (variant, expected) in [("Time", "time"), ("DateTime", "datetime-local")] {
        let html = render(&format!(
            r#"{{"id":"f1","kind":{{"$type":"Form","fields":[{{"id":"w","kind":{{"$type":"DateRange","variant":"{variant}"}},"label":"W","required":false}}],"onSubmit":{{"$type":"Dispatch"}},"submitLabel":"Go"}}}}"#
        ));
        assert_eq!(
            html.matches(&format!("type=\"{expected}\"")).count(),
            2,
            "{variant}: {html}"
        );
    }
}

// ─── Class-vocabulary parity ─────────────────────────────────────────────────

/// The rendering analogue of the wire corpus: every `fuaran-*` class this host
/// emits must appear in the reference renderer's vocabulary.
///
/// This host had **no such oracle at all** — Phase 747 found the Go one dormant
/// (a stale sibling path) and assumed the same hid Rust's known class
/// divergence. It did not: there was nothing to be dormant. Added here so the
/// divergence is measured rather than asserted, and so a future wrong class name
/// in this host is caught by a gate instead of by review.
/// Extracts (exact classes, composition prefixes) from the reference renderer
/// sources. `None` only on a genuine standalone checkout.
fn reference_vocabulary() -> Option<(std::collections::BTreeSet<String>, Vec<String>)> {
    let root = reference_host_root()?;
    let sources = [
        root.join("src")
            .join("Fuaran.UI.Renderer.Server")
            .join("Render.fs"),
        root.join("src")
            .join("Fuaran.UI.Renderer")
            .join("Render.fs"),
        root.join("src")
            .join("Fuaran.UI.Renderer.Core")
            .join("Theme.fs"),
        root.join("src")
            .join("Fuaran.UI.Renderer.Core")
            .join("DrawingSvg.fs"),
        // Phase 1128 — `Css.fs` was MISSING from this list, and its absence made
        // the oracle under-report the reference vocabulary by 36 tokens.
        //
        // It is the file the reference EXTRACTED its class composition into
        // precisely so the spellings could not drift ("each spell
        // `\"fuaran-metric fuaran-metric-\" + tone` inline can drift by a …"), so
        // omitting it dropped `fuaran-badge`, `fuaran-metric`, `fuaran-toast`,
        // `fuaran-layout-stack` and thirty-odd more out of the set this host is
        // measured against — every one of them a class the reference does emit.
        //
        // The gate never SAID so, because it was masked: the walk panicked on the
        // first fixture carrying a kind this host had not adopted, so it never
        // reached its own offender assertion. Adopting the kind is what surfaced
        // the stale list, which is the class of finding the manifest-driven
        // obligation suite next door exists to make loud rather than incidental.
        root.join("src")
            .join("Fuaran.UI.Renderer.Core")
            .join("Css.fs"),
        // Markdown.fs and MathMl.fs likewise compose emitted classes for the two
        // sub-renderers whose output this host also emits.
        root.join("src")
            .join("Fuaran.UI.Renderer.Core")
            .join("Markdown.fs"),
        root.join("src")
            .join("Fuaran.UI.Renderer.Core")
            .join("MathMl.fs"),
    ];

    // A token ending in '-' is a composition prefix (fuaran-metric- styles
    // fuaran-metric-brand). The BARE namespace is not: it occurs in the
    // reference sources as a string-concatenation fragment, and admitting it
    // would match every class this oracle collects, making the whole check
    // vacuous — the exact defect the Go twin was carrying.
    let mut exact: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut prefixes: Vec<String> = Vec::new();
    for path in &sources {
        let raw = std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!("reference renderer source missing inside the located host {path:?}: {e}")
        });
        for token in class_tokens(&raw) {
            if token.ends_with('-') {
                if token != "fuaran-" {
                    prefixes.push(token);
                }
            } else {
                exact.insert(token);
            }
        }
    }
    assert!(
        exact.len() > 50 && exact.contains("fuaran-node"),
        "reference vocabulary extraction looks broken: {} exact classes",
        exact.len()
    );
    Some((exact, prefixes))
}

#[test]
fn emitted_class_vocabulary_matches_the_reference_renderer() {
    let Some((exact, prefixes)) = reference_vocabulary() else {
        eprintln!("reference renderer not found; skipping class parity (standalone checkout)");
        return;
    };
    let in_vocab = |cls: &str| exact.contains(cls) || prefixes.iter().any(|p| cls.starts_with(p));

    let Some(corpus) = find_corpus_dir() else {
        eprintln!("corpus not found; skipping class parity");
        return;
    };
    let manifest_raw =
        std::fs::read_to_string(corpus.join("manifest.json")).expect("reading manifest");
    let manifest = parse(&manifest_raw).expect("manifest parses");
    let JVal::Arr(fixtures) = manifest.field("fixtures").expect("fixtures array") else {
        panic!("manifest fixtures is not an array");
    };

    let mut ran = 0usize;
    let mut checked = 0usize;
    let mut offenders: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for fx in fixtures {
        let Some(JVal::Str(kind)) = fx.field("kind") else {
            continue;
        };
        if kind != "node-round-trip" {
            continue;
        }
        let Some(JVal::Str(input)) = fx.field("inputFile") else {
            continue;
        };
        let text = std::fs::read_to_string(corpus.join(input)).expect("reading fixture");
        let tree = decode_node(text.trim_end()).expect("fixture decodes");
        let html = render_to_html(&tree, &BindingSources::default());
        ran += 1;
        for cls in emitted_classes(&html) {
            checked += 1;
            if !in_vocab(&cls) {
                offenders.insert(cls);
            }
        }
    }

    // A pass proves nothing unless the oracle looked at something.
    assert!(
        ran > 0 && checked > 0,
        "class-vocabulary oracle checked nothing ({ran} fixtures, {checked} class occurrences)"
    );
    eprintln!(
        "class-vocabulary parity EXECUTED: {ran} fixtures, {checked} emitted-class occurrences, {} reference classes",
        exact.len()
    );
    assert!(
        offenders.is_empty(),
        "emitted classes absent from the reference renderer vocabulary: {offenders:?}"
    );
}

fn class_tokens(source: &str) -> Vec<String> {
    // Equivalent of the Go twin's `fuaran-[a-zA-Z0-9-]*`, hand-scanned to keep
    // the crate dependency-free (regex is not a dependency here, by mandate).
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = source[i..].find("fuaran-") {
        let start = i + rel;
        let mut end = start + "fuaran-".len();
        while end < bytes.len() {
            let c = bytes[end] as char;
            if c.is_ascii_alphanumeric() || c == '-' {
                end += 1;
            } else {
                break;
            }
        }
        out.push(source[start..end].to_string());
        i = end.max(start + 1);
    }
    out
}

fn emitted_classes(html: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut rest = html;
    while let Some(idx) = rest.find("class=\"") {
        let after = &rest[idx + 7..];
        let Some(end) = after.find('"') else { break };
        for tok in after[..end].split_whitespace() {
            if tok.starts_with("fuaran-") {
                out.insert(tok.to_string());
            }
        }
        rest = &after[end..];
    }
    out
}

fn find_corpus_dir() -> Option<PathBuf> {
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

/// The corpus alone does NOT measure the form-control class vocabulary: it
/// carries `Range` only through the `Filters` carrier (one fixture,
/// `filters-declarative`), and a filter chip renders through a different arm with
/// a different — and already correct — `fuaran-filter-range*` vocabulary. So the
/// form-field `Range` arm's divergent `fuaran-form-range*` spelling was invisible
/// to every gate, including the corpus-driven oracle above.
///
/// This drives a synthetic Form carrying every `FormFieldKind` through
/// `render_form_control` and holds the emitted classes to the same reference
/// vocabulary. Phase 747 — the finding that motivated it is that a parity oracle
/// is only as good as the render paths its inputs reach.
#[test]
fn form_control_class_vocabulary_matches_the_reference_renderer() {
    let Some((exact, prefixes)) = reference_vocabulary() else {
        eprintln!("reference renderer not found; skipping (standalone checkout)");
        return;
    };
    let in_vocab = |cls: &str| exact.contains(cls) || prefixes.iter().any(|p| cls.starts_with(p));

    // One field per control kind — the whole FormFieldKind vocabulary.
    let fields = [
        r#"{"id":"f-text","kind":{"$type":"Text"},"label":"T","required":false}"#,
        r#"{"id":"f-num","kind":{"$type":"Number"},"label":"N","required":false}"#,
        r#"{"id":"f-check","kind":{"$type":"Checkbox"},"label":"C","required":false}"#,
        r#"{"id":"f-toggle","kind":{"$type":"Toggle"},"label":"Tg","required":false}"#,
        r#"{"id":"f-choice","kind":{"$type":"Choice","options":[{"label":"A","value":"a"}]},"label":"Ch","required":false}"#,
        r#"{"id":"f-seg","kind":{"$type":"SegmentedChoice","options":[{"label":"A","value":"a"}],"orientation":"Horizontal"},"label":"S","required":false}"#,
        r#"{"id":"f-area","kind":{"$type":"TextArea","rows":3},"label":"A","required":false}"#,
        r#"{"id":"f-rnum","kind":{"$type":"RangedNumber","max":10,"min":0},"label":"R","required":false}"#,
        r#"{"id":"f-range","kind":{"$type":"Range","max":10,"min":0,"value":{"max":8,"min":2}},"label":"Rg","required":false}"#,
        r#"{"id":"f-date","kind":{"$type":"Date","variant":"Date"},"label":"D","required":false}"#,
        r#"{"id":"f-drange","kind":{"$type":"DateRange","value":{"from":"2026-03-01","to":"2026-03-08"},"variant":"Date"},"label":"DR","required":false}"#,
    ];
    let json = format!(
        r#"{{"id":"form","kind":{{"$type":"Form","fields":[{}],"onSubmit":{{"$type":"Dispatch"}},"submitLabel":"Go"}}}}"#,
        fields.join(",")
    );
    let html = render(&json);

    let emitted = emitted_classes(&html);
    assert!(
        emitted.len() > 15,
        "synthetic form emitted too few classes ({}) — it is not reaching the control arms",
        emitted.len()
    );
    let offenders: Vec<&String> = emitted.iter().filter(|c| !in_vocab(c)).collect();
    eprintln!(
        "form-control class parity EXECUTED: {} distinct classes over {} control kinds",
        emitted.len(),
        fields.len()
    );
    assert!(
        offenders.is_empty(),
        "form-control classes absent from the reference renderer vocabulary: {offenders:?}"
    );
}

#[test]
fn drawing_label_rotation_anchors_at_the_label_position() {
    // Phase 877 — the rotation transform pivots on the label's OWN (x, y), so it
    // composes with `text_anchor` rather than fighting it, and the numbers use
    // the shared canonical form. Byte-for-byte the strings the F# reference
    // emitter produces for the same shapes: the corpus is the oracle for the
    // codec, and this is the emission half it does not cover.
    let html = render(
        r#"{"id":"d","kind":{"$type":"Drawing","shapes":[
            {"$type":"Label","style":{"rotation":-30},"text":"Q1","x":30,"y":100},
            {"$type":"Label","style":{"rotation":12.34},"text":"F","x":110,"y":100},
            {"$type":"Label","style":{"rotation":0},"text":"Z","x":150,"y":100},
            {"$type":"Label","style":{},"text":"U","x":100,"y":20}
        ],"style":{},"viewBox":{"height":120,"minX":0,"minY":0,"width":200}}}"#,
    );

    assert!(
        html.contains(
            "<text class=\"fuaran-drawing-label\" x=\"30\" y=\"100\" transform=\"rotate(-30 30 100)\""
        ),
        "rotation not anchored at the label position:\n{html}"
    );
    assert!(html.contains("transform=\"rotate(12.34 110 100)\""));
    // An explicit 0 is a PRESENT value and must still emit — absent and zero are
    // different wire shapes, and a renderer that conflates them re-introduces
    // downstream the distinction the codec is careful to preserve.
    assert!(
        html.contains("transform=\"rotate(0 150 100)\""),
        "explicit zero must still emit:\n{html}"
    );
    // The unrotated label carries no transform — the byte-unchanged guarantee
    // for every pre-877 drawing.
    assert!(html.contains("<text class=\"fuaran-drawing-label\" x=\"100\" y=\"20\">U</text>"));
    assert_eq!(html.matches("transform=\"rotate(").count(), 3);
}

#[test]
fn drawing_rotation_is_inert_off_label() {
    // Unlike the other text-only `DrawStyle` fields, an SVG `transform` on a
    // `<rect>` would MOVE GEOMETRY rather than be ignored — so uniform emission
    // would silently distort drawings. The emitter writes it only for `Label`.
    let html = render(
        r#"{"id":"d","kind":{"$type":"Drawing","shapes":[
            {"$type":"Rectangle","height":10,"style":{"rotation":45},"width":10,"x":0,"y":0},
            {"$type":"Circle","cx":5,"cy":5,"r":2,"style":{"rotation":45}}
        ],"style":{},"viewBox":{"height":100,"minX":0,"minY":0,"width":100}}}"#,
    );

    assert!(
        !html.contains("transform="),
        "rotation must be inert on non-Label shapes:\n{html}"
    );
}

// ── Phase 921 — the drawing root's ANNOUNCED accessible name ─────────────────
//
// `role="img"` (Phase 532's R3) presents the drawing as ONE graphic and does not
// traverse into it, and `<desc>` is not uniformly mapped to the accessible
// description (Chromium has never exposed it) — so the description the markup
// has carried since Phase 525 was a value no reader could reach. The root now
// emits `aria-label` composing the title and the description. Byte-parity with
// the F# builder's `DrawingSvgTests` block of the same name.

fn drawing_root(title: Option<&str>, description: Option<&str>) -> String {
    let mut fields = String::new();
    if let Some(t) = title {
        fields.push_str(&format!(r#","title":"{t}""#));
    }
    if let Some(d) = description {
        fields.push_str(&format!(r#","description":"{d}""#));
    }
    render(&format!(
        r#"{{"id":"d","kind":{{"$type":"Drawing","shapes":[],"style":{{}},"viewBox":{{"height":100,"minX":0,"minY":0,"width":200}}{fields}}}}}"#
    ))
}

#[test]
fn drawing_root_composes_title_and_description_into_aria_label() {
    let html = drawing_root(
        Some("Sales vs target"),
        Some("Bar chart. 2 series: sales, target."),
    );
    assert!(
        html.contains(concat!(
            r#"<svg class="fuaran-drawing" role="img" viewBox="0 0 200 100" "#,
            r#"aria-label="Sales vs target. Bar chart. 2 series: sales, target.">"#,
            "<title>Sales vs target</title>",
            "<desc>Bar chart. 2 series: sales, target.</desc></svg>"
        )),
        "root wiring drifted:\n{html}"
    );
}

#[test]
fn drawing_root_terminates_the_title_only_when_needed() {
    assert!(
        drawing_root(Some("Ends in a period."), Some("D."))
            .contains(r#"aria-label="Ends in a period. D.""#)
    );
    assert!(drawing_root(Some("Really?"), Some("D.")).contains(r#"aria-label="Really? D.""#));
    assert!(drawing_root(Some("Now!"), Some("D.")).contains(r#"aria-label="Now! D.""#));
    assert!(drawing_root(Some("Plain"), Some("D.")).contains(r#"aria-label="Plain. D.""#));
    // An EMPTY title contributes nothing rather than a bare period.
    assert!(drawing_root(Some(""), Some("D.")).contains(r#"aria-label="D.""#));
}

#[test]
fn drawing_root_without_a_description_is_byte_identical_to_pre_921() {
    let titled = drawing_root(Some("Bars"), None);
    assert!(
        titled.contains(
            r#"<svg class="fuaran-drawing" role="img" viewBox="0 0 200 100"><title>Bars</title></svg>"#
        ),
        "a title-only root gained an attribute:\n{titled}"
    );
    assert!(!drawing_root(None, None).contains("aria-label"));
}

#[test]
fn drawing_root_announces_a_description_only_root_on_its_own() {
    let html = drawing_root(None, Some("One filled circle."));
    assert!(
        html.contains(concat!(
            r#"<svg class="fuaran-drawing" role="img" viewBox="0 0 200 100" aria-label="One filled circle.">"#,
            "<desc>One filled circle.</desc></svg>"
        )),
        "description-only wiring drifted:\n{html}"
    );
}

#[test]
fn drawing_root_escapes_hostile_text_inside_the_aria_label_attribute() {
    // The builder emits raw markup, so its own XML escape is the whole defence —
    // and an attribute value needs the quote entities the element-content path
    // also emits. The chart lowering feeds this seam untrusted series and
    // category strings straight off the data feed.
    let html = drawing_root(
        Some(r#"a\"b"#),
        Some(r#"<script>alert('x') & \"y\"</script>"#),
    );
    assert!(
        html.contains(
            r#"aria-label="a&quot;b. &lt;script&gt;alert(&#39;x&#39;) &amp; &quot;y&quot;&lt;/script&gt;""#
        ),
        "hostile text is not fully escaped in the attribute:\n{html}"
    );
    assert!(!html.contains("<script>"));
}

// ─── The a11y projection, driven by the SHARED CORPUS (Phase 956) ────────────
//
// `accessibility_projects_onto_the_semantic_element` above asserts WHERE the
// projection lands, but every node in it is hand-built in this file — so it
// measures this host against this host's own idea of the trait. The Phase-955
// fixture family is the oracle every host answers to: all six slots, both role
// classes (a named lower-case `region` and a deliberately-cased custom
// `doc-pageFooter`), both binding forms (Static and State), all three
// `liveRegion` tokens, and both placement shapes.
//
// Placement-sensitive, for the reason recorded above the placement block.

/// One fixture's expectation: the element carrying the projection (`None` for
/// the wrapper), what its own open tag must contain, and what must not appear.
struct A11yCorpusCase {
    fixture: &'static str,
    element: Option<&'static str>,
    want: &'static [&'static str],
    absent_from_carrier: &'static [&'static str],
}

const A11Y_CORPUS: &[A11yCorpusCase] = &[
    // All six slots at once on an ordinary wrapper kind. `hidden` is an
    // explicit Static FALSE — distinct on the wire from omitted, and it must
    // emit nothing (`aria-hidden` is not a tri-state).
    A11yCorpusCase {
        fixture: "a11y-wrapper-all-slots",
        element: None,
        want: &[
            r#"aria-label="Channel performance summary""#,
            r#"aria-labelledby="a11y-wrapper-heading""#,
            r#"aria-describedby="a11y-wrapper-note""#,
            r#"role="region""#,
            r#"aria-live="polite""#,
        ],
        absent_from_carrier: &["aria-hidden"],
    },
    // The State forms. `label` resolves through its declared `defaultValue`
    // with no host sources (the reference host's default law); the custom
    // role's CASE is carried verbatim — the exact spelling a fold bug once
    // rewrote — and `off` is a real `liveRegion` token, not an absence.
    A11yCorpusCase {
        fixture: "a11y-wrapper-state-bound",
        element: None,
        want: &[
            r#"aria-label="Site footer""#,
            r#"role="doc-pageFooter""#,
            r#"aria-live="off""#,
        ],
        absent_from_carrier: &["aria-hidden"],
    },
    A11yCorpusCase {
        fixture: "a11y-alert-assertive",
        element: None,
        want: &[r#"role="alert""#, r#"aria-live="assertive""#],
        absent_from_carrier: &[],
    },
    // D4 forwarding: the body IS the semantic element. The accessible name
    // OVERRIDES the visible "Read more".
    A11yCorpusCase {
        fixture: "a11y-link-labelled",
        element: Some("a"),
        want: &[r#"aria-label="Read the 2026 annual report (PDF)""#],
        absent_from_carrier: &[],
    },
    A11yCorpusCase {
        fixture: "a11y-button-named",
        element: Some("button"),
        want: &[
            r#"aria-label="Refresh revenue figures""#,
            r#"role="button""#,
        ],
        absent_from_carrier: &[],
    },
    // The decorative shape: empty alt + `hidden` Static TRUE — the slot two
    // hosts dropped entirely before the Phase 951 port.
    A11yCorpusCase {
        fixture: "a11y-image-decorative",
        element: Some("img"),
        want: &[r#"aria-hidden="true""#],
        absent_from_carrier: &[],
    },
];

#[test]
fn a11y_corpus_projection_lands_on_the_right_element() {
    if corpus_nodes_dir().is_none() {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    }
    // A table-driven leg that silently enumerated nothing would be a gate that
    // checked nothing.
    assert_eq!(
        A11Y_CORPUS.len(),
        6,
        "the Phase 955 node family is six fixtures"
    );

    for case in A11Y_CORPUS {
        let tree = load_fixture(case.fixture).expect("the corpus is present");
        let html = render_to_html(&tree, &BindingSources::default());
        let wrapper = wrapper_tag(&html);
        let carrier = match case.element {
            None => wrapper,
            Some(tag) => open_tag(&html, tag),
        };

        for want in case.want {
            assert!(
                carrier.contains(want),
                "{}: carrier missing {want}: {carrier}",
                case.fixture
            );
        }
        for absent in case.absent_from_carrier {
            assert!(
                !carrier.contains(absent),
                "{}: carrier must not emit {absent}: {carrier}",
                case.fixture
            );
        }
        // A forwarding kind must not leave the projection behind.
        if case.element.is_some() {
            for want in case.want {
                let attr = &want[..want.find('=').expect("an attribute assertion")];
                assert!(
                    !wrapper.contains(attr),
                    "{}: {attr} leaked onto the wrapper: {wrapper}",
                    case.fixture
                );
            }
        }
        // The wrapper keeps the node's ADDRESS whichever element carries the
        // projection.
        assert!(
            wrapper.contains(&format!(r#"data-fuaran-node-id="{}""#, tree.id)),
            "{}: the wrapper must keep the node address: {wrapper}",
            case.fixture
        );
    }
}

// ─── Media-vocabulary render obligations (§3.6.2–§3.6.6) ─────────────────────
//
// These are the claims a host can satisfy the BYTES of and still get wrong.
// Every one of them is stated NORMATIVELY in the wire specification precisely
// because a codec-conformant host that broke it would round-trip the corpus
// perfectly — so the corpus cannot be the oracle for any of them, and this is
// where they are pinned instead.

/// §3.6.6 — `aria-label` ALWAYS. The label is mandatory on the wire and has no
/// decorative case, so unlike `Image`'s `alt` there is no branch to take.
#[test]
fn media_always_carries_an_accessible_name() {
    for json in [
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Video"},"label":"Studio walkthrough","src":{"$type":"Static","value":"/w.mp4"}}}"#,
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Audio"},"label":"Curator commentary","src":{"$type":"Static","value":"/c.mp3"}}}"#,
    ] {
        let html = render(json);
        assert!(
            html.contains(r#"aria-label=""#),
            "a transport is never decorative: {html}"
        );
    }
    // A node-level a11y label OVERRIDES the spec's own, and the element carries
    // exactly ONE accessible name — this host serialises attributes to text,
    // where a duplicate resolves first-wins rather than by a props merge, so
    // emitting both would silently invert that precedence instead of applying
    // it.
    let overridden = render(&format!(
        r#"{{"id":"m","kind":{{"$type":"Media","kind":{{"$type":"Audio"}},"label":"Spec name","src":{{"$type":"Static","value":"/c.mp3"}}}},{A11Y}}}"#
    ));
    let tag = open_tag(&overridden, "audio");
    assert!(tag.contains(r#"aria-label="Home""#), "{tag}");
    assert!(
        !tag.contains("Spec name"),
        "one accessible name only: {tag}"
    );
    assert_eq!(tag.matches("aria-label").count(), 1, "{tag}");
}

/// §3.6.6 — `autoplay` NEVER without `muted`, and never `muted` without
/// `autoplay`. The pairing is not a default a caller overrides; it is what the
/// declaration MEANS, which is why the wire carries no separate `muted` slot to
/// fall out of step with it. Every mainstream browser blocks unmuted autoplay,
/// so an unmuted emission would produce a player that silently never starts —
/// the declaration would be a lie and the failure would be invisible.
#[test]
fn autoplay_is_never_emitted_without_muted() {
    let declared = render(
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Video","autoplay":true},"label":"Ambient loop","src":{"$type":"Static","value":"/a.mp4"}}}"#,
    );
    let tag = open_tag(&declared, "video");
    assert!(tag.contains(" autoplay"), "{tag}");
    assert!(tag.contains(" muted"), "autoplay without muted: {tag}");

    // The converse holds too: muting a video the reader pressed play on is a
    // defect of the same family in the other direction.
    let undeclared = render(
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Video"},"label":"Studio walkthrough","src":{"$type":"Static","value":"/w.mp4"}}}"#,
    );
    let tag = open_tag(&undeclared, "video");
    assert!(!tag.contains(" autoplay"), "{tag}");
    assert!(!tag.contains(" muted"), "muted without autoplay: {tag}");
}

/// §3.6.6 — `Audio` has NO autoplay pathway: in the type, on the wire, or in
/// the emission. Stronger than a default of `false` — a slot that defaults to
/// off is one a document can switch on, and there is no document this format
/// wants to be able to state in which a page begins making sound unbidden. The
/// `Audio` case declares no such slot, so an `autoplay` member on the wire has
/// nowhere to land.
#[test]
fn audio_has_no_autoplay_pathway_even_when_the_wire_states_one() {
    let json = r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Audio","autoplay":true},"label":"Commentary","src":{"$type":"Static","value":"/c.mp3"}}}"#;
    let html = render(json);
    assert!(!html.contains("autoplay"), "{html}");
    assert!(!html.contains("muted"), "{html}");
    // And it does not survive the round trip either — there is no slot for it,
    // so re-encoding drops it rather than preserving a declaration no renderer
    // will honour.
    assert!(!encode_node(&node(json)).contains("autoplay"));
}

/// §3.6.6 — both URLs cross the destination floor, and they differ in what a
/// REFUSAL means. An element must have a source, so `src` collapses to the
/// refusal URL and carries its marker; a poster simply leaves, because a
/// `<video>` with no poster shows its first frame — a working rendering —
/// whereas a poster pointing at the refusal URL is a broken image painted over
/// the player.
#[test]
fn a_refused_poster_is_dropped_where_a_refused_src_collapses() {
    let html = render(
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Video","poster":{"$type":"Static","value":"https://collector.example/p.jpg"}},"label":"Walkthrough","src":{"$type":"Static","value":"https://collector.example/w.mp4"}}}"#,
    );
    assert!(
        html.contains(r#"src="about:blank#fuaran-egress-refused""#),
        "{html}"
    );
    assert!(html.contains("data-fuaran-egress-refused"), "{html}");
    assert!(
        !html.contains("poster="),
        "a refused poster is dropped: {html}"
    );

    // Permitted, the poster is emitted normally — so the assertion above is
    // about the refusal, not about posters never being emitted at all.
    let allowed = render_permissive(
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Video","poster":{"$type":"Static","value":"https://cdn.example/p.jpg"}},"label":"Walkthrough","src":{"$type":"Static","value":"https://cdn.example/w.mp4"}}}"#,
    );
    assert!(
        allowed.contains(r#"poster="https://cdn.example/p.jpg""#),
        "{allowed}"
    );
}

/// §3.6.6 — `controls` omits at TRUE and `loop` at `false`, the two opposite
/// polarities, and this is the RENDERED side of that pair: a document that says
/// nothing gets the accessible setting, and only taking the transport away
/// costs a key.
#[test]
fn media_transport_defaults_to_present_controls() {
    let quiet = render(
        r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Audio"},"label":"Commentary","src":{"$type":"Static","value":"/c.mp3"}}}"#,
    );
    let tag = open_tag(&quiet, "audio");
    assert!(tag.contains(" controls"), "{tag}");
    assert!(!tag.contains(" loop"), "{tag}");

    let stated = render(
        r#"{"id":"m","kind":{"$type":"Media","controls":false,"kind":{"$type":"Audio"},"label":"Commentary","loop":true,"src":{"$type":"Static","value":"/c.mp3"}}}"#,
    );
    let tag = open_tag(&stated, "audio");
    assert!(!tag.contains(" controls"), "{tag}");
    assert!(tag.contains(" loop"), "{tag}");
}

/// §3.6.2 — the presentation tokens map to CLASSES and nothing else: no value
/// from the tree ever reaches a style attribute, which is the free-form escape
/// the token vocabularies exist to close. `Natural` emits no class on either
/// axis, so a pre-phase image's class attribute is unchanged.
#[test]
fn image_presentation_tokens_map_to_classes_never_to_styles() {
    let html = render(
        r#"{"id":"i","kind":{"$type":"Image","alt":"Hero","aspectRatio":"SixteenNine","fit":"Cover","loading":"Lazy","src":{"$type":"Static","value":"/hero.jpg"},"variant":"Default"}}"#,
    );
    let tag = open_tag(&html, "img");
    assert!(
        tag.contains("fuaran-image fuaran-image-fit-cover fuaran-image-aspect-sixteen-nine"),
        "{tag}"
    );
    assert!(tag.contains(r#"loading="lazy""#), "{tag}");
    assert!(
        !tag.contains("style="),
        "no token reaches a style attribute: {tag}"
    );

    // The identity defaults emit nothing at all — `Eager` leaves the browser's
    // own default in place rather than declaring it, because deferring an
    // above-the-fold image is a regression and only the author knows.
    let plain = render(
        r#"{"id":"i","kind":{"$type":"Image","alt":"Hero","src":{"$type":"Static","value":"/hero.jpg"},"variant":"Default"}}"#,
    );
    let tag = open_tag(&plain, "img");
    assert!(tag.contains(r#"class="fuaran-image""#), "{tag}");
    assert!(!tag.contains("loading="), "{tag}");
    assert!(!tag.contains("aspect"), "{tag}");
}

/// §3.6.4 — the wire keeps the AUTHORED order and the renderer canonicalises
/// ASCENDING BY WIDTH. The two rules answer different questions, and putting
/// the sort in the renderer is what lets both be true — so this fixture, which
/// is authored DESCENDING, must re-encode descending and render ascending.
#[test]
fn srcset_renders_ascending_while_the_wire_keeps_authored_order() {
    let json = r#"{"id":"i","kind":{"$type":"Image","alt":"Boats","src":{"$type":"Static","value":"/h.jpg"},"srcSet":[{"src":{"$type":"Static","value":"/h-1600.jpg"},"width":1600},{"src":{"$type":"Static","value":"/h-800.jpg"},"width":800},{"src":{"$type":"Static","value":"/h-400.jpg"},"width":400}],"variant":"Default"}}"#;
    assert_eq!(encode_node(&node(json)), json, "the codec must not re-sort");
    let html = render(json);
    let tag = open_tag(&html, "img");
    assert!(
        tag.contains(r#"srcset="/h-400.jpg 400w, /h-800.jpg 800w, /h-1600.jpg 1600w""#),
        "{tag}"
    );
    assert!(tag.contains(r#"sizes="100vw""#), "{tag}");
}

/// §3.6.4 — a candidate that fails the floor is DROPPED rather than emitted in
/// neutered form. The primary `src` must exist so it collapses to the refusal
/// URL; a candidate has no such obligation, and offering a client a rendition
/// guaranteed to fail is worse than offering it one fewer.
#[test]
fn a_refused_srcset_candidate_is_dropped_not_neutered() {
    // The ambient deny-non-local policy: the local candidate is served, the
    // remote one refused.
    let html = render(
        r#"{"id":"i","kind":{"$type":"Image","alt":"Boats","src":{"$type":"Static","value":"/h.jpg"},"srcSet":[{"src":{"$type":"Static","value":"/h-400.jpg"},"width":400},{"src":{"$type":"Static","value":"https://collector.example/h-800.jpg"},"width":800}],"variant":"Default"}}"#,
    );
    let tag = open_tag(&html, "img");
    assert!(tag.contains(r#"src="/h.jpg""#), "{tag}");
    assert!(tag.contains("/h-400.jpg 400w"), "{tag}");
    assert!(!tag.contains("collector.example"), "{tag}");
    assert!(
        !tag.contains("about:blank"),
        "a dropped candidate must not be emitted in neutered form: {tag}"
    );
}

/// §3.6.5 — the rendered baseline is a REAL LINK, marked for an enhancement
/// tier. A host that emitted a scripted control instead, or a marked-up element
/// with no navigable target, would be conformant to nothing: the declaration
/// would render as a dead affordance for every reader without JavaScript. The
/// marker is VALUELESS, because the slot is a bool whose `false` is the absence
/// of the attribute.
#[test]
fn expandable_emits_a_real_anchor_around_the_image() {
    let html = render(
        r#"{"id":"i","kind":{"$type":"Image","alt":"Boats","expandable":true,"src":{"$type":"Static","value":"/h.jpg"},"variant":"Default"}}"#,
    );
    let anchor = open_tag(&html, "a");
    assert!(
        anchor.contains(r#"class="fuaran-image-expand""#),
        "{anchor}"
    );
    assert!(anchor.contains(r#"href="/h.jpg""#), "{anchor}");
    assert!(anchor.contains("data-fuaran-expandable"), "{anchor}");
    assert!(
        !anchor.contains(r#"data-fuaran-expandable=""#),
        "the marker is valueless: {anchor}"
    );
    assert!(
        !html.contains("onclick"),
        "nothing crosses the dispatch gate: {html}"
    );
    // The anchor WRAPS the image.
    assert!(html.contains("</a>"), "{html}");
    assert!(
        html.find("<a ").expect("the anchor") < html.find("<img").expect("the image"),
        "{html}"
    );
}

/// §3.6.5 — a `src` the render-time URL floor refused emits NO anchor. This is
/// the dropped-candidate rule turned on the affordance: a link to the refusal
/// URL is exactly the dead control the design exists to avoid. The image still
/// renders, carrying its refusal marker, and the reader is simply not offered
/// an expansion that could not work.
#[test]
fn a_refused_src_offers_no_expansion() {
    let html = render(
        r#"{"id":"i","kind":{"$type":"Image","alt":"Boats","expandable":true,"src":{"$type":"Static","value":"https://collector.example/h.jpg"},"variant":"Default"}}"#,
    );
    assert!(
        !html.contains("<a "),
        "no anchor over a refused src: {html}"
    );
    assert!(
        !html.contains("data-fuaran-expandable"),
        "and no marker: {html}"
    );
    assert!(html.contains("data-fuaran-egress-refused"), "{html}");
    assert!(html.contains("<img"), "the image still renders: {html}");
}

/// §3.6.3 + §3.6.5 — the composition: `<figure>` wraps `<a>` wraps `<img>`,
/// with the `<figcaption>` OUTSIDE the link target. A caption is prose a reader
/// selects, quotes and reads, not a second click surface, and putting
/// interactive content inside the element whose job is to LABEL the image
/// inverts the relationship `<figure>` / `<figcaption>` exists to express.
#[test]
fn a_captioned_expandable_image_nests_figure_anchor_image() {
    let html = render(
        r#"{"id":"i","kind":{"$type":"Image","alt":"Boats","caption":"The harbour at dawn, 1908.","expandable":true,"src":{"$type":"Static","value":"/h.jpg"},"variant":"Default"}}"#,
    );
    let figure = html.find("<figure").expect("the figure");
    let anchor = html.find("<a ").expect("the anchor");
    let img = html.find("<img").expect("the image");
    let caption = html.find("<figcaption").expect("the caption");
    let anchor_end = html.find("</a>").expect("the anchor close");
    assert!(figure < anchor && anchor < img, "{html}");
    assert!(
        anchor_end < caption,
        "the caption sits outside the link target: {html}"
    );
    assert!(
        html.contains(r#"<figcaption class="fuaran-image-figure-caption">"#),
        "{html}"
    );

    // Absent, there is NO wrapper at all — not an empty `<figure>`, not a
    // wrapper with an empty caption. The bare `<img>` a pre-1078 document
    // always produced.
    let uncaptioned = render(
        r#"{"id":"i","kind":{"$type":"Image","alt":"Boats","src":{"$type":"Static","value":"/h.jpg"},"variant":"Default"}}"#,
    );
    assert!(!uncaptioned.contains("figure"), "{uncaptioned}");
}
