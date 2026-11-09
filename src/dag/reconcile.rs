//! Merge by fold: the reconciliation policy over Core's conflict report
//! (WIRE_FORMAT.md §34, Phase 2644).
//!
//! A merge of two branches is a DERIVATION: both branches' op deltas are folded
//! from their common base, and the merge records only what the fold cannot
//! decide on its own — a script. Which op pairs interfere is Core's report over
//! the base-aware footprint ([`conflicts_in`]). Each reported pair is exactly one
//! of:
//!
//! 1. An AGREEMENT — the two ops' canonical encodings are byte-identical, so the
//!    fold already holds the effect when it reaches the second. Admitted.
//! 2. An INTERLEAVE — an `InsertPositionClash` at parent `p` where each op
//!    appends a node to `p` (an `InsertChild` under it, or a `MoveNode` to it)
//!    and the two appended ids differ. Admitted, ordered by the child-order rule
//!    below.
//! 3. A REFUSAL — anything else. `DeleteModify` when exactly one of the two ops
//!    removes (a `RemoveNode`, or a `Batch` holding one), `Conflict` otherwise.
//!
//! THE CHILD-ORDER RULE (§34.4). Under an interleaved parent the children
//! neither branch appended keep the fold's order and the appended ones follow:
//! each branch's appends in its own order, the two runs merged by comparing
//! heads ordinally by NodeId, the smaller first. Where that differs from the
//! fold's order the merge records `ReorderChildren(p, order)`.
//!
//! CHECKED, not assumed: the admitted classes lie outside Core's independence
//! theorem, so the fold runs in both orders, each followed by the script, and
//! two trees that differ refuse the merge as `OrderSensitive`.
//!
//! Every op and tree this host holds has a canonical encoding (the encoder is
//! total), so the policy has no "unencodable" refusal.
//!
//! The tree merge ([`super::merge3_way`]) is untouched and stays normative for
//! the other merge-conformance keys; the `foldFixtures` family certifies this
//! module.

use std::collections::BTreeSet;

use super::footprint::{
    ConflictShape, OpConflict, conflicts_in, find_structural, structural_children,
};
use crate::canonical::{escape_string, ordinal_cmp};
use crate::ops::{ApplyError, apply};
use crate::wire::{Node, TreeOp, encode_node, encode_op};

/// Which branch of a two-branch reconcile an op or a fault belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldSide {
    A,
    B,
}

/// How the policy classes a pair it refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldRefusalClass {
    DeleteModify,
    Conflict,
}

impl FoldRefusalClass {
    /// The class's wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            FoldRefusalClass::DeleteModify => "DeleteModify",
            FoldRefusalClass::Conflict => "Conflict",
        }
    }
}

/// One refused pair: Core's report entry with the policy's class.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldRefusal {
    pub conflict: OpConflict,
    pub class: FoldRefusalClass,
}

/// Why a reconcile refused.
#[derive(Debug, Clone, PartialEq)]
pub enum FoldFault {
    /// A branch's own delta does not apply from the base: the side, the index of
    /// the first failing op, and its apply error.
    DeltaDoesNotApply {
        side: FoldSide,
        index: usize,
        error: ApplyError,
    },
    /// Core reported pairs the policy does not admit, in canonical order.
    Refused(Vec<FoldRefusal>),
    /// Every pair was admitted and the two fold orders reached different trees
    /// (their canonical encodings).
    OrderSensitive { a_then_b: String, b_then_a: String },
    /// Every pair was admitted and the fold faulted applying an op.
    FoldFaulted(ApplyError),
}

/// A gated reconcile's refusal: the fold's own, or the validator gate's.
#[derive(Debug, Clone, PartialEq)]
pub enum GatedFoldFault<D> {
    Fold(FoldFault),
    /// The derived tree carries defects neither branch head had.
    IntroducedDefects(Vec<D>),
}

/// A derived merge: the tree, the script the merge records, how many distinct
/// agreeing ops the fold applied as no change, and how many parents it
/// interleaved.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldReconciled {
    pub tree: Node,
    pub script: Vec<TreeOp>,
    pub agreements: usize,
    pub interleaves: usize,
}

/// Whether the effect of `op` is already held by `tree` — the two cases where
/// re-applying an identical op FAILS; every other op is idempotent under apply.
fn held(op: &TreeOp, tree: &Node) -> bool {
    match op {
        TreeOp::RemoveNode { target } => find_structural(tree, target).is_none(),
        TreeOp::InsertChild { child, parent_id } => {
            find_structural(tree, parent_id).is_some_and(|p| {
                let want = encode_node(child);
                structural_children(p)
                    .iter()
                    .any(|c| c.id == child.id && encode_node(c) == want)
            })
        }
        _ => false,
    }
}

