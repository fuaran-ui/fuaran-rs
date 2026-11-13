//! The footprint of a tree op, and Fuaran Core's conflict report over two
//! deltas (WIRE_FORMAT.md §34.2, Phase 2644).
//!
//! A footprint says what an op reads and writes, as Fuaran Core's `Footprint`
//! record: the ids it depends on (`reads`), the named parents whose child list it
//! changes (`structure_writes`), the nodes it authors, destroys, relocates or
//! rewrites (`content_writes`), the nodes it removes or rewrites under a parent
//! it cannot name (`unknown_parent_writes`), and the `(node, slot)` pairs it
//! reads or writes without touching the whole node. Two scripts whose
//! footprints fail no clause of Core's `Ops.independent` commute.
//!
//! §34.2 makes the projection itself part of the contract: the pairs a merge by
//! fold judges are exactly the pairs Core's `Dag.conflictsIn` reports over the
//! reference's base-aware footprint (`TreeOpFootprint.ofOpIn`). A coarser
//! projection refuses pairs the reference admits, a finer one admits pairs it
//! refuses, and the `foldFixtures` family catches both. So this module ports the
//! reference's rules arm for arm, and nothing else:
//!
//! - The PURE form ([`footprint_of`]) is computed from the op alone. The
//!   structural ops are Core's skeleton clauses; an in-place edit is Core's
//!   `UpdateNode` footprint — the node read and content-written with every id of
//!   a subtree the op puts in, and an unknown-parent write of the node.
//! - The BASE-AWARE form ([`footprint_in`]) is computed at the tree the op
//!   applies to (Core's `Ops.footprintIn`, fuaran-core Phase 500). A removal
//!   whose target the STRUCTURAL walk locates under a structural parent names
//!   that parent (a read and a structure write) and content-writes the whole
//!   subtree it destroys; a move located the same way, whose destination is
//!   located too, also reads the destination's proper ancestors (the cycle check
//!   reads them). A located in-place edit that applies is declared at the SLOT it
//!   changes — `kind`, `state`, `style`, `style.<field>` beside a read of
//!   `style`, `accessibility`, `tooltip` — with every id it puts in or takes
//!   out. Anything the walk does not locate takes the pure arm. A `Batch` is
//!   threaded member by member, and from an op that does not apply onwards every
//!   op takes the pure arm.
//! - THE CHILD LIST (fuaran#2691, Core 0.42.0 D246). Core compares a structure
//!   write of `p` against a slot access as a read and a write of
//!   `(p, "#children")`, so every arm that reads or rewrites a node's structural
//!   children by slot declares that slot: a resolved reorder and a resolved
//!   move's source parent (beside the `kind` slot the tier holds the list in),
//!   an `EditNode`, and any in-place edit whose held set moved.
//! - A resolved MOVE (fuaran#2651, #2670, #2691) touches no parent whole: the
//!   target is read and written at its `parent` slot, the source parent at its
//!   `kind` and child-list slots, the destination and every ancestor above it
//!   are read at their `parent` slots (the cycle check reads the chain), and
//!   the destination keeps Core's named structure write and read. A resolved
//!   REORDER (fuaran#2670) reads and writes its parent at the `kind` and
//!   child-list slots and reads each listed child at its `parent` slot; one
//!   whose list is the parent's child order where it applies (fuaran#2696)
//!   changes nothing, so it reads both slots and writes neither.
//!
//! Ids are counted over every position a node holds another in (the apply
//! engine's `child_nodes`, the reference's keyed walk: what the kind holds, the
//! subtree of every slot argument, the envelope's `state` alternatives and its
//! `fallback`); the base-aware arms LOCATE through the ordered
//! child lists alone (the apply engine's `layout_children`), as Core's resolution
//! does, so a node held only in a keyed position takes the pure arm.
//!
//! Sets are ordered maps; wherever order is observable (the conflict report)
//! they enumerate in ordinal (UTF-16) order. Pure, total, no IO.

use std::collections::BTreeSet;

use crate::canonical::ordinal_cmp;
use crate::ops::{apply, child_nodes, inserted_nodes, kind_child_nodes, layout_children};
use crate::wire::{Node, StyleDelta, TreeOp};

