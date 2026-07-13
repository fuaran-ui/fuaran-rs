//! Facet-refined author-agnostic 3-way tree merge — the "parallel universes"
//! substrate (Git for Interfaces, the Counterfactual corner). A node decomposes
//! into independent **facets**, each 3-way-merged on its own:
//!
//! - `kind` — the node's own kind fields (children neutralised in the probe);
//! - `style.{tone,weight,emphasis,role,voice}` — the `SemanticStyle` sub-fields,
//!   merged **independently** (A's tone + B's voice auto-blend);
//! - `state` — the `StateBehaviour` block;
//! - `accessibility` — the `Accessibility` block;
//! - `children` — the ordered child-id list (structural).
//!
//! A facet changed on at most one side takes that side's value; both sides
//! changing it differently is a **conflict** (returned, not silently picked).
//! The one structural case auto-merged across both sides is disjoint pure
//! inserts into the same parent, ordered by NodeId Ordinal bytes — the
//! deterministic, wall-clock-free tie-break. Facet equality is `encode_node`
//! canonical bytes (the same oracle the merge corpus commits to), except the
//! closure-free style sub-fields, compared directly.
//!
//! Byte-identical to the sibling hosts' `TreeMerge.merge3Way`; certified against
//! the shared `merge-conformance/` corpus (merged tree + `sha256(tree)` outcome).

use std::collections::{HashMap, HashSet};

use crate::canonical::ordinal_cmp;
use crate::wire::{
    Accessibility, BoxSpec, DisclosureSpec, ModalSpec, Node, NodeKind, ScrollAreaSpec,
    SemanticStyle, SplitPanelSpec, StateBehaviour, StepperSpec, SummaryListSpec, TabsSpec,
    encode_node,
};

/// A `(node_id, facet)` cell that could not be auto-merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    pub node_id: String,
    pub facet: String,
}

/// The outcome of a 3-way merge: the merged tree, or the conflicting cells.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeResult {
    Merged(Box<Node>),
    Conflicts(Vec<MergeConflict>),
}

// ─── child traversal (child-bearing layout kinds — the structural facet) ─────

fn children_of(node: &Node) -> &[Node] {
    match &node.kind {
        NodeKind::Box(s) => &s.children,
        NodeKind::SplitPanel(s) => &s.children,
        NodeKind::Tabs(s) => &s.children,
        NodeKind::Stepper(s) => &s.children,
        NodeKind::SummaryList(s) => &s.children,
        NodeKind::Disclosure(s) => &s.children,
        NodeKind::Modal(s) => &s.children,
        NodeKind::ScrollArea(s) => &s.children,
        _ => &[],
    }
}

/// The kind with its (layout) children replaced. Non-child-bearing kinds carry
/// no structural children, so they are returned unchanged.
fn with_children(kind: &NodeKind, children: Vec<Node>) -> NodeKind {
    match kind {
        NodeKind::Box(s) => NodeKind::Box(BoxSpec {
            children,
            ..s.clone()
        }),
        NodeKind::SplitPanel(s) => NodeKind::SplitPanel(SplitPanelSpec {
            children,
            ..s.clone()
        }),
        NodeKind::Tabs(s) => NodeKind::Tabs(TabsSpec {
            children,
            ..s.clone()
        }),
        NodeKind::Stepper(s) => NodeKind::Stepper(StepperSpec {
            children,
            ..s.clone()
        }),
        NodeKind::SummaryList(s) => NodeKind::SummaryList(SummaryListSpec {
            children,
            ..s.clone()
        }),
        NodeKind::Disclosure(s) => NodeKind::Disclosure(DisclosureSpec {
            children,
            ..s.clone()
        }),
        NodeKind::Modal(s) => NodeKind::Modal(ModalSpec {
            children,
            ..s.clone()
        }),
        NodeKind::ScrollArea(s) => NodeKind::ScrollArea(ScrollAreaSpec {
            children,
            ..s.clone()
        }),
        other => other.clone(),
    }
}

fn childless_kind(kind: &NodeKind) -> NodeKind {
    with_children(kind, vec![])
}

/// Build a node with controlled facets (the merge model: id + kind + state +
/// style + accessibility — `motion` / `extraAttributes` are wire-omitted here).
fn mk_node(
    id: &str,
    kind: NodeKind,
    style: SemanticStyle,
    state: StateBehaviour,
    accessibility: Option<Accessibility>,
) -> Node {
    Node {
        id: id.to_string(),
        kind,
        state,
        style,
        accessibility,
    }
}

// ─── facet-isolation canonical probes (hold every OTHER facet fixed) ─────────

