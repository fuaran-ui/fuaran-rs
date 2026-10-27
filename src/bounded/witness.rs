//! The UI tier as an instantiation of the generic Program core.
//!
//! The loop's algebra — the fold over the eight view shapes, the budget's cascade and tree costs,
//! re-resolution of the fixed base tree — is the generic core's (`fuaran-program`), and nothing
//! here re-implements it. What this module holds is the UI vocabulary's WITNESS: how each `Action`
//! arm views as one of the algebra's shapes, what a leaf does at dispatch, how a binding resolves
//! against the store, and how the tree is walked. The arms map as the tree wire specification's
//! lowering table states them (WIRE_FORMAT §30.1, [`super::actions::lowers_to`]): a `Chain` is a
//! sequence, a `SetState` (and a commit the loop can flush) is the one store write, a `Call` is
//! recognised at every depth, and every other arm is a leaf. None views as a guard, a branch, a
//! repeat or an iteration — the UI tier branches in the tree and repeats through bindings — so the
//! fold never halts here.

use fuaran_program::witness::{
    ActionView, DispatchFold, DispatchWitness, ExprResolution, LeafOutcome, WalkWitness,
};

use crate::canonical::JVal;
use crate::render::BindingSources;
use crate::render::bindings::{Resolution, resolve};
use crate::wire::{Action, Binding, Node};

use super::actions::{
    Flush, HOST_RESERVED_STATE_PREFIX, commit_destination, describe_action, flushed_value,
    lower_leaf, to_state_value,
};
use super::budget::node_data_cost;
use super::resolve::{replace_children, resolve_node, structural_children};
use super::validate::handler_for;

/// An action at a position: the action, and — when the loop ADDRESSED the position — its
/// structural path inside the node's action (dot-joined chain positions and continuation names,
/// the empty path naming the action itself), which a question there carries in its token.
#[derive(Debug, Clone)]
pub(crate) struct UiAction {
    pub(crate) action: Action,
    pub(crate) path: Option<String>,
}

impl UiAction {
    /// An unaddressed position: a confirm reached there cannot be answered, and declines.
    pub(crate) fn unaddressed(action: &Action) -> Self {
        UiAction {
            action: action.clone(),
            path: None,
        }
    }

    /// A position the loop addressed at `path`.
    pub(crate) fn at(action: &Action, path: &str) -> Self {
        UiAction {
            action: action.clone(),
            path: Some(path.to_string()),
        }
    }
}

fn child_path(path: &str, index: usize) -> String {
    if path.is_empty() {
        index.to_string()
    } else {
        format!("{path}.{index}")
    }
}

/// The UI dispatch witness. `flush` is what the loop hands the fold so a commit can write: the
/// fixed tree its field is found in and the admitted event's payload (Phase 2198); without it a
/// commit has no key and is the leaf that writes nothing.
#[derive(Clone, Copy, Default)]
pub(crate) struct UiFold<'a> {
    pub(crate) flush: Option<Flush<'a>>,
}

