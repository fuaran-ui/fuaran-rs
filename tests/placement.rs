//! The placement algebra's acceptance + correspondence set.
//!
//! Two obligations, stated over an EXHAUSTIVE enumeration of fixture trees and
//! checked against the REAL apply engine ([`fuaran_rs::ops::apply`]), never a
//! re-derivation of its logic:
//!
//!  1. **No false permit** — every op a helper emits is accepted by the apply
//!     engine, and the applied tree exhibits the placement's declared order (the
//!     moved / inserted node sits exactly where the `Placement` said, with the
//!     other siblings' order preserved).
//!
//!  2. **No false refuse** — every helper rejection corresponds to an apply-side
//!     rejection of the op the helper would otherwise have emitted. For the
//!     insert path the correspondence is exact CODE equality, because the helper
//!     checks in the engine's own order. For the move path it is refusal ↔
//!     refusal: the engine checks self / descendant before it checks whether the
//!     node is structurally addressable, so a node that fails both is named by
//!     one code here and the other there — both refuse, which is the property.
//!     `UnknownAnchor` is the one deliberate tightening and is checked against
//!     the `OrderingMismatch` refusal of the only op that could have honoured
//!     the anchor.
//!
//! The clone verbs add the tree-wide id obligations: a duplicate never collides
//! with any id in the target tree (including ids held in non-structural
//! positions), the clone is structurally equal to its source modulo ids, and a
//! paste preserves non-colliding ids while remapping colliding ones.
//!
//! **The enumeration is exhaustive rather than sampled.** Every fixture tree is
//! crossed with every node id in it plus a `ghost`, every parent, and every
//! placement over every anchor — so the legal case and every illegal class are
//! all generated, deterministically, with no generator to seed and no sample
//! size to argue about.

use fuaran_rs::introspect;
use fuaran_rs::ops::placement::{
    DerivedIds, PlaceError, Placement, SequentialIds, Target, can_place, duplicate_op,
    duplicate_op_with, move_op, nudge_op, paste_op, paste_op_with, place_op,
};
use fuaran_rs::ops::{ApplyErrorCode, all_node_ids, apply};
use fuaran_rs::wire::{Node, TreeOp, decode_node};

// ─── Fixtures ────────────────────────────────────────────────────────────────

fn node(json: &str) -> Node {
    decode_node(json).expect("test tree decodes")
}

fn leaf_json(id: &str) -> String {
    format!(
        r#"{{"id":"{id}","kind":{{"$type":"Markdown","text":{{"$type":"Literal","text":"body"}}}}}}"#
    )
}

fn container_json(id: &str, children: &[String]) -> String {
    format!(
        r#"{{"id":"{id}","kind":{{"$type":"Box","children":[{}],"layout":{{"$type":"Flex","direction":"Vertical","wrap":false}},"role":"Group"}}}}"#,
        children.join(",")
    )
}

fn leaf(id: &str) -> Node {
    node(&leaf_json(id))
}

fn container(id: &str, children: &[String]) -> Node {
    node(&container_json(id, children))
}

/// root ── left [a; b; c] · solo (childless leaf) · right [d] · empty []
fn fixture() -> Node {
    container(
        "root",
        &[
            container_json("left", &[leaf_json("a"), leaf_json("b"), leaf_json("c")]),
            leaf_json("solo"),
            container_json("right", &[leaf_json("d")]),
            container_json("empty", &[]),
        ],
    )
}

/// A node whose `state.onLoading` placeholder is inside the tree-wide id
/// contract but outside every structural child list.
fn non_structural_fixture() -> Node {
    container(
        "root",
        &[
            format!(
                r#"{{"id":"m","kind":{{"$type":"Markdown","text":{{"$type":"Literal","text":"body"}}}},"state":{{"onLoading":{}}}}}"#,
                leaf_json("ph")
            ),
            container_json("box", &[]),
        ],
    )
}

/// The fixture set the exhaustive enumeration runs over: nesting, a childless
/// sibling, an empty container, a non-structural slot, and a bare root.
fn corpus() -> Vec<Node> {
    vec![
        fixture(),
        container(
            "root",
            &[container_json(
                "x",
                &[container_json("y", &[leaf_json("z")])],
            )],
        ),
        container("root", &[leaf_json("p"), leaf_json("q")]),
        non_structural_fixture(),
        container("root", &[]),
    ]
}

// ─── Shared readers ──────────────────────────────────────────────────────────

/// The structural child ids of `parent_id` — `[]` for an absent or childless
/// node, exactly as the reference host's reader reports them.
fn child_ids(root: &Node, parent_id: &str) -> Vec<String> {
    introspect::get_node_facts(root, parent_id)
        .map(|f| f.child_ids)
        .unwrap_or_default()
}