/// Kind-own canonical: children + style + state + accessibility neutralised.
fn kind_canonical(n: &Node) -> String {
    encode_node(&mk_node(
        &n.id,
        childless_kind(&n.kind),
        SemanticStyle::default(),
        StateBehaviour::default(),
        None,
    ))
}

fn state_canonical(shell: &NodeKind, n: &Node) -> String {
    encode_node(&mk_node(
        &n.id,
        shell.clone(),
        SemanticStyle::default(),
        n.state.clone(),
        None,
    ))
}

fn accessibility_canonical(shell: &NodeKind, n: &Node) -> String {
    encode_node(&mk_node(
        &n.id,
        shell.clone(),
        SemanticStyle::default(),
        StateBehaviour::default(),
        n.accessibility.clone(),
    ))
}

// ─── facet pickers ───────────────────────────────────────────────────────────

/// Pick one style sub-field; record a conflict on a genuine divergence.
fn pick_field<T: Copy + PartialEq>(
    conflicts: &mut Vec<MergeConflict>,
    node_id: &str,
    facet: &str,
    base_v: T,
    a_v: T,
    b_v: T,
) -> T {
    let a_changed = a_v != base_v;
    let b_changed = b_v != base_v;
    if a_changed && b_changed && a_v != b_v {
        conflicts.push(MergeConflict {
            node_id: node_id.to_string(),
            facet: facet.to_string(),
        });
        return base_v;
    }
    if a_changed {
        a_v
    } else if b_changed {
        b_v
    } else {
        base_v
    }
}

/// Pick a canonical-compared facet; returns which side won (0=base, 1=a, 2=b).
fn pick_canonical(
    conflicts: &mut Vec<MergeConflict>,
    node_id: &str,
    facet: &str,
    base_c: &str,
    a_c: &str,
    b_c: &str,
) -> u8 {
    let a_changed = a_c != base_c;
    let b_changed = b_c != base_c;
    if a_changed && b_changed && a_c != b_c {
        conflicts.push(MergeConflict {
            node_id: node_id.to_string(),
            facet: facet.to_string(),
        });
        return 0;
    }
    if a_changed {
        1
    } else if b_changed {
        2
    } else {
        0
    }
}

fn merge_style(
    conflicts: &mut Vec<MergeConflict>,
    id: &str,
    base: &Node,
    a: &Node,
    b: &Node,
) -> SemanticStyle {
    let (bs, as_, bsb) = (&base.style, &a.style, &b.style);
    SemanticStyle {
        tone: pick_field(conflicts, id, "style.tone", bs.tone, as_.tone, bsb.tone),
        weight: pick_field(
            conflicts,
            id,
            "style.weight",
            bs.weight,
            as_.weight,
            bsb.weight,
        ),
        emphasis: pick_field(
            conflicts,
            id,
            "style.emphasis",
            bs.emphasis,
            as_.emphasis,
            bsb.emphasis,
        ),
        role: pick_field(conflicts, id, "style.role", bs.role, as_.role, bsb.role),
        voice: pick_field(conflicts, id, "style.voice", bs.voice, as_.voice, bsb.voice),
    }
}

/// `true` when `head_ids` is `base_ids` with zero removals and zero reorders.
fn is_pure_addition(base_ids: &[String], head_ids: &[String]) -> bool {
    let head_set: HashSet<&String> = head_ids.iter().collect();
    let survive: Vec<&String> = base_ids.iter().filter(|i| head_set.contains(i)).collect();
    let base_set: HashSet<&String> = base_ids.iter().collect();
    let head_kept: Vec<&String> = head_ids.iter().filter(|i| base_set.contains(i)).collect();
    survive.len() == base_ids.len()
        && survive.iter().zip(base_ids).all(|(v, b)| **v == *b)
        && head_kept.len() == base_ids.len()
        && head_kept.iter().zip(base_ids).all(|(v, b)| **v == *b)
}

fn ids_of(node: &Node) -> Vec<String> {
    children_of(node).iter().map(|c| c.id.clone()).collect()
}

