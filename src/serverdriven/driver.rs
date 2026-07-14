//! The server-side driver: it holds the current tree, validates each inbound
//! event default-deny by shape (the trust boundary), lets the host handler decide
//! the `TreeOp`s, **consults the [`CapabilityGate`] before applying any op**
//! (FGP 3 — default-deny by authority), then applies them via the apply engine to
//! keep the server tree authoritative, returning the applied ops as the frame
//! content. State beyond the tree lives in the host handler's closure.

use std::collections::BTreeSet;

use crate::gate::{CapabilityGate, GateDecision};
use crate::introspect::get_node;
use crate::ops::apply;
use crate::wire::{Node, SemanticStyle, StateBehaviour, TreeOp};

use super::frame::Event;

/// Classifies a refused event (parity with the sibling driver's reject
/// vocabulary, extended with the Rust host's capability gate). A reject mutates
/// no state and pushes no frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The event targets a node absent from the current server tree (stale/forged id).
    UnknownNode,
    /// The event is not one the node's kind accepts.
    IllegitimateEvent,
    /// The host handler refused, or produced ops that do not apply.
    DispatchDenied,
    /// A driven op was refused by the capability gate (an ungranted mount).
    CapabilityDenied,
}

impl RejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectReason::UnknownNode => "UnknownNode",
            RejectReason::IllegitimateEvent => "IllegitimateEvent",
            RejectReason::DispatchDenied => "DispatchDenied",
            RejectReason::CapabilityDenied => "CapabilityDenied",
        }
    }
}

/// A structured refusal: a reason, the offending node, and a human/AI-readable
/// detail (plus the ungranted capabilities for a [`RejectReason::CapabilityDenied`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
    pub reason: RejectReason,
    pub node_id: String,
    pub message: String,
    pub missing_capabilities: Vec<String>,
}

/// The host's per-event decision function: given the current tree and a
/// structurally-validated event, return the ops to apply (the frame), or an
/// error string to refuse the event (the dispatch gate — default-deny). The host
/// holds its model in the closure.
pub type Handler = Box<dyn Fn(&Node, &Event) -> Result<Vec<TreeOp>, String>>;

/// The event names a node's kind accepts (the legitimacy check). A kind not
/// listed accepts nothing (default-deny — only interactive kinds take events).
fn legitimate_event(kind: &str, event: &str) -> bool {
    matches!(
        (kind, event),
        ("Button", "click")
            | ("Select", "change")
            | ("Form", "submit" | "change" | "input")
            | ("Filters", "change" | "input" | "click")
            | ("FileUpload", "change" | "file-read")
            | ("Tabs", "click" | "change")
            | ("Stepper", "click" | "change")
            | ("Disclosure", "click" | "change" | "toggle")
    )
}

/// The server-held tree, the host event handler, and the authority gate. Not safe
/// for concurrent [`Session::step`] calls — a [`super::Connection`] owns one
/// session and serialises events (a live connection is one evolving model).
pub struct Session {
    tree: Node,
    handler: Handler,
    gate: CapabilityGate,
}

impl Session {
    /// Build a session over an initial tree and the host's event handler. The
    /// default gate grants nothing (default-deny); ops introducing no gated
    /// surface (no `Mount`) are unaffected.
    pub fn new(tree: Node, handler: Handler) -> Self {
        Session {
            tree,
            handler,
            gate: CapabilityGate::default(),
        }
    }

    /// Set the authority gate (builder-style) — the grants a driven op's mounts
    /// are vetted against.
    pub fn with_gate(mut self, gate: CapabilityGate) -> Self {
        self.gate = gate;
        self
    }

    /// The current server-held tree.
    pub fn tree(&self) -> &Node {
        &self.tree
    }

