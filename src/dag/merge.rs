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
    Accessibility, BoxSpec, DisclosureSpec, Emphasis, FontVoice, ModalSpec, Node, NodeKind,
    ScrollAreaSpec, SemanticStyle, SplitPanelSpec, StateBehaviour, StepperSpec, StyleRole,
    StyleWeight, SummaryListSpec, TabsSpec, TextDirection, ToneVariant, encode_node,
};

/// The CLASS of a refusal — the closed cross-host vocabulary the envelope's
/// `class` field spells. This host emits `ConcurrentEdit` and
/// `ReorderVsStructural`; the rest are carried because the vocabulary is the
/// wire contract, not one host's subset of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeConflictClass {
    /// Both sides edited the same cell to different values (the canonical case).
    ConcurrentEdit,
    /// Both sides moved the same node to different parents.
    ConcurrentMove,
    /// One side deleted a node the other modified.
    DeleteModify,
    /// A kind swap destroyed a pinned cell.
    KindSwapOrphansPin,
    /// One side reordered a parent the other structurally changed.
    ReorderVsStructural,
    /// Post-merge whole-tree validation found a combined illegality no single
    /// op produced.
    CombinedCycle,
}

impl MergeConflictClass {
    /// The wire-stable spelling the envelope's `class` field carries.
    pub fn as_str(self) -> &'static str {
        match self {
            MergeConflictClass::ConcurrentEdit => "ConcurrentEdit",
            MergeConflictClass::ConcurrentMove => "ConcurrentMove",
            MergeConflictClass::DeleteModify => "DeleteModify",
            MergeConflictClass::KindSwapOrphansPin => "KindSwapOrphansPin",
            MergeConflictClass::ReorderVsStructural => "ReorderVsStructural",
            MergeConflictClass::CombinedCycle => "CombinedCycle",
        }
    }
}

/// One SIDE of a two-sided refusal: that branch's value for the contended cell,
/// plus the branch's own opaque provenance tag.
///
/// The tag is per-side because `secondary_tag` cannot be: that slot names the
/// tag of the side that LOST TO A PIN, so with no pin held there is no such
/// side, and filling it from the first-argument branch would make the envelope
/// a function of the order the caller passed its branches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeSide {
    pub value: String,
    pub tag: Option<String>,
}

/// A `(node_id, facet)` cell that could not be auto-merged, as a recovery
/// envelope carrying **two views of the same refusal**.
///
/// * `a` / `b` are the SIDES view: the first- and second-argument branches'
///   values for the contended cell, populated on every two-sided refusal. This
///   is what a host needs to show a human what each side wanted, and what a
///   second replica merging the same pair in the OPPOSITE order must agree
///   with — swapping the branches transposes `a` and `b` and rewrites nothing
///   else in the envelope.
/// * `base` / `primary` / `secondary` / `secondary_tag` are the PRECEDENCE
///   view: the LCA value, the pinned winner, and the side that lost to it.
///
/// This host's only entry point is [`merge3_way`], which is **author-agnostic**
/// — neither side is `Primary`, so no pin is ever held and the three precedence
/// slots are always `None`. That is not an omission: a value in either slot IS
/// a precedence claim, so populating one under no pin would assert a precedence
/// the merge never established. They are carried so a precedence-bearing entry
/// point fills them without re-shaping the envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    pub node_id: String,
    pub facet: String,
    pub class: MergeConflictClass,
    /// The LCA's value for the cell — the empty string for an `insert` facet,
    /// whose id exists on neither side of the LCA and so has no base value.
    pub base: String,
    pub a: Option<MergeSide>,
    pub b: Option<MergeSide>,
    pub primary: Option<String>,
    pub secondary: Option<String>,
    pub secondary_tag: Option<String>,
    pub primacy_held: bool,
}

/// A two-sided refusal under the author-agnostic merge: both branches' values
/// recorded, no precedence pin claimed.
fn two_sided(
    node_id: &str,
    facet: &str,
    class: MergeConflictClass,
    base: &str,
    a_value: &str,
    b_value: &str,
) -> MergeConflict {
    MergeConflict {
        node_id: node_id.to_string(),
        facet: facet.to_string(),
        class,
        base: base.to_string(),
        a: Some(MergeSide {
            value: a_value.to_string(),
            tag: None,
        }),
        b: Some(MergeSide {
            value: b_value.to_string(),
            tag: None,
        }),
        primary: None,
        secondary: None,
        secondary_tag: None,
        primacy_held: false,
    }
}

/// The refusal-envelope spelling of a style sub-field's value: the enum CASE
/// NAME, which is what the reference host's envelope carries.
///
/// For five of the six sub-fields that is also the wire spelling, so `as_str`
/// serves. `TextDirection` is the exception — its wire spelling is lowercase
/// (`"auto"`) while its case name is `Auto` — so it spells its own, and a
/// `style.direction` refusal stays byte-identical to the reference host's.
trait StyleFacetValue: Copy + PartialEq {
    fn facet_value(self) -> &'static str;
}

macro_rules! wire_spelled_facet {
    ($($t:ty),+ $(,)?) => {
        $(impl StyleFacetValue for $t {
            fn facet_value(self) -> &'static str {
                self.as_str()
            }
        })+
    };
}

wire_spelled_facet!(ToneVariant, StyleWeight, Emphasis, StyleRole, FontVoice);

impl StyleFacetValue for TextDirection {
    fn facet_value(self) -> &'static str {
        match self {
            TextDirection::Auto => "Auto",
            TextDirection::Ltr => "Ltr",
            TextDirection::Rtl => "Rtl",
        }
    }
}

