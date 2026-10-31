//! Structural tree diff → op-script (the What-If mechanic) — given a *before*
//! and an *after* tree, derive the `TreeOp` script that transforms one into the
//! other, so a "what if I changed this?" preview is a real, replayable edit
//! rather than a re-render. The script is **correct by construction**:
//! `apply(diff(a, b), a) == b` for every pair (certified in `tests/diff.rs`).
//!
//! The strategy recurses through structurally-matched children (same ids, same
//! order, unchanged own-content) and, at the shallowest node that actually
//! differs, emits a whole-node replacement — `ReplaceRoot` at the root, else a
//! `RemoveNode` + `InsertChild` at the node's own position (which captures every
//! field: kind, style, state, accessibility). It favours a localised edit over
//! replacing the whole tree, but never at the cost of correctness.
//!
//! The style and the two traits are the granular facets (Phase 2314, the
//! reference engine's rule since Phase 2291, and Phase 2527): when a matched node
//! differs from its counterpart in those and nothing else of its own, the diff
//! emits their edits instead of the replace — `PatchStyle` naming exactly the
//! fields that moved, the whole-record `UpdateStyle` only when all six did, then
//! `UpdateAccessibility` and `UpdateTooltip` — and a layout node's children are
//! then recursed as usual. Every host's diff records these edits as the same ops,
//! so two restyles of different fields of one node carry disjoint field sets
//! wherever they were recorded.

use crate::wire::{Node, NodeKind, SemanticStyle, StyleDelta, TreeOp, encode_node};

// The structural (positional, layout) children of a node — the set a recursion
// or an `InsertChild`/`RemoveNode` addresses. `None` for kinds whose children
// are not a flat positional list (ErrorBoundary / Switch / FragmentDecl): any
// difference there escalates to a whole-node replace.
fn structural_children(node: &Node) -> Option<&[Node]> {
    match &node.kind {
        NodeKind::Box(s) => Some(&s.children),
        NodeKind::SplitPanel(s) => Some(&s.children),
        NodeKind::Tabs(s) => Some(&s.children),
        NodeKind::Stepper(s) => Some(&s.children),
        NodeKind::SummaryList(s) => Some(&s.children),
        NodeKind::Disclosure(s) => Some(&s.children),
        NodeKind::Modal(s) => Some(&s.children),
        NodeKind::ScrollArea(s) => Some(&s.children),
        _ => None,
    }
}

// A clone of `node` with its structural children emptied — for comparing a
// node's *own content* (kind-minus-children, style, state, accessibility)
// independently of what its children became.
fn strip_structural_children(node: &Node) -> Node {
    let mut n = node.clone();
    match &mut n.kind {
        NodeKind::Box(s) => s.children.clear(),
        NodeKind::SplitPanel(s) => s.children.clear(),
        NodeKind::Tabs(s) => s.children.clear(),
        NodeKind::Stepper(s) => s.children.clear(),
        NodeKind::SummaryList(s) => s.children.clear(),
        NodeKind::Disclosure(s) => s.children.clear(),
        NodeKind::Modal(s) => s.children.clear(),
        NodeKind::ScrollArea(s) => s.children.clear(),
        _ => {}
    }
    n
}

fn child_ids(children: &[Node]) -> Vec<&str> {
    children.iter().map(|c| c.id.as_str()).collect()
}

fn own_content_same(a: &Node, b: &Node) -> bool {
    encode_node(&strip_structural_children(a)) == encode_node(&strip_structural_children(b))
}

// The op taking `before` to `after`: none when the records are equal;
// `PatchStyle` naming exactly the fields that moved (a field returning to its
// default is set explicitly — in a delta that is an edit, not an omission); and
// the whole-record `UpdateStyle` only when every field moved.
fn style_edit(target: &str, before: &SemanticStyle, after: &SemanticStyle) -> Option<TreeOp> {
    fn moved<T: PartialEq + Copy>(before: T, after: T) -> Option<T> {
        (before != after).then_some(after)
    }
    let delta = StyleDelta {
        direction: moved(before.direction, after.direction),
        emphasis: moved(before.emphasis, after.emphasis),
        role: moved(before.role, after.role),
        tone: moved(before.tone, after.tone),
        voice: moved(before.voice, after.voice),
        weight: moved(before.weight, after.weight),
    };
    let total = delta.direction.is_some()
        && delta.emphasis.is_some()
        && delta.role.is_some()
        && delta.tone.is_some()
        && delta.voice.is_some()
        && delta.weight.is_some();
    if delta.is_empty() {
        None
    } else if total {
        Some(TreeOp::UpdateStyle {
            target: target.to_string(),
            style: *after,
        })
    } else {
        Some(TreeOp::PatchStyle {
            target: target.to_string(),
            style: delta,
        })
    }
}

