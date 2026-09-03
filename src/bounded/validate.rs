//! The inbound trust boundary — default-deny, before anything folds.
//!
//! A surface sends raw `(nodeId, event, payload)`. None of it is trusted: a
//! forged or stale node id, an event the node's kind does not accept, an
//! out-of-range payload, or an action this host's policy declines are all attack
//! surface, and the deployments that most want a program carried as data are
//! exactly the ones an authorisation bypass breaks.
//!
//! Four checks, in order, first failure winning:
//!
//! 1. the node id exists in the **current** tree;
//! 2. the event is legitimate for that node's kind — a button accepts a click, a
//!    select a change, and a heading accepts nothing;
//! 3. the payload is inside the node's value space, so far as that space is
//!    statically resolvable;
//! 4. the resolved action passes the host's dispatch policy.
//!
//! Pure, and a function of `(tree, event)` alone: no transport, no renderer, no
//! clock. It compiles and behaves identically on every target this crate builds
//! for.

use std::collections::BTreeMap;

use crate::introspect::get_node;
use crate::render::BindingSources;
use crate::render::bindings::resolve_options;
use crate::wire::{Action, FormFieldKind, Node, NodeKind};

/// One payload value off the wire — the closure-free, portable subset a surface
/// can send. A small closed enum rather than an erased carrier, so the bounds
/// checks below can be precise.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveValue {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
}

/// A raw inbound event. **Untrusted** until [`validate`] returns `Ok`.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveEvent {
    pub node_id: String,
    pub event: String,
    pub payload: BTreeMap<String, LiveValue>,
}

/// Why the boundary refused an event. Every refusal path produces one of these,
/// and none of them mutates anything.
#[derive(Debug, Clone, PartialEq)]
pub enum RejectReason {
    /// No node with this id in the current tree — stale, or forged.
    UnknownNode { node_id: String },
    /// The event is not one the node's kind accepts.
    IllegitimateEvent {
        node_id: String,
        event: String,
        kind: &'static str,
    },
    /// The payload is outside the node's value space.
    PayloadOutOfBounds { node_id: String, detail: String },
    /// The resolved action was declined by the host's dispatch policy.
    DispatchDenied { node_id: String, action: String },
}

impl RejectReason {
    /// A short, log-safe description. It carries the reason, the node id and the
    /// kind; it never carries a payload value, since those are the part a caller
    /// chose.
    pub fn describe(&self) -> String {
        match self {
            RejectReason::UnknownNode { node_id } => {
                format!("unknown node '{node_id}' (stale or forged id)")
            }
            RejectReason::IllegitimateEvent {
                node_id,
                event,
                kind,
            } => format!("event '{event}' is not legitimate for node '{node_id}' (a {kind})"),
            RejectReason::PayloadOutOfBounds { node_id, detail } => {
                format!("payload out of bounds for node '{node_id}': {detail}")
            }
            RejectReason::DispatchDenied { node_id, action } => {
                format!("dispatch declined by policy for node '{node_id}': {action}")
            }
        }
    }
}

/// The boundary's output: the resolved node, and the action to interpret.
///
/// `action` is `None` for a legitimate event that resolves to no action — which
/// is **not** a refusal and must not be reported as one. The commonest cause is
/// structural rather than exceptional: a handler slot that is a closure does not
/// survive the wire, so a control whose action would have come from one has no
/// action to resolve on a decoded tree.
#[derive(Debug, Clone)]
pub struct ValidatedEvent<'a> {
    pub node: &'a Node,
    pub action: Option<Action>,
}