impl DispatchFold for UiFold<'_> {
    type Action = UiAction;
    type Expr = Binding;
    type Store = BindingSources;
    type Effect = crate::bounded::effect::ClientEffect;

    fn view(&self, at: &UiAction) -> ActionView<UiAction, Binding> {
        match &at.action {
            Action::Chain(inner) => ActionView::Sequence(
                inner
                    .iter()
                    .enumerate()
                    .map(|(index, a)| UiAction {
                        action: a.clone(),
                        path: at.path.as_deref().map(|p| child_path(p, index)),
                    })
                    .collect(),
            ),
            Action::SetState {
                key,
                value,
                value_from,
            } => ActionView::Assign {
                key: key.clone(),
                value: value.clone(),
                from: value_from.as_deref().cloned(),
            },
            // A commit the loop can resolve is the store write it lowers to (D46 of the reference:
            // a write whose key is known only with the tree in view, and whose value arrives on the
            // event, is resolved by the domain before the fold and viewed as the assign arm). One
            // whose value cannot be written, or with no tree in view, is a leaf.
            Action::CommitLocal { node_id: field_id } => match self
                .flush
                .and_then(|f| commit_destination(f.tree, field_id).map(|d| (f, d)))
                .map(|(f, (key, numeric))| (key, flushed_value(numeric, f.payload.get(field_id))))
            {
                Some((key, Ok(value))) => ActionView::Assign {
                    key: key.to_string(),
                    value: Some(value),
                    from: None,
                },
                _ => ActionView::Leaf,
            },
            Action::Call { endpoint, into, .. } => ActionView::Call {
                endpoint: endpoint.clone(),
                declares_target: into.is_some(),
            },
            Action::Notify { .. }
            | Action::Navigate { .. }
            | Action::AiTool { .. }
            | Action::WriteToClipboard { .. }
            | Action::Print
            | Action::Confirm { .. }
            | Action::Focus { .. }
            | Action::ReadFileBody { .. }
            | Action::Invoke { .. }
            | Action::Dispatch => ActionView::Leaf,
        }
    }

    fn lower(
        &self,
        node_id: &str,
        at: &UiAction,
        store: &BindingSources,
    ) -> LeafOutcome<Self::Effect> {
        match &at.action {
            // Reached as a leaf only when the commit cannot write: the refusal says why, and a
            // commit with no destination is the documented no-op it always was.
            Action::CommitLocal { node_id: field_id } => {
                match self
                    .flush
                    .and_then(|f| commit_destination(f.tree, field_id).map(|d| (f, d)))
                {
                    Some((f, (_, numeric))) => {
                        match flushed_value(numeric, f.payload.get(field_id)) {
                            Err(reason) => LeafOutcome::Refuse(reason),
                            Ok(_) => LeafOutcome::Decline,
                        }
                    }
                    None => LeafOutcome::Decline,
                }
            }
            action => lower_leaf(node_id, at.path.as_deref(), action, store),
        }
    }

    fn describe(&self, at: &UiAction) -> String {
        describe_action(&at.action).to_string()
    }

    fn resolve(&self, store: &BindingSources, binding: &Binding) -> ExprResolution {
        match resolve(store, binding) {
            Resolution::Resolved(resolved) => match to_state_value(&resolved) {
                Some(json) => ExprResolution::Resolved(json),
                None => ExprResolution::Errored(
                    "the bound source resolved to a collection, which has no state-slot form"
                        .to_string(),
                ),
            },
            Resolution::NotResolved => ExprResolution::NotResolved,
            Resolution::I18nUnresolved(_) => {
                ExprResolution::Errored("the bound source is an unresolved i18n key".to_string())
            }
            // The seam's error channel, VERBATIM: it already names the remedy.
            Resolution::Errored(msg) => ExprResolution::Errored(msg),
        }
    }

    fn assign(&self, key: &str, value: JVal, mut store: BindingSources) -> BindingSources {
        store.state.insert(key.to_string(), value);
        store
    }

    /// The core's read of one store key (Phase 2268): the same slot `assign` writes, so a
    /// windowed limit's counter under the reserved prefix reads back exactly as it was committed.
    fn read(&self, key: &str, store: &BindingSources) -> Option<JVal> {
        store.state.get(key).cloned()
    }

    fn is_reserved(&self, key: &str) -> bool {
        key.starts_with(HOST_RESERVED_STATE_PREFIX)
    }

    fn reserved_prefix(&self) -> &str {
        HOST_RESERVED_STATE_PREFIX
    }
}

impl DispatchWitness for UiFold<'_> {
    type Node = Node;

    /// The action an event dispatches on this node: a gesture, addressed at its root.
    fn handler(&self, node: &Node, event: &str) -> Option<UiAction> {
        handler_for(node, event).map(|a| UiAction::at(a, ""))
    }

    fn resolve_node(&self, store: &BindingSources, node: &Node) -> Node {
        resolve_node(store, node)
    }
}

/// The UI walk witness: the structural child list the structural tree-ops address, and a
/// data-bearing node weighted by the data it carries.
#[derive(Clone, Copy, Default)]
pub(crate) struct UiWalk;

impl WalkWitness for UiWalk {
    type Node = Node;

    fn id<'n>(&self, node: &'n Node) -> &'n str {
        &node.id
    }

    fn children<'n>(&self, node: &'n Node) -> Vec<&'n Node> {
        structural_children(&node.kind)
    }

    fn replace_children(&self, node: &Node, children: Vec<Node>) -> Node {
        replace_children(node, children)
    }

    fn cost(&self, node: &Node) -> u64 {
        node_data_cost(node)
    }
}