// The ops taking `a`'s two traits to `b`'s, each only when it moved. An absent
// accessibility and an empty one are the same trait, as the apply engine stores it.
fn trait_edits(a: &Node, b: &Node) -> Vec<TreeOp> {
    let mut edits = Vec::new();
    let after = b.accessibility.clone().unwrap_or_default();
    if a.accessibility.clone().unwrap_or_default() != after {
        edits.push(TreeOp::UpdateAccessibility {
            target: b.id.clone(),
            accessibility: after,
        });
    }
    if a.tooltip != b.tooltip {
        edits.push(TreeOp::UpdateTooltip {
            target: b.id.clone(),
            tooltip: b.tooltip.clone(),
        });
    }
    edits
}

// Recurse a matched-id pair. `parent`, when present, is `(parent_id, position)`
// — the seat for a `RemoveNode`+`InsertChild` whole-node replace; `None` marks
// the root (a `ReplaceRoot`).
/// `parent` is the node's seat: the parent id, its index there, and the
/// parent's full ordered child id list. The id list is needed because 0.4.0's
/// `InsertChild` APPENDS — a re-inserted node lands last, so unless the seat was
/// already last the original order has to be restated.
fn diff_node(a: &Node, b: &Node, parent: Option<(&str, usize, &[String])>, ops: &mut Vec<TreeOp>) {
    if encode_node(a) == encode_node(b) {
        return; // identical subtree — nothing to do
    }
    // `a` given `b`'s style and traits: what still differs from `b` is everything else.
    let faceted = Node {
        style: b.style,
        accessibility: b.accessibility.clone(),
        tooltip: b.tooltip.clone(),
        ..a.clone()
    };
    let mut edits: Vec<TreeOp> = style_edit(&a.id, &a.style, &b.style).into_iter().collect();
    edits.extend(trait_edits(a, b));
    if encode_node(&faceted) == encode_node(b) {
        ops.extend(edits); // the facets are the only difference
        return;
    }
    let recurse = match (structural_children(a), structural_children(b)) {
        (Some(ak), Some(bk)) => own_content_same(&faceted, b) && child_ids(ak) == child_ids(bk),
        _ => false,
    };
    if recurse {
        ops.extend(edits);
        let ak = structural_children(a).unwrap();
        let bk = structural_children(b).unwrap();
        let sibling_ids: Vec<String> = bk.iter().map(|n| n.id.clone()).collect();
        for (i, (ca, cb)) in ak.iter().zip(bk.iter()).enumerate() {
            diff_node(ca, cb, Some((a.id.as_str(), i, &sibling_ids)), ops);
        }
    } else {
        match parent {
            None => ops.push(TreeOp::ReplaceRoot { node: b.clone() }),
            Some((parent_id, position, sibling_ids)) => {
                // Remove the old node and re-insert it (same id, so no
                // duplicate-id collision). InsertChild appends, so unless the
                // seat was already last, restate the parent's order — an exact
                // permutation, since remove+insert leaves the id SET unchanged.
                ops.push(TreeOp::RemoveNode {
                    target: a.id.clone(),
                });
                ops.push(TreeOp::InsertChild {
                    parent_id: parent_id.to_string(),
                    child: b.clone(),
                });
                if position + 1 != sibling_ids.len() {
                    ops.push(TreeOp::ReorderChildren {
                        parent_id: parent_id.to_string(),
                        new_order: sibling_ids.to_vec(),
                    });
                }
            }
        }
    }
}

/// Derive the `TreeOp` script transforming `before` into `after`. Empty when the
/// trees are identical. When their roots have different ids the whole tree is
/// replaced (`ReplaceRoot`); otherwise the diff localises to the changed nodes.
/// Guarantee: `apply(diff(before, after), before) == after`.
pub fn diff(before: &Node, after: &Node) -> Vec<TreeOp> {
    let mut ops = Vec::new();
    if before.id != after.id {
        ops.push(TreeOp::ReplaceRoot {
            node: after.clone(),
        });
        return ops;
    }
    diff_node(before, after, None, &mut ops);
    ops
}
