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
            {{"id":"m","kind":{{"$type":"Metric","emphasis":"Normal","format":{{"$type":"None"}},"label":{{"$type":"Literal","text":{metric_label:?}}},"value":{{"$type":"Static","value":1}},"tone":"Default","weight":"Standard"}}}}
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
            {"id":"m","kind":{"$type":"Metric","emphasis":"Normal","format":{"$type":"None"},"label":{"$type":"Literal","text":"Revenue"},"value":{"$type":"Static","value":1},"tone":"Default","weight":"Standard"}},
            {"id":"tag","kind":{"$type":"Badge","label":{"$type":"Literal","text":"New"},"variant":"Brand"}}
        ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Card"}}"#);
    let ops = diff(&a, &b);
    assert_eq!(encode_node(&apply_all(&a, &ops)), encode_node(&b));
}

// ── the style facet (Phase 2314) ─────────────────────────────────────────────

// `card("Sales", "Revenue")` with a style on the root and on the metric; an empty
// style leaves that slot absent.
fn styled_card(root_style: &str, metric_style: &str) -> Node {
    let with = |style: &str| {
        if style.is_empty() {
            String::new()
        } else {
            format!(r#","style":{style}"#)
        }
    };
    let (root, metric) = (with(root_style), with(metric_style));
    n(&format!(
        r#"{{"id":"card","kind":{{"$type":"Box","children":[
            {{"id":"h","kind":{{"$type":"Heading","level":1,"text":"Sales","variant":"Standard"}}}},
            {{"id":"m","kind":{{"$type":"Metric","label":"Revenue","value":{{"$type":"Static","value":1}}}}{metric}}}
        ],"layout":{{"$type":"Flex","direction":"Vertical","wrap":false}},"role":"Card"}}{root}}}"#
    ))
}

fn op_bytes(ops: &[TreeOp]) -> Vec<String> {
    ops.iter().map(fuaran_rs::wire::encode_op).collect()
}

#[test]
fn a_style_only_change_is_one_style_edit() {
    let cases = [
        (
            styled_card("", r#"{"tone":"Success","weight":"Spacious"}"#),
            styled_card("", r#"{"tone":"Critical","weight":"Spacious"}"#),
            r#"{"$type":"PatchStyle","style":{"tone":"Critical"},"target":"m"}"#,
        ),
        (
            styled_card(r#"{"tone":"Brand","weight":"Spacious"}"#, ""),
            styled_card(r#"{"emphasis":"Loud","tone":"Brand"}"#, ""),
            r#"{"$type":"PatchStyle","style":{"emphasis":"Loud","weight":"Standard"},"target":"card"}"#,
        ),
        (
            styled_card(
                "",
                r#"{"direction":"ltr","emphasis":"Loud","role":"Data","tone":"Success","voice":"Display","weight":"Spacious"}"#,
            ),
            styled_card(
                "",
                r#"{"direction":"rtl","emphasis":"Quiet","role":"Caption","tone":"Info","voice":"Structural","weight":"Compact"}"#,
            ),
            r#"{"$type":"UpdateStyle","style":{"direction":"rtl","emphasis":"Quiet","role":"Caption","tone":"Info","voice":"Structural","weight":"Compact"},"target":"m"}"#,
        ),
        (
            styled_card("", r#"{"emphasis":"Loud","tone":"Warning"}"#),
            styled_card("", ""),
            r#"{"$type":"PatchStyle","style":{"emphasis":"Normal","tone":"Default"},"target":"m"}"#,
        ),
    ];
    for (a, b, want) in cases {
        let ops = diff(&a, &b);
        assert_eq!(op_bytes(&ops), vec![want.to_string()]);
        assert_eq!(encode_node(&apply_all(&a, &ops)), encode_node(&b));
    }
}

#[test]
fn a_style_change_beside_a_content_change_keeps_the_seat_replace() {
    let a = styled_card("", r#"{"tone":"Success"}"#);
    let mut b = styled_card("", r#"{"tone":"Critical"}"#);
    b = n(&encode_node(&b).replace("Revenue", "Profit"));
    let ops = diff(&a, &b);
    assert!(matches!(
        ops.as_slice(),
        [TreeOp::RemoveNode { .. }, TreeOp::InsertChild { .. }]
    ));
    assert_eq!(encode_node(&apply_all(&a, &ops)), encode_node(&b));
}

#[test]
fn a_root_restyle_beside_a_child_change_recurses() {
    let a = styled_card(r#"{"tone":"Brand"}"#, "");
    let b = n(&encode_node(&styled_card(r#"{"tone":"Info"}"#, "")).replace("Revenue", "Profit"));
    let ops = diff(&a, &b);
    assert!(matches!(
        ops.as_slice(),
        [
            TreeOp::PatchStyle { .. },
            TreeOp::RemoveNode { .. },
            TreeOp::InsertChild { .. }
        ]
    ));
    assert_eq!(encode_node(&apply_all(&a, &ops)), encode_node(&b));
}
