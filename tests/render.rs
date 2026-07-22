//! Server-HTML renderer behaviour: the parity-locked class vocabulary, inert
//! interactivity, sanitiser posture, binding resolution, the islands
//! partial-hydration laws, and the reference-CSS byte-copy.

use std::collections::HashMap;
use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::client::ClientSession;
use fuaran_rs::render::server::{hydrate_script_id, island_script_id};
use fuaran_rs::render::{BindingSources, render_hydratable, render_to_html, render_with_islands};
use fuaran_rs::wire::{Node, NodeKind, decode_node, encode_node};

fn node(json: &str) -> Node {
    decode_node(json).expect("test tree decodes")
}

fn render(json: &str) -> String {
    render_to_html(&node(json), &BindingSources::default())
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
    let html = render(
        r#"{"id":"l","kind":{"$type":"Link","download":false,"href":{"$type":"Static","value":"javascript:alert(1)"},"label":{"$type":"Literal","text":"Docs"}}}"#,
    );
    assert!(html.contains("href=\"about:blank\""));
    let ok = render(
        r#"{"id":"l","kind":{"$type":"Link","download":false,"href":{"$type":"Static","value":"https://x.dev/docs"},"label":{"$type":"Literal","text":"Docs"}}}"#,
    );
    assert!(ok.contains("href=\"https://x.dev/docs\""));
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

#[test]
fn accessibility_projects_onto_the_wrapper() {
    let html = render(
        r#"{"id":"a","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"x"}},"accessibility":{"label":{"$type":"Static","value":"Summary"},"role":"region","liveRegion":"polite"}}"#,
    );
    assert!(html.contains("aria-label=\"Summary\""));
    assert!(html.contains("role=\"region\""));
    assert!(html.contains("aria-live=\"polite\""));
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

// ─── Reference-CSS byte-copy parity ──────────────────────────────────────────

#[test]
fn reference_css_byte_copy_matches_the_reference_artefact() {
    let crate_dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    let local = crate_dir.join("css").join("fuaran.css");
    let local_bytes = std::fs::read(&local).expect("css/fuaran.css ships with the crate");
    // The reference artefact lives in the sibling checkout; skip standalone.
    let mut dir = crate_dir.clone();
    let reference = loop {
        let candidate = dir
            .join("fuaran")
            .join("src")
            .join("Fuaran.UI.Renderer")
            .join("content")
            .join("fuaran-reference.css");
        if candidate.is_file() {
            break Some(candidate);
        }
        if !dir.pop() {
            break None;
        }
    };
    let Some(reference) = reference else {
        eprintln!("reference stylesheet not found; skipping byte-parity (standalone checkout)");
        return;
    };
    let reference_bytes = std::fs::read(&reference).expect("reading reference stylesheet");
    assert_eq!(
        local_bytes, reference_bytes,
        "css/fuaran.css must stay a byte-copy of the reference stylesheet"
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