/// The event names a node's kind accepts. Everything non-interactive accepts
/// nothing — the default-deny half, spelled out rather than implied.
///
/// Exhaustive with **no catch-all**: a kind added to the vocabulary is a build
/// error here until somebody decides what, if anything, it accepts. Falling
/// through to "nothing" silently would be the same decision made by nobody.
pub fn legitimate_events(kind: &NodeKind) -> &'static [&'static str] {
    match kind {
        NodeKind::Button(_) => &["click"],
        NodeKind::Select(_) => &["change"],
        // A form receives its own submit, plus the field-level change and input
        // that bubble to it.
        NodeKind::Form(_) => &["submit", "change", "input"],
        // Field-level events bubble to the filters node; a segmented control's
        // selection arrives as a bubbled click.
        NodeKind::Filters(_) => &["change", "input", "click"],
        NodeKind::FileUpload(_) => &["change", "file-read"],
        // Tab headers, step headers and disclosure summaries arrive as bubbled
        // clicks.
        NodeKind::Tabs(_) => &["click", "change"],
        NodeKind::Stepper(_) => &["click", "change"],
        NodeKind::Disclosure(_) => &["click", "change", "toggle"],
        // Phase 1120 — a `Tree` row is a control the reader drives: the row
        // selection arrives as a bubbled click (which is also where a keyboard
        // Enter/Space lands), and the expansion toggle as a change. Same
        // reasoning as `Tabs` and `Stepper`, and the kind carries the matching
        // wire surface — `onSelect` and two named State slots the host writes.
        NodeKind::Tree(_) => &["click", "change"],

        NodeKind::Box(_)
        | NodeKind::SplitPanel(_)
        | NodeKind::SummaryList(_)
        | NodeKind::Modal(_)
        | NodeKind::ScrollArea(_)
        | NodeKind::Heading(_)
        | NodeKind::Markdown(_)
        | NodeKind::Metric(_)
        | NodeKind::Badge(_)
        | NodeKind::Sparkline(_)
        | NodeKind::Callout(_)
        | NodeKind::Progress(_)
        | NodeKind::Skeleton(_)
        | NodeKind::Icon(_)
        | NodeKind::LabelValueRow(_)
        | NodeKind::Fact(_)
        | NodeKind::Link(_)
        | NodeKind::Image(_)
        // Phase 1076 — `Media` accepts NO event, and that is a decision rather
        // than an omission. A `<video controls>` is a complete interactive
        // control the browser owns: play, pause and seek are the transport's,
        // not a dispatch site's. The kind declares no handler slot and reaches
        // no closure-bearing position (§4), so there is nothing an inbound
        // event could legitimately be addressed to.
        | NodeKind::Media(_)
        // Phase 1111 — `Embed` accepts NO event, and that is a decision rather
        // than an omission. The framed document is a separate browsing context
        // this host deliberately isolates; events inside it are the guest's and
        // never cross the sandbox, and the kind declares no handler slot and
        // reaches no closure-bearing position (§4), so there is nothing an
        // inbound event could legitimately be addressed to. `Mount` is the kind
        // for a COOPERATING guest with a declared channel.
        | NodeKind::Embed(_)
        | NodeKind::List(_)
        | NodeKind::Toast(_)
        | NodeKind::CodeBlock(_)
        | NodeKind::Math(_)
        | NodeKind::Drawing(_)
        | NodeKind::DataGrid(_)
        | NodeKind::Chart(_)
        | NodeKind::Map(_)
        | NodeKind::Custom(_)
        | NodeKind::ErrorBoundary(_)
        | NodeKind::Switch(_)
        | NodeKind::FragmentDecl(_)
        | NodeKind::FragmentRef(_)
        | NodeKind::Mount(_) => &[],
    }
}

