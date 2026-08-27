//! Apply-engine behaviour: every op's happy path, every structured error code,
//! batch atomicity, and the apply-envelope law (`can_apply ≡ apply is-ok`).
//! Trees and ops are authored as canonical wire JSON and decoded through the
//! host's own codec — the same artefacts a driving service would hold.

use fuaran_rs::ops::{ApplyErrorCode, apply, can_apply};
use fuaran_rs::wire::{
    DecodeErrorCode, Node, TreeOp, decode_node, decode_op, encode_node, encode_op,
};

fn node(json: &str) -> Node {
    decode_node(json).expect("test tree decodes")
}

fn op(json: &str) -> TreeOp {
    decode_op(json).expect("test op decodes")
}

/// A three-child vertical Box with a heading child, a markdown child, and a
/// nested Box holding a metric.
fn sample_tree() -> Node {
    node(
        r#"{"id":"root","kind":{"$type":"Box","children":[
            {"id":"h1","kind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"Title"},"variant":"Standard"}},
            {"id":"md","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"Body"}}},
            {"id":"inner","kind":{"$type":"Box","children":[
                {"id":"m1","kind":{"$type":"Metric","label":"Revenue","value":{"$type":"Static","value":42}}}
            ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}
        ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#,
    )
}

fn assert_code(
    result: Result<fuaran_rs::ops::ApplyOutcome, fuaran_rs::ops::ApplyError>,
    code: ApplyErrorCode,
) {
    match result {
        Ok(_) => panic!("expected {code:?}, got success"),
        Err(e) => assert_eq!(e.code, code, "message: {}", e.message),
    }
}

#[test]
fn edit_node_swaps_the_kind() {
    let tree = sample_tree();
    let edit = op(
        r#"{"$type":"EditNode","newKind":{"$type":"Markdown","text":{"$type":"Literal","text":"Replaced"}},"target":"h1"}"#,
    );
    let out = apply(&tree, &edit).expect("applies");
    assert!(encode_node(&out.new_tree).contains("Replaced"));
    assert_eq!(out.emitted_telemetry.len(), 1);
    assert_eq!(out.emitted_telemetry[0].op, "EditNode");
    // The input tree is untouched.
    assert_eq!(tree, sample_tree());
}

#[test]
fn edit_node_missing_target_is_node_not_found() {
    let edit = op(
        r#"{"$type":"EditNode","newKind":{"$type":"Markdown","text":{"$type":"Literal","text":"x"}},"target":"ghost"}"#,
    );
    assert_code(apply(&sample_tree(), &edit), ApplyErrorCode::NodeNotFound);
}

#[test]
fn update_prop_top_level_field() {
    let tree = sample_tree();
    let up = op(r#"{"$type":"UpdateProp","path":"Level","target":"h1","value":2}"#);
    let out = apply(&tree, &up).expect("applies");
    assert!(encode_node(&out.new_tree).contains("\"level\":2"));
}

#[test]
fn update_prop_bare_string_text_source() {
    // The lenient op-value spelling: a bare string in a TextSource position.
    let up = op(r#"{"$type":"UpdateProp","path":"Text","target":"md","value":"New body"}"#);
    let out = apply(&sample_tree(), &up).expect("applies");
    assert!(encode_node(&out.new_tree).contains("New body"));
}

#[test]
fn update_prop_unknown_field_is_field_not_found() {
    let up = op(r#"{"$type":"UpdateProp","path":"Bogus","target":"h1","value":1}"#);
    assert_code(apply(&sample_tree(), &up), ApplyErrorCode::FieldNotFound);
}

#[test]
fn update_prop_wrong_value_type_is_kind_mismatch() {
    let up = op(r#"{"$type":"UpdateProp","path":"Level","target":"h1","value":"two"}"#);
    assert_code(apply(&sample_tree(), &up), ApplyErrorCode::KindMismatch);
}

#[test]
fn update_prop_malformed_path_is_path_invalid() {
    for bad in [
        r#"{"$type":"UpdateProp","path":"Columns[x].Label","target":"h1","value":1}"#,
        r#"{"$type":"UpdateProp","path":"Columns[01].Label","target":"h1","value":1}"#,
        r#"{"$type":"UpdateProp","path":"Columns[#id]","target":"h1","value":1}"#,
        r#"{"$type":"UpdateProp","path":".","target":"h1","value":1}"#,
    ] {
        assert_code(apply(&sample_tree(), &op(bad)), ApplyErrorCode::PathInvalid);
    }
}

fn grid_tree() -> Node {
    node(
        r#"{"id":"grid-1","kind":{"$type":"DataGrid","columns":[
            {"format":{"$type":"None"},"kind":{"$type":"Text"},"label":"Name","width":{"$type":"Auto"}},
            {"format":{"$type":"None"},"kind":{"$type":"Numeric"},"label":"Total","width":{"$type":"Auto"}}
        ],"editable":false,"source":{"$type":"Static","value":"<opaque>"}}}"#,
    )
}

#[test]
fn update_prop_nested_column_label() {
    let up = op(
        r#"{"$type":"UpdateProp","path":"Columns[1].Label","target":"grid-1","value":"Grand total"}"#,
    );
    let out = apply(&grid_tree(), &up).expect("applies");
    assert!(encode_node(&out.new_tree).contains("Grand total"));
}

#[test]
fn update_prop_nested_object_value_round_trips_through_apply() {
    // An object-valued nested UpdateProp (a CellFormat) applies faithfully.
    let up = op(
        r#"{"$type":"UpdateProp","path":"Columns[0].Format","target":"grid-1","value":{"$type":"Currency","code":"GBP"}}"#,
    );
    let out = apply(&grid_tree(), &up).expect("applies");
    assert!(encode_node(&out.new_tree).contains("\"code\":\"GBP\""));
}

#[test]
fn update_prop_nested_missing_index_is_path_invalid() {
    let up = op(r#"{"$type":"UpdateProp","path":"Columns.Label","target":"grid-1","value":"x"}"#);
    assert_code(apply(&grid_tree(), &up), ApplyErrorCode::PathInvalid);
}

#[test]
fn update_prop_nested_index_out_of_range() {
    let up =
        op(r#"{"$type":"UpdateProp","path":"Columns[9].Label","target":"grid-1","value":"x"}"#);
    assert_code(apply(&grid_tree(), &up), ApplyErrorCode::PositionOutOfRange);
}

#[test]
fn update_prop_nested_unknown_leaf_is_field_not_found() {
    let up =
        op(r#"{"$type":"UpdateProp","path":"Columns[0].Bogus","target":"grid-1","value":"x"}"#);
    assert_code(apply(&grid_tree(), &up), ApplyErrorCode::FieldNotFound);
}

#[test]
fn update_prop_closure_leaf_is_not_supported() {
    let up =
        op(r#"{"$type":"UpdateProp","path":"Columns[0].Value","target":"grid-1","value":"x"}"#);
    assert_code(
        apply(&grid_tree(), &up),
        ApplyErrorCode::PathNotSupportedYet,
    );
}

#[test]
fn replace_binding_known_slot() {
    let rb = op(
        r#"{"$type":"ReplaceBinding","binding":{"$type":"State","defaultValue":0,"key":"revenue"},"slot":"Value","target":"m1"}"#,
    );
    let out = apply(&sample_tree(), &rb).expect("applies");
    assert!(encode_node(&out.new_tree).contains("\"key\":\"revenue\""));
}

#[test]
fn replace_binding_unknown_slot_is_slot_not_found() {
    let rb = op(
        r#"{"$type":"ReplaceBinding","binding":{"$type":"Filter","name":"f"},"slot":"Nope","target":"m1"}"#,
    );
    assert_code(apply(&sample_tree(), &rb), ApplyErrorCode::SlotNotFound);
}

#[test]
fn update_style_and_state() {
    let us = op(
        r#"{"$type":"UpdateStyle","style":{"emphasis":"Loud","tone":"Brand","weight":"Standard"},"target":"h1"}"#,
    );
    let out = apply(&sample_tree(), &us).expect("applies");
    assert!(encode_node(&out.new_tree).contains("\"emphasis\":\"Loud\""));

    let ust = op(r#"{"$type":"UpdateState","state":{"onError":"<closure>"},"target":"md"}"#);
    let out = apply(&sample_tree(), &ust).expect("applies");
    assert!(encode_node(&out.new_tree).contains("\"onError\":\"<closure>\""));
}

#[test]
fn insert_child_appends_and_the_retired_position_never_reaches_apply() {
    // 0.4.0: InsertChild APPENDS. There is no ordinal and so no out-of-range
    // rejection left to assert. Phase 687 closed the migration window during
    // which a legacy `position` was accepted and ignored, so the field is now
    // refused at DECODE — it never reaches this apply engine at all.
    //
    // That is the assertion worth making here rather than in the decoder tests:
    // the apply engine has no ordinal handling left to exercise, and the reason
    // it needs none is that the wire cannot express one.
    for pos in [0_i64, 3, 4, -1] {
        let raw = format!(
            r#"{{"$type":"InsertChild","child":{{"id":"new-1","kind":{{"$type":"Markdown","text":{{"$type":"Literal","text":"n"}}}}}},"parentId":"root","position":{pos}}}"#
        );
        let err = decode_op(&raw)
            .expect_err("the retired `position` was accepted — the migration window is closed");
        assert_eq!(err.code, DecodeErrorCode::WrongType, "position {pos}");
        assert_eq!(err.path, "$.position", "the error names the retired field");
    }

    // The canonical, positionless form applies as an append.
    let canonical = op(
        r#"{"$type":"InsertChild","child":{"id":"new-1","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"n"}}},"parentId":"root"}"#,
    );
    let out = apply(&sample_tree(), &canonical).expect("applies");
    assert_eq!(
        child_ids_of(&out.new_tree, "root"),
        vec!["h1", "md", "inner", "new-1"]
    );
    // Re-encoding never writes the field back: one wire dialect, not two.
    assert!(!encode_op(&canonical).contains("position"));
}

#[test]
fn batch_insert_then_reorder_places_a_node_precisely() {
    // The pair that supersedes the ordinal: append, then state the order.
    let batch = op(
        r#"{"$type":"Batch","ops":[{"$type":"InsertChild","child":{"id":"new-1","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"n"}}},"parentId":"root"},{"$type":"ReorderChildren","newOrder":["new-1","h1","md","inner"],"parentId":"root"}]}"#,
    );
    let out = apply(&sample_tree(), &batch).expect("applies");
    assert_eq!(
        child_ids_of(&out.new_tree, "root"),
        vec!["new-1", "h1", "md", "inner"]
    );
}

#[test]
fn reorder_that_is_not_an_exact_permutation_is_refused() {
    let bad = op(r#"{"$type":"ReorderChildren","newOrder":["h1"],"parentId":"root"}"#);
    assert_code(
        apply(&sample_tree(), &bad),
        ApplyErrorCode::OrderingMismatch,
    );
}

#[test]
fn insert_child_duplicate_id_refused() {
    let ins = op(
        r#"{"$type":"InsertChild","child":{"id":"m1","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"n"}}},"parentId":"root"}"#,
    );
    assert_code(apply(&sample_tree(), &ins), ApplyErrorCode::DuplicateNodeId);
}

#[test]
fn insert_child_into_childless_kind_refused() {
    let ins = op(
        r#"{"$type":"InsertChild","child":{"id":"new-1","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"n"}}},"parentId":"md"}"#,
    );
    assert_code(apply(&sample_tree(), &ins), ApplyErrorCode::ChildlessKind);
}

#[test]
fn remove_node_and_root_guard() {
    let rm = op(r#"{"$type":"RemoveNode","target":"md"}"#);
    let out = apply(&sample_tree(), &rm).expect("applies");
    assert!(!encode_node(&out.new_tree).contains("\"id\":\"md\""));

    let rm_root = op(r#"{"$type":"RemoveNode","target":"root"}"#);
    assert_code(
        apply(&sample_tree(), &rm_root),
        ApplyErrorCode::KindMismatch,
    );
}

#[test]
fn move_node_reparents_and_refuses_cycles() {
    let mv = op(r#"{"$type":"MoveNode","newParentId":"inner","target":"md"}"#);
    let out = apply(&sample_tree(), &mv).expect("applies");
    let encoded = encode_node(&out.new_tree);
    // md now precedes m1 inside `inner`.
    let inner_pos = encoded.find("\"id\":\"inner\"").unwrap();
    let md_pos = encoded.find("\"id\":\"md\"").unwrap();
    assert!(md_pos > inner_pos);

    let self_mv = op(r#"{"$type":"MoveNode","newParentId":"inner","target":"inner"}"#);
    assert_code(
        apply(&sample_tree(), &self_mv),
        ApplyErrorCode::KindMismatch,
    );

    let cycle = op(r#"{"$type":"MoveNode","newParentId":"inner","target":"root"}"#);
    assert_code(apply(&sample_tree(), &cycle), ApplyErrorCode::KindMismatch);
}

#[test]
fn reorder_children_permutation_only() {
    let ok_reorder =
        op(r#"{"$type":"ReorderChildren","newOrder":["inner","h1","md"],"parentId":"root"}"#);
    let out = apply(&sample_tree(), &ok_reorder).expect("applies");
    let encoded = encode_node(&out.new_tree);
    assert!(encoded.find("\"id\":\"inner\"").unwrap() < encoded.find("\"id\":\"h1\"").unwrap());

    let bad = op(r#"{"$type":"ReorderChildren","newOrder":["h1","md"],"parentId":"root"}"#);
    assert_code(
        apply(&sample_tree(), &bad),
        ApplyErrorCode::OrderingMismatch,
    );
}

#[test]
fn replace_root_swaps_the_whole_tree() {
    let rr = op(
        r#"{"$type":"ReplaceRoot","node":{"id":"brand-new","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"fresh"}}}}"#,
    );
    let out = apply(&sample_tree(), &rr).expect("applies");
    assert_eq!(out.new_tree.id, "brand-new");
}

#[test]
fn batch_is_atomic_and_telemetry_per_leaf() {
    let batch = op(r#"{"$type":"Batch","ops":[
            {"$type":"UpdateProp","path":"Level","target":"h1","value":3},
            {"$type":"RemoveNode","target":"md"}
        ]}"#);
    let out = apply(&sample_tree(), &batch).expect("applies");
    assert_eq!(out.emitted_telemetry.len(), 2);
    assert!(encode_node(&out.new_tree).contains("\"level\":3"));

    // Second op fails: the whole batch aborts with the inner index; the input
    // tree is untouched.
    let tree = sample_tree();
    let aborting = op(r#"{"$type":"Batch","ops":[
            {"$type":"UpdateProp","path":"Level","target":"h1","value":3},
            {"$type":"RemoveNode","target":"ghost"}
        ]}"#);
    let err = apply(&tree, &aborting).expect_err("aborts");
    assert_eq!(err.code, ApplyErrorCode::BatchAborted);
    assert_eq!(err.batch_index, Some(1));
    assert_eq!(tree, sample_tree());
}

#[test]
fn can_apply_agrees_with_apply() {
    let tree = sample_tree();
    let cases = [
        r#"{"$type":"UpdateProp","path":"Level","target":"h1","value":2}"#,
        r#"{"$type":"UpdateProp","path":"Level","target":"ghost","value":2}"#,
        r#"{"$type":"RemoveNode","target":"root"}"#,
        r#"{"$type":"RemoveNode","target":"md"}"#,
        r#"{"$type":"ReorderChildren","newOrder":["h1"],"parentId":"root"}"#,
        r#"{"$type":"InsertChild","child":{"id":"m1","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"n"}}},"parentId":"root"}"#,
    ];
    for case in cases {
        let o = op(case);
        assert_eq!(
            can_apply(&tree, &o),
            apply(&tree, &o).is_ok(),
            "law violated for {case}"
        );
    }
}

#[test]
fn state_surface_children_are_addressable() {
    // A node nested under `state.onLoading` is reachable by every node-targeted op.
    let tree = node(
        r#"{"id":"root","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"b"}},"state":{"onLoading":{"id":"spinner","kind":{"$type":"Skeleton","rows":3}}}}"#,
    );
    let up = op(r#"{"$type":"UpdateProp","path":"Rows","target":"spinner","value":5}"#);
    let out = apply(&tree, &up).expect("applies");
    assert!(encode_node(&out.new_tree).contains("\"rows\":5"));
}

/// The ordered child ids of `parent_id` in `tree` — what the placement tests
/// above assert on.
fn child_ids_of(tree: &Node, parent_id: &str) -> Vec<String> {
    fn find(n: &Node, id: &str) -> Option<Vec<String>> {
        if n.id == id {
            if let fuaran_rs::wire::NodeKind::Box(s) = &n.kind {
                return Some(s.children.iter().map(|c| c.id.clone()).collect());
            }
        }
        if let fuaran_rs::wire::NodeKind::Box(s) = &n.kind {
            for c in &s.children {
                if let Some(v) = find(c, id) {
                    return Some(v);
                }
            }
        }
        None
    }
    find(tree, parent_id).expect("parent is a Box in the sample tree")
}

// ─── Phase 867: UpdateProp over `Metric.TrendPolarity` ───────────────────────

/// A `Metric` carrying no polarity — the omitted-when-default state an op must
/// be able to move off, and move back to.
const POLARITY_TREE: &str = r#"{"id":"m","kind":{"$type":"Metric","label":"Avg wait","trend":{"$type":"Static","value":-0.0734},"value":{"$type":"Static","value":80}}}"#;

#[test]
fn update_prop_sets_trend_polarity_in_parity_with_the_reference() {
    // The reference carries this arm (`Fuaran.UI.Ops/Apply.fs`) and lists the
    // field in its introspection surface, so a host that answered FieldNotFound
    // here would refuse an op the reference accepts. The struct-update spread
    // meant the compiler could not force the arm — the field rode through
    // untouched — so this is asserted rather than assumed.
    let tree = node(POLARITY_TREE);
    let up =
        op(r#"{"$type":"UpdateProp","path":"TrendPolarity","target":"m","value":"LowerIsBetter"}"#);
    assert!(can_apply(&tree, &up), "the field must be updatable");
    let out = apply(&tree, &up).expect("the op applies");
    assert!(
        encode_node(&out.new_tree).contains(r#""trendPolarity":"LowerIsBetter""#),
        "{}",
        encode_node(&out.new_tree)
    );
}

#[test]
fn update_prop_back_to_the_default_re_encodes_to_nothing() {
    // Setting the DEFAULT is not the same as never having set it on the type,
    // but it must be on the wire — otherwise a round-trip through an op would
    // leave a document that no longer round-trips minimally.
    let tree = node(POLARITY_TREE);
    let set =
        op(r#"{"$type":"UpdateProp","path":"TrendPolarity","target":"m","value":"LowerIsBetter"}"#);
    let unset = op(
        r#"{"$type":"UpdateProp","path":"TrendPolarity","target":"m","value":"HigherIsBetter"}"#,
    );
    let inverted = apply(&tree, &set).expect("applies").new_tree;
    let back = apply(&inverted, &unset).expect("applies").new_tree;
    assert_eq!(
        encode_node(&back),
        POLARITY_TREE,
        "returning to the default returns to the original bytes"
    );
}

#[test]
fn update_prop_refuses_the_reserved_polarity_case() {
    // A tree the codec would not accept must not be reachable by mutating one
    // it did — the coercion routes through the same decode function, so the
    // reservation holds at apply time too.
    let tree = node(POLARITY_TREE);
    let up = op(r#"{"$type":"UpdateProp","path":"TrendPolarity","target":"m","value":"Neutral"}"#);
    let err = apply(&tree, &up).expect_err("`Neutral` is RESERVED");
    assert_eq!(err.code, ApplyErrorCode::KindMismatch, "{err:?}");
    // The apply envelope law holds across the refusal.
    assert!(!can_apply(&tree, &up), "can_apply agrees with apply");
}
