//! The bounded action interpreter for the UI tier — one resolved action, one store, one
//! outcome.
//!
//! ## The fold is the generic core's
//!
//! There is no evaluating match over `Action` in this file. A fold of an action is the generic
//! Program core's fold (`fuaran_program::fold`), run over the UI witness ([`super::witness`]): a
//! `Chain` is the core's sequence, a `SetState` its one store write, a `Call` its call — recognised
//! at every depth — and every other arm a leaf. What this file holds is what is the UI vocabulary's
//! own: what each LEAF does at dispatch ([`lower_leaf`]), the bounded flush a commit writes through,
//! the confirm round trip's addressing, and the lowering table the specification's `lowers-to/`
//! vectors certify.
//!
//! ## The safety property, stated and tested
//!
//! A program tree that arrived over the wire is **data, not code**. The wire format cannot carry a
//! closure, so the decoder erases every closure slot; the core's fold reads an action only through
//! the witness's view, lowering and description, so there is nothing in the decoded model for it to
//! invoke. The only store mutation is the state write; the only outward reach is the closed
//! client-effect vocabulary.
//!
//! ## Documented no-ops, never silent ones
//!
//! Several arms have no form on this path. Each is a **documented no-op that emits a readable
//! diagnostic**, not a silent nothing: a program that intended one of them is observable to whoever
//! is debugging its emission.
//!
//! ## Where a handler-running placement attaches
//!
//! A call is recognised by the core's fold at every depth, and what it MEANS is the placement's,
//! through the core's handler arm. This host implements the client placement, which registers no
//! handlers, so it folds with the core's inert arm and a call is the documented no-op it has always
//! been.

use std::collections::BTreeMap;

use fuaran_program::fold;
use fuaran_program::witness::LeafOutcome;

use crate::canonical::{JVal, parse};
use crate::render::BindingSources;
use crate::render::bindings::{Value, try_string};
use crate::render::sanitize::sanitize_url;
use crate::wire::{
    Action, Binding, FileReadEncoding, FormField, FormFieldKind, Format, NavigateTarget, Node,
    NodeKind, StaticValue, TextSource,
};

use super::effect::ClientEffect;
use super::resolve::structural_children;
use super::validate::LiveValue;
use super::witness::{UiAction, UiFold};

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

/// A readable "this did nothing, on purpose" signal — the generic core's diagnostic, under the
/// name this crate has always published it by. The two arms mean opposite things to whoever is
/// debugging an emission — *this path does not implement that* versus *that was not allowed* —
/// and both name the action's **discriminator** only: every string inside an action came off an
/// untrusted wire.
pub use fuaran_program::fold::Diagnostic as BoundedDiagnostic;

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
    /// Phase 2107 — the gesture stopped part-way: a `Require` that did not hold,
    /// a `Choose` whose condition did not resolve, or an exit assertion violated.
    /// Nothing after the halt ran; what ran before it is in `store` and `effects`.
    pub halted: bool,
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
        Action::Choose { .. } => "Choose",
        Action::Require { .. } => "Require",
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

