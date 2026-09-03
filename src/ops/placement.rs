//! The placement algebra — placed insert / move / nudge, and the clone verbs
//! (duplicate / paste) built on top of them.
//!
//! The op vocabulary is deliberately positionless: [`TreeOp::InsertChild`] and
//! [`TreeOp::MoveNode`] append, and an explicit order is stated only by
//! [`TreeOp::ReorderChildren`] naming every sibling id (an id is checkable; an
//! ordinal is not). Placing a node anywhere but last is therefore
//! `Batch [InsertChild|MoveNode; ReorderChildren]` — correct, but it leaves
//! every consumer deriving the full sibling permutation itself. This module
//! ships that derivation once, purely additively: every helper emits ops built
//! from the existing vocabulary, so the wire format, the apply engine, and the
//! node contract are untouched — and the reorder leg is dropped whenever
//! appending already yields the wanted order, keeping the common case a single
//! bare op.
//!
//! **Pre-checks mirror this host's own apply engine**, not a re-derivation of
//! it: a helper refusal names the refusal the emitted op would have met
//! ([`super::apply`]'s `ParentNotFound` / `ChildlessKind` / `NodeNotFound` /
//! `DuplicateNodeId` / `KindMismatch` / `OrderingMismatch`), so an editor can
//! grey out an illegal drop without a dry-run apply. There is one deliberate
//! tightening: an anchor that is not among the destination's post-op children is
//! REFUSED ([`PlaceError::UnknownAnchor`]) rather than silently appended. The
//! only op that could honour such an anchor is a `ReorderChildren` naming it,
//! which the apply engine refuses as `OrderingMismatch`; saying so before
//! emission is friendlier than a rejection after it.
//!
//! The clone verbs remap a copied subtree's ids to a fresh, collision-free set
//! before the insert. The remap runs over the WHOLE traversal surface — the same
//! walk [`super::all_node_ids`] performs, not just the structural child lists —
//! because the id-uniqueness contract is tree-wide, and a clone that kept an old
//! id inside a `Switch` case, an `ErrorBoundary` slot or a `State` placeholder
//! would smuggle a duplicate past it.
//!
//! Nothing here is target-specific: the module compiles unchanged for the native
//! host and for the browser-native `wasm32` client, and the C-ABI in
//! [`crate::ffi`] exposes the verbs at session level for a native binding.

use std::collections::{HashMap, HashSet};

use crate::wire::{Node, NodeId, TreeOp};

use super::{
    all_node_ids, child_nodes_mut, find_layout_parent, find_node, is_ancestor, layout_children,
};

// ─── Placement vocabulary ────────────────────────────────────────────────────

/// Where a node should sit among its destination siblings, stated the only way
/// the op vocabulary allows: by naming an existing sibling, or an end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// Append — what `InsertChild` / `MoveNode` do on their own.
    Last,
    /// Prepend — before every current sibling.
    First,
    /// Immediately before the named sibling.
    Before(NodeId),
    /// Immediately after the named sibling.
    After(NodeId),
}

impl Placement {
    /// The anchor this placement names, if it names one.
    pub fn anchor(&self) -> Option<&str> {
        match self {
            Placement::Last | Placement::First => None,
            Placement::Before(a) | Placement::After(a) => Some(a.as_str()),
        }
    }
}

/// A structural destination: which parent, and where among its children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The destination parent's node id.
    pub parent_id: NodeId,
    /// Where among that parent's children the node should sit.
    pub placement: Placement,
}

impl Target {
    /// A destination naming a parent and a placement.
    pub fn new(parent_id: impl Into<NodeId>, placement: Placement) -> Self {
        Target {
            parent_id: parent_id.into(),
            placement,
        }
    }
}

