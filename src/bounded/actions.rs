//! The bounded action interpreter — one resolved action, one store, one
//! outcome.
//!
//! ## The safety property, stated and tested
//!
//! A program tree that arrived over the wire is **data, not code**. The wire
//! format cannot carry a closure, so the decoder erases every closure slot; this
//! interpreter enforces the other half, and it enforces it by construction
//! rather than by remembering to: there is no closure in the decoded model for
//! it to invoke. The only store mutation it performs is the state write; the
//! only outward reach is the closed client-effect vocabulary. Together —
//! bounded wire, so no foreign code in the tree; this fold, so nothing foreign
//! is ever called — running an emitted program has no arbitrary-code-execution
//! surface.
//!
//! ## One evaluating match, in one file
//!
//! This is the **only** place anything here interprets an action, which is a
//! property a reader can check rather than a claim they have to trust. The
//! budget's cost accounting walks the same closed vocabulary without
//! interpreting it — it performs, mutates and resolves nothing — and that
//! distinction is why it is not a second evaluator.
//!
//! ## Documented no-ops, never silent ones
//!
//! Several arms have no form on this path. Each is a **documented no-op that
//! emits a readable diagnostic**, not a silent nothing: a program that intended
//! one of them is observable to whoever is debugging its emission. The no-op is
//! the correct behaviour; the diagnostic is what makes it debuggable.
//!
//! ## Where a handler-running placement would attach
//!
//! A call action is recognised by this fold **at every depth**, because the fold
//! is the only thing that knows where in a chain a call sits — a placement that
//! matched on the action itself to find nested calls would be a second
//! evaluator. What a call *means* is placement-specific; *where* it is
//! recognised is not. This host implements the client placement, which registers
//! no handlers, so a call resolves to the documented no-op it has always been
//! where nothing is registered. The seam a handler-running placement would fill
//! is the one arm below, and it is deliberately absent rather than stubbed:
//! a placement that ran handlers would thread its own accumulation through this
//! fold, and inventing that shape before there is a second placement to fit it
//! would bake this one's assumptions into the contract.

use crate::canonical::JVal;
use crate::render::BindingSources;
use crate::render::bindings::{Resolution, Value, resolve};
use crate::render::sanitize::sanitize_url;
use crate::wire::{Action, FileReadEncoding, StaticValue};

use super::effect::ClientEffect;

/// The state namespace a host reserves for itself. A program is untrusted by
/// construction, so a program writing under it is exactly the case the namespace
/// exists for — and is refused, not quietly honoured.
pub const HOST_RESERVED_STATE_PREFIX: &str = "host.";

/// A readable "this did nothing, on purpose" signal.
///
/// The two arms mean opposite things to whoever is debugging an emission —
/// *this path does not implement that* versus *that was not allowed* — so they
/// are kept apart. Both name the action's **discriminator** only: every string
/// inside an action came off an untrusted wire, and a diagnostic that echoed one
/// would be the leak every other rule here avoids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundedDiagnostic {
    /// The action is inert on the bounded path — it has no form here.
    UnsupportedOnBoundedPath { node_id: String, action: String },
    /// The action was **refused**, not merely inert.
    Refused {
        node_id: String,
        action: String,
        reason: String,
    },
}

impl BoundedDiagnostic {
    /// A log-safe description.
    pub fn describe(&self) -> String {
        match self {
            BoundedDiagnostic::UnsupportedOnBoundedPath { node_id, action } => format!(
                "action '{action}' on node '{node_id}' is inert on the bounded path (it has no form for this loop)"
            ),
            BoundedDiagnostic::Refused {
                node_id,
                action,
                reason,
            } => format!("action '{action}' on node '{node_id}' was refused: {reason}"),
        }
    }
}

/// Interpreting one action: the store as the fold left it, the client effects
/// the action reached in order, and the diagnostics it wants observed.
///
/// The store is returned rather than mutated in place, so a placement threads it
/// functionally — one interaction, one new store value, and no half-applied
/// cascade is representable.
#[derive(Debug, Clone)]
pub struct BoundedOutcome {
    pub store: BindingSources,
    pub effects: Vec<ClientEffect>,
    pub diagnostics: Vec<BoundedDiagnostic>,
}

/// The action's discriminator — log-safe by construction, since the vocabulary
/// is closed and this host controls every string it can return.
pub fn describe_action(action: &Action) -> &'static str {
    match action {
        Action::Dispatch => "Dispatch",
        Action::Call { .. } => "Call",
        Action::Notify { .. } => "Notify",
        Action::Navigate { .. } => "Navigate",
        Action::SetState { .. } => "SetState",
        Action::AiTool { .. } => "AiTool",
        Action::Chain(_) => "Chain",
        Action::CommitLocal { .. } => "CommitLocal",
        Action::WriteToClipboard { .. } => "WriteToClipboard",
        Action::ReadFileBody { .. } => "ReadFileBody",
        Action::Invoke { .. } => "Invoke",
    }
}