fn merge3(conflicts: &mut Vec<MergeConflict>, base: &Node, a: &Node, b: &Node) -> Node {
    let id = base.id.clone();
    let shell = childless_kind(&base.kind);

    // kind facet
    let kind_pick = pick_canonical(
        conflicts,
        &id,
        "kind",
        &kind_canonical(base),
        &kind_canonical(a),
        &kind_canonical(b),
    );
    let kind_source = match kind_pick {
        1 => a,
        2 => b,
        _ => base,
    };

    // style sub-fields (independent)
    let merged_style = merge_style(conflicts, &id, base, a, b);

    // state facet
    let state_pick = pick_canonical(
        conflicts,
        &id,
        "state",
        &state_canonical(&shell, base),
        &state_canonical(&shell, a),
        &state_canonical(&shell, b),
    );
    let merged_state = match state_pick {
        1 => a.state.clone(),
        2 => b.state.clone(),
        _ => base.state.clone(),
    };

    // accessibility facet
    let acc_pick = pick_canonical(
        conflicts,
        &id,
        "accessibility",
        &accessibility_canonical(&shell, base),
        &accessibility_canonical(&shell, a),
        &accessibility_canonical(&shell, b),
    );
    let merged_acc = match acc_pick {
        1 => a.accessibility.clone(),
        2 => b.accessibility.clone(),
        _ => base.accessibility.clone(),
    };

    // children facet (structural)
    let base_ids = ids_of(base);
    let a_ids = ids_of(a);
    let b_ids = ids_of(b);
    let a_struct = a_ids != base_ids;
    let b_struct = b_ids != base_ids;

    let base_map: HashMap<&str, &Node> = children_of(base)
        .iter()
        .map(|c| (c.id.as_str(), c))
        .collect();
    let a_map: HashMap<&str, &Node> = children_of(a).iter().map(|c| (c.id.as_str(), c)).collect();
    let b_map: HashMap<&str, &Node> = children_of(b).iter().map(|c| (c.id.as_str(), c)).collect();

    let recurse_child = |conflicts: &mut Vec<MergeConflict>, cid: &str| -> Node {
        if let Some(bc) = base_map.get(cid) {
            let ac = a_map.get(cid).copied().unwrap_or(bc);
            let bb = b_map.get(cid).copied().unwrap_or(bc);
            merge3(conflicts, bc, ac, bb)
        } else if let Some(ac) = a_map.get(cid) {
            (*ac).clone()
        } else if let Some(bb) = b_map.get(cid) {
            (*bb).clone()
        } else {
            unreachable!("merge3: child id {cid} vanished")
        }
    };

    let merged_children: Vec<Node> = if !a_struct && !b_struct {
        base_ids
            .iter()
            .map(|cid| recurse_child(conflicts, cid))
            .collect()
    } else if a_struct && !b_struct {
        a_ids
            .iter()
            .map(|cid| recurse_child(conflicts, cid))
            .collect()
    } else if !a_struct && b_struct {
        b_ids
            .iter()
            .map(|cid| recurse_child(conflicts, cid))
            .collect()
    } else {
        let base_set: HashSet<&String> = base_ids.iter().collect();
        let a_new: Vec<String> = a_ids
            .iter()
            .filter(|i| !base_set.contains(i))
            .cloned()
            .collect();
        let b_new: Vec<String> = b_ids
            .iter()
            .filter(|i| !base_set.contains(i))
            .cloned()
            .collect();
        let a_new_set: HashSet<&String> = a_new.iter().collect();
        let disjoint = is_pure_addition(&base_ids, &a_ids)
            && is_pure_addition(&base_ids, &b_ids)
            && !b_new.iter().any(|i| a_new_set.contains(i));
        if disjoint {
            let mut merged: Vec<Node> = base_ids
                .iter()
                .map(|cid| recurse_child(conflicts, cid))
                .collect();
            let mut new_ids: Vec<String> = a_new;
            for id in b_new {
                if !new_ids.contains(&id) {
                    new_ids.push(id);
                }
            }
            new_ids.sort_by(|x, y| ordinal_cmp(x, y));
            for cid in &new_ids {
                merged.push(recurse_child(conflicts, cid));
            }
            merged
        } else {
            conflicts.push(MergeConflict {
                node_id: id.clone(),
                facet: "children".to_string(),
            });
            base_ids
                .iter()
                .map(|cid| recurse_child(conflicts, cid))
                .collect()
        }
    };

    let merged_kind = with_children(&childless_kind(&kind_source.kind), merged_children);
    mk_node(&id, merged_kind, merged_style, merged_state, merged_acc)
}

/// Author-agnostic facet 3-way merge of `a` and `b` over their common `base`
/// (all three share the root id). Returns the merged tree on full auto-merge,
/// or the conflicting cells. Deterministic + host-reproducible (NodeId-byte
/// tie-break, no wall-clock).
pub fn merge3_way(base: &Node, a: &Node, b: &Node) -> MergeResult {
    let mut conflicts = Vec::new();
    let merged = merge3(&mut conflicts, base, a, b);
    if conflicts.is_empty() {
        MergeResult::Merged(Box::new(merged))
    } else {
        MergeResult::Conflicts(conflicts)
    }
}