/// The state-slot form of a resolved value. The collection payloads have no
/// state-slot form on this wire, and are reported rather than coerced: writing
/// some flattened stand-in would be the interpreter inventing a value nobody
/// asked for.
pub(crate) fn to_state_value(value: &Value<'_>) -> Option<JVal> {
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
/// reaches. A `CommitLocal` reached here has no tree to find its field in, so it
/// writes nothing; a loop flushes it through [`Flush`].
pub fn run_bounded_action(node_id: &str, action: &Action, store: BindingSources) -> BoundedOutcome {
    run_at(node_id, UiAction::unaddressed(action), None, store)
}

/// Interpret a **gesture** — the action an admitted, non-answer event resolves
/// to — with every confirm it reaches addressed, so each asks its question
/// carrying the token its answer will name (Phase 2106), and every commit it
/// reaches flushed from `flush` when a loop supplies one (Phase 2198).
pub fn run_gesture(
    node_id: &str,
    action: &Action,
    store: BindingSources,
    flush: Option<Flush<'_>>,
) -> BoundedOutcome {
    run_at(node_id, UiAction::at(action, ""), flush, store)
}

/// The one fold: the generic core's, over the UI witness, at the client placement — which answers
/// no call. Since Phase 2107 the vocabulary has a guard (`Require`) and a selection (`Choose`), so
/// the core's halt flag is carried on the outcome.
fn run_at(
    node_id: &str,
    at: UiAction,
    flush: Option<Flush<'_>>,
    store: BindingSources,
) -> BoundedOutcome {
    let outcome = fold::run_inert(&UiFold { flush }, node_id, &at, store);
    BoundedOutcome {
        store: outcome.store,
        effects: outcome.effects,
        diagnostics: outcome.diagnostics,
        halted: outcome.halted,
    }
}

// ── The bounded flush (WIRE_FORMAT §30.1, Phase 2198) ────────────────────────
//
// `CommitLocal` names a FORM FIELD, and the key it writes is that field's
// `Local` binding's `commit_to`: a fact of the TREE, which the action alone
// does not carry. So a loop flushes a commit where the tree is in view. The key
// is found in the fixed base tree, and the value is the event's flush payload
// member named by the field's id. The commit then writes exactly as a
// `SetState` with a literal value does, reserved namespace included. A commit
// the tree cannot resolve writes nothing: no form field has its id, or the
// field's value is not a `Local` declaring `commit_to`.

/// What a loop hands the fold so a commit can flush: the fixed tree its field
/// is found in, and the admitted event's payload its value is read from.
#[derive(Debug, Clone, Copy)]
pub struct Flush<'a> {
    pub tree: &'a Node,
    pub payload: &'a BTreeMap<String, LiveValue>,
}

/// Where a commit writes. The key is the `commit_to` of the `Local` binding on
/// the form field it names, the first in document order (a `Form`'s own fields
/// before its children). The flag says whether that buffer declares the
/// `Number` codec, which admits only a number. `None` when the commit writes
/// nothing.
pub fn commit_destination<'a>(tree: &'a Node, field_id: &str) -> Option<(&'a str, bool)> {
    match field_value(&form_field(tree, field_id)?.kind) {
        Binding::Local {
            commit_to: Some(key),
            codec,
            ..
        } => Some((key.as_str(), matches!(codec, Some(Format::Number { .. })))),
        _ => None,
    }
}

/// The value binding every form-field kind carries. Exhaustive with no
/// wildcard, so a kind added to the vocabulary is a build error here until it
/// says where its value is.
fn field_value(kind: &FormFieldKind) -> &Binding {
    match kind {
        FormFieldKind::Text { value, .. }
        | FormFieldKind::Number { value, .. }
        | FormFieldKind::Checkbox { value, .. }
        | FormFieldKind::Toggle { value, .. }
        | FormFieldKind::Choice { value, .. }
        | FormFieldKind::Combobox { value, .. }
        | FormFieldKind::RangedNumber { value, .. }
        | FormFieldKind::Range { value, .. }
        | FormFieldKind::SegmentedChoice { value, .. }
        | FormFieldKind::TextArea { value, .. }
        | FormFieldKind::DateTime { value, .. }
        | FormFieldKind::DateTimeRange { value, .. }
        | FormFieldKind::Tokens { value, .. }
        | FormFieldKind::Rating { value, .. }
        | FormFieldKind::Color { value, .. } => value,
    }
}

fn form_field<'a>(node: &'a Node, field_id: &str) -> Option<&'a FormField> {
    if let NodeKind::Form(spec) = &node.kind {
        if let Some(field) = spec.fields.iter().find(|f| f.id == field_id) {
            return Some(field);
        }
    }
    structural_children(&node.kind)
        .into_iter()
        .find_map(|child| form_field(child, field_id))
}