    /// Drive one inbound event: validate structurally, run the host handler,
    /// consult the capability gate for each produced op, apply each op to advance
    /// the server tree, and return the applied ops (the frame content). A refused
    /// event returns a typed [`Reject`] and leaves the tree untouched. An empty op
    /// list is a legitimate no-op. Never panics.
    pub fn step(&mut self, ev: &Event) -> Result<Vec<TreeOp>, Reject> {
        let Some(node) = get_node(&self.tree, &ev.node_id) else {
            return Err(Reject {
                reason: RejectReason::UnknownNode,
                node_id: ev.node_id.clone(),
                message: format!("unknown node '{}' (stale or forged id)", ev.node_id),
                missing_capabilities: vec![],
            });
        };
        let kind = node.kind.type_name();
        if !legitimate_event(kind, &ev.event) {
            return Err(Reject {
                reason: RejectReason::IllegitimateEvent,
                node_id: ev.node_id.clone(),
                message: format!("event '{}' is not legitimate for a {kind}", ev.event),
                missing_capabilities: vec![],
            });
        }

        let ops = (self.handler)(&self.tree, ev).map_err(|e| Reject {
            reason: RejectReason::DispatchDenied,
            node_id: ev.node_id.clone(),
            message: format!("dispatch denied for node '{}': {e}", ev.node_id),
            missing_capabilities: vec![],
        })?;

        // Authority gate — every driven op is vetted before it can touch the
        // tree (FGP 3). A denied op names the ungranted capabilities.
        for op in &ops {
            if let GateDecision::Deny { missing } = op_gate_decision(&self.gate, op) {
                return Err(Reject {
                    reason: RejectReason::CapabilityDenied,
                    node_id: ev.node_id.clone(),
                    message: format!(
                        "driven op refused by the capability gate; ungranted: {}",
                        missing.join(", ")
                    ),
                    missing_capabilities: missing,
                });
            }
        }

        // Apply each op to advance the authoritative tree; an op that does not
        // apply is a rejected step (no partial mutation — the tree only advances
        // on a fully-applying set).
        let mut next = self.tree.clone();
        for op in &ops {
            match apply(&next, op) {
                Ok(outcome) => next = outcome.new_tree,
                Err(e) => {
                    return Err(Reject {
                        reason: RejectReason::DispatchDenied,
                        node_id: ev.node_id.clone(),
                        message: format!("handler produced an inapplicable op: {e}"),
                        missing_capabilities: vec![],
                    });
                }
            }
        }
        self.tree = next;
        Ok(ops)
    }
}

/// The capability-gate decision for a single driven op: any node the op
/// introduces (an inserted child, a replaced root, an edited kind, or a batch
/// thereof) has its `Mount` capability declarations vetted against the gate.
/// Ops that introduce no gated surface are `Allow`.
pub fn op_gate_decision(gate: &CapabilityGate, op: &TreeOp) -> GateDecision {
    let mut missing: BTreeSet<String> = BTreeSet::new();
    collect_op_missing(gate, op, &mut missing);
    if missing.is_empty() {
        GateDecision::Allow
    } else {
        GateDecision::Deny {
            missing: missing.into_iter().collect(),
        }
    }
}

fn collect_node_missing(gate: &CapabilityGate, node: &Node, missing: &mut BTreeSet<String>) {
    for (_, decision) in gate.audit_mounts(node) {
        if let GateDecision::Deny { missing: m } = decision {
            missing.extend(m);
        }
    }
}

fn collect_op_missing(gate: &CapabilityGate, op: &TreeOp, missing: &mut BTreeSet<String>) {
    match op {
        TreeOp::InsertChild { child, .. } => collect_node_missing(gate, child, missing),
        TreeOp::ReplaceRoot { node } => collect_node_missing(gate, node, missing),
        TreeOp::EditNode { target, new_kind } => {
            let scratch = Node {
                id: target.clone(),
                kind: new_kind.clone(),
                state: StateBehaviour::default(),
                style: SemanticStyle::default(),
                accessibility: None,
            };
            collect_node_missing(gate, &scratch, missing);
        }
        TreeOp::Batch(ops) => {
            for inner in ops {
                collect_op_missing(gate, inner, missing);
            }
        }
        _ => {}
    }
}
