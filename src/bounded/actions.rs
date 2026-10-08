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
use crate::render::bindings::{Resolution, Value, resolve, try_string};
use crate::render::sanitize::sanitize_url;
use crate::wire::{Action, FileReadEncoding, NavigateTarget, StaticValue, TextSource};

use super::effect::ClientEffect;

/// The state namespace a host reserves for itself. A program is untrusted by
/// construction, so a program writing under it is exactly the case the namespace
/// exists for — and is refused, not quietly honoured.
///
/// The reservation binds **this** placement as well as the one the specification
/// spells it under: §4.3 rules it a property of the namespace rather than of a
/// handler document, because untrustedness does not vary by which loop is
/// running. What is placement-specific is only the refusal's shape — a client
/// write under `host.` is a legitimate document whose action does nothing, not a
/// decode failure.
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
        Action::Print => "Print",
        Action::Confirm { .. } => "Confirm",
        Action::Focus { .. } => "Focus",
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

/// A `TextSource` resolved at DISPATCH time, through the same binding
/// resolution the host renders text slots with (§3.6.16 / §3.6's Navigate
/// obligation). `None` means the source did not resolve — the two call sites
/// answer that differently, and the difference is the specification's rather
/// than this function's: an unresolvable clipboard payload is the empty string,
/// where an unresolvable route navigates nowhere.
///
/// An `I18n` source is `None` here for the same reason: the loud
/// `[i18n:<key>]` sentinel a label renders is a relative path at a route slot,
/// which a permissive policy would fetch.
fn resolve_text_source(store: &BindingSources, text: &TextSource) -> Option<String> {
    match text {
        TextSource::Literal(literal) => Some(literal.clone()),
        TextSource::Bound(binding) => try_string(store, binding),
        TextSource::I18n { .. } => None,
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
///
/// A `Confirm` reached here is **unaddressed** and declined: with no address
/// there is no token, so its question could never be answered. A loop folds a
/// gesture through [`run_gesture`] instead, which addresses every confirm it
/// reaches.
pub fn run_bounded_action(node_id: &str, action: &Action, store: BindingSources) -> BoundedOutcome {
    run_at(node_id, None, action, store)
}

/// Interpret a **gesture** — the action an admitted, non-answer event resolves
/// to — with every confirm it reaches addressed, so each asks its question
/// carrying the token its answer will name (Phase 2106).
pub fn run_gesture(node_id: &str, action: &Action, store: BindingSources) -> BoundedOutcome {
    run_at(node_id, Some(""), action, store)
}

/// The token a question carries: the originating node and the confirm's
/// structural path inside that node's action — dot-joined chain positions and
/// continuation names, the empty path naming the action itself. The node is in
/// it so a token minted for one node cannot address another's action.
pub fn confirm_token(node_id: &str, path: &str) -> String {
    format!("{node_id}#{path}")
}

fn child_path(path: &str, index: usize) -> String {
    if path.is_empty() {
        index.to_string()
    } else {
        format!("{path}.{index}")
    }
}

fn branch_path(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

/// The confirm a token addresses inside the node's **current** action, with its
/// path. The token is untrusted payload: its node half must be this node, and a
/// segment that names nothing is an ordinary miss — a tree that moved on, or a
/// forged token — never a panic.
pub fn addressed_confirm<'a>(
    node_id: &str,
    token: &str,
    action: &'a Action,
) -> Option<(String, &'a Action)> {
    let path = token.strip_prefix(&format!("{node_id}#"))?;
    let mut at = action;
    if !path.is_empty() {
        for segment in path.split('.') {
            at = match (segment, at) {
                ("onConfirm", Action::Confirm { on_confirm, .. }) => on_confirm,
                (
                    "onCancel",
                    Action::Confirm {
                        on_cancel: Some(on_cancel),
                        ..
                    },
                ) => on_cancel,
                (index, Action::Chain(inner)) => {
                    let i: usize = index.parse().ok()?;
                    if i.to_string() != index {
                        return None;
                    }
                    inner.get(i)?
                }
                _ => return None,
            };
        }
    }
    match at {
        Action::Confirm { .. } => Some((path.to_string(), at)),
        _ => None,
    }
}

/// The continuation an answer selects: `onConfirm` on yes, `onCancel` on no —
/// `None` when the reader declined and the author declared no cancel branch,
/// which is what "nothing happens" is.
pub fn answer_branch(confirm: &Action, accepted: bool) -> Option<&Action> {
    match confirm {
        Action::Confirm {
            on_confirm,
            on_cancel,
            ..
        } => {
            if accepted {
                Some(on_confirm)
            } else {
                on_cancel.as_deref()
            }
        }
        _ => None,
    }
}

/// Interpret an **answer**: the addressed confirm folded as a selection over the
/// reader's answer — the core's `Choose` with the answer as its entry. Yes runs
/// `onConfirm`, no runs `onCancel` or nothing, each addressed under its own
/// continuation name.
pub fn run_answer(
    node_id: &str,
    path: &str,
    accepted: bool,
    confirm: &Action,
    store: BindingSources,
) -> BoundedOutcome {
    let name = if accepted { "onConfirm" } else { "onCancel" };
    match answer_branch(confirm, accepted) {
        Some(branch) => run_at(node_id, Some(&branch_path(path, name)), branch, store),
        None => unchanged(store),
    }
}

/// The one evaluating match. `path` is `Some` when the loop addressed this
/// position — a confirm there asks — and `None` when it did not.
fn run_at(
    node_id: &str,
    path: Option<&str>,
    action: &Action,
    store: BindingSources,
) -> BoundedOutcome {
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
                    // Phase 1667 — the seam's error channel. Reported with the
                    // message VERBATIM rather than wrapped: it already names the
                    // remedy, and a write is exactly where a fabricated default
                    // would be least recoverable — the wrong value would then be
                    // in the store, indistinguishable from one a reader typed.
                    Resolution::Errored(msg) => Err(format!("{msg} — no write performed")),
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
        //
        // The PREDICATE is not this host's choice: it is the tree wire
        // specification's renderer URL floor, which names the navigation
        // destination among the slots it governs, and reaches here as a
        // referenced value (program wire §3). What §10.5 adds is the RESPONSE —
        // decline the action rather than substitute a destination, since a loop
        // emits no markup for the floor's own rejection rule to govern. A host
        // whose policy is stricter than the floor must declare the divergence;
        // `sanitize_url` is exactly the floor and no more.
        //
        // So the §14.1 DESTINATION policy for a route is consulted one step
        // later, at the performer seam (`EffectPolicy::decide`, whose
        // `EgressFloor` judges a `Navigate` under `EgressClass::Route`), and
        // deliberately not here. Two reasons, and neither is convenience. The
        // interpreter's predicate is fixed by the specification above, and a
        // host that narrowed it here would fold differently from every other
        // conformant host on a scenario the corpus pins. And a step must still
        // REPORT an effect it reached even when the host declines it — a
        // declined effect dropped in the fold is indistinguishable from one that
        // was never reached, which is the one thing the denial record exists to
        // tell apart.
        //
        // Phase 1536 — RESOLVE, THEN GATE, in that order. A bound route is
        // resolved when the reader raises the action, and the scheme floor is
        // applied to the RESOLVED string: checking the declaration would
        // consult the floor about a template nobody navigates to while the
        // string the router actually receives went unexamined.
        //
        // An UNRESOLVED route performs no navigation, and this is where the
        // obligation differs from the clipboard's. At an ordinary text slot an
        // unresolvable binding resolves to the empty string; here `""` is a
        // real navigation — the current document with its query and fragment
        // stripped — so the honest answer is a diagnostic and nothing else.
        //
        // Phase 1664 — the target is CARRIED, where Phase 1536 had to refuse it.
        // The client-effect envelope now names the browsing context (an optional
        // member, omitted at `Self`), so the two targets are one arm again and
        // the interpreter has nothing to decide about them: it resolves the
        // route, applies the scheme floor, and reports what the document asked
        // for. That is the whole of the change on this side — dropping the
        // target was the defect, not refusing it, because a seam that cannot
        // open a second context must not pretend to and a seam that CAN must not
        // pretend it cannot.
        Action::Navigate { route, target } => match resolve_text_source(&store, route) {
            None => refused(
                node_id,
                action,
                "the route did not resolve — an unresolved route navigates nowhere",
                store,
            ),
            Some(resolved) => match sanitize_url(&resolved) {
                Some(safe) => emitted(
                    store,
                    ClientEffect::Navigate {
                        route: safe.into_owned(),
                        target: *target,
                    },
                ),
                None => refused(node_id, action, "the route is not a safe URL", store),
            },
        },
        // Phase 1126 — resolution happens at DISPATCH time, so what is copied is
        // what the reader was looking at. Unlike the route above, an
        // unresolvable payload resolves to the EMPTY STRING, as it does at every
        // text slot: the asymmetry is the specification's, and it is why the two
        // arms read differently.
        Action::WriteToClipboard { text } => {
            let resolved = resolve_text_source(&store, text).unwrap_or_default();
            emitted(store, ClientEffect::WriteToClipboard { text: resolved })
        }
        // Phase 1537 — the focus move: a bare node id in THIS document, so
        // there is nothing to resolve and nothing to gate.
        Action::Focus { node_id: target } => emitted(
            store,
            ClientEffect::Focus {
                node_id: target.clone(),
            },
        ),
        // `nodeId` is the node the EVENT came from, which §5.2 now states: the
        // surface holds the selected file against that node, so a reference
        // taken from the action would name something it cannot resolve.
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
        Action::Chain(inner) => {
            inner
                .iter()
                .enumerate()
                .fold(unchanged(store), |acc, (index, next)| {
                    let mut acc = acc;
                    let at = path.map(|p| child_path(p, index));
                    let step = run_at(node_id, at.as_deref(), next, acc.store);
                    acc.store = step.store;
                    acc.effects.extend(step.effects);
                    acc.diagnostics.extend(step.diagnostics);
                    acc
                })
        }

        // ── Documented no-ops ────────────────────────────────────────────────
        //
        // Host-channel and capability arms fan out to machinery this placement
        // does not have; a dispatch carries only an erased payload and there is
        // no update function to fold it through; a local-buffer commit is a host
        // concern whose flushed value arrives as the event payload instead.
        //
        Action::Notify { .. }
        | Action::AiTool { .. }
        | Action::Invoke { .. }
        | Action::Dispatch
        | Action::CommitLocal { .. } => no_op(node_id, action, store),

        // Phase 2106 — `Confirm` is a ROUND TRIP on the bounded path: the
        // gesture asks, and the answer — the originating event re-delivered with
        // `confirmToken` / `confirmAccepted` — runs one continuation
        // ([`run_answer`]). Here is the ask. The question carries the token
        // its answer will name, and what a yes will DO does not go with it.
        //
        // A prompt that resolves to nothing, or to nothing but whitespace, is no
        // question — a yes/no with no subject is worse than no dialogue — so it
        // is refused rather than asked. And a confirm no loop ADDRESSED is
        // declined as it always was: with no token, its question could never be
        // answered, and asking it would tell an author something was pending that
        // nothing will ever run.
        Action::Confirm { prompt, .. } => match path {
            None => no_op(node_id, action, store),
            Some(p) => match resolve_text_source(&store, prompt) {
                Some(text) if !text.trim().is_empty() => emitted(
                    store,
                    ClientEffect::Confirm {
                        prompt: text,
                        token: confirm_token(node_id, p),
                    },
                ),
                Some(_) => refused(
                    node_id,
                    action,
                    "the prompt resolved to no text — nothing was asked",
                    store,
                ),
                None => refused(
                    node_id,
                    action,
                    "the prompt did not resolve to a value — nothing was asked",
                    store,
                ),
            },
        },

        // `Print` has no round trip to model. It is payload-free, it returns
        // nothing, and format version 2 gives it a declared arm — so the whole
        // of performing it is reporting that the program asked, which is
        // exactly what this fold's `effects` are. Emitting it rather than
        // diagnosing it is the completion the codec arm exists for: a no-op
        // would now be this placement declining an instruction it can express,
        // for no reason it could state.
        Action::Print => emitted(store, ClientEffect::Print),

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

// ── The lowering onto the bounded core (WIRE_FORMAT §30) ─────────────────────
//
// The tree wire specification states, for every action arm, the core arm it
// lowers to and the leaf declaration it carries. `run_bounded_action` above is
// this placement's interpreter of exactly that table — a `Chain` folds as a
// sequence, a `SetState` is the one store write, a `Call` is recognised at
// every depth, and every other arm is one leaf — and `lowers_to` states the
// table as data, so the specification's `lowers-to/` vectors can certify it.
// The two are kept honest against each other by the conformance leg, which
// also checks that what the interpreter EMITS for an arm is within what this
// lowering DECLARES for it.

/// A host call a leaf names: the channel it reaches, and the name on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCall {
    pub channel: &'static str,
    pub name: String,
}

/// What a leaf may demand — an upper bound, not a promise to emit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LeafDeclaration {
    /// Client-effect kinds, spelled as the effect registry keys them.
    pub effect_kinds: Vec<&'static str>,
    pub host_calls: Vec<HostCall>,
}

/// The core arm an action lowers to (WIRE_FORMAT §30.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreArm {
    /// The composition arm: the members, each lowered, in order.
    Sequence(Vec<CoreArm>),
    /// The one store write; `from` is whether the value is an expression
    /// resolved at dispatch rather than a literal.
    Assign {
        key: String,
        from: bool,
    },
    /// A call; one declaring its own result target is refused.
    Call {
        endpoint: String,
        declares_target: bool,
    },
    Leaf(LeafDeclaration),
    /// The round trip (Phase 2106): the gesture's leaf, which asks, and the
    /// two arms the ANSWER's selection chooses between — the core's `Choose`
    /// over the reader's answer.
    Ask {
        leaf: LeafDeclaration,
        when_true: Box<CoreArm>,
        when_false: Box<CoreArm>,
    },
}