fn applied(root: &Node, op: &TreeOp) -> Node {
    match apply(root, op) {
        Ok(outcome) => outcome.new_tree,
        Err(e) => panic!("apply refused an op the helper emitted: {e}"),
    }
}

fn refused_as(root: &Node, op: &TreeOp) -> ApplyErrorCode {
    match apply(root, op) {
        Ok(_) => panic!("apply accepted an op the helper refused"),
        Err(e) => e.code,
    }
}

/// Pre-order kind tags over the whole traversal surface — structural equality
/// modulo ids.
fn kind_shape(n: &Node) -> Vec<&'static str> {
    introspect::all_nodes(n)
        .into_iter()
        .map(|x| x.kind.type_name())
        .collect()
}

fn all_distinct(root: &Node) -> bool {
    let ids = all_node_ids(root);
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    sorted.len() == ids.len()
}

/// The child the emitted placed insert carries.
fn inserted_child(op: &TreeOp) -> &Node {
    match op {
        TreeOp::InsertChild { child, .. } => child,
        TreeOp::Batch(ops) => match ops.first() {
            Some(TreeOp::InsertChild { child, .. }) => child,
            _ => panic!("expected a placed insert, got {op:?}"),
        },
        other => panic!("expected a placed insert, got {other:?}"),
    }
}

fn at(parent_id: &str, placement: Placement) -> Target {
    Target::new(parent_id, placement)
}

// ─── Unit: place_op ──────────────────────────────────────────────────────────

#[test]
fn place_last_emits_a_bare_insert_child_and_appends() {
    let t = fixture();
    let op = place_op(&t, &leaf("x"), &at("left", Placement::Last)).expect("places");
    assert!(matches!(op, TreeOp::InsertChild { .. }), "{op:?}");
    assert_eq!(child_ids(&applied(&t, &op), "left"), ["a", "b", "c", "x"]);
}

#[test]
fn place_first_emits_batch_insert_then_reorder_and_lands_first() {
    let t = fixture();
    let op = place_op(&t, &leaf("x"), &at("left", Placement::First)).expect("places");
    match &op {
        TreeOp::Batch(ops) => {
            assert!(matches!(ops[0], TreeOp::InsertChild { .. }));
            assert!(matches!(ops[1], TreeOp::ReorderChildren { .. }));
            assert_eq!(ops.len(), 2);
        }
        other => panic!("expected Batch [InsertChild; ReorderChildren], got {other:?}"),
    }
    assert_eq!(child_ids(&applied(&t, &op), "left"), ["x", "a", "b", "c"]);
}

#[test]
fn place_first_into_an_empty_container_stays_a_bare_insert() {
    let t = fixture();
    let op = place_op(&t, &leaf("x"), &at("empty", Placement::First)).expect("places");
    assert!(matches!(op, TreeOp::InsertChild { .. }), "{op:?}");
    assert_eq!(child_ids(&applied(&t, &op), "empty"), ["x"]);
}

#[test]
fn place_before_an_interior_sibling_lands_immediately_before_it() {
    let t = fixture();
    let op = place_op(
        &t,
        &leaf("x"),
        &at("left", Placement::Before("b".to_string())),
    )
    .expect("places");
    assert_eq!(child_ids(&applied(&t, &op), "left"), ["a", "x", "b", "c"]);
}

#[test]
fn place_after_the_last_sibling_stays_a_bare_insert() {
    let t = fixture();
    let op = place_op(
        &t,
        &leaf("x"),
        &at("left", Placement::After("c".to_string())),
    )
    .expect("places");
    assert!(matches!(op, TreeOp::InsertChild { .. }), "{op:?}");
    assert_eq!(child_ids(&applied(&t, &op), "left"), ["a", "b", "c", "x"]);
}

#[test]
fn place_absent_parent_is_refused_as_the_apply_engine_would_refuse_it() {
    let t = fixture();
    assert_eq!(
        place_op(&t, &leaf("x"), &at("ghost", Placement::Last)).unwrap_err(),
        PlaceError::ParentNotFound("ghost".to_string())
    );
    assert_eq!(
        refused_as(
            &t,
            &TreeOp::InsertChild {
                parent_id: "ghost".to_string(),
                child: leaf("x"),
            }
        ),
        ApplyErrorCode::ParentNotFound
    );
}

#[test]
fn place_childless_parent_is_refused_as_the_apply_engine_would_refuse_it() {
    let t = fixture();
    assert_eq!(
        place_op(&t, &leaf("x"), &at("solo", Placement::Last)).unwrap_err(),
        PlaceError::ChildlessKind("solo".to_string())
    );
    assert_eq!(
        refused_as(
            &t,
            &TreeOp::InsertChild {
                parent_id: "solo".to_string(),
                child: leaf("x"),
            }
        ),
        ApplyErrorCode::ChildlessKind
    );
}