/// A set of node ids.
pub type IdSet = BTreeSet<String>;

/// A set of `(node id, slot)` pairs.
pub type SlotSet = BTreeSet<(String, String)>;

/// The read/write footprint of a tree op — Fuaran Core's `Footprint` record,
/// every address an id. See the module header for each set's role.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint {
    pub reads: IdSet,
    pub structure_writes: IdSet,
    pub content_writes: IdSet,
    pub unknown_parent_writes: IdSet,
    pub slot_reads: SlotSet,
    pub slot_writes: SlotSet,
}

impl Footprint {
    fn union(mut self, other: Footprint) -> Footprint {
        self.reads.extend(other.reads);
        self.structure_writes.extend(other.structure_writes);
        self.content_writes.extend(other.content_writes);
        self.unknown_parent_writes
            .extend(other.unknown_parent_writes);
        self.slot_reads.extend(other.slot_reads);
        self.slot_writes.extend(other.slot_writes);
        self
    }
}

fn id_set<const N: usize>(ids: [&str; N]) -> IdSet {
    ids.into_iter().map(str::to_owned).collect()
}

// ─── walks ───────────────────────────────────────────────────────────────────

/// A node's ordered child list — the surface the structural ops edit.
pub(crate) fn structural_children(n: &Node) -> &[Node] {
    layout_children(n).map_or(&[], Vec::as_slice)
}

fn state_alternatives(n: &Node) -> Vec<&Node> {
    [&n.state.on_loading, &n.state.on_empty]
        .into_iter()
        .filter_map(|b| b.as_deref())
        .collect()
}

/// Adds every id in `n`'s subtree, `n` included, through every position — the
/// apply engine's own lookup walk (Phase 2706), so the footprint and the apply
/// agree on which positions hold a node.
fn keyed_ids_into(n: &Node, into: &mut IdSet) {
    into.insert(n.id.clone());
    for c in child_nodes(n) {
        keyed_ids_into(c, into);
    }
}

fn keyed_ids(n: &Node) -> IdSet {
    let mut ids = IdSet::new();
    keyed_ids_into(n, &mut ids);
    ids
}

fn ids_of(nodes: &[&Node]) -> IdSet {
    let mut ids = IdSet::new();
    for n in nodes {
        keyed_ids_into(n, &mut ids);
    }
    ids
}

/// Every id `n` holds below itself, through every position.
fn held_ids(n: &Node) -> IdSet {
    let mut ids = keyed_ids(n);
    ids.remove(&n.id);
    ids
}

/// The path from `root` to `id` over the ordered child lists, both ends
/// included, or `None` when the walk does not reach `id`.
fn structural_path<'a>(root: &'a Node, id: &str) -> Option<Vec<&'a Node>> {
    if root.id == id {
        return Some(vec![root]);
    }
    structural_children(root).iter().find_map(|c| {
        structural_path(c, id).map(|mut p| {
            p.insert(0, root);
            p
        })
    })
}

pub(crate) fn find_structural<'a>(root: &'a Node, id: &str) -> Option<&'a Node> {
    structural_path(root, id).and_then(|p| p.last().copied())
}

/// The node whose ordered child list holds `id`, preorder first.
fn structural_parent_of<'a>(root: &'a Node, id: &str) -> Option<&'a Node> {
    let kids = structural_children(root);
    if kids.iter().any(|c| c.id == id) {
        return Some(root);
    }
    kids.iter().find_map(|c| structural_parent_of(c, id))
}

// ─── the pure form ───────────────────────────────────────────────────────────

/// Core's `UpdateNode` footprint: an in-place rewrite of `id` putting
/// `incoming` in.
fn vertical(id: &str, incoming: &[&Node]) -> Footprint {
    let mut touched = ids_of(incoming);
    touched.insert(id.to_owned());
    Footprint {
        reads: touched.clone(),
        content_writes: touched,
        unknown_parent_writes: id_set([id]),
        ..Footprint::default()
    }
}

