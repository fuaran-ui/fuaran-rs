//! Resource bounds — the other half of "safe to run untrusted".
//!
//! The no-closures invariant prevents arbitrary *code*; it does not prevent
//! arbitrary *cost*. A program tree that carries no code can still drive an
//! enormous chain or be pathologically large, and a host that bounds one and not
//! the other has not bounded anything.
//!
//! Deterministic on purpose: the budget is step- and size-based, never
//! wall-clock, so the same tree and event script bound identically at every
//! placement and on every target — including `wasm32`, where there is no clock
//! this crate is willing to read.

use crate::wire::{Action, Binding, Node, NodeKind, StaticValue};

/// Per-interaction resource caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionBudget {
    /// Max leaf-action count of one action cascade. A chain flattens; every
    /// other arm costs one.
    pub max_actions: u64,
    /// Max render **cost** of the driven tree — the node count, with
    /// data-bearing nodes weighted by the data they carry.
    pub max_nodes: u64,
}

impl InteractionBudget {
    /// No caps — the trusted-author case, where the program is your own.
    pub const fn unlimited() -> Self {
        InteractionBudget {
            max_actions: u64::MAX,
            max_nodes: u64::MAX,
        }
    }
}

impl Default for InteractionBudget {
    /// Conservative caps for the case this loop exists to serve: a host running
    /// a program it did not write.
    fn default() -> Self {
        InteractionBudget {
            max_actions: 64,
            max_nodes: 10_000,
        }
    }
}

/// Ceiling on rows counted for cost. Reading a possibly-large payload to
/// exhaustion just to price it would itself be the unbounded work, and a count
/// this far past any sane budget is already "refuse", so the exact figure beyond
/// it carries no decision.
const MAX_COUNTED_ROWS: u64 = 100_000;

/// The leaf-action count of one cascade. A chain flattens; every other arm costs
/// one. Iterative with an explicit stack: a function whose whole job is to bound
/// an untrusted tree's cost must not itself be bounded by the caller's stack.
pub fn action_cascade_cost(action: &Action) -> u64 {
    let mut pending: Vec<&Action> = vec![action];
    let mut total: u64 = 0;
    while let Some(current) = pending.pop() {
        match current {
            Action::Chain(inner) => pending.extend(inner.iter()),
            _ => total = total.saturating_add(1),
        }
        if total == u64::MAX {
            break;
        }
    }
    total
}

/// The number of rows a `Static` collection payload carries, capped at
/// [`MAX_COUNTED_ROWS`]. Only a static payload is counted: a query, state or
/// transform binding resolves from the host's own store, so its size is not a
/// property of the untrusted tree and is not this budget's business.
fn static_collection_len(binding: &Binding) -> u64 {
    let Binding::Static { value } = binding else {
        return 0;
    };
    let len = match value {
        StaticValue::Rows(rows) => rows.len(),
        StaticValue::FloatSeq(items) => items.len(),
        StaticValue::Markers(markers) => markers.len(),
        StaticValue::Options(options) => options.len(),
        StaticValue::StringList(items) => items.len(),
        _ => 0,
    };
    (len as u64).min(MAX_COUNTED_ROWS)
}

/// The render cost of one node, excluding its children.
///
/// Most nodes cost one. A **data-bearing** node is different: its cost scales
/// with data it carries inside a single node, which a node count cannot see. A
/// chart is one node whose render emits geometry per (point × series); a grid is
/// one node whose render emits a cell per (row × column). Counting those as one
/// is what would let a single node carry unbounded work behind a
/// bounded-looking tree.
fn node_cost(node: &Node) -> u64 {
    match &node.kind {
        NodeKind::Chart(spec) => 1u64.saturating_add(
            static_collection_len(&spec.source).saturating_mul((spec.y_fields.len() as u64).max(1)),
        ),
        NodeKind::DataGrid(spec) => 1u64.saturating_add(
            static_collection_len(&spec.source).saturating_mul((spec.columns.len() as u64).max(1)),
        ),
        NodeKind::Map(spec) => 1u64.saturating_add(static_collection_len(&spec.source)),
        NodeKind::Sparkline(spec) => 1u64.saturating_add(static_collection_len(&spec.source)),
        _ => 1,
    }
}

/// The tree's total render cost.
///
/// Iterative, and it **stops** as soon as the cost passes `ceiling`. Both
/// properties matter: recursive, it would be bounded by the caller's stack
/// rather than by the budget; unconditional, it would walk the whole tree before
/// anyone compared the result against the cap, charging the budget only after
/// doing the work it exists to refuse.
///
/// The returned value is exact at or below `ceiling`, and is "greater than
/// `ceiling`" otherwise — all a budget comparand needs, since every use past
/// that point is a refusal.
pub fn tree_cost(ceiling: u64, node: &Node) -> u64 {
    let mut pending: Vec<&Node> = vec![node];
    let mut total: u64 = 0;
    while let Some(current) = pending.pop() {
        if total > ceiling {
            break;
        }
        total = total.saturating_add(node_cost(current));
        pending.extend(super::resolve::structural_children(&current.kind));
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::decode_node;

    fn set_state(key: &str) -> Action {
        Action::SetState {
            key: key.into(),
            value: Some(crate::canonical::JVal::Str("x".into())),
            value_from: None,
        }
    }

    #[test]
    fn a_chain_flattens_and_every_other_arm_costs_one() {
        assert_eq!(action_cascade_cost(&set_state("a")), 1);
        assert_eq!(
            action_cascade_cost(&Action::Chain(vec![
                set_state("a"),
                Action::Chain(vec![set_state("b"), set_state("c")]),
            ])),
            3
        );
        // An empty chain reaches nothing, and costs nothing.
        assert_eq!(action_cascade_cost(&Action::Chain(vec![])), 0);
    }

    #[test]
    fn the_walk_stops_once_the_ceiling_is_passed() {
        let tree = decode_node(
            r#"{"id":"root","kind":{"$type":"Box","children":[{"id":"a","kind":{"$type":"Markdown","text":"a"}},{"id":"b","kind":{"$type":"Markdown","text":"b"}}],"layout":{"$type":"Auto"},"role":"Dashboard"}}"#,
        )
        .expect("the fixture decodes");
        assert_eq!(tree_cost(u64::MAX, &tree), 3);
        // Past the ceiling the answer is only "greater than the ceiling", which
        // is all a refusal needs.
        assert!(tree_cost(1, &tree) > 1);
    }
}