/// The value a commit writes, read from the event's flush payload: written as
/// it arrives, except that a buffer with the `Number` codec writes only a
/// number, and reads a string under the JSON number grammar. An absent or
/// `null` member writes nothing. Every refusal text is this host's own; no
/// payload string is echoed.
pub(crate) fn flushed_value(numeric: bool, value: Option<&LiveValue>) -> Result<JVal, String> {
    let not_a_number = || {
        "the committed value is not a number, and the field's codec takes only numbers — no write performed"
            .to_string()
    };
    match value {
        None | Some(LiveValue::Null) => Err(
            "the event carried no value for the committed field — no write performed".to_string(),
        ),
        Some(LiveValue::Num(n)) => Ok(JVal::Num(*n)),
        Some(LiveValue::Str(text)) if numeric => {
            match parse(text.trim_matches([' ', '\t', '\n', '\r'])) {
                Ok(JVal::Num(n)) => Ok(JVal::Num(n)),
                _ => Err(not_a_number()),
            }
        }
        Some(LiveValue::Bool(_)) if numeric => Err(not_a_number()),
        Some(LiveValue::Str(text)) => Ok(JVal::Str(text.clone())),
        Some(LiveValue::Bool(b)) => Ok(JVal::Bool(*b)),
    }
}

/// The writes a fold of `action` flushes, each as the `SetState` it is, so a
/// loop can put each one to its dispatch gate on its own before anything
/// folds. Walks what a fold reaches: a chain's members, and nothing beneath a
/// question, whose continuations an answer reaches on its own branch. A walk
/// over the closed vocabulary that performs nothing, as the budget's is.
pub fn commit_writes(action: &Action, flush: Flush<'_>) -> Vec<Action> {
    match action {
        Action::Chain(inner) => inner.iter().flat_map(|a| commit_writes(a, flush)).collect(),
        // Phase 2107 — whichever arm the condition picks at dispatch may run, so
        // both arms' writes are put to the gate (the conservative reading).
        Action::Choose {
            when_true,
            when_false,
            ..
        } => {
            let mut writes = commit_writes(when_true, flush);
            if let Some(when_false) = when_false {
                writes.extend(commit_writes(when_false, flush));
            }
            writes
        }
        Action::CommitLocal { node_id: field_id } => commit_destination(flush.tree, field_id)
            .and_then(|(key, numeric)| {
                flushed_value(numeric, flush.payload.get(field_id))
                    .ok()
                    .map(|value| Action::SetState {
                        key: key.to_string(),
                        value: Some(value),
                        value_from: None,
                    })
            })
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// The token a question carries: the originating node and the confirm's
/// structural path inside that node's action — dot-joined chain positions and
/// continuation names, the empty path naming the action itself. The node is in
/// it so a token minted for one node cannot address another's action.
pub fn confirm_token(node_id: &str, path: &str) -> String {
    format!("{node_id}#{path}")
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
                // Phase 2107 — a selection's arms, named as continuations are.
                ("whenTrue", Action::Choose { when_true, .. }) => when_true,
                (
                    "whenFalse",
                    Action::Choose {
                        when_false: Some(when_false),
                        ..
                    },
                ) => when_false,
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

pub(crate) fn branch_path(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
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
    flush: Option<Flush<'_>>,
) -> BoundedOutcome {
    let name = if accepted { "onConfirm" } else { "onCancel" };
    match answer_branch(confirm, accepted) {
        Some(branch) => run_at(
            node_id,
            UiAction::at(branch, &branch_path(path, name)),
            flush,
            store,
        ),
        None => BoundedOutcome {
            store,
            effects: Vec::new(),
            diagnostics: Vec::new(),
            halted: false,
        },
    }
}

/// What a LEAF does at dispatch — every arm the UI witness views as a leaf, lowered to at most one
/// effect, a refusal with its reason, or a documented decline. `path` is `Some` when the loop
/// addressed this position, so a confirm there asks. Exhaustive with no wildcard, so an arm added
/// to the vocabulary is a build error here until somebody says what it does.
pub(crate) fn lower_leaf(
    node_id: &str,
    path: Option<&str>,
    action: &Action,
    store: &BindingSources,
) -> LeafOutcome<ClientEffect> {
    match action {
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
        // Phase 1664 — the target is CARRIED: the client-effect envelope names
        // the browsing context (an optional member, omitted at `Self`), so the
        // interpreter resolves the route, applies the scheme floor, and reports
        // what the document asked for.
        Action::Navigate { route, target } => match resolve_text_source(store, route) {
            None => LeafOutcome::Refuse(
                "the route did not resolve — an unresolved route navigates nowhere".to_string(),
            ),
            Some(resolved) => match sanitize_url(&resolved) {
                Some(safe) => LeafOutcome::Emit(ClientEffect::Navigate {
                    route: safe.into_owned(),
                    target: *target,
                }),
                None => LeafOutcome::Refuse("the route is not a safe URL".to_string()),
            },
        },
        // Phase 1126 — resolution happens at DISPATCH time, so what is copied is
        // what the reader was looking at. Unlike the route above, an
        // unresolvable payload resolves to the EMPTY STRING, as it does at every
        // text slot: the asymmetry is the specification's, and it is why the two
        // arms read differently.
        Action::WriteToClipboard { text } => LeafOutcome::Emit(ClientEffect::WriteToClipboard {
            text: resolve_text_source(store, text).unwrap_or_default(),
        }),
        // Phase 1537 — the focus move: a bare node id in THIS document, so
        // there is nothing to resolve and nothing to gate.
        Action::Focus { node_id: target } => LeafOutcome::Emit(ClientEffect::Focus {
            node_id: target.clone(),
        }),
        // `nodeId` is the node the EVENT came from, which §5.2 now states: the
        // surface holds the selected file against that node, so a reference
        // taken from the action would name something it cannot resolve.
        Action::ReadFileBody { encoding, .. } => LeafOutcome::Emit(ClientEffect::ReadFileBody {
            node_id: node_id.to_string(),
            encoding: file_read_encoding(*encoding).to_string(),
        }),

        // ── Documented no-ops ────────────────────────────────────────────────
        //
        // Host-channel and capability arms fan out to machinery this placement
        // does not have; a dispatch carries only an erased payload and there is
        // no update function to fold it through. A commit reaches here only when
        // it cannot write (the witness views a writable one as the store write).
        Action::Notify { .. }
        | Action::AiTool { .. }
        | Action::Invoke { .. }
        | Action::Dispatch
        | Action::CommitLocal { .. } => LeafOutcome::Decline,

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
            None => LeafOutcome::Decline,
            Some(p) => match resolve_text_source(store, prompt) {
                Some(text) if !text.trim().is_empty() => LeafOutcome::Emit(ClientEffect::Confirm {
                    prompt: text,
                    token: confirm_token(node_id, p),
                }),
                Some(_) => LeafOutcome::Refuse(
                    "the prompt resolved to no text — nothing was asked".to_string(),
                ),
                None => LeafOutcome::Refuse(
                    "the prompt did not resolve to a value — nothing was asked".to_string(),
                ),
            },
        },

        // `Print` has no round trip to model. It is payload-free, it returns
        // nothing, and format version 2 gives it a declared arm — so the whole
        // of performing it is reporting that the program asked, which is
        // exactly what this fold's `effects` are.
        Action::Print => LeafOutcome::Emit(ClientEffect::Print),

        // Never views as a leaf: the core folds these arms itself. Declining is the answer that
        // could not do harm if the obligation ever broke.
        Action::Chain(_)
        | Action::SetState { .. }
        | Action::Call { .. }
        | Action::Choose { .. }
        | Action::Require { .. } => LeafOutcome::Decline,
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

/// A leaf that is an escape no walk can see into (Phase 2194): the reason
/// class that makes it so, and the act's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueLeaf {
    pub reason: &'static str,
    pub name: &'static str,
}

/// What a leaf may demand — an upper bound, not a promise to emit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LeafDeclaration {
    /// Client-effect kinds, spelled as the effect registry keys them.
    pub effect_kinds: Vec<&'static str>,
    pub host_calls: Vec<HostCall>,
    /// Set when the leaf is an escape. `None` claims the two lists above are
    /// the whole of what the leaf can do; `Some` says they are not, so "does
    /// nothing" and "cannot be analysed" never read alike.
    pub opaque: Option<OpaqueLeaf>,
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
    /// Phase 2107 — the core's selection: the two arms, each lowered. The
    /// condition and any exit assertion resolve at dispatch.
    Choose {
        when_true: Box<CoreArm>,
        when_false: Box<CoreArm>,
    },
    /// Phase 2107 — the core's halting guard; its condition resolves at dispatch.
    Require,
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
        ..LeafDeclaration::default()
    })
}

