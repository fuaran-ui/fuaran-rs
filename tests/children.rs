//! The one child enumeration (`ops::children`, Phase 2662): its slot list is the
//! generated slot table, its match reaches every row of it, and every walk that
//! used to keep a private copy now answers at the reach it names. Each walk
//! test below was red against that walk's private copy.

mod common;

use std::collections::BTreeSet;

use fuaran_rs::introspect::{all_nodes, get_node};
use fuaran_rs::ops::children::{Position, Reach, SLOT_MEMBERS, children, map_children, slots};
use fuaran_rs::render::server::island_script_id;
use fuaran_rs::render::{BindingSources, render_to_html, render_with_islands};
use fuaran_rs::validator::validate;
use fuaran_rs::wire::{Node, decode_node};

fn node(json: &str) -> Node {
    decode_node(json).expect("test tree decodes")
}

const FLEX: &str =
    r#""layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group""#;

/// One tree holding a node in every envelope position: both state alternatives
/// and the author-declared fallback. The corpus's node family carries no state
/// alternative, so the census below adds this to it.
fn envelope_tree() -> Node {
    node(&format!(
        r#"{{"fallback":{{"id":"fb","kind":{{"$type":"Markdown","text":"fallback"}}}},
            "id":"root","kind":{{"$type":"Box","children":[],{FLEX}}},
            "state":{{"onEmpty":{{"id":"empty","kind":{{"$type":"Markdown","text":"none"}}}},
                      "onLoading":{{"id":"loading","kind":{{"$type":"Markdown","text":"wait"}}}}}}}}"#
    ))
}

fn members_reached(n: &Node, into: &mut BTreeSet<&'static str>) {
    for slot in slots(n, Reach::KEYED) {
        into.insert(slot.member);
        members_reached(slot.node, into);
    }
}

#[test]
fn the_match_reaches_every_row_of_the_slot_list() {
    let corpus = common::corpus_root().join("nodes");
    let mut reached = BTreeSet::new();
    for entry in std::fs::read_dir(&corpus).expect("the corpus node family reads") {
        let path = entry.expect("a corpus entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a node fixture reads");
        if let Ok(tree) = decode_node(&text) {
            members_reached(&tree, &mut reached);
        }
    }
    members_reached(&envelope_tree(), &mut reached);
    let rows: BTreeSet<&str> = SLOT_MEMBERS.iter().map(|(m, _)| *m).collect();
    assert_eq!(
        reached, rows,
        "every slot-list row is reached by the match, and the match reaches nothing else"
    );
}

#[test]
fn each_slot_carries_the_class_its_row_declares() {
    let mut seen = Vec::new();
    for slot in slots(&envelope_tree(), Reach::KEYED) {
        seen.push((slot.member, slot.position));
    }
    assert_eq!(
        seen,
        vec![
            ("StateBehaviour.onLoading", Position::StateArm),
            ("StateBehaviour.onEmpty", Position::StateArm),
            ("Node.fallback", Position::Fallback),
        ]
    );
    for (member, position) in seen {
        assert!(SLOT_MEMBERS.contains(&(member, position)));
    }
}

#[test]
fn a_reach_selects_its_positions_and_map_children_rewrites_only_those() {
    let tree = envelope_tree();
    assert!(children(&tree, Reach::KIND_HELD).is_empty());
    let ids = |r| -> Vec<String> { children(&tree, r).iter().map(|c| c.id.clone()).collect() };
    assert_eq!(ids(Reach::FRAGMENT_SCOPE), vec!["loading", "empty"]);
    assert_eq!(ids(Reach::KEYED), vec!["loading", "empty", "fb"]);

    let renamed = map_children(&tree, Reach::FRAGMENT_SCOPE, |c| Node {
        id: format!("x.{}", c.id),
        ..c.clone()
    });
    let renamed_ids: Vec<String> = children(&renamed, Reach::KEYED)
        .iter()
        .map(|c| c.id.clone())
        .collect();
    assert_eq!(renamed_ids, vec!["x.loading", "x.empty", "fb"]);
}

// ─── The walks, one test per disagreement found ─────────────────────────────

/// The fragment registry walked the kind only, so a declaration held in a
/// `state` alternative never registered and its reference rendered the
/// unresolved placeholder. Fragment scope includes the state alternatives.
#[test]
fn a_fragment_declared_in_a_state_alternative_resolves() {
    let html = render_to_html(
        &node(&format!(
            r#"{{"id":"root","kind":{{"$type":"Box","children":[
                {{"id":"use-1","kind":{{"$type":"FragmentRef","name":"tile"}}}}
            ],{FLEX}}},
            "state":{{"onLoading":{{"id":"decl","kind":{{"$type":"FragmentDecl","body":
                {{"id":"frag-body","kind":{{"$type":"Markdown","text":"tile"}}}},"name":"tile"}}}}}}}}"#
        )),
        &BindingSources::default(),
    );
    assert!(!html.contains("fragment unresolved"), "{html}");
    assert!(
        html.contains("data-fuaran-node-id=\"use-1.frag-body\""),
        "{html}"
    );
}