/// Why a placement could not become an op. Each case is a pre-statement of the
/// apply-time refusal the emitted op would have met, so a helper rejection and
/// an apply rejection agree — no false permit, no false refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceError {
    /// The destination parent is not in the tree (apply: `ParentNotFound`).
    ParentNotFound(NodeId),
    /// The destination parent's kind carries no `children` list (apply:
    /// `ChildlessKind`).
    ChildlessKind(NodeId),
    /// The node to move / nudge / duplicate is not structurally addressable —
    /// absent, or held in a non-structural position (a `Switch` case, an
    /// `ErrorBoundary` slot, a `State` placeholder) the structural ops cannot
    /// reach (apply: `NodeNotFound`).
    NodeNotFound(NodeId),
    /// The placement anchor is not among the destination's post-op children.
    /// The only op that could honour it — a `ReorderChildren` naming it — is
    /// refused by the apply engine as `OrderingMismatch`.
    UnknownAnchor(NodeId),
    /// The subtree being inserted carries an id already present in the tree
    /// (apply: `DuplicateNodeId`).
    DuplicateId(NodeId),
    /// The node would become its own parent (apply: `KindMismatch`).
    MoveIntoSelf(NodeId),
    /// The destination sits inside the node's own subtree — a cycle (apply:
    /// `KindMismatch`).
    MoveIntoDescendant {
        /// The node being moved.
        node_id: NodeId,
        /// The destination inside its subtree.
        parent_id: NodeId,
    },
    /// The root has no siblings to nudge among.
    CannotNudgeRoot(NodeId),
    /// The nudge would leave the sibling range (already first / already last).
    NudgeOutOfRange {
        /// The node being nudged.
        node_id: NodeId,
        /// The requested signed offset.
        delta: i32,
    },
}

impl PlaceError {
    /// The stable code name of this refusal — the discriminator a host reports
    /// across the C-ABI. Exhaustive over the closed enum, so a new refusal class
    /// is a build error until its name lands.
    pub fn code(&self) -> &'static str {
        match self {
            PlaceError::ParentNotFound(_) => "ParentNotFound",
            PlaceError::ChildlessKind(_) => "ChildlessKind",
            PlaceError::NodeNotFound(_) => "NodeNotFound",
            PlaceError::UnknownAnchor(_) => "UnknownAnchor",
            PlaceError::DuplicateId(_) => "DuplicateId",
            PlaceError::MoveIntoSelf(_) => "MoveIntoSelf",
            PlaceError::MoveIntoDescendant { .. } => "MoveIntoDescendant",
            PlaceError::CannotNudgeRoot(_) => "CannotNudgeRoot",
            PlaceError::NudgeOutOfRange { .. } => "NudgeOutOfRange",
        }
    }
}

impl std::fmt::Display for PlaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaceError::ParentNotFound(p) => {
                write!(f, "ParentNotFound: parent node '{p}' not found in tree.")
            }
            PlaceError::ChildlessKind(p) => write!(
                f,
                "ChildlessKind: node '{p}' has no children field — only child-bearing layout kinds accept structural child ops."
            ),
            PlaceError::NodeNotFound(n) => write!(
                f,
                "NodeNotFound: node '{n}' is not in the tree, or sits in a position the structural ops cannot reach."
            ),
            PlaceError::UnknownAnchor(a) => write!(
                f,
                "UnknownAnchor: '{a}' is not among the destination's post-op children."
            ),
            PlaceError::DuplicateId(n) => write!(
                f,
                "DuplicateId: NodeId '{n}' is already present in the tree; ids must be unique."
            ),
            PlaceError::MoveIntoSelf(n) => {
                write!(f, "MoveIntoSelf: cannot move node '{n}' into itself.")
            }
            PlaceError::MoveIntoDescendant { node_id, parent_id } => write!(
                f,
                "MoveIntoDescendant: '{parent_id}' sits inside '{node_id}'s own subtree (would create a cycle)."
            ),
            PlaceError::CannotNudgeRoot(n) => write!(
                f,
                "CannotNudgeRoot: the root '{n}' has no siblings to nudge among."
            ),
            PlaceError::NudgeOutOfRange { node_id, delta } => write!(
                f,
                "NudgeOutOfRange: nudging '{node_id}' by {delta} would leave the sibling range."
            ),
        }
    }
}

impl std::error::Error for PlaceError {}

// ─── Fresh-id strategy (the clone verbs' id-minting seam) ────────────────────

/// How the clone verbs mint replacement ids: given the id being replaced and a
/// predicate over every id already claimed (the whole target tree, the whole
/// incoming subtree, and ids minted earlier in the same remap), return an id the
/// predicate refuses.
///
/// A trait rather than a bare `Fn` because the deterministic strategy carries a
/// counter: the seam is inherently stateful, and `&mut self` says so.
pub trait FreshIds {
    /// Mint a replacement for `old_id` that `taken` refuses.
    fn mint(&mut self, old_id: &str, taken: &dyn Fn(&str) -> bool) -> String;
}

/// The default: `<oldId>-copy`, then `<oldId>-copy-2`, `-copy-3`, … — the first
/// candidate not already taken. Deterministic (derived from the id it replaces,
/// with no ambient state) and collision-free by probing.
#[derive(Debug, Default, Clone, Copy)]
pub struct DerivedIds;