#[test]
fn place_duplicate_id_is_refused_as_the_apply_engine_would_refuse_it() {
    let t = fixture();
    assert_eq!(
        place_op(&t, &leaf("a"), &at("right", Placement::Last)).unwrap_err(),
        PlaceError::DuplicateId("a".to_string())
    );
    assert_eq!(
        refused_as(
            &t,
            &TreeOp::InsertChild {
                parent_id: "right".to_string(),
                child: leaf("a"),
            }
        ),
        ApplyErrorCode::DuplicateNodeId
    );
}

#[test]
fn place_unknown_anchor_is_refused_matching_the_reorders_ordering_mismatch() {
    let t = fixture();
    // "d" exists in the tree but is not a child of "left".
    assert_eq!(
        place_op(
            &t,
            &leaf("x"),
            &at("left", Placement::Before("d".to_string()))
        )
        .unwrap_err(),
        PlaceError::UnknownAnchor("d".to_string())
    );
    // The only op that could honour the anchor names it in a reorder, which the
    // apply engine refuses.
    assert_eq!(
        refused_as(
            &t,
            &TreeOp::ReorderChildren {
                parent_id: "left".to_string(),
                new_order: vec![
                    "d".to_string(),
                    "a".to_string(),
                    "b".to_string(),
                    "c".to_string()
                ],
            }
        ),
        ApplyErrorCode::OrderingMismatch
    );
}

// ─── Unit: move_op / can_place ───────────────────────────────────────────────

#[test]
fn move_cross_parent_last_emits_a_bare_move_node() {
    let t = fixture();
    let op = move_op(&t, "a", &at("right", Placement::Last)).expect("moves");
    assert!(matches!(op, TreeOp::MoveNode { .. }), "{op:?}");
    let updated = applied(&t, &op);
    assert_eq!(child_ids(&updated, "right"), ["d", "a"]);
    assert_eq!(child_ids(&updated, "left"), ["b", "c"]);
}

#[test]
fn move_same_parent_re_placement_emits_batch_move_then_reorder() {
    let t = fixture();
    let op = move_op(&t, "c", &at("left", Placement::Before("a".to_string()))).expect("moves");
    match &op {
        TreeOp::Batch(ops) => {
            assert!(matches!(ops[0], TreeOp::MoveNode { .. }));
            assert!(matches!(ops[1], TreeOp::ReorderChildren { .. }));
        }
        other => panic!("expected Batch [MoveNode; ReorderChildren], got {other:?}"),
    }
    assert_eq!(child_ids(&applied(&t, &op), "left"), ["c", "a", "b"]);
}

#[test]
fn move_into_itself_is_refused_as_the_apply_engine_would_refuse_it() {
    let t = fixture();
    assert_eq!(
        move_op(&t, "left", &at("left", Placement::Last)).unwrap_err(),
        PlaceError::MoveIntoSelf("left".to_string())
    );
    assert_eq!(
        refused_as(
            &t,
            &TreeOp::MoveNode {
                target: "left".to_string(),
                new_parent_id: "left".to_string(),
            }
        ),
        ApplyErrorCode::KindMismatch
    );
}

#[test]
fn move_into_a_descendant_is_refused_as_the_apply_engine_would_refuse_it() {
    let t = fixture();
    assert_eq!(
        move_op(&t, "root", &at("left", Placement::Last)).unwrap_err(),
        PlaceError::MoveIntoDescendant {
            node_id: "root".to_string(),
            parent_id: "left".to_string(),
        }
    );
    assert_eq!(
        refused_as(
            &t,
            &TreeOp::MoveNode {
                target: "root".to_string(),
                new_parent_id: "left".to_string(),
            }
        ),
        ApplyErrorCode::KindMismatch
    );
}

#[test]
fn move_absent_node_is_refused_as_the_apply_engine_would_refuse_it() {
    let t = fixture();
    assert_eq!(
        move_op(&t, "ghost", &at("left", Placement::Last)).unwrap_err(),
        PlaceError::NodeNotFound("ghost".to_string())
    );
    assert_eq!(
        refused_as(
            &t,
            &TreeOp::MoveNode {
                target: "ghost".to_string(),
                new_parent_id: "left".to_string(),
            }
        ),
        ApplyErrorCode::NodeNotFound
    );
}