fn host_call_leaf(channel: &'static str, name: &str) -> CoreArm {
    CoreArm::Leaf(LeafDeclaration {
        host_calls: vec![HostCall {
            channel,
            name: name.to_string(),
        }],
        ..LeafDeclaration::default()
    })
}

/// Lower one action onto the bounded core — the WIRE_FORMAT §30.1 table — with
/// no tree in view: a `CommitLocal` has no field to find, so it is the leaf that
/// writes nothing. A loop lowers in its tree through [`lowers_to_in`]. No
/// wildcard: an arm added to the vocabulary fails to compile here until the
/// table has its row.
pub fn lowers_to(action: &Action) -> CoreArm {
    lower(action, None)
}

/// Lower one action in the tree it sits in (Phase 2198): a `CommitLocal` names
/// the form field whose `commit_to` it writes, which only the tree holds.
pub fn lowers_to_in(tree: &Node, action: &Action) -> CoreArm {
    lower(action, Some(tree))
}

fn lower(action: &Action, tree: Option<&Node>) -> CoreArm {
    match action {
        Action::Chain(inner) => CoreArm::Sequence(inner.iter().map(|a| lower(a, tree)).collect()),
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
                ..LeafDeclaration::default()
            },
            when_true: Box::new(lower(on_confirm, tree)),
            when_false: Box::new(
                on_cancel
                    .as_deref()
                    .map(|a| lower(a, tree))
                    .unwrap_or(CoreArm::Sequence(Vec::new())),
            ),
        },
        // Phase 2107 — the core's own arms, identity rows of §30.1. An absent
        // `whenFalse` is the empty sequence, which is what "nothing happens" is.
        Action::Choose {
            when_true,
            when_false,
            ..
        } => CoreArm::Choose {
            when_true: Box::new(lower(when_true, tree)),
            when_false: Box::new(
                when_false
                    .as_deref()
                    .map(|a| lower(a, tree))
                    .unwrap_or(CoreArm::Sequence(Vec::new())),
            ),
        },
        Action::Require { .. } => CoreArm::Require,
        // Phase 2194 — a message for the host's own update, which no walk can
        // see into: an OPAQUE leaf, reason class `in-process`.
        Action::Dispatch => CoreArm::Leaf(LeafDeclaration {
            opaque: Some(OpaqueLeaf {
                reason: "in-process",
                name: "Dispatch",
            }),
            ..LeafDeclaration::default()
        }),
        // Phase 2198 — the one arm whose lowering reads the tree: the store
        // write keyed by its field's `commit_to`, its value a literal the loop
        // reads from the event. With no tree, or no destination, it writes
        // nothing, and is the leaf that says so.
        Action::CommitLocal { node_id: field_id } => {
            match tree.and_then(|t| commit_destination(t, field_id)) {
                Some((key, _)) => CoreArm::Assign {
                    key: key.to_string(),
                    from: false,
                },
                None => CoreArm::Leaf(LeafDeclaration::default()),
            }
        }
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

    // ── Phase 2107 — the core's selection and guard on the UI loop ──────────

    fn state(key: &str) -> crate::wire::Binding {
        crate::wire::Binding::State {
            default_declared: false,
            key: key.into(),
            default_value: StaticValue::StringOpt(None),
        }
    }

    fn flag(key: &str, value: bool) -> BindingSources {
        let mut sources = BindingSources::default();
        sources.state.insert(key.to_string(), JVal::Bool(value));
        sources
    }

    fn choose(when_false: Option<Action>, exit: Option<&str>) -> Action {
        Action::Choose {
            condition: state("flag"),
            when_true: Box::new(set("out", "yes")),
            when_false: when_false.map(Box::new),
            exit: exit.map(state),
        }
    }

    #[test]
    fn choose_runs_the_true_arm_on_the_boolean_true_and_the_false_arm_otherwise() {
        let yes = run_bounded_action(
            "n",
            &choose(Some(set("out", "no")), None),
            flag("flag", true),
        );
        assert_eq!(yes.store.state.get("out"), Some(&JVal::Str("yes".into())));
        assert!(!yes.halted);
        let no = run_bounded_action(
            "n",
            &choose(Some(set("out", "no")), None),
            store_with("flag", "true"),
        );
        assert_eq!(
            no.store.state.get("out"),
            Some(&JVal::Str("no".into())),
            "a string is not the boolean"
        );
        let nothing = run_bounded_action("n", &choose(None, None), flag("flag", false));
        assert_eq!(
            nothing.store.state.get("out"),
            None,
            "an absent whenFalse does nothing"
        );
        assert!(!nothing.halted);
    }

    #[test]
    fn choose_reads_its_condition_at_dispatch_and_halts_when_it_does_not_resolve() {
        let action = Action::Chain(vec![
            Action::SetState {
                key: "flag".into(),
                value: Some(JVal::Bool(true)),
                value_from: None,
            },
            choose(Some(set("out", "no")), None),
        ]);
        let out = run_bounded_action("n", &action, flag("flag", false));
        assert_eq!(out.store.state.get("out"), Some(&JVal::Str("yes".into())));
        let unresolved = run_bounded_action("n", &choose(None, None), BindingSources::default());
        assert!(unresolved.halted, "an unresolved entry halts");
        assert_eq!(unresolved.store.state.get("out"), None);
    }

    #[test]
    fn require_halts_the_gesture_keeping_what_ran_before_it() {
        let action = Action::Chain(vec![
            set("before", "1"),
            Action::Require {
                condition: state("ok"),
            },
            set("after", "2"),
            Action::Print,
        ]);
        let held = run_bounded_action("n", &action, flag("ok", true));
        assert!(!held.halted);
        assert!(held.store.state.contains_key("after"));
        let halted = run_bounded_action("n", &action, flag("ok", false));
        assert!(halted.halted);
        assert!(halted.store.state.contains_key("before"));
        assert!(!halted.store.state.contains_key("after"));
        assert!(
            halted.effects.is_empty(),
            "the effect after the guard never ran"
        );
    }

    #[test]
    fn a_violated_exit_assertion_halts_after_the_arm() {
        // The true arm writes "out" but the exit names "flag", which stays true:
        // it must hold after the true arm, so this one holds.
        let held = run_bounded_action("n", &choose(None, Some("flag")), flag("flag", true));
        assert!(!held.halted);
        // After the false arm (empty) the exit must NOT hold; "flag" is false, so it holds the rule.
        let held_false = run_bounded_action("n", &choose(None, Some("flag")), flag("flag", false));
        assert!(!held_false.halted);
        // An exit that resolves to true after the false arm is a violation.
        let mut sources = flag("flag", false);
        sources.state.insert("other".into(), JVal::Bool(true));
        let violated = run_bounded_action(
            "n",
            &Action::Choose {
                condition: state("flag"),
                when_true: Box::new(set("out", "yes")),
                when_false: None,
                exit: Some(state("other")),
            },
            sources,
        );
        assert!(violated.halted, "the exit held after the false arm");
    }

    #[test]
    fn choose_and_require_lower_to_the_cores_own_arms() {
        assert_eq!(
            lowers_to(&choose(None, None)),
            CoreArm::Choose {
                when_true: Box::new(CoreArm::Assign {
                    key: "out".into(),
                    from: false
                }),
                when_false: Box::new(CoreArm::Sequence(Vec::new())),
            }
        );
        assert_eq!(
            lowers_to(&Action::Require {
                condition: state("ok")
            }),
            CoreArm::Require
        );
    }

    #[test]
    fn a_confirm_inside_a_choose_arm_is_addressed_under_the_arms_name() {
        let action = Action::Choose {
            condition: state("flag"),
            when_true: Box::new(Action::Confirm {
                prompt: TextSource::Literal("Sure?".into()),
                on_confirm: Box::new(Action::Print),
                on_cancel: None,
            }),
            when_false: None,
            exit: None,
        };
        let found = addressed_confirm("n", "n#whenTrue", &action);
        assert_eq!(found.map(|(p, _)| p), Some("whenTrue".to_string()));
        assert!(addressed_confirm("n", "n#whenFalse", &action).is_none());
    }
}