/// Expansion namespacing walked the kind only, so a fragment body's state
/// alternative kept its declared id in every expansion — two references, one
/// id twice in the page.
#[test]
fn an_expanded_fragment_namespaces_its_state_alternatives() {
    let html = render_to_html(
        &node(&format!(
            r#"{{"id":"root","kind":{{"$type":"Box","children":[
                {{"id":"decl","kind":{{"$type":"FragmentDecl","body":
                    {{"id":"m","kind":{{"$type":"Metric","format":{{"$type":"Number"}},"label":"Revenue",
                      "value":{{"$type":"State","key":"not-yet"}}}},
                      "state":{{"onLoading":{{"id":"wait","kind":{{"$type":"Markdown","text":"wait"}}}}}}}},
                    "name":"tile"}}}},
                {{"id":"use-1","kind":{{"$type":"FragmentRef","name":"tile"}}}},
                {{"id":"use-2","kind":{{"$type":"FragmentRef","name":"tile"}}}}
            ],{FLEX}}}}}"#
        )),
        &BindingSources::default(),
    );
    assert!(
        html.contains("data-fuaran-node-id=\"use-1.wait\""),
        "{html}"
    );
    assert!(
        html.contains("data-fuaran-node-id=\"use-2.wait\""),
        "{html}"
    );
    assert!(!html.contains("data-fuaran-node-id=\"wait\""), "{html}");
}

/// The island walk read an `ErrorBoundary`'s child but not its fallback, and no
/// `Switch` case at all, so an island there got no hydrate payload. The island
/// walk is the kind-held relation.
#[test]
fn an_island_inside_a_switch_gets_its_payload() {
    let tree = node(
        r#"{"id":"sw","kind":{"$type":"Switch","cases":[],"default":
            {"id":"widget","kind":{"$type":"Button","label":{"$type":"Literal","text":"Refresh"},
             "onClick":{"$type":"Call","endpoint":"api/refresh","into":{"$type":"State","key":"r"}},
             "variant":"Primary"}},"stateKey":"view"}}"#,
    );
    let html = render_with_islands(&tree, &BindingSources::default(), &["widget"]);
    assert!(
        html.contains(&format!("id=\"{}\"", island_script_id("widget"))),
        "{html}"
    );
}

const BLANK_LINK: &str = r#"{"id":"bad","kind":{"$type":"Link","download":false,"href":{"$type":"Static","value":""},"label":{"$type":"Literal","text":"Docs"}}}"#;

fn mount_with_input(tree: &str) -> Node {
    node(&format!(
        r#"{{"id":"mount","kind":{{"$type":"Mount","capabilities":[],"channel":{{"direction":"OutOnly"}},
            "inputs":{{"seed":{{"$type":"SlotArg","tree":{tree}}}}},"scopeId":"guest"}}}}"#
    ))
}

fn with_fallback(tree: &str) -> Node {
    node(&format!(
        r#"{{"fallback":{tree},"id":"root","kind":{{"$type":"Box","children":[],{FLEX}}}}}"#
    ))
}

/// The validator's walk skipped slot arguments and the envelope fallback, while
/// claiming to be the apply engine's traversal: a defect the engine reaches went
/// unreported. It walks the keyed reach.
#[test]
fn the_validator_reaches_slot_arguments_and_the_fallback() {
    for tree in [mount_with_input(BLANK_LINK), with_fallback(BLANK_LINK)] {
        let found: Vec<(String, String)> = validate(&tree)
            .into_iter()
            .map(|f| (f.code.to_string(), f.node_id))
            .collect();
        assert!(
            found.contains(&("FUARAN063".to_string(), "bad".to_string())),
            "{found:?}"
        );
    }
}

/// Introspection skipped the same two positions, so a node the apply engine
/// finds and edits there was invisible to `get_node` and `all_nodes`.
#[test]
fn introspection_finds_a_node_in_a_slot_argument_or_the_fallback() {
    for tree in [mount_with_input(BLANK_LINK), with_fallback(BLANK_LINK)] {
        assert!(get_node(&tree, "bad").is_some());
        assert!(all_nodes(&tree).iter().any(|n| n.id == "bad"));
    }
}
