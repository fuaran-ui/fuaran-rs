//! Structural tree diff → op-script (What-If). The guaranteed property:
//! `apply(diff(a, b), a) == b` for every pair — a "what if" preview is a real,
//! replayable edit, and it localises to the changed nodes rather than replacing
//! the whole tree.

use fuaran_rs::diff::diff;
use fuaran_rs::ops::apply;
use fuaran_rs::wire::{Node, TreeOp, decode_node, encode_node};

fn n(json: &str) -> Node {
    decode_node(json).expect("decodes")
}

// Fold the op-script over `before` and return the resulting tree.
fn apply_all(before: &Node, ops: &[TreeOp]) -> Node {
    let mut tree = before.clone();
    for op in ops {
        tree = apply(&tree, op).expect("op applies").new_tree;
    }
    tree
}

fn card(title: &str, metric_label: &str) -> String {
    format!(
        r#"{{"id":"card","kind":{{"$type":"Box","children":[
            {{"id":"h","kind":{{"$type":"Heading","level":1,"text":{{"$type":"Literal","text":{title:?}}},"variant":"Standard"}}}},
            {{"id":"m","kind":{{"$type":"Metric","emphasis":"Normal","format":{{"$type":"None"}},"label":{{"$type":"Literal","text":{metric_label:?}}},"source":{{"$type":"Static","value":1}},"tone":"Default","weight":"Standard"}}}}
        ],"layout":{{"$type":"Flex","direction":"Vertical","wrap":false}},"role":"Card"}}}}"#
    )
}

#[test]
fn identical_trees_diff_to_no_ops() {
    let a = n(&card("Sales", "Revenue"));
    assert!(diff(&a, &a).is_empty());
}

#[test]
fn a_changed_leaf_localises_and_replays_exactly() {
    let a = n(&card("Sales", "Revenue"));
    let b = n(&card("Sales", "Profit")); // only the metric label changed
    let ops = diff(&a, &b);
    assert!(!ops.is_empty());
    // It does NOT replace the whole root — the edit is localised to the metric.
    assert!(!ops.iter().any(|o| matches!(o, TreeOp::ReplaceRoot { .. })));
    // And it replays exactly.
    assert_eq!(encode_node(&apply_all(&a, &ops)), encode_node(&b));
}

#[test]
fn a_changed_heading_replays_exactly() {
    let a = n(&card("Sales", "Revenue"));
    let b = n(&card("Marketing", "Revenue"));
    assert_eq!(encode_node(&apply_all(&a, &diff(&a, &b))), encode_node(&b));
}

#[test]
fn a_different_root_id_replaces_the_whole_tree() {
    let a = n(&card("Sales", "Revenue"));
    let b = n(
        r#"{"id":"other","kind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"Hi"},"variant":"Standard"}}"#,
    );
    let ops = diff(&a, &b);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], TreeOp::ReplaceRoot { .. }));
    assert_eq!(encode_node(&apply_all(&a, &ops)), encode_node(&b));
}

#[test]
fn a_removed_child_replays_exactly() {
    let a = n(&card("Sales", "Revenue"));
    // `b` drops the metric child, keeping only the heading.
    let b = n(r#"{"id":"card","kind":{"$type":"Box","children":[
            {"id":"h","kind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"Sales"},"variant":"Standard"}}
        ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Card"}}"#);
    let ops = diff(&a, &b);
    assert_eq!(encode_node(&apply_all(&a, &ops)), encode_node(&b));
}

#[test]
fn an_added_child_replays_exactly() {
    let a = n(&card("Sales", "Revenue"));
    // `b` appends a badge after the metric.
    let b = n(r#"{"id":"card","kind":{"$type":"Box","children":[
            {"id":"h","kind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"Sales"},"variant":"Standard"}},
            {"id":"m","kind":{"$type":"Metric","emphasis":"Normal","format":{"$type":"None"},"label":{"$type":"Literal","text":"Revenue"},"source":{"$type":"Static","value":1},"tone":"Default","weight":"Standard"}},
            {"id":"tag","kind":{"$type":"Badge","label":{"$type":"Literal","text":"New"},"variant":"Brand"}}
        ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Card"}}"#);
    let ops = diff(&a, &b);
    assert_eq!(encode_node(&apply_all(&a, &ops)), encode_node(&b));
}