#[test]
fn move_into_a_childless_destination_is_refused_as_the_apply_engine_would_refuse_it() {
    let t = fixture();
    assert_eq!(
        move_op(&t, "a", &at("solo", Placement::Last)).unwrap_err(),
        PlaceError::ChildlessKind("solo".to_string())
    );
    assert_eq!(
        refused_as(
            &t,
            &TreeOp::MoveNode {
                target: "a".to_string(),
                new_parent_id: "solo".to_string(),
            }
        ),
        ApplyErrorCode::ChildlessKind
    );
}

#[test]
fn anchoring_a_move_on_the_moved_node_itself_is_an_unknown_anchor() {
    let t = fixture();
    assert_eq!(
        move_op(&t, "a", &at("right", Placement::After("a".to_string()))).unwrap_err(),
        PlaceError::UnknownAnchor("a".to_string())
    );
}

#[test]
fn a_node_in_a_non_structural_position_is_not_movable() {
    let t = non_structural_fixture();
    assert_eq!(
        move_op(&t, "ph", &at("box", Placement::Last)).unwrap_err(),
        PlaceError::NodeNotFound("ph".to_string())
    );
    assert_eq!(
        refused_as(
            &t,
            &TreeOp::MoveNode {
                target: "ph".to_string(),
                new_parent_id: "box".to_string(),
            }
        ),
        ApplyErrorCode::NodeNotFound
    );
}

#[test]
fn can_place_agrees_with_move_op_on_the_legal_drop() {
    let t = fixture();
    assert_eq!(
        can_place(&t, "a", &at("right", Placement::Before("d".to_string()))),
        Ok(())
    );
}

// ─── Unit: nudge_op ──────────────────────────────────────────────────────────

#[test]
fn nudge_minus_one_swaps_with_the_previous_sibling() {
    let t = fixture();
    let op = nudge_op(&t, "b", -1).expect("nudges");
    assert!(matches!(op, TreeOp::ReorderChildren { .. }), "{op:?}");
    assert_eq!(child_ids(&applied(&t, &op), "left"), ["b", "a", "c"]);
}

#[test]
fn nudge_plus_two_swaps_across_the_list() {
    let t = fixture();
    let op = nudge_op(&t, "a", 2).expect("nudges");
    assert_eq!(child_ids(&applied(&t, &op), "left"), ["c", "b", "a"]);
}

#[test]
fn the_first_sibling_cannot_move_up() {
    assert_eq!(
        nudge_op(&fixture(), "a", -1).unwrap_err(),
        PlaceError::NudgeOutOfRange {
            node_id: "a".to_string(),
            delta: -1,
        }
    );
}

#[test]
fn the_last_sibling_cannot_move_down() {
    assert_eq!(
        nudge_op(&fixture(), "c", 1).unwrap_err(),
        PlaceError::NudgeOutOfRange {
            node_id: "c".to_string(),
            delta: 1,
        }
    );
}

#[test]
fn the_root_has_no_siblings_to_nudge_among() {
    assert_eq!(
        nudge_op(&fixture(), "root", 1).unwrap_err(),
        PlaceError::CannotNudgeRoot("root".to_string())
    );
}

#[test]
fn nudging_an_absent_node_is_refused() {
    assert_eq!(
        nudge_op(&fixture(), "ghost", 1).unwrap_err(),
        PlaceError::NodeNotFound("ghost".to_string())
    );
}

// ─── Unit: duplicate_op / paste_op ───────────────────────────────────────────

#[test]
fn duplicate_places_a_fresh_id_clone_beside_its_source() {
    let t = fixture();
    let op = duplicate_op(
        &t,
        "left",
        &at("root", Placement::After("left".to_string())),
    )
    .expect("duplicates");
    let updated = applied(&t, &op);
    assert_eq!(
        child_ids(&updated, "root"),
        ["left", "left-copy", "solo", "right", "empty"]
    );
    assert_eq!(
        child_ids(&updated, "left-copy"),
        ["a-copy", "b-copy", "c-copy"]
    );
    assert!(all_distinct(&updated), "no id collides anywhere");
}

#[test]
fn duplicate_is_structurally_equal_to_its_source_modulo_ids() {
    let t = fixture();
    let op = duplicate_op(&t, "left", &at("right", Placement::Last)).expect("duplicates");
    let source = introspect::get_node(&t, "left").expect("fixture");
    assert_eq!(kind_shape(inserted_child(&op)), kind_shape(source));
}

#[test]
fn the_injectable_strategy_mints_deterministic_sequential_ids() {
    let t = fixture();
    let op = duplicate_op_with(
        &mut SequentialIds::new("dup"),
        &t,
        "left",
        &at("root", Placement::Last),
    )
    .expect("duplicates");
    let updated = applied(&t, &op);
    assert_eq!(child_ids(&updated, "root").last().unwrap(), "dup-1");
    assert_eq!(child_ids(&updated, "dup-1"), ["dup-2", "dup-3", "dup-4"]);
}

