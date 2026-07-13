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

use crate::wire::{Node, NodeKind, TreeOp, encode_node};

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

// Recurse a matched-id pair. `parent`, when present, is `(parent_id, position)`
// — the seat for a `RemoveNode`+`InsertChild` whole-node replace; `None` marks
// the root (a `ReplaceRoot`).
fn diff_node(a: &Node, b: &Node, parent: Option<(&str, usize)>, ops: &mut Vec<TreeOp>) {
    if encode_node(a) == encode_node(b) {
        return; // identical subtree — nothing to do
    }
    let recurse = match (structural_children(a), structural_children(b)) {
        (Some(ak), Some(bk)) => own_content_same(a, b) && child_ids(ak) == child_ids(bk),
        _ => false,
    };
    if recurse {
        let ak = structural_children(a).unwrap();
        let bk = structural_children(b).unwrap();
        for (i, (ca, cb)) in ak.iter().zip(bk.iter()).enumerate() {
            diff_node(ca, cb, Some((a.id.as_str(), i)), ops);
        }
    } else {
        match parent {
            None => ops.push(TreeOp::ReplaceRoot { node: b.clone() }),
            Some((parent_id, position)) => {
                // Remove the old node, re-insert the new one at the same seat —
                // one net-zero length change that preserves sibling positions.
                ops.push(TreeOp::RemoveNode {
                    target: a.id.clone(),
                });
                ops.push(TreeOp::InsertChild {
                    parent_id: parent_id.to_string(),
                    position: position as i64,
                    child: b.clone(),
                });
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