/// [`apply`] with an op whose effect `tree` already holds applied as no change —
/// the fold's reading of an agreement. A `Batch` is applied member by member
/// under the same rule; the first member that fails and is not held fails it.
pub fn apply_held(op: &TreeOp, tree: &Node) -> Result<Node, ApplyError> {
    if let TreeOp::Batch(members) = op {
        return fold_held(members, tree);
    }
    match apply(tree, op) {
        Ok(outcome) => Ok(outcome.new_tree),
        Err(_) if held(op, tree) => Ok(tree.clone()),
        Err(e) => Err(e),
    }
}

fn fold_held(script: &[TreeOp], start: &Node) -> Result<Node, ApplyError> {
    script
        .iter()
        .try_fold(start.clone(), |t, op| apply_held(op, &t))
}

/// The index and error of the first op of `delta` that does not apply from
/// `base`.
fn first_failure(delta: &[TreeOp], base: &Node) -> Option<(usize, ApplyError)> {
    let mut t = base.clone();
    for (i, op) in delta.iter().enumerate() {
        match apply(&t, op) {
            Ok(outcome) => t = outcome.new_tree,
            Err(e) => return Some((i, e)),
        }
    }
    None
}

fn removes(op: &TreeOp) -> bool {
    match op {
        TreeOp::RemoveNode { .. } => true,
        TreeOp::Batch(members) => members.iter().any(removes),
        _ => false,
    }
}

/// The node `op` appends to `parent`, when it is an append there.
fn append_into<'a>(parent: &str, op: &'a TreeOp) -> Option<&'a str> {
    match op {
        TreeOp::InsertChild { child, parent_id } if parent_id == parent => Some(&child.id),
        TreeOp::MoveNode {
            target,
            new_parent_id,
        } if new_parent_id == parent => Some(target),
        _ => None,
    }
}

enum Verdict {
    Agreement,
    Interleave,
    Refuse(FoldRefusalClass),
}

fn judge(c: &OpConflict) -> Verdict {
    if encode_op(&c.left) == encode_op(&c.right) {
        return Verdict::Agreement;
    }
    if c.shape == ConflictShape::InsertPositionClash
        && let (Some(l), Some(r)) = (
            append_into(&c.address, &c.left),
            append_into(&c.address, &c.right),
        )
        && l != r
    {
        return Verdict::Interleave;
    }
    if removes(&c.left) != removes(&c.right) {
        Verdict::Refuse(FoldRefusalClass::DeleteModify)
    } else {
        Verdict::Refuse(FoldRefusalClass::Conflict)
    }
}

/// A refusal's §34.5 sort key: address, shape, slot (empty when absent), left
/// op bytes, right op bytes.
fn order_key(r: &FoldRefusal) -> [String; 5] {
    let c = &r.conflict;
    [
        c.address.clone(),
        c.shape.as_str().to_owned(),
        c.slot.clone().unwrap_or_default(),
        encode_op(&c.left),
        encode_op(&c.right),
    ]
}