/// Mirror of the canonical encoder's string escape, kept local so the merge
/// module takes no codec dependency for the envelope (only `"`, `\` and the
/// control characters need escaping; non-ASCII rides through as UTF-8).
fn append_escaped(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn append_side(out: &mut String, side: Option<&MergeSide>) {
    match side {
        None => out.push_str("null"),
        Some(s) => {
            out.push_str("{\"tag\":");
            match &s.tag {
                None => out.push_str("null"),
                Some(t) => append_escaped(out, t),
            }
            out.push_str(",\"value\":");
            append_escaped(out, &s.value);
            out.push('}');
        }
    }
}

/// Order a refusal set deterministically. `(node_id, facet)` is unique within
/// one merge — a facet of a node is merged once — so it totally orders an
/// envelope regardless of the fold's internal emission order.
pub fn sort_canonical(conflicts: &mut [MergeConflict]) {
    conflicts.sort_by(|x, y| {
        ordinal_cmp(&x.node_id, &y.node_id).then_with(|| ordinal_cmp(&x.facet, &y.facet))
    });
}

/// Canonical JSON of a REFUSAL envelope: the conflict set as a sorted array of
/// `{a,b,base,class,facet,nodeId,primacyHeld}` objects (object keys
/// alphabetical, entries in `(node_id, facet)` order). Byte-stable across
/// hosts, so `sha256_hex` over it is the cross-host refusal hash — the
/// determinism artefact for a REFUSED merge, as the outcome hash is for an
/// auto-merge.
///
/// The precedence view is projected as `primacyHeld` alone: `primary` /
/// `secondary` are derivable from the sides plus the pin, and a corpus that
/// committed both would pin the same value twice.
pub fn encode_envelope(conflicts: &[MergeConflict]) -> String {
    let mut sorted = conflicts.to_vec();
    sort_canonical(&mut sorted);
    let mut out = String::from("[");
    for (i, c) in sorted.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"a\":");
        append_side(&mut out, c.a.as_ref());
        out.push_str(",\"b\":");
        append_side(&mut out, c.b.as_ref());
        out.push_str(",\"base\":");
        append_escaped(&mut out, &c.base);
        out.push_str(",\"class\":");
        append_escaped(&mut out, c.class.as_str());
        out.push_str(",\"facet\":");
        append_escaped(&mut out, &c.facet);
        out.push_str(",\"nodeId\":");
        append_escaped(&mut out, &c.node_id);
        out.push_str(",\"primacyHeld\":");
        out.push_str(if c.primacy_held { "true" } else { "false" });
        out.push('}');
    }
    out.push(']');
    out
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
        tooltip: None,
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
fn pick_field<T: StyleFacetValue>(
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
        conflicts.push(two_sided(
            node_id,
            facet,
            MergeConflictClass::ConcurrentEdit,
            base_v.facet_value(),
            a_v.facet_value(),
            b_v.facet_value(),
        ));
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
        conflicts.push(two_sided(
            node_id,
            facet,
            MergeConflictClass::ConcurrentEdit,
            base_c,
            a_c,
            b_c,
        ));
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
        // Phase 1472 - the declared direction is a style facet like every other,
        // merged per field so two lanes changing different facets of one node
        // still fold cleanly.
        direction: pick_field(
            conflicts,
            id,
            "style.direction",
            bs.direction,
            as_.direction,
            bsb.direction,
        ),
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
        } else {
            match (a_map.get(cid), b_map.get(cid)) {
                (Some(ac), Some(bb)) => {
                    // BOTH branches introduced this id. There is no base to
                    // merge against, so agreement is the only clean outcome:
                    // identical content is the shared value, and DIFFERENT
                    // content is a refusal naming the id. Taking the A side
                    // unconditionally — which is what this did before the
                    // shared-children guard below reached the case — is a
                    // silent, arrival-order-dependent pick.
                    let ac_c = encode_node(ac);
                    let bb_c = encode_node(bb);
                    if ac_c == bb_c {
                        return (*ac).clone();
                    }
                    conflicts.push(two_sided(
                        cid,
                        "insert",
                        MergeConflictClass::ConcurrentEdit,
                        // The id exists on neither side of the LCA, so it has
                        // no base value — the empty string, not an encoding of
                        // some node that was never there.
                        "",
                        &ac_c,
                        &bb_c,
                    ));
                    // The merge has already refused, so this value reaches no
                    // caller — but it must not depend on which branch arrived
                    // first either. Same doctrine as the insert tie-break:
                    // order by canonical bytes.
                    if ordinal_cmp(&ac_c, &bb_c) != std::cmp::Ordering::Greater {
                        (*ac).clone()
                    } else {
                        (*bb).clone()
                    }
                }
                (Some(ac), None) => (*ac).clone(),
                (None, Some(bb)) => (*bb).clone(),
                (None, None) => unreachable!("merge3: child id {cid} vanished"),
            }
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
    } else if a_ids == b_ids {
        // Both sides changed the children to the SAME id list — agreement, not
        // a conflict, and the guard every other facet already has. Its absence
        // here made `merge3_way base a a` refuse for any branch that touched
        // children at all. The shared ids' CONTENTS are checked by
        // `recurse_child`, which refuses a same-id-different-content insert
        // rather than defaulting to a side.
        a_ids
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
            conflicts.push(two_sided(
                &id,
                "children",
                MergeConflictClass::ReorderVsStructural,
                &base_ids.join(","),
                &a_ids.join(","),
                &b_ids.join(","),
            ));
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