/// The footprint of one op computed from the op alone. Total over the
/// [`TreeOp`] union.
pub fn footprint_of(op: &TreeOp) -> Footprint {
    match op {
        TreeOp::InsertChild { child, parent_id } => {
            let inserted = keyed_ids(child);
            let mut reads = inserted.clone();
            reads.insert(parent_id.clone());
            Footprint {
                reads,
                structure_writes: id_set([parent_id]),
                content_writes: inserted,
                ..Footprint::default()
            }
        }
        TreeOp::RemoveNode { target } => Footprint {
            reads: id_set([target]),
            content_writes: id_set([target]),
            unknown_parent_writes: id_set([target]),
            ..Footprint::default()
        },
        TreeOp::MoveNode {
            target,
            new_parent_id,
        } => Footprint {
            reads: id_set([target, new_parent_id]),
            structure_writes: id_set([new_parent_id]),
            content_writes: id_set([target]),
            unknown_parent_writes: id_set([target]),
            ..Footprint::default()
        },
        TreeOp::ReorderChildren {
            parent_id,
            new_order,
        } => {
            let mut reads: IdSet = new_order.iter().cloned().collect();
            reads.insert(parent_id.clone());
            Footprint {
                reads,
                structure_writes: id_set([parent_id]),
                ..Footprint::default()
            }
        }
        TreeOp::EditNode { target, .. } | TreeOp::UpdateState { target, .. } => {
            vertical(target, &inserted_nodes(op))
        }
        TreeOp::UpdateProp { target, .. }
        | TreeOp::ReplaceBinding { target, .. }
        | TreeOp::UpdateStyle { target, .. }
        | TreeOp::PatchStyle { target, .. }
        | TreeOp::UpdateAccessibility { target, .. }
        | TreeOp::UpdateTooltip { target, .. } => vertical(target, &[]),
        TreeOp::ReplaceRoot { node } => {
            let ids = keyed_ids(node);
            Footprint {
                reads: ids.clone(),
                content_writes: ids,
                structure_writes: id_set([&node.id]),
                unknown_parent_writes: id_set([&node.id]),
                ..Footprint::default()
            }
        }
        TreeOp::Batch(members) => members
            .iter()
            .fold(Footprint::default(), |acc, m| acc.union(footprint_of(m))),
    }
}

// ─── the base-aware form ─────────────────────────────────────────────────────

/// A slot-precise in-place rewrite of `target`, with every id it moves in or
/// out.
fn slotted(writes: &[String], read_slots: &[&str], target: &str, touched: IdSet) -> Footprint {
    let mut slot_reads = SlotSet::new();
    let mut slot_writes = SlotSet::new();
    for s in writes {
        slot_reads.insert((target.to_owned(), s.clone()));
        slot_writes.insert((target.to_owned(), s.clone()));
    }
    for s in read_slots {
        slot_reads.insert((target.to_owned(), (*s).to_owned()));
    }
    Footprint {
        reads: touched.clone(),
        content_writes: touched,
        slot_reads,
        slot_writes,
        ..Footprint::default()
    }
}

/// The `style.<field>` slots a `PatchStyle` delta sets, by wire member name.
fn style_fields(delta: &StyleDelta) -> Vec<String> {
    let StyleDelta {
        direction,
        emphasis,
        role,
        tone,
        voice,
        weight,
    } = delta;
    // Ordinal order of the wire names, which is also their declaration order.
    [
        ("direction", direction.is_some()),
        ("emphasis", emphasis.is_some()),
        ("role", role.is_some()),
        ("tone", tone.is_some()),
        ("voice", voice.is_some()),
        ("weight", weight.is_some()),
    ]
    .into_iter()
    .filter(|(_, set)| *set)
    .map(|(name, _)| format!("style.{name}"))
    .collect()
}

fn one(slot: &str) -> [String; 1] {
    [slot.to_owned()]
}

/// Core's child-list slot (`Footprint.childListSlot`, fuaran-core Phase 625).
const CHILD_LIST_SLOT: &str = "#children";

/// Core's `slotEdit`: a read and a write of one slot of a node.
fn slot_edit(node: &str, slot: &str) -> Footprint {
    let pair = (node.to_owned(), slot.to_owned());
    Footprint {
        slot_reads: SlotSet::from([pair.clone()]),
        slot_writes: SlotSet::from([pair]),
        ..Footprint::default()
    }
}