/// Orders refusals canonically — address, shape, slot (empty when absent), left
/// op bytes, right op bytes, each ordinal — with exact duplicates removed
/// (§34.5).
pub fn sort_fold_refusals(refusals: &[FoldRefusal]) -> Vec<FoldRefusal> {
    let mut keyed: Vec<([String; 5], &FoldRefusal)> =
        refusals.iter().map(|r| (order_key(r), r)).collect();
    keyed.sort_by(|(x, _), (y, _)| {
        x.iter()
            .zip(y)
            .map(|(a, b)| ordinal_cmp(a, b))
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    keyed.dedup_by(|(x, _), (y, _)| x == y);
    keyed.into_iter().map(|(_, r)| r.clone()).collect()
}

/// The child-order rule's run-merge: each run in its own order, heads compared
/// ordinally.
fn interleave(a: &[String], b: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if ordinal_cmp(&a[i], &b[j]).is_le() {
            out.push(a[i].clone());
            i += 1;
        } else {
            out.push(b[j].clone());
            j += 1;
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
    out
}

/// The `ReorderChildren` the rule asks of `parent`, or none when the fold's
/// order is already the rule's.
fn reorder_for(tree: &Node, parent: &str, run_a: &[String], run_b: &[String]) -> Option<TreeOp> {
    let p = find_structural(tree, parent)?;
    let kids: Vec<String> = structural_children(p)
        .iter()
        .map(|c| c.id.clone())
        .collect();
    let present: BTreeSet<&String> = kids.iter().collect();
    let appended: BTreeSet<&String> = run_a.iter().chain(run_b).collect();
    let keep = |run: &[String]| -> Vec<String> {
        let mut seen = BTreeSet::new();
        run.iter()
            .filter(|x| present.contains(x) && seen.insert(*x))
            .cloned()
            .collect()
    };
    let mut order: Vec<String> = kids
        .iter()
        .filter(|k| !appended.contains(k))
        .cloned()
        .collect();
    order.extend(interleave(&keep(run_a), &keep(run_b)));
    (order != kids).then(|| TreeOp::ReorderChildren {
        new_order: order,
        parent_id: parent.to_owned(),
    })
}

fn appends_of(parent: &str, delta: &[TreeOp]) -> Vec<String> {
    delta
        .iter()
        .filter_map(|op| append_into(parent, op).map(str::to_owned))
        .collect()
}

/// Reconciles two branch deltas from their common base under the §34 policy:
/// the derived tree and the script a merge records, or a typed refusal. Both
/// deltas apply from `base`, op by op, as recorded on their branches.
pub fn reconcile_fold(
    base: &Node,
    delta_a: &[TreeOp],
    delta_b: &[TreeOp],
) -> Result<FoldReconciled, FoldFault> {
    for (side, delta) in [(FoldSide::A, delta_a), (FoldSide::B, delta_b)] {
        if let Some((index, error)) = first_failure(delta, base) {
            return Err(FoldFault::DeltaDoesNotApply { side, index, error });
        }
    }

    let mut refusals = Vec::new();
    let mut interleaved: BTreeSet<String> = BTreeSet::new();
    let mut agreed: BTreeSet<String> = BTreeSet::new();
    for c in conflicts_in(base, delta_a, delta_b) {
        match judge(&c) {
            // A pair is reported once per address it collides on: count
            // distinct agreeing ops.
            Verdict::Agreement => {
                agreed.insert(encode_op(&c.left));
            }
            Verdict::Interleave => {
                interleaved.insert(c.address.clone());
            }
            Verdict::Refuse(class) => refusals.push(FoldRefusal { conflict: c, class }),
        }
    }
    if !refusals.is_empty() {
        return Err(FoldFault::Refused(sort_fold_refusals(&refusals)));
    }

    let mut parents: Vec<String> = interleaved.into_iter().collect();
    parents.sort_by(|x, y| ordinal_cmp(x, y));
    let derive = |first: &[TreeOp], second: &[TreeOp]| -> Result<(Node, Vec<TreeOp>), ApplyError> {
        let script: Vec<TreeOp> = first.iter().chain(second).cloned().collect();
        let folded = fold_held(&script, base)?;
        let reorders: Vec<TreeOp> = parents
            .iter()
            .filter_map(|p| {
                reorder_for(&folded, p, &appends_of(p, delta_a), &appends_of(p, delta_b))
            })
            .collect();
        let tree = fold_held(&reorders, &folded)?;
        Ok((tree, reorders))
    };

    let (ab, script) = derive(delta_a, delta_b).map_err(FoldFault::FoldFaulted)?;
    let (ba, _) = derive(delta_b, delta_a).map_err(FoldFault::FoldFaulted)?;
    let (ab_text, ba_text) = (encode_node(&ab), encode_node(&ba));
    if ab_text != ba_text {
        return Err(FoldFault::OrderSensitive {
            a_then_b: ab_text,
            b_then_a: ba_text,
        });
    }
    Ok(FoldReconciled {
        tree: ab,
        script,
        agreements: agreed.len(),
        interleaves: parents.len(),
    })
}

/// [`reconcile_fold`], then the validator gate: `introduced` is handed the two
/// branch heads and the derived tree and answers the defects the derived tree
/// carries that neither head does (§27.8.2's introduced-defect diff under the
/// domain's validator). Any refuses the merge as
/// [`GatedFoldFault::IntroducedDefects`].
pub fn reconcile_fold_gated<D>(
    introduced: impl Fn(&Node, &Node, &Node) -> Vec<D>,
    base: &Node,
    delta_a: &[TreeOp],
    delta_b: &[TreeOp],
) -> Result<FoldReconciled, GatedFoldFault<D>> {
    let r = reconcile_fold(base, delta_a, delta_b).map_err(GatedFoldFault::Fold)?;
    // Both deltas applied from base inside reconcile_fold, so the heads exist.
    let head = |delta: &[TreeOp]| fold_held(delta, base).map_err(FoldFault::FoldFaulted);
    let head_a = head(delta_a).map_err(GatedFoldFault::Fold)?;
    let head_b = head(delta_b).map_err(GatedFoldFault::Fold)?;
    let defects = introduced(&head_a, &head_b, &r.tree);
    if defects.is_empty() {
        Ok(r)
    } else {
        Err(GatedFoldFault::IntroducedDefects(defects))
    }
}

/// The refusal envelope (§34.5): a JSON array of the refused pairs in canonical
/// order, each `{"address","class","left","right","shape"[,"slot"]}` with the
/// ops as their canonical encodings and `slot` present only for a `SlotClash`.
/// `sha256` over it is the cross-host refusal hash.
pub fn encode_fold_envelope(refusals: &[FoldRefusal]) -> String {
    let entries: Vec<String> = sort_fold_refusals(refusals)
        .iter()
        .map(|r| {
            let c = &r.conflict;
            let slot = match (&c.slot, c.shape) {
                (Some(s), ConflictShape::SlotClash) => format!(",\"slot\":{}", escape_string(s)),
                _ => String::new(),
            };
            format!(
                "{{\"address\":{},\"class\":{},\"left\":{},\"right\":{},\"shape\":{}{slot}}}",
                escape_string(&c.address),
                escape_string(r.class.as_str()),
                encode_op(&c.left),
                encode_op(&c.right),
                escape_string(c.shape.as_str()),
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}