fn unchanged(store: BindingSources) -> BoundedOutcome {
    BoundedOutcome {
        store,
        effects: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn no_op(node_id: &str, action: &Action, store: BindingSources) -> BoundedOutcome {
    BoundedOutcome {
        store,
        effects: Vec::new(),
        diagnostics: vec![BoundedDiagnostic::UnsupportedOnBoundedPath {
            node_id: node_id.to_string(),
            action: describe_action(action).to_string(),
        }],
    }
}

fn refused(
    node_id: &str,
    action: &Action,
    reason: impl Into<String>,
    store: BindingSources,
) -> BoundedOutcome {
    BoundedOutcome {
        store,
        effects: Vec::new(),
        diagnostics: vec![BoundedDiagnostic::Refused {
            node_id: node_id.to_string(),
            action: describe_action(action).to_string(),
            reason: reason.into(),
        }],
    }
}

fn emitted(store: BindingSources, effect: ClientEffect) -> BoundedOutcome {
    BoundedOutcome {
        store,
        effects: vec![effect],
        diagnostics: Vec::new(),
    }
}

/// The state-slot form of a resolved value. The collection payloads have no
/// state-slot form on this wire, and are reported rather than coerced: writing
/// some flattened stand-in would be the interpreter inventing a value nobody
/// asked for.
fn to_state_value(value: &Value<'_>) -> Option<JVal> {
    match value {
        Value::Json(json) => Some((*json).clone()),
        Value::Text(text) => Some(JVal::Str(text.clone())),
        Value::Static(StaticValue::Ast(json)) => Some(json.clone()),
        Value::Static(StaticValue::StringOpt(Some(s))) => Some(JVal::Str(s.clone())),
        Value::Static(StaticValue::StringOpt(None)) => Some(JVal::Null),
        Value::Static(StaticValue::StringList(items)) => Some(JVal::Arr(
            items.iter().map(|s| JVal::Str(s.clone())).collect(),
        )),
        Value::Static(StaticValue::FloatSeq(items)) => {
            Some(JVal::Arr(items.iter().map(|n| JVal::Num(*n)).collect()))
        }
        Value::Static(_) => None,
    }
}

fn file_read_encoding(encoding: FileReadEncoding) -> &'static str {
    match encoding {
        FileReadEncoding::Text => "Text",
        FileReadEncoding::Base64 => "Base64",
        FileReadEncoding::DataUrl => "DataUrl",
    }
}

/// Interpret one action against the store.
///
/// `node_id` is the originating event's node — the address a node-addressed
/// client effect carries.
pub fn run_bounded_action(node_id: &str, action: &Action, store: BindingSources) -> BoundedOutcome {
    match action {
        // ── The one store mutation ───────────────────────────────────────────
        Action::SetState {
            key,
            value,
            value_from,
        } => {
            if key.starts_with(HOST_RESERVED_STATE_PREFIX) {
                return refused(
                    node_id,
                    action,
                    format!(
                        "the state key is under the host-reserved '{HOST_RESERVED_STATE_PREFIX}' namespace"
                    ),
                    store,
                );
            }
            // `value` XOR `value_from`, which the decoder enforces. A bound
            // source evaluates AT DISPATCH TIME against the store itself, and a
            // source that does not resolve performs NO write and is diagnosed —
            // never a silent skip and never a fabricated default.
            let payload = match (value_from, value) {
                (Some(binding), _) => match resolve(&store, binding) {
                    Resolution::Resolved(resolved) => match to_state_value(&resolved) {
                        Some(json) => Ok(json),
                        None => Err(
                            "the bound source resolved to a collection, which has no state-slot form — no write performed"
                                .to_string(),
                        ),
                    },
                    Resolution::NotResolved => Err(
                        "the bound source did not resolve to a value — no write performed"
                            .to_string(),
                    ),
                    Resolution::I18nUnresolved(_) => Err(
                        "the bound source is an unresolved i18n key — no write performed".to_string(),
                    ),
                },
                (None, Some(literal)) => Ok(literal.clone()),
                (None, None) => Err(
                    "the write declares neither a literal nor a bound source — no write performed"
                        .to_string(),
                ),
            };
            match payload {
                Ok(json) => {
                    let mut store = store;
                    store.state.insert(key.clone(), json);
                    unchanged(store)
                }
                Err(reason) => refused(node_id, action, reason, store),
            }
        }

        // ── The inherently-surface arms ──────────────────────────────────────
        //
        // The route is checked before the effect is shipped: the host navigates
        // with its own router, so an unsafe scheme reaching it would land as a
        // client-side sink. A refusal emits NO effect and one diagnostic — never
        // a silently-neutered destination, which would leave an author believing
        // the navigation happened somewhere.
        Action::Navigate { route } => match sanitize_url(route) {
            Some(safe) => emitted(
                store,
                ClientEffect::Navigate {
                    route: safe.into_owned(),
                },
            ),
            None => refused(node_id, action, "the route is not a safe URL", store),
        },
        Action::WriteToClipboard { text } => {
            emitted(store, ClientEffect::WriteToClipboard { text: text.clone() })
        }
        Action::ReadFileBody { encoding, .. } => emitted(
            store,
            ClientEffect::ReadFileBody {
                node_id: node_id.to_string(),
                encoding: file_read_encoding(*encoding).to_string(),
            },
        ),

        // ── Composition ──────────────────────────────────────────────────────
        //
        // Fold in order, threading the store and concatenating effects and
        // diagnostics. Threading the store is what makes an action mid-chain see
        // the write before it and be seen by the write after it — and it is why
        // a nested call behaves exactly as a top-level one, the chain being the
        // only structure that could have made them differ.
        Action::Chain(inner) => inner.iter().fold(unchanged(store), |acc, next| {
            let mut acc = acc;
            let step = run_bounded_action(node_id, next, acc.store);
            acc.store = step.store;
            acc.effects.extend(step.effects);
            acc.diagnostics.extend(step.diagnostics);
            acc
        }),

        // ── Documented no-ops ────────────────────────────────────────────────
        //
        // Host-channel and capability arms fan out to machinery this placement
        // does not have; a dispatch carries only an erased payload and there is
        // no update function to fold it through; a local-buffer commit is a host
        // concern whose flushed value arrives as the event payload instead.
        Action::Notify { .. }
        | Action::AiTool { .. }
        | Action::Invoke { .. }
        | Action::Dispatch
        | Action::CommitLocal { .. } => no_op(node_id, action, store),

        // A call that ALSO declares where its answer should land is refused
        // rather than honoured or quietly ignored. Result-target ownership sits
        // with the handler — its stages name landing slots, one per result — and
        // a tree-declared target is a second mechanism for the same job that no
        // placement honours. Refusing makes that observable; ignoring would
        // leave an author believing an answer lands somewhere it never does.
        //
        // The reason names neither the endpoint nor the target: both come off
        // the wire.
        Action::Call { into: Some(_), .. } => refused(
            node_id,
            action,
            "the call declares a result target; a handler declares where its own results land",
            store,
        ),
        Action::Call { into: None, .. } => no_op(node_id, action, store),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(key: &str, value: &str) -> BindingSources {
        let mut sources = BindingSources::default();
        sources
            .state
            .insert(key.to_string(), JVal::Str(value.to_string()));
        sources
    }

    fn set(key: &str, value: &str) -> Action {
        Action::SetState {
            key: key.into(),
            value: Some(JVal::Str(value.into())),
            value_from: None,
        }
    }

    #[test]
    fn a_chain_of_two_writes_to_one_key_lets_the_second_win() {
        let outcome = run_bounded_action(
            "n",
            &Action::Chain(vec![set("msg", "first"), set("msg", "second")]),
            BindingSources::default(),
        );
        assert_eq!(
            outcome.store.state.get("msg"),
            Some(&JVal::Str("second".into()))
        );
    }

    #[test]
    fn an_unsafe_route_emits_no_effect_and_says_so() {
        let outcome = run_bounded_action(
            "n",
            &Action::Navigate {
                route: "javascript:alert(1)".into(),
            },
            BindingSources::default(),
        );
        assert!(outcome.effects.is_empty(), "no effect is shipped");
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [BoundedDiagnostic::Refused { .. }]
        ));
    }

    #[test]
    fn a_write_under_the_host_reserved_namespace_is_refused() {
        let outcome = run_bounded_action(
            "n",
            &set("host.session", "forged"),
            BindingSources::default(),
        );
        assert!(outcome.store.state.is_empty());
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [BoundedDiagnostic::Refused { .. }]
        ));
    }

    #[test]
    fn a_bound_write_reads_the_store_at_dispatch_time() {
        let action = Action::SetState {
            key: "copy".into(),
            value: None,
            value_from: Some(Box::new(crate::wire::Binding::State {
                key: "msg".into(),
                default_value: StaticValue::Ast(JVal::Str("fallback".into())),
            })),
        };
        let outcome = run_bounded_action("n", &action, store_with("msg", "live"));
        assert_eq!(
            outcome.store.state.get("copy"),
            Some(&JVal::Str("live".into()))
        );
    }

    #[test]
    fn a_diagnostic_names_the_discriminator_and_never_a_wire_carried_string() {
        let outcome = run_bounded_action(
            "n",
            &Action::Call {
                endpoint: "/handlers/secret-endpoint".into(),
                into: None,
                on_result: None,
            },
            BindingSources::default(),
        );
        let described = outcome.diagnostics[0].describe();
        assert!(described.contains("'Call'"));
        assert!(
            !described.contains("secret-endpoint"),
            "the endpoint came off the wire and must not be echoed: {described}"
        );
    }
}