/// The effect kind a leaf declares, named THROUGH the effect's own
/// discriminator rather than as a literal, so a declared kind and a
/// registered one cannot drift apart.
fn effect_leaf(sample: ClientEffect) -> CoreArm {
    CoreArm::Leaf(LeafDeclaration {
        effect_kinds: vec![sample.capability()],
        host_calls: Vec::new(),
    })
}

fn host_call_leaf(channel: &'static str, name: &str) -> CoreArm {
    CoreArm::Leaf(LeafDeclaration {
        effect_kinds: Vec::new(),
        host_calls: vec![HostCall {
            channel,
            name: name.to_string(),
        }],
    })
}

/// Lower one action onto the bounded core — the WIRE_FORMAT §30.1 table. No
/// wildcard: an arm added to the vocabulary fails to compile here until the
/// table has its row.
pub fn lowers_to(action: &Action) -> CoreArm {
    match action {
        Action::Chain(inner) => CoreArm::Sequence(inner.iter().map(lowers_to).collect()),
        Action::SetState {
            key, value_from, ..
        } => CoreArm::Assign {
            key: key.clone(),
            from: value_from.is_some(),
        },
        Action::Call { endpoint, into, .. } => CoreArm::Call {
            endpoint: endpoint.clone(),
            declares_target: into.is_some(),
        },
        Action::Navigate { .. } => effect_leaf(ClientEffect::Navigate {
            route: String::new(),
            target: NavigateTarget::Self_,
        }),
        Action::Focus { .. } => effect_leaf(ClientEffect::Focus {
            node_id: String::new(),
        }),
        Action::WriteToClipboard { .. } => effect_leaf(ClientEffect::WriteToClipboard {
            text: String::new(),
        }),
        Action::ReadFileBody { .. } => effect_leaf(ClientEffect::ReadFileBody {
            node_id: String::new(),
            encoding: String::new(),
        }),
        Action::Print => effect_leaf(ClientEffect::Print),
        Action::Invoke { capability_id, .. } => host_call_leaf("Invoke", capability_id),
        Action::Notify { channel, .. } => host_call_leaf("Notify", channel),
        Action::AiTool { tool_name, .. } => host_call_leaf("AiTool", tool_name),
        // Phase 2106 — the one round-trip arm. The gesture lowers to the leaf
        // that asks; the ANSWER lowers to a selection over the reader's answer,
        // `onConfirm` the true arm and `onCancel` — or the empty sequence,
        // which is what "nothing happens" is — the false.
        Action::Confirm {
            on_confirm,
            on_cancel,
            ..
        } => CoreArm::Ask {
            leaf: LeafDeclaration {
                effect_kinds: vec![
                    ClientEffect::Confirm {
                        prompt: String::new(),
                        token: String::new(),
                    }
                    .capability(),
                ],
                host_calls: Vec::new(),
            },
            when_true: Box::new(lowers_to(on_confirm)),
            when_false: Box::new(
                on_cancel
                    .as_deref()
                    .map(lowers_to)
                    .unwrap_or(CoreArm::Sequence(Vec::new())),
            ),
        },
        Action::Dispatch | Action::CommitLocal { .. } => CoreArm::Leaf(LeafDeclaration::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::NavigateTarget;

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
                route: TextSource::Literal("javascript:alert(1)".to_string()),
                target: NavigateTarget::Self_,
            },
            BindingSources::default(),
        );
        assert!(outcome.effects.is_empty(), "no effect is shipped");
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [BoundedDiagnostic::Refused { .. }]
        ));
    }

    /// Phase 1664 — the target reaches the effect, and the scheme floor still
    /// runs on the way. Both halves in one test: a `Blank` target that only
    /// stopped being refused is worth nothing if it stopped being sanitised too.
    #[test]
    fn a_blank_target_ships_the_effect_carrying_it_and_still_meets_the_scheme_floor() {
        let outcome = run_bounded_action(
            "n",
            &Action::Navigate {
                route: TextSource::Literal("https://docs.example/orders".to_string()),
                target: NavigateTarget::Blank,
            },
            BindingSources::default(),
        );
        assert_eq!(
            outcome.effects,
            vec![ClientEffect::Navigate {
                route: "https://docs.example/orders".into(),
                target: NavigateTarget::Blank,
            }]
        );
        assert!(outcome.diagnostics.is_empty(), "nothing was refused");

        let unsafe_blank = run_bounded_action(
            "n",
            &Action::Navigate {
                route: TextSource::Literal("javascript:alert(1)".to_string()),
                target: NavigateTarget::Blank,
            },
            BindingSources::default(),
        );
        assert!(unsafe_blank.effects.is_empty(), "the floor still refuses");
    }

    /// Phase 1126's clipboard widening, on the DISPATCH side. The corpus's
    /// `btn-copy-bound` fixture certifies that a bound payload decodes and
    /// re-encodes; it says nothing about what is copied, which is the half a
    /// reader pastes with authority. So: the store is read when the reader
    /// raises the action, and an unresolvable payload is the EMPTY STRING —
    /// never the binding's own JSON, and never the loud i18n sentinel.
    #[test]
    fn a_bound_clipboard_payload_resolves_at_dispatch_and_degrades_to_empty() {
        let bound = |key: &str| Action::WriteToClipboard {
            text: TextSource::Bound(Box::new(crate::wire::Binding::State {
                default_declared: false,
                key: key.into(),
                default_value: StaticValue::StringOpt(None),
            })),
        };

        let outcome =
            run_bounded_action("n", &bound("shareUrl"), store_with("shareUrl", "/o/4417"));
        assert_eq!(
            outcome.effects,
            vec![ClientEffect::WriteToClipboard {
                text: "/o/4417".into()
            }]
        );

        let unresolved = run_bounded_action("n", &bound("absent"), BindingSources::default());
        assert_eq!(
            unresolved.effects,
            vec![ClientEffect::WriteToClipboard {
                text: String::new()
            }]
        );
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
                default_declared: false,
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