/// Core's `readingSlot`: a read of one slot of a node and no write.
fn reading_slot(node: &str, slot: &str) -> Footprint {
    Footprint {
        slot_reads: SlotSet::from([(node.to_owned(), slot.to_owned())]),
        ..Footprint::default()
    }
}

/// `f` with a read and a write of `node`'s child-list slot beside it.
fn child_list(node: &str, f: Footprint) -> Footprint {
    f.union(slot_edit(node, CHILD_LIST_SLOT))
}

/// [`slotted`] for an op whose contract moves no node: where its held set moved
/// anyway (`touched` non-empty), the child list is written too. Empty for every
/// such op the engine has, so the slot is a guard, not an arm.
fn guarded(writes: &[String], read_slots: &[&str], target: &str, touched: IdSet) -> Footprint {
    let empty = touched.is_empty();
    let f = slotted(writes, read_slots, target, touched);
    if empty { f } else { child_list(target, f) }
}

/// The base-aware arm of one non-batch op at `state`, given the tree it reaches
/// (`None` when it does not apply).
fn resolved_at(state: &Node, after: Option<&Node>, op: &TreeOp) -> Footprint {
    // The ids an op that puts no node in moved anyway: a guard, empty for every
    // such op the engine has.
    let held_change = |id: &str, node: &Node| -> IdSet {
        let before = held_ids(node);
        match after.and_then(|t| find_structural(t, id)).map(held_ids) {
            None => before,
            Some(a) if a == before => IdSet::new(),
            Some(a) => before.union(&a).cloned().collect(),
        }
    };

    // A vertical op is slot-precise only where the structural walk locates its
    // node and the op applies; otherwise it is the pure arm.
    let vertical_at = |target: &str, arm: &dyn Fn(&Node) -> Footprint| -> Footprint {
        match (find_structural(state, target), after) {
            (Some(node), Some(_)) => arm(node),
            _ => footprint_of(op),
        }
    };

    match op {
        TreeOp::RemoveNode { target } => {
            match (
                find_structural(state, target),
                structural_parent_of(state, target),
            ) {
                (Some(sub), Some(parent)) => Footprint {
                    reads: id_set([target, &parent.id]),
                    structure_writes: id_set([&parent.id]),
                    content_writes: keyed_ids(sub),
                    ..Footprint::default()
                },
                _ => footprint_of(op),
            }
        }
        // A resolved move (fuaran#2651, #2670, #2691) touches no parent WHOLE.
        // The source parent's child list, held in its `kind`, is the only part
        // of it the move changes: a read and a write of its `kind` and
        // child-list slots. The destination and every ancestor above it are
        // read at their `parent` slot (the cycle check reads the chain), and the
        // destination keeps Core's named read and structure write — the address
        // the fold interleaves two appends at. The target changes its position
        // and nothing of itself: a read and a write of its `parent` slot. A move
        // within one parent keeps the parent whole; an unresolved move is the
        // pure arm.
        TreeOp::MoveNode {
            target,
            new_parent_id,
        } => {
            let located = find_structural(state, target).is_some();
            match (
                located,
                structural_parent_of(state, target),
                structural_path(state, new_parent_id),
            ) {
                (true, Some(parent), Some(destination)) => {
                    let source = (parent.id != *new_parent_id).then_some(parent.id.as_str());
                    let mut f = Footprint {
                        reads: id_set([new_parent_id]),
                        structure_writes: id_set([new_parent_id]),
                        ..Footprint::default()
                    }
                    .union(slot_edit(target, "parent"))
                    .union(reading_slot(new_parent_id, "parent"));
                    for n in &destination[..destination.len() - 1] {
                        if n.id != *target && Some(n.id.as_str()) != source {
                            f = f.union(reading_slot(&n.id, "parent"));
                        }
                    }
                    if let Some(p) = source {
                        f = f.union(child_list(p, slot_edit(p, "kind")));
                    }
                    f
                }
                _ => footprint_of(op),
            }
        }
        // A resolved reorder (fuaran#2670, #2691) permutes the parent's child
        // list, held in its `kind`, and touches no child: the parent is read and
        // written at its `kind` and child-list slots, and each listed child is
        // read at its `parent` slot. An unresolved reorder is the pure arm.
        //
        // An UNCHANGED reorder (fuaran#2696) — its list is the parent's child
        // order in `state`: the same ids, in the same order, of the same length —
        // changes nothing where it applies, so it declares the reads above and no
        // write. Every op that changes the child list writes one of the two slots
        // it still reads, so it stays ordered against each of them, a real
        // reorder of the same parent included (applied second, the unchanged
        // reorder would restore the order it lists). What it frees is a pair of
        // reorders that each restate their own order, and an op that only reads
        // the list.
        TreeOp::ReorderChildren {
            parent_id,
            new_order,
        } => match (after, find_structural(state, parent_id)) {
            (Some(_), Some(parent)) => {
                let held = structural_children(parent);
                let unchanged = held.len() == new_order.len()
                    && held.iter().zip(new_order).all(|(c, id)| c.id == *id);
                let list_slots = if unchanged {
                    reading_slot(parent_id, "kind").union(reading_slot(parent_id, CHILD_LIST_SLOT))
                } else {
                    child_list(parent_id, slot_edit(parent_id, "kind"))
                };
                new_order
                    .iter()
                    .fold(list_slots, |acc, c| acc.union(reading_slot(c, "parent")))
            }
            _ => footprint_of(op),
        },
        TreeOp::EditNode { target, .. } => vertical_at(target, &|node| {
            let mut touched = ids_of(&kind_child_nodes(&node.kind));
            touched.extend(ids_of(&inserted_nodes(op)));
            slotted(
                &["kind".to_owned(), CHILD_LIST_SLOT.to_owned()],
                &[],
                target,
                touched,
            )
        }),
        TreeOp::UpdateProp { target, .. } | TreeOp::ReplaceBinding { target, .. } => {
            vertical_at(target, &|node| {
                guarded(&one("kind"), &[], target, held_change(target, node))
            })
        }
        TreeOp::UpdateState { target, .. } => vertical_at(target, &|node| {
            let mut touched = ids_of(&state_alternatives(node));
            touched.extend(ids_of(&inserted_nodes(op)));
            slotted(&one("state"), &[], target, touched)
        }),
        TreeOp::UpdateStyle { target, .. } => vertical_at(target, &|node| {
            guarded(&one("style"), &[], target, held_change(target, node))
        }),
        TreeOp::UpdateAccessibility { target, .. } => vertical_at(target, &|node| {
            guarded(
                &one("accessibility"),
                &[],
                target,
                held_change(target, node),
            )
        }),
        TreeOp::UpdateTooltip { target, .. } => vertical_at(target, &|node| {
            guarded(&one("tooltip"), &[], target, held_change(target, node))
        }),
        TreeOp::PatchStyle { target, style } => vertical_at(target, &|node| {
            guarded(
                &style_fields(style),
                &["style"],
                target,
                held_change(target, node),
            )
        }),
        TreeOp::ReplaceRoot { .. } => {
            let mut plain = footprint_of(op);
            let replaced = keyed_ids(state);
            plain.reads.extend(replaced.iter().cloned());
            plain.content_writes.extend(replaced);
            plain.structure_writes.insert(state.id.clone());
            plain
        }
        // InsertChild: the pure arm names everything it touches. A Batch never
        // reaches here (`threaded` walks its members).
        TreeOp::InsertChild { .. } | TreeOp::Batch(_) => footprint_of(op),
    }
}