#[test]
fn duplicate_remaps_ids_held_in_non_structural_positions_too() {
    // `ph` lives in a State slot — invisible to the structural child lists, but
    // inside the tree-wide id-uniqueness contract.
    let t = non_structural_fixture();
    let op = duplicate_op(&t, "m", &at("root", Placement::Last)).expect("duplicates");
    let updated = applied(&t, &op);
    assert!(all_distinct(&updated), "the State-slot id was remapped");
    assert!(all_node_ids(&updated).contains(&"ph-copy".to_string()));
}

#[test]
fn duplicate_of_an_absent_source_is_refused() {
    assert_eq!(
        duplicate_op(&fixture(), "ghost", &at("root", Placement::Last)).unwrap_err(),
        PlaceError::NodeNotFound("ghost".to_string())
    );
}

#[test]
fn paste_remaps_colliding_ids_and_preserves_the_rest() {
    let t = fixture();
    // Lifted from a different tree: "left" and "a" collide with the target,
    // "z" does not.
    let foreign = container("left", &[leaf_json("a"), leaf_json("z")]);
    let op = paste_op(&t, &foreign, &at("right", Placement::Last)).expect("pastes");
    let updated = applied(&t, &op);
    assert_eq!(child_ids(&updated, "right"), ["d", "left-copy"]);
    assert_eq!(child_ids(&updated, "left-copy"), ["a-copy", "z"]);
    assert!(all_distinct(&updated), "no id collides anywhere");
}

#[test]
fn paste_with_no_collisions_preserves_every_id() {
    let t = fixture();
    let foreign = container("p", &[leaf_json("q")]);
    let op = paste_op(&t, &foreign, &at("empty", Placement::Last)).expect("pastes");
    let updated = applied(&t, &op);
    assert_eq!(child_ids(&updated, "empty"), ["p"]);
    assert_eq!(child_ids(&updated, "p"), ["q"]);
}

#[test]
fn paste_under_the_deterministic_strategy_mints_sequential_ids() {
    let t = fixture();
    let foreign = container("left", &[leaf_json("a")]);
    let op = paste_op_with(
        &mut SequentialIds::new("pasted"),
        &t,
        &foreign,
        &at("empty", Placement::Last),
    )
    .expect("pastes");
    let updated = applied(&t, &op);
    assert_eq!(child_ids(&updated, "empty"), ["pasted-1"]);
    assert_eq!(child_ids(&updated, "pasted-1"), ["pasted-2"]);
}

#[test]
fn the_derived_strategy_probes_past_an_id_already_taken() {
    // `left-copy` is already in the tree, so duplicating `left` must reach for
    // `left-copy-2` rather than colliding.
    let t = container(
        "root",
        &[
            container_json("left", &[leaf_json("a")]),
            leaf_json("left-copy"),
        ],
    );
    let op = duplicate_op_with(&mut DerivedIds, &t, "left", &at("root", Placement::Last))
        .expect("duplicates");
    let updated = applied(&t, &op);
    assert_eq!(
        child_ids(&updated, "root"),
        ["left", "left-copy", "left-copy-2"]
    );
    assert!(all_distinct(&updated));
}

// ─── The exhaustive correspondence enumeration ───────────────────────────────

/// Every node id in `t`, plus an id no tree carries — so the enumeration
/// generates the legal case and every illegal class alike.
fn universe(t: &Node) -> Vec<String> {
    let mut ids = all_node_ids(t);
    ids.push("ghost".to_string());
    ids
}

fn placements(t: &Node) -> Vec<Placement> {
    let mut out = vec![Placement::Last, Placement::First];
    for id in universe(t) {
        out.push(Placement::Before(id.clone()));
        out.push(Placement::After(id));
    }
    out
}

fn targets(t: &Node) -> Vec<Target> {
    let mut out = Vec::new();
    for parent_id in universe(t) {
        for placement in placements(t) {
            out.push(Target::new(parent_id.clone(), placement));
        }
    }
    out
}

/// The moved / inserted node sits exactly where the placement declared, and the
/// other siblings keep their relative order.
fn declared_order_holds(before: &Node, after: &Node, moved: &str, target: &Target) -> bool {
    let ids = child_ids(after, &target.parent_id);
    let Some(idx) = ids.iter().position(|id| id == moved) else {
        return false;
    };
    let others_after: Vec<&String> = ids.iter().filter(|id| *id != moved).collect();
    let before_ids = child_ids(before, &target.parent_id);
    let others_before: Vec<&String> = before_ids.iter().filter(|id| *id != moved).collect();
    if others_after != others_before {
        return false;
    }
    match &target.placement {
        Placement::Last => idx == ids.len() - 1,
        Placement::First => idx == 0,
        Placement::Before(a) => idx + 1 < ids.len() && &ids[idx + 1] == a,
        Placement::After(a) => idx > 0 && &ids[idx - 1] == a,
    }
}