impl FreshIds for DerivedIds {
    fn mint(&mut self, old_id: &str, taken: &dyn Fn(&str) -> bool) -> String {
        let mut n: u32 = 1;
        loop {
            let candidate = if n == 1 {
                format!("{old_id}-copy")
            } else {
                format!("{old_id}-copy-{n}")
            };
            if !taken(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }
}

/// Sequential ids under a fixed prefix (`<prefix>-1`, `-2`, …) — the
/// deterministic-replay option: the minted sequence depends only on the prefix
/// and the order of requests, never on the ids being replaced. Each value starts
/// its own counter.
#[derive(Debug, Clone)]
pub struct SequentialIds {
    prefix: String,
    counter: u32,
}

impl SequentialIds {
    /// A fresh sequential minter under `prefix`, starting at `<prefix>-1`.
    pub fn new(prefix: impl Into<String>) -> Self {
        SequentialIds {
            prefix: prefix.into(),
            counter: 0,
        }
    }
}

impl FreshIds for SequentialIds {
    fn mint(&mut self, _old_id: &str, taken: &dyn Fn(&str) -> bool) -> String {
        loop {
            self.counter += 1;
            let candidate = format!("{}-{}", self.prefix, self.counter);
            if !taken(&candidate) {
                return candidate;
            }
        }
    }
}

// ─── Shared derivation ───────────────────────────────────────────────────────

/// The destination's current child ids, or the mirrored apply-side refusal
/// (absent parent / childless kind), in the order `apply` checks them.
fn container_children(root: &Node, parent_id: &str) -> Result<Vec<NodeId>, PlaceError> {
    let Some(parent) = find_node(root, parent_id) else {
        return Err(PlaceError::ParentNotFound(parent_id.to_string()));
    };
    let Some(children) = layout_children(parent) else {
        return Err(PlaceError::ChildlessKind(parent_id.to_string()));
    };
    Ok(children.iter().map(|c| c.id.clone()).collect())
}

/// Place `moved` within `order` (which already contains it) per `placement`. An
/// anchor that is not in the list is refused — the honest alternative (silently
/// appending) would emit an op that does not honour the caller's stated intent.
fn reposition(
    order: &[NodeId],
    moved: &str,
    placement: &Placement,
) -> Result<Vec<NodeId>, PlaceError> {
    let rest: Vec<NodeId> = order.iter().filter(|id| *id != moved).cloned().collect();
    let anchored = |anchor: &str, offset: usize| -> Result<Vec<NodeId>, PlaceError> {
        let Some(i) = rest.iter().position(|id| id == anchor) else {
            return Err(PlaceError::UnknownAnchor(anchor.to_string()));
        };
        let mut out = rest.clone();
        out.insert(i + offset, moved.to_string());
        Ok(out)
    };
    match placement {
        Placement::Last => {
            let mut out = rest;
            out.push(moved.to_string());
            Ok(out)
        }
        Placement::First => {
            let mut out = rest;
            out.insert(0, moved.to_string());
            Ok(out)
        }
        Placement::Before(anchor) => anchored(anchor, 0),
        Placement::After(anchor) => anchored(anchor, 1),
    }
}

/// Whether `node_id` is addressable by the structural ops: the root, or a node
/// held in some layout kind's `children`. A node in a non-structural position is
/// visible to traversal but not movable, and the apply engine refuses structural
/// ops against it as `NodeNotFound` (its `MoveNode` leg removes before it
/// inserts, and the removal looks for a *layout* parent).
fn structurally_present(root: &Node, node_id: &str) -> bool {
    root.id == node_id || find_layout_parent(root, node_id).is_some()
}

/// The first id `incoming` shares with `root`, in `incoming`'s traversal order —
/// the duplicate the apply engine's `InsertChild` would refuse.
fn first_shared_id(root: &Node, incoming: &Node) -> Option<NodeId> {
    let existing: HashSet<String> = all_node_ids(root).into_iter().collect();
    all_node_ids(incoming)
        .into_iter()
        .find(|id| existing.contains(id))
}

/// The post-op child membership of a destination that is gaining `moved`: the
/// siblings WITHOUT it, plus it. Correct for a cross-parent move, a same-parent
/// re-placement, and a fresh insert alike.
fn membership_with(siblings: &[NodeId], moved: &str) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = siblings.iter().filter(|id| *id != moved).cloned().collect();
    out.push(moved.to_string());
    out
}

// ─── The verbs ───────────────────────────────────────────────────────────────

/// Whether `moved` may legally take up residence at `target` — the pre-check an
/// editor uses to grey out an illegal drop without a dry-run apply. Mirrors the
/// apply engine's rejections: absent (or non-structural) node, move into itself,
/// move into its own descendant, absent or childless destination, unknown
/// anchor.
///
/// # Errors
/// The [`PlaceError`] naming the apply-side refusal the move would have met.
pub fn can_place(root: &Node, moved: &str, target: &Target) -> Result<(), PlaceError> {
    if !structurally_present(root, moved) {
        return Err(PlaceError::NodeNotFound(moved.to_string()));
    }
    if target.parent_id == moved {
        return Err(PlaceError::MoveIntoSelf(moved.to_string()));
    }
    if is_ancestor(moved, &target.parent_id, root) {
        return Err(PlaceError::MoveIntoDescendant {
            node_id: moved.to_string(),
            parent_id: target.parent_id.clone(),
        });
    }
    let siblings = container_children(root, &target.parent_id)?;
    let membership = membership_with(&siblings, moved);
    reposition(&membership, moved, &target.placement).map(|_| ())
}

/// The op an insertion becomes. `InsertChild` appends, so the wanted order is
/// computed over the post-insert membership and stated by `ReorderChildren`
/// naming every sibling id; the reorder leg is dropped when appending already
/// produces that order.
///
/// # Errors
/// The [`PlaceError`] naming the apply-side refusal the insert would have met.
pub fn place_op(root: &Node, child: &Node, target: &Target) -> Result<TreeOp, PlaceError> {
    let siblings = container_children(root, &target.parent_id)?;
    if let Some(dup) = first_shared_id(root, child) {
        return Err(PlaceError::DuplicateId(dup));
    }
    let appended = membership_with(&siblings, &child.id);
    let wanted = reposition(&appended, &child.id, &target.placement)?;
    let insert = TreeOp::InsertChild {
        parent_id: target.parent_id.clone(),
        child: child.clone(),
    };
    Ok(if wanted == appended {
        insert
    } else {
        TreeOp::Batch(vec![
            insert,
            TreeOp::ReorderChildren {
                parent_id: target.parent_id.clone(),
                new_order: wanted,
            },
        ])
    })
}

/// The op a move becomes. `MoveNode` appends under the new parent, and the node
/// may already be one of that parent's children (a re-placement within one
/// parent), so the post-move membership is the siblings WITHOUT it plus it.
///
/// # Errors
/// The [`PlaceError`] naming the apply-side refusal the move would have met.
pub fn move_op(root: &Node, moved: &str, target: &Target) -> Result<TreeOp, PlaceError> {
    can_place(root, moved, target)?;
    let siblings = container_children(root, &target.parent_id)?;
    let appended = membership_with(&siblings, moved);
    let wanted = reposition(&appended, moved, &target.placement)?;
    let mv = TreeOp::MoveNode {
        target: moved.to_string(),
        new_parent_id: target.parent_id.clone(),
    };
    Ok(if wanted == appended {
        mv
    } else {
        TreeOp::Batch(vec![
            mv,
            TreeOp::ReorderChildren {
                parent_id: target.parent_id.clone(),
                new_order: wanted,
            },
        ])
    })
}

/// The op a keyboard move-up (`-1`) / move-down (`+1`) becomes: the node swapped
/// with the sibling `delta` positions away, stated as the FULL sibling id order
/// (which is what `ReorderChildren` requires — a partial list is refused by the
/// apply engine, and rightly, since a partial order is not one).
///
/// # Errors
/// [`PlaceError::CannotNudgeRoot`] for the root, [`PlaceError::NodeNotFound`]
/// for a node no layout parent holds, [`PlaceError::NudgeOutOfRange`] when the
/// swap would leave the sibling range.
pub fn nudge_op(root: &Node, node_id: &str, delta: i32) -> Result<TreeOp, PlaceError> {
    if root.id == node_id {
        return Err(PlaceError::CannotNudgeRoot(node_id.to_string()));
    }
    let Some(parent) = find_layout_parent(root, node_id) else {
        return Err(PlaceError::NodeNotFound(node_id.to_string()));
    };
    let parent_id = parent.id.clone();
    let ids: Vec<NodeId> = layout_children(parent)
        .expect("a layout parent by construction")
        .iter()
        .map(|c| c.id.clone())
        .collect();
    let index = ids
        .iter()
        .position(|id| id == node_id)
        .expect("the layout parent holds it by construction");
    let swap_with = index as i64 + i64::from(delta);
    if swap_with < 0 || swap_with >= ids.len() as i64 {
        return Err(PlaceError::NudgeOutOfRange {
            node_id: node_id.to_string(),
            delta,
        });
    }
    let mut reordered = ids;
    reordered.swap(index, swap_with as usize);
    Ok(TreeOp::ReorderChildren {
        parent_id,
        new_order: reordered,
    })
}

// ─── Clone verbs ─────────────────────────────────────────────────────────────

/// Rewrite every id in `incoming` that collides with an id in `target_root` to a
/// fresh, collision-free one. Ids with no collision are preserved — a pasted
/// subtree keeps its identity where it can; a subtree duplicated within its own
/// tree remaps every id, since every one collides.
fn remap_for_insert(fresh: &mut dyn FreshIds, target_root: &Node, incoming: &Node) -> Node {
    let existing: HashSet<String> = all_node_ids(target_root).into_iter().collect();
    // Minted ids must also dodge the incoming subtree's own ids — one colliding
    // with a not-yet-visited incoming node would re-introduce the duplicate the
    // remap exists to remove — and each other.
    let mut taken: HashSet<String> = existing.clone();
    for id in all_node_ids(incoming) {
        taken.insert(id);
    }
    let mut rename: HashMap<String, String> = HashMap::new();
    for old_id in all_node_ids(incoming) {
        if existing.contains(&old_id) && !rename.contains_key(&old_id) {
            let minted = fresh.mint(&old_id, &|candidate: &str| taken.contains(candidate));
            taken.insert(minted.clone());
            rename.insert(old_id, minted);
        }
    }
    if rename.is_empty() {
        return incoming.clone();
    }
    let mut cloned = incoming.clone();
    rewrite_ids(&mut cloned, &rename);
    cloned
}

/// Apply a rename map over the WHOLE traversal surface — the same walk
/// [`all_node_ids`] performs, so no id the uniqueness contract covers is missed.
fn rewrite_ids(node: &mut Node, rename: &HashMap<String, String>) {
    if let Some(fresh) = rename.get(&node.id) {
        node.id = fresh.clone();
    }
    for child in child_nodes_mut(node) {
        rewrite_ids(child, rename);
    }
}

/// Duplicate the subtree rooted at `source` and place the clone at `target`,
/// minting replacement ids with `fresh`. The emitted op is an ordinary placed
/// insert — the clone is a fresh subtree, so the standard apply gate (including
/// the tree-wide duplicate-id check) accepts it unchanged.
///
/// # Errors
/// [`PlaceError::NodeNotFound`] for an absent source, else the refusal
/// [`place_op`] would have raised for the clone.
pub fn duplicate_op_with(
    fresh: &mut dyn FreshIds,
    root: &Node,
    source: &str,
    target: &Target,
) -> Result<TreeOp, PlaceError> {
    let Some(sub) = find_node(root, source) else {
        return Err(PlaceError::NodeNotFound(source.to_string()));
    };
    let clone = remap_for_insert(fresh, root, sub);
    place_op(root, &clone, target)
}

/// [`duplicate_op_with`] under the default derived-suffix id strategy.
///
/// # Errors
/// As [`duplicate_op_with`].
pub fn duplicate_op(root: &Node, source: &str, target: &Target) -> Result<TreeOp, PlaceError> {
    duplicate_op_with(&mut DerivedIds, root, source, target)
}

/// Place a subtree lifted from a DIFFERENT tree into `target_root`, remapping
/// any id that collides with one already present (ids with no collision are
/// preserved). The incoming subtree's ids must be unique within itself — a
/// subtree extracted from any well-formed tree is.
///
/// # Errors
/// The refusal [`place_op`] would have raised for the remapped subtree.
pub fn paste_op_with(
    fresh: &mut dyn FreshIds,
    target_root: &Node,
    incoming: &Node,
    target: &Target,
) -> Result<TreeOp, PlaceError> {
    let clone = remap_for_insert(fresh, target_root, incoming);
    place_op(target_root, &clone, target)
}

/// [`paste_op_with`] under the default derived-suffix id strategy.
///
/// # Errors
/// As [`paste_op_with`].
pub fn paste_op(
    target_root: &Node,
    incoming: &Node,
    target: &Target,
) -> Result<TreeOp, PlaceError> {
    paste_op_with(&mut DerivedIds, target_root, incoming, target)
}