/// The tree `op` reaches from `tree`, or `None` when it does not apply.
pub(crate) fn applied(tree: &Node, op: &TreeOp) -> Option<Node> {
    apply(tree, op).ok().map(|o| o.new_tree)
}

/// One op's footprint at the state the ops before it reached (`None`: an
/// earlier op did not apply, so the pure arm), and the state it reaches.
fn threaded(state: Option<Node>, op: &TreeOp) -> (Footprint, Option<Node>) {
    let Some(state) = state else {
        return (footprint_of(op), None);
    };
    if let TreeOp::Batch(members) = op {
        return members
            .iter()
            .fold((Footprint::default(), Some(state)), |(acc, st), m| {
                let (f, next) = threaded(st, m);
                (acc.union(f), next)
            });
    }
    let after = applied(&state, op);
    (resolved_at(&state, after.as_ref(), op), after)
}

/// The footprint of one op at the tree it applies to: [`footprint_of`]'s
/// address kinds with every parent the pure form cannot name resolved against
/// `state`, and a located in-place edit declared at its slot. A `Batch` is
/// threaded member by member. Total; sound where [`footprint_of`] is and never
/// less precise.
pub fn footprint_in(state: &Node, op: &TreeOp) -> Footprint {
    threaded(Some(state.clone()), op).0
}

