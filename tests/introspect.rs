//! Structural introspection + search + assertion behaviour, and layout-flag
//! derivation — the "machine can see the UI" tier (Unit Test, Grep, Pattern
//! Bank, Kintsugi, Blind Surveyor).

use fuaran_rs::introspect::layout::{LayoutFlag, LayoutInput, LayoutOptions, derive};
use fuaran_rs::introspect::{
    Assertion, AssertionOutcome, Query, all_facts, assert_all, assert_tree, find, get_node_facts,
};
use fuaran_rs::wire::{NodeCategory, ToneVariant, decode_node};

// Node-level `style` carries the SemanticStyle a structural query keys on (the
// Metric spec's own tone is a separate display concern); `rev` is Brand-toned +
// State-bound, `margin` Success-toned + Static.
const TREE: &str = r#"{"id":"dash","kind":{"$type":"Box","children":[
    {"id":"title","kind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"Q3 Revenue"},"variant":"Standard"}},
    {"id":"rev","kind":{"$type":"Metric","emphasis":"Loud","format":{"$type":"Currency","code":"GBP"},"label":{"$type":"Literal","text":"Revenue"},"source":{"$type":"State","defaultValue":0,"key":"revenue"},"tone":"Brand","weight":"Standard"},"style":{"emphasis":"Loud","tone":"Brand","weight":"Standard"}},
    {"id":"margin","kind":{"$type":"Metric","emphasis":"Normal","format":{"$type":"None"},"label":{"$type":"Literal","text":"Margin"},"source":{"$type":"Static","value":0.3},"tone":"Success","weight":"Standard"},"style":{"emphasis":"Normal","tone":"Success","weight":"Standard"}}
],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;

fn tree() -> fuaran_rs::wire::Node {
    decode_node(TREE).expect("tree decodes")
}

#[test]
fn node_facts_read_structure_not_pixels() {
    let facts = get_node_facts(&tree(), "rev").expect("rev exists");
    assert_eq!(facts.kind, "Metric");
    assert_eq!(facts.category, NodeCategory::Display);
    assert_eq!(facts.tone, ToneVariant::Brand);
    assert_eq!(facts.text.as_deref(), Some("Revenue"));

    let root = get_node_facts(&tree(), "dash").unwrap();
    assert_eq!(root.child_ids, vec!["title", "rev", "margin"]);
}

#[test]
fn all_facts_walks_in_document_order() {
    let ids: Vec<String> = all_facts(&tree()).into_iter().map(|f| f.id).collect();
    assert_eq!(ids, vec!["dash", "title", "rev", "margin"]);
}

#[test]
fn structural_search_matches_running_nodes() {
    // Grep: "every Metric" → the two metric ids glow.
    let metrics = find(&tree(), &Query::Kind("Metric".into()));
    assert_eq!(metrics, vec!["rev", "margin"]);

    // "Brand-toned Display nodes" (AND).
    let brand_display = find(
        &tree(),
        &Query::All(vec![
            Query::Category(NodeCategory::Display),
            Query::Tone(ToneVariant::Brand),
        ]),
    );
    assert_eq!(brand_display, vec!["rev"]);

    // "text contains Revenue" across the tree (heading + metric label).
    let revenue = find(&tree(), &Query::TextContains("Revenue".into()));
    assert_eq!(revenue, vec!["title", "rev"]);

    // A live (reactive-bound) node — the State-bound metric only.
    let live = find(&tree(), &Query::HasReactiveBinding);
    assert_eq!(live, vec!["rev"]);
}

#[test]
fn assertions_pass_on_structure() {
    let suite = [
        Assertion::NodeExists("rev".into()),
        Assertion::NodeHasKind {
            id: "title".into(),
            kind: "Heading".into(),
        },
        Assertion::AtLeast {
            query: Query::Kind("Metric".into()),
            count: 2,
        },
        Assertion::None(Query::Kind("Chart".into())),
    ];
    assert!(
        assert_all(&tree(), &suite)
            .iter()
            .all(|o| *o == AssertionOutcome::Pass)
    );
}

#[test]
fn assertions_are_restyle_proof() {
    // Restyle the whole tree (a tone swap on every node) — the structural
    // assertions stay green because they test structure, not appearance.
    let restyled = decode_node(
        &TREE
            .replace("\"tone\":\"Brand\"", "\"tone\":\"Critical\"")
            .replace("\"tone\":\"Success\"", "\"tone\":\"Warning\"")
            .replace("\"emphasis\":\"Loud\"", "\"emphasis\":\"Quiet\""),
    )
    .unwrap();
    let suite = [
        Assertion::NodeHasKind {
            id: "rev".into(),
            kind: "Metric".into(),
        },
        Assertion::Exactly {
            query: Query::Kind("Metric".into()),
            count: 2,
        },
    ];
    assert!(
        assert_all(&restyled, &suite)
            .iter()
            .all(|o| *o == AssertionOutcome::Pass)
    );
}

#[test]
fn a_failing_assertion_reports_a_reason() {
    let out = assert_tree(&tree(), &Assertion::NodeExists("ghost".into()));
    match out {
        AssertionOutcome::Fail(reason) => assert!(reason.contains("ghost")),
        AssertionOutcome::Pass => panic!("expected a failure"),
    }
}

#[test]
fn layout_flags_read_overflow_from_measurements() {
    // The Blind Surveyor / geometric Unit-Test mechanic: content wider than the
    // clip region on a clipping element → OverflowHorizontal, derived from
    // measured geometry with no pixels in the conclusion.
    let overflowing = LayoutInput {
        width: 320.0,
        height: 60.0,
        scroll_width: Some(480.0),
        client_width: Some(320.0),
        overflow_x: Some("hidden".into()),
        ..Default::default()
    };
    assert_eq!(
        derive(LayoutOptions::default(), &overflowing),
        vec![LayoutFlag::OverflowHorizontal]
    );

    // A visible-overflow element is never flagged (we can't tell it clips).
    let visible = LayoutInput {
        overflow_x: Some("visible".into()),
        ..overflowing.clone()
    };
    assert!(derive(LayoutOptions::default(), &visible).is_empty());

    // A collapsed dimension fires ZeroDimension.
    let collapsed = LayoutInput {
        width: 0.0,
        height: 40.0,
        ..Default::default()
    };
    assert_eq!(
        derive(LayoutOptions::default(), &collapsed),
        vec![LayoutFlag::ZeroDimension("width")]
    );
}