/// The `UnknownAnchor` tightening, checked rather than asserted. The anchor must
/// genuinely be absent from the destination's post-op children — a refusal where
/// the anchor WAS available is a false refuse — and then one of exactly two
/// honest cases holds:
///
///  * **The anchor names the moved node itself.** No op in the vocabulary places
///    a node relative to itself, so there is no apply-side twin to point at, and
///    claiming one would be dishonest. The refusal is the only available answer.
///  * **The anchor is not a child of the destination at all.** The only op that
///    could have honoured it is a `ReorderChildren` naming it, and the engine
///    refuses that as `OrderingMismatch` — asserted here against the real engine.
fn unknown_anchor_corresponds(t: &Node, target: &Target, moved: &str, anchor: &str) -> bool {
    if target.placement.anchor() != Some(anchor) {
        return false;
    }
    let current = child_ids(t, &target.parent_id);
    if current.iter().any(|id| id == anchor && id != moved) {
        return false;
    }
    if anchor == moved {
        return true;
    }
    let mut order = vec![anchor.to_string()];
    order.extend(current);
    apply(
        t,
        &TreeOp::ReorderChildren {
            parent_id: target.parent_id.clone(),
            new_order: order,
        },
    )
    .err()
    .map(|e| e.code)
        == Some(ApplyErrorCode::OrderingMismatch)
}

fn place_corresponds(t: &Node, target: &Target) -> Result<(), String> {
    let fresh = leaf("fresh-child");
    let naive = TreeOp::InsertChild {
        parent_id: target.parent_id.clone(),
        child: fresh.clone(),
    };
    match place_op(t, &fresh, target) {
        Ok(op) => match apply(t, &op) {
            Err(e) => Err(format!("emitted op refused: {e}")),
            Ok(outcome) => {
                if declared_order_holds(t, &outcome.new_tree, "fresh-child", target) {
                    Ok(())
                } else {
                    Err("the applied tree does not exhibit the declared order".to_string())
                }
            }
        },
        // The insert path checks in the engine's own order, so the
        // correspondence here is exact CODE equality.
        Err(PlaceError::ParentNotFound(p)) if p == target.parent_id => {
            expect_code(t, &naive, ApplyErrorCode::ParentNotFound)
        }
        Err(PlaceError::ChildlessKind(p)) if p == target.parent_id => {
            expect_code(t, &naive, ApplyErrorCode::ChildlessKind)
        }
        Err(PlaceError::DuplicateId(_)) => expect_code(t, &naive, ApplyErrorCode::DuplicateNodeId),
        Err(PlaceError::UnknownAnchor(a)) => {
            if unknown_anchor_corresponds(t, target, "fresh-child", &a) {
                Ok(())
            } else {
                Err(format!("UnknownAnchor('{a}') has no apply-side twin"))
            }
        }
        Err(other) => Err(format!("unexpected refusal {other:?}")),
    }
}

fn expect_code(t: &Node, op: &TreeOp, code: ApplyErrorCode) -> Result<(), String> {
    match apply(t, op) {
        Err(e) if e.code == code => Ok(()),
        Err(e) => Err(format!("apply refused as {:?}, expected {code:?}", e.code)),
        Ok(_) => Err(format!(
            "apply accepted an op the helper refused ({code:?})"
        )),
    }
}

fn move_corresponds(t: &Node, moved: &str, target: &Target) -> Result<(), String> {
    let naive = TreeOp::MoveNode {
        target: moved.to_string(),
        new_parent_id: target.parent_id.clone(),
    };
    match move_op(t, moved, target) {
        Ok(op) => match apply(t, &op) {
            Err(e) => Err(format!("emitted op refused: {e}")),
            Ok(outcome) => {
                if !declared_order_holds(t, &outcome.new_tree, moved, target) {
                    return Err("the applied tree does not exhibit the declared order".to_string());
                }
                // It MOVED — it did not get copied.
                let occurrences = all_node_ids(&outcome.new_tree)
                    .iter()
                    .filter(|id| *id == moved)
                    .count();
                if occurrences == 1 {
                    Ok(())
                } else {
                    Err(format!(
                        "'{moved}' occurs {occurrences} times after the move"
                    ))
                }
            }
        },
        // Refusal ↔ refusal: the engine checks self / descendant before it checks
        // structural addressability, so a node failing both is named by one code
        // here and the other there. What must hold is that the bare MoveNode is
        // refused too — no false refuse.
        Err(PlaceError::NodeNotFound(_))
        | Err(PlaceError::MoveIntoSelf(_))
        | Err(PlaceError::MoveIntoDescendant { .. })
        | Err(PlaceError::ParentNotFound(_))
        | Err(PlaceError::ChildlessKind(_)) => {
            if apply(t, &naive).is_err() {
                Ok(())
            } else {
                Err("the bare MoveNode the helper refused was accepted by apply".to_string())
            }
        }
        Err(PlaceError::UnknownAnchor(a)) => {
            if unknown_anchor_corresponds(t, target, moved, &a) {
                Ok(())
            } else {
                Err(format!("UnknownAnchor('{a}') has no apply-side twin"))
            }
        }
        Err(other) => Err(format!("unexpected refusal {other:?}")),
    }
}