/// Each op of a script at the tree the ops before it reach, in order.
pub fn footprint_each_in(base: &Node, script: &[TreeOp]) -> Vec<Footprint> {
    let mut st = Some(base.clone());
    script
        .iter()
        .map(|op| {
            let (f, next) = threaded(st.take(), op);
            st = next;
            f
        })
        .collect()
}

// ─── Core's conflict report (`Dag.conflicts` / `Dag.conflictsIn`) ────────────

/// The four conflict shapes of Core's `MergeConflictShape`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictShape {
    ConcurrentUpdate,
    InsertPositionClash,
    MoveVsRemove,
    SlotClash,
}

impl ConflictShape {
    /// The shape's wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictShape::ConcurrentUpdate => "ConcurrentUpdate",
            ConflictShape::InsertPositionClash => "InsertPositionClash",
            ConflictShape::MoveVsRemove => "MoveVsRemove",
            ConflictShape::SlotClash => "SlotClash",
        }
    }
}

/// One entry of Core's conflict report: the op from the first delta (`left`),
/// the op from the second (`right`), the address they collide on (a node id, or
/// a parent id for `InsertPositionClash`), and the shape. `slot` is `Some`
/// exactly when `shape` is [`ConflictShape::SlotClash`].
#[derive(Debug, Clone, PartialEq)]
pub struct OpConflict {
    pub left: TreeOp,
    pub right: TreeOp,
    pub address: String,
    pub shape: ConflictShape,
    pub slot: Option<String>,
}

/// `ids` in ordinal (UTF-16) order.
fn ordinal_sorted(ids: IdSet) -> Vec<String> {
    let mut out: Vec<String> = ids.into_iter().collect();
    out.sort_by(|x, y| ordinal_cmp(x, y));
    out
}

fn meet(a: &IdSet, b: &IdSet) -> IdSet {
    a.intersection(b).cloned().collect()
}

/// The child-list slots a footprint's structure writes denote — Core's
/// `Footprint.childListSlots` (fuaran-core Phase 625).
fn child_list_slots(f: &Footprint) -> SlotSet {
    f.structure_writes
        .iter()
        .map(|p| (p.clone(), CHILD_LIST_SLOT.to_owned()))
        .collect()
}

/// Every slot a footprint reads or writes.
fn accessed_slots(f: &Footprint) -> SlotSet {
    f.slot_writes.union(&f.slot_reads).cloned().collect()
}

/// The slots one side writes and the other reads or writes, and the child-list
/// slot of each parent one side structure-writes that the other accesses as a
/// slot — Core's `Footprint.slotClash` (0.42.0), ordered by node then slot.
fn slot_clash(a: &Footprint, b: &Footprint) -> Vec<(String, String)> {
    let mut hits = SlotSet::new();
    hits.extend(a.slot_writes.intersection(&b.slot_writes).cloned());
    hits.extend(a.slot_writes.intersection(&b.slot_reads).cloned());
    hits.extend(a.slot_reads.intersection(&b.slot_writes).cloned());
    hits.extend(
        child_list_slots(a)
            .intersection(&accessed_slots(b))
            .cloned(),
    );
    hits.extend(
        accessed_slots(a)
            .intersection(&child_list_slots(b))
            .cloned(),
    );
    let mut out: Vec<(String, String)> = hits.into_iter().collect();
    out.sort_by(|x, y| ordinal_cmp(&x.0, &y.0).then_with(|| ordinal_cmp(&x.1, &y.1)));
    out
}

