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
//!
//! Both walks are the generic Program core's (`fuaran_program::budget`), over the UI witness; what
//! is this tier's is only the per-node data cost below.

use crate::wire::{Action, Binding, Node, NodeKind, StaticValue};

use super::witness::{UiAction, UiFold, UiWalk};

/// Per-interaction resource caps — the generic core's: `max_actions` caps one cascade's priced
/// cost (a chain flattens; every other UI arm costs one), `max_nodes` the driven tree's render
/// cost (the node count, with data-bearing nodes weighted by the data they carry).
pub use fuaran_program::budget::InteractionBudget;

/// Ceiling on rows counted for cost. Reading a possibly-large payload to
/// exhaustion just to price it would itself be the unbounded work, and a count
/// this far past any sane budget is already "refuse", so the exact figure beyond
/// it carries no decision.
const MAX_COUNTED_ROWS: u64 = 100_000;

/// The leaf-action count of one cascade: a chain flattens; every other arm costs one — the core's
/// pricing over the UI view, iterative with an explicit stack, since a function whose whole job is
/// to bound an untrusted tree's cost must not itself be bounded by the caller's stack.
pub fn action_cascade_cost(action: &Action) -> u64 {
    fuaran_program::budget::action_cascade_cost(&UiFold::default(), &UiAction::unaddressed(action))
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

/// The DATA cost of one node, beyond the one every node costs, excluding its children.
///
/// Most nodes carry none. A **data-bearing** node is different: its cost scales
/// with data it carries inside a single node, which a node count cannot see. A
/// chart is one node whose render emits geometry per (point × series); a grid is
/// one node whose render emits a cell per (row × column). Counting those as one
/// is what would let a single node carry unbounded work behind a
/// bounded-looking tree.
pub(crate) fn node_data_cost(node: &Node) -> u64 {
    match &node.kind {
        NodeKind::Chart(spec) => {
            static_collection_len(&spec.source).saturating_mul((spec.y_fields.len() as u64).max(1))
        }
        NodeKind::DataGrid(spec) => {
            static_collection_len(&spec.source).saturating_mul((spec.columns.len() as u64).max(1))
        }
        NodeKind::Map(spec) => static_collection_len(&spec.source),
        NodeKind::Sparkline(spec) => static_collection_len(&spec.source),
        _ => 0,
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
    fuaran_program::budget::tree_cost(&UiWalk, ceiling, node)
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