fn can_place_agrees_with_move_op(t: &Node, moved: &str, target: &Target) -> Result<(), String> {
    match (can_place(t, moved, target), move_op(t, moved, target)) {
        (Ok(()), Ok(_)) => Ok(()),
        (Err(a), Err(b)) if a == b => Ok(()),
        (a, b) => Err(format!("can_place said {a:?}, move_op said {:?}", b.err())),
    }
}

fn nudge_corresponds(t: &Node, node_id: &str, delta: i32) -> Result<(), String> {
    let parent = introspect::all_nodes(t)
        .into_iter()
        .find(|p| child_ids(t, &p.id).iter().any(|c| c == node_id))
        .map(|p| p.id.clone());
    match nudge_op(t, node_id, delta) {
        Ok(op) => {
            let Some(parent_id) = parent else {
                return Err("nudge emitted an op for a node with no layout parent".to_string());
            };
            let before = child_ids(t, &parent_id);
            let index = before.iter().position(|id| id == node_id).expect("held");
            let swap = (index as i64 + i64::from(delta)) as usize;
            let mut expected = before.clone();
            expected.swap(index, swap);
            match apply(t, &op) {
                Err(e) => Err(format!("emitted op refused: {e}")),
                Ok(outcome) if child_ids(&outcome.new_tree, &parent_id) == expected => Ok(()),
                Ok(_) => Err("the nudge did not land the declared swap".to_string()),
            }
        }
        Err(PlaceError::CannotNudgeRoot(n)) if n == node_id && t.id == node_id => Ok(()),
        Err(PlaceError::NodeNotFound(_)) if t.id != node_id && parent.is_none() => Ok(()),
        Err(PlaceError::NudgeOutOfRange { .. }) => {
            let Some(parent_id) = parent else {
                return Err("out-of-range refusal for a node with no layout parent".to_string());
            };
            let siblings = child_ids(t, &parent_id);
            let index = siblings.iter().position(|id| id == node_id).expect("held") as i64;
            if index + i64::from(delta) < 0 || index + i64::from(delta) >= siblings.len() as i64 {
                Ok(())
            } else {
                Err("out-of-range refusal for an in-range nudge".to_string())
            }
        }
        Err(other) => Err(format!("unexpected refusal {other:?}")),
    }
}

fn duplicate_corresponds(t: &Node, source: &str, target: &Target) -> Result<(), String> {
    match duplicate_op(t, source, target) {
        Ok(op) => {
            let Some(source_node) = introspect::get_node(t, source) else {
                return Err("duplicate emitted an op for an absent source".to_string());
            };
            let expected_growth = all_node_ids(source_node).len();
            match apply(t, &op) {
                Err(e) => Err(format!("emitted op refused: {e}")),
                Ok(outcome) => {
                    let updated = outcome.new_tree;
                    if !all_distinct(&updated) {
                        return Err("the duplicate collided with an existing id".to_string());
                    }
                    if all_node_ids(&updated).len() != all_node_ids(t).len() + expected_growth {
                        return Err("the tree did not grow by exactly the subtree".to_string());
                    }
                    if kind_shape(inserted_child(&op)) != kind_shape(source_node) {
                        return Err("the clone is not structurally equal to its source".to_string());
                    }
                    Ok(())
                }
            }
        }
        Err(PlaceError::NodeNotFound(n)) if n == source => {
            if introspect::get_node(t, source).is_none() {
                Ok(())
            } else {
                Err("refused a source the tree carries".to_string())
            }
        }
        Err(PlaceError::ParentNotFound(p)) if p == target.parent_id => Ok(()),
        Err(PlaceError::ChildlessKind(p)) if p == target.parent_id => Ok(()),
        Err(PlaceError::UnknownAnchor(a)) => {
            if target.placement.anchor() == Some(a.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "UnknownAnchor('{a}') names no anchor of this target"
                ))
            }
        }
        Err(other) => Err(format!("unexpected refusal {other:?}")),
    }
}