fn payload_str<'a>(payload: &'a BTreeMap<String, LiveValue>, key: &str) -> Option<&'a str> {
    match payload.get(key) {
        Some(LiveValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Check the payload is inside the node's value space, so far as this host can
/// decide it **without** binding context. A dynamically-sourced option list is
/// accepted here and bounded downstream against the live sources; refusing it
/// would refuse a correct event for want of data the check cannot see.
fn bounds_check(node: &Node, event: &LiveEvent) -> Result<(), RejectReason> {
    let empty = BindingSources::default();
    match &node.kind {
        NodeKind::Select(spec) if event.event == "change" => {
            let Some(chosen) = payload_str(&event.payload, "value") else {
                return Ok(());
            };
            let options = resolve_options(&empty, &spec.source);
            // No statically-resolvable options ⇒ a dynamic source; accept.
            if options.is_empty() || options.iter().any(|o| o.value == chosen) {
                Ok(())
            } else {
                Err(RejectReason::PayloadOutOfBounds {
                    node_id: event.node_id.clone(),
                    detail: "the chosen value is not among the select's options".into(),
                })
            }
        }
        NodeKind::Filters(items) => {
            // A name-addressed filter event must name a filter the node
            // declares: a forged or stale name is attack surface exactly as a
            // forged node id is. An event carrying no name resolves to no action
            // downstream, so there is nothing to bound.
            let Some(name) = payload_str(&event.payload, "name") else {
                return Ok(());
            };
            let Some(filter) = items.iter().find(|f| f.name == name) else {
                return Err(RejectReason::PayloadOutOfBounds {
                    node_id: event.node_id.clone(),
                    detail: "the named filter is not one this node declares".into(),
                });
            };
            let options = match &filter.kind {
                FormFieldKind::Choice { options, .. } => Some(options),
                FormFieldKind::SegmentedChoice { options, .. } => Some(options),
                _ => None,
            };
            let (Some(options), Some(chosen)) = (options, payload_str(&event.payload, "value"))
            else {
                return Ok(());
            };
            // The empty string is the clear-to-none option, not a value to bound.
            if chosen.is_empty() {
                return Ok(());
            }
            let resolved = resolve_options(&empty, options);
            if resolved.is_empty() || resolved.iter().any(|o| o.value == chosen) {
                Ok(())
            } else {
                Err(RejectReason::PayloadOutOfBounds {
                    node_id: event.node_id.clone(),
                    detail: "the chosen value is not among that filter's options".into(),
                })
            }
        }
        _ => Ok(()),
    }
}

/// Resolve the action a legitimate `(node, event, payload)` names.
///
/// **Two kinds carry a whole action on the wire** — a button's click and a
/// form's submit — and those are the two that resolve here. Every other
/// interactive control names its behaviour through a *handler slot*, which is a
/// closure: closures do not survive the wire, the decoder erases them, and this
/// host models that erasure faithfully rather than inventing something to
/// dispatch. So a select's change, a disclosure's toggle, a tab's or stepper's
/// selection resolve to **no action** on a decoded tree, and the loop treats
/// that as the legitimate no-op it is.
///
/// This is not a gap being deferred: on a decoded tree there is nothing to
/// recover. A surface that wants those controls to move state does it the way
/// the wire provides for — the write-back over the control's own writable slot —
/// which is a host act, not an action this boundary could resolve.
///
/// Specified since §10.5: an event on a closure-slotted control resolves to no
/// action **at every host**, and a host MUST NOT recover behaviour from such a
/// slot. Whether a decoder erases the slot or substitutes an inert placeholder
/// is a mechanism, and the two are observationally identical — which is what
/// closes the cross-host divergence this comment used to record as open. The
/// resulting step is inert and, per the same section, is **not** a refusal.
pub fn resolve_action(node: &Node, event: &LiveEvent) -> Option<Action> {
    match &node.kind {
        NodeKind::Button(spec) if event.event == "click" => Some(spec.on_click.clone()),
        NodeKind::Form(spec) if event.event == "submit" => Some(spec.on_submit.clone()),
        _ => None,
    }
}

/// Validate one untrusted event against the current tree and the host's dispatch
/// policy. First failed check wins; success yields the resolved node and the
/// gated action.
pub fn validate<'a>(
    can_dispatch: &dyn Fn(&Action) -> bool,
    tree: &'a Node,
    event: &LiveEvent,
) -> Result<ValidatedEvent<'a>, RejectReason> {
    let Some(node) = get_node(tree, &event.node_id) else {
        return Err(RejectReason::UnknownNode {
            node_id: event.node_id.clone(),
        });
    };
    if !legitimate_events(&node.kind).contains(&event.event.as_str()) {
        return Err(RejectReason::IllegitimateEvent {
            node_id: event.node_id.clone(),
            event: event.event.clone(),
            kind: node.kind.type_name(),
        });
    }
    bounds_check(node, event)?;
    match resolve_action(node, event) {
        None => Ok(ValidatedEvent { node, action: None }),
        Some(action) => {
            if can_dispatch(&action) {
                Ok(ValidatedEvent {
                    node,
                    action: Some(action),
                })
            } else {
                Err(RejectReason::DispatchDenied {
                    node_id: event.node_id.clone(),
                    action: super::actions::describe_action(&action).to_string(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::decode_node;

    const TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[{"id":"set","kind":{"$type":"Button","label":"set","onClick":{"$type":"SetState","key":"msg","value":"updated"},"variant":"Secondary"}},{"id":"readout","kind":{"$type":"Markdown","text":"x"}}],"layout":{"$type":"Auto"},"role":"Dashboard"}}"#;

    fn event(node_id: &str, name: &str) -> LiveEvent {
        LiveEvent {
            node_id: node_id.into(),
            event: name.into(),
            payload: BTreeMap::new(),
        }
    }

    #[test]
    fn a_forged_node_id_and_an_illegitimate_event_are_both_refused() {
        let tree = decode_node(TREE).expect("the fixture decodes");
        let permit = |_: &Action| true;
        assert!(matches!(
            validate(&permit, &tree, &event("ghost", "click")),
            Err(RejectReason::UnknownNode { .. })
        ));
        // A markdown node accepts nothing at all.
        assert!(matches!(
            validate(&permit, &tree, &event("readout", "click")),
            Err(RejectReason::IllegitimateEvent { .. })
        ));
        // And a button accepts a click, not a submit.
        assert!(matches!(
            validate(&permit, &tree, &event("set", "submit")),
            Err(RejectReason::IllegitimateEvent { .. })
        ));
    }

    #[test]
    fn the_policy_gate_is_consulted_and_defaults_to_deny() {
        let tree = decode_node(TREE).expect("the fixture decodes");
        assert!(matches!(
            validate(&|_| false, &tree, &event("set", "click")),
            Err(RejectReason::DispatchDenied { .. })
        ));
        let validated =
            validate(&|_| true, &tree, &event("set", "click")).expect("a permitted click resolves");
        assert!(validated.action.is_some());
    }
}