fn slot_nodes(s: &SlotSet) -> IdSet {
    s.iter().map(|(node, _)| node.clone()).collect()
}

/// The nodes `a` accesses through a slot that `b` WRITES WHOLE — Core's
/// `slotsAgainstNode` (0.42.0): a read of the node is of its existence and a
/// structure write is compared at the child-list slot, so only a content write
/// collides here.
fn slots_against_node(into: &mut IdSet, a: &Footprint, b: &Footprint) {
    into.extend(meet(&slot_nodes(&accessed_slots(a)), &b.content_writes));
}

/// Does `f` touch structure — a named parent's child list, an unknown source
/// parent, or a child-list slot it reads or writes? Core's
/// `Footprint.structural` non-empty (0.42.0), the set the unknown-parent clause
/// meets.
fn writes_structure(f: &Footprint) -> bool {
    !f.structure_writes.is_empty()
        || !f.unknown_parent_writes.is_empty()
        || accessed_slots(f)
            .iter()
            .any(|(_, slot)| slot == CHILD_LIST_SLOT)
}

/// Core's `Dag.conflicts`: every op pair (a ∈ left, b ∈ right) whose footprints
/// are not independent, one entry per address by shape priority (content, then
/// position, then move/remove), plus one per clashing slot. Left order × right
/// order × ordinal addresses. Detection only.
fn conflicts_over(
    left: &[(&TreeOp, Footprint)],
    right: &[(&TreeOp, Footprint)],
) -> Vec<OpConflict> {
    let mut out = Vec::new();
    for (a, fa) in left {
        for (b, fb) in right {
            let mut c1 = meet(&fa.content_writes, &fb.content_writes);
            c1.extend(meet(&fa.content_writes, &fb.reads));
            c1.extend(meet(&fb.content_writes, &fa.reads));
            slots_against_node(&mut c1, fa, fb);
            slots_against_node(&mut c1, fb, fa);
            let c2: IdSet = meet(&fa.structure_writes, &fb.structure_writes)
                .difference(&c1)
                .cloned()
                .collect();
            let mut move_remove = IdSet::new();
            if !fa.unknown_parent_writes.is_empty() && writes_structure(fb) {
                move_remove.extend(fa.unknown_parent_writes.iter().cloned());
            }
            if !fb.unknown_parent_writes.is_empty() && writes_structure(fa) {
                move_remove.extend(fb.unknown_parent_writes.iter().cloned());
            }
            let c3: IdSet = move_remove
                .into_iter()
                .filter(|k| !c1.contains(k) && !c2.contains(k))
                .collect();
            let entry = |address: String, shape: ConflictShape, slot: Option<String>| OpConflict {
                left: (*a).clone(),
                right: (*b).clone(),
                address,
                shape,
                slot,
            };
            for (ids, shape) in [
                (c1, ConflictShape::ConcurrentUpdate),
                (c2, ConflictShape::InsertPositionClash),
                (c3, ConflictShape::MoveVsRemove),
            ] {
                for address in ordinal_sorted(ids) {
                    out.push(entry(address, shape, None));
                }
            }
            for (node, slot) in slot_clash(fa, fb) {
                out.push(entry(node, ConflictShape::SlotClash, Some(slot)));
            }
        }
    }
    out
}

/// Each op of a delta with its footprint at the state the ops before it reach;
/// an op that does not apply leaves the state where it was, as Core's does.
fn thread_delta<'a>(base: &Node, delta: &'a [TreeOp]) -> Vec<(&'a TreeOp, Footprint)> {
    let mut st = base.clone();
    delta
        .iter()
        .map(|op| {
            let f = footprint_in(&st, op);
            if let Some(next) = applied(&st, op) {
                st = next;
            }
            (op, f)
        })
        .collect()
}

/// Core's `Dag.conflictsIn` over the base-aware footprint: each delta threaded
/// through [`apply`] from `base`, and the report over the footprints so
/// computed. An empty report promises the two deltas commute from `base`.
pub fn conflicts_in(base: &Node, delta_a: &[TreeOp], delta_b: &[TreeOp]) -> Vec<OpConflict> {
    conflicts_over(&thread_delta(base, delta_a), &thread_delta(base, delta_b))
}