fn paste_corresponds(
    source_tree: &Node,
    target_tree: &Node,
    source: &str,
    target: &Target,
) -> Result<(), String> {
    let Some(lifted) = introspect::get_node(source_tree, source) else {
        return Ok(()); // a ghost source is not the paste contract under test
    };
    match paste_op(target_tree, lifted, target) {
        Ok(op) => match apply(target_tree, &op) {
            Err(e) => Err(format!("emitted op refused: {e}")),
            Ok(outcome) => {
                let updated = outcome.new_tree;
                if !all_distinct(&updated) {
                    return Err("the paste collided with an existing id".to_string());
                }
                let before: Vec<String> = all_node_ids(target_tree);
                let ids = all_node_ids(&updated);
                // An id the target did NOT already carry survives the paste.
                for id in all_node_ids(lifted) {
                    if !before.contains(&id) && !ids.contains(&id) {
                        return Err(format!("the non-colliding id '{id}' was remapped anyway"));
                    }
                }
                Ok(())
            }
        },
        Err(PlaceError::ParentNotFound(p)) if p == target.parent_id => Ok(()),
        Err(PlaceError::ChildlessKind(p)) if p == target.parent_id => Ok(()),
        Err(PlaceError::UnknownAnchor(a)) => {
            if target.placement.anchor() == Some(a.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "UnknownAnchor('{a}') names no anchor of this target"
                ))
            }
        }
        Err(other) => Err(format!("unexpected refusal {other:?}")),
    }
}

fn report(failures: &[String], checked: usize, what: &str) {
    assert!(
        failures.is_empty(),
        "{} of {checked} {what} cases failed:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(checked > 0, "the {what} enumeration checked nothing");
}

#[test]
fn place_op_emits_the_declared_order_and_mirrors_the_apply_engine() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for t in corpus() {
        for target in targets(&t) {
            checked += 1;
            if let Err(why) = place_corresponds(&t, &target) {
                failures.push(format!("{target:?}: {why}"));
            }
        }
    }
    report(&failures, checked, "place");
}

#[test]
fn move_op_emits_the_declared_order_and_mirrors_the_apply_engine() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for t in corpus() {
        for moved in universe(&t) {
            for target in targets(&t) {
                checked += 1;
                if let Err(why) = move_corresponds(&t, &moved, &target) {
                    failures.push(format!("move '{moved}' → {target:?}: {why}"));
                }
            }
        }
    }
    report(&failures, checked, "move");
}

#[test]
fn can_place_agrees_with_move_op_verdict_for_verdict() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for t in corpus() {
        for moved in universe(&t) {
            for target in targets(&t) {
                checked += 1;
                if let Err(why) = can_place_agrees_with_move_op(&t, &moved, &target) {
                    failures.push(format!("move '{moved}' → {target:?}: {why}"));
                }
            }
        }
    }
    report(&failures, checked, "can_place");
}

#[test]
fn nudge_op_lands_the_declared_swap_and_its_refusals_are_honest() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for t in corpus() {
        for node_id in universe(&t) {
            for delta in -2i32..=2 {
                checked += 1;
                if let Err(why) = nudge_corresponds(&t, &node_id, delta) {
                    failures.push(format!("nudge '{node_id}' by {delta}: {why}"));
                }
            }
        }
    }
    report(&failures, checked, "nudge");
}

#[test]
fn duplicate_op_never_collides_grows_by_the_subtree_and_clones_the_shape() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for t in corpus() {
        for source in universe(&t) {
            for target in targets(&t) {
                checked += 1;
                if let Err(why) = duplicate_corresponds(&t, &source, &target) {
                    failures.push(format!("duplicate '{source}' → {target:?}: {why}"));
                }
            }
        }
    }
    report(&failures, checked, "duplicate");
}

#[test]
fn paste_op_remaps_collisions_preserves_the_rest_and_never_duplicates() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    let trees = corpus();
    for (i, source_tree) in trees.iter().enumerate() {
        // Every ordered pair of distinct fixture trees, so a lifted subtree meets
        // both a target that already carries its ids and one that does not.
        for (j, target_tree) in trees.iter().enumerate() {
            if i == j {
                continue;
            }
            for source in universe(source_tree) {
                for target in targets(target_tree) {
                    checked += 1;
                    if let Err(why) = paste_corresponds(source_tree, target_tree, &source, &target)
                    {
                        failures.push(format!("paste '{source}' → {target:?}: {why}"));
                    }
                }
            }
        }
    }
    report(&failures, checked, "paste");
}
