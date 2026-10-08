//! The client placement of the bounded program loop.
//!
//! Validate, interpret, re-resolve — and hand the resolved tree back to the host
//! to render. No hand-authored update function, no message type, no server: the
//! "model" is the tree's own store, and the "update" is the bounded interpreter
//! applied to it.
//!
//! The interpreter and the re-resolution pass are not reimplemented here. They
//! are the ones beside this file, which is what makes "one algebra, many
//! placements" a property of the code rather than a claim in a document.
//!
//! ## What this loop does not depend on
//!
//! Not a renderer, not a transport, not a clock, not a filesystem. A host reads
//! the resolved tree off each step and renders it however it renders things, and
//! performs (or declines) the effects the step reached. That is what lets the
//! identical loop run in a browser under `wasm32`, in a native service, and
//! under a headless test with nothing stubbed out.
//!
//! ## Two refusals, and they are not the same refusal
//!
//! A step can be refused before it folds — by the trust boundary or by the
//! resource budget — and that is what [`StepOutput::rejected`] reports: nothing
//! happened, and the store is exactly what it was. An **effect** can separately
//! be declined by the host's policy, and that is what [`StepOutput::denials`]
//! reports. The second does not touch [`StepOutput::effects`], deliberately: a
//! conformance claim over the effect vocabulary fixes *which arm a step reached
//! with which values*, and leaves what a host then does with it host-defined. A
//! host that declines every effect folds identically to one that performs them
//! all — which is exactly why a declined effect must still be *reported*, and
//! must never be dropped in silence.

use crate::render::BindingSources;
use crate::wire::{Action, Node};

use super::actions::{
    BoundedDiagnostic, addressed_confirm, answer_branch, describe_action, run_answer, run_gesture,
};
use super::budget::{InteractionBudget, action_cascade_cost, tree_cost};
use super::effect::{ClientEffect, Denial, EffectPolicy};
use super::resolve::resolve_tree;
use super::validate::{LiveEvent, LiveValue, RejectReason, validate};

/// The two payload members a confirm answer carries (Phase 2106). An event is an
/// answer when it carries both, `confirmToken` a string and `confirmAccepted`
/// a boolean; anything less is the gesture it otherwise is.
pub const CONFIRM_TOKEN_KEY: &str = "confirmToken";
/// See [`CONFIRM_TOKEN_KEY`].
pub const CONFIRM_ACCEPTED_KEY: &str = "confirmAccepted";

fn answer_of(event: &LiveEvent) -> Option<(&str, bool)> {
    match (
        event.payload.get(CONFIRM_TOKEN_KEY),
        event.payload.get(CONFIRM_ACCEPTED_KEY),
    ) {
        (Some(LiveValue::Str(token)), Some(LiveValue::Bool(accepted))) => Some((token, *accepted)),
        _ => None,
    }
}

fn addresses_nothing(node_id: &str) -> RejectReason {
    RejectReason::PayloadOutOfBounds {
        node_id: node_id.to_string(),
        detail: "the confirm answer addresses no Confirm in this node's action".to_string(),
    }
}

/// Why a step produced no new tree. Either way the store is unchanged.
#[derive(Debug, Clone, PartialEq)]
pub enum ProgramReject {
    /// The trust boundary refused the event.
    Gate(RejectReason),
    /// The per-interaction resource budget was exceeded.
    BudgetExceeded { detail: String },
}

/// The observable result of one step.
#[derive(Debug, Clone)]
pub struct StepOutput {
    /// The resolved tree after this step — unchanged on a refusal.
    pub resolved: Node,
    /// The client effects this step **reached**, in order. Reported rather than
    /// performed: performing is the host's, and is host-defined.
    pub effects: Vec<ClientEffect>,
    /// The effects this host's policy declined, each naming the capability it
    /// declined and nothing else.
    pub denials: Vec<Denial>,
    /// Set when the step was refused before it folded.
    pub rejected: Option<ProgramReject>,
    /// The interpreter's documented-no-op and refusal signals.
    pub diagnostics: Vec<BoundedDiagnostic>,
}

/// A running bounded program: the **fixed** tree it started from, the store, the
/// current resolved tree, the tree's cached cost, and the host's policies.
pub struct BoundedProgram {
    base_tree: Node,
    store: BindingSources,
    resolved: Node,
    node_cost: u64,
    budget: InteractionBudget,
    effects: EffectPolicy,
    can_dispatch: Box<dyn Fn(&Action) -> bool>,
    /// The questions asked and not yet answered or withdrawn (Phase 2106) — the
    /// loop's own state, never in the store, so no binding can read one and no
    /// tree can write one.
    pending: Vec<String>,
}

impl BoundedProgram {
    /// Build a program over a decoded tree.
    ///
    /// **Default-deny throughout**: no effect registered, no dispatch permitted,
    /// and the conservative budget. This loop exists to run programs a host did
    /// not write, so the permissive constructions are named opt-ins rather than
    /// the default.
    ///
    /// The tree is **priced before it is resolved**, so a tree already over
    /// budget is not walked a second time by the very construction meant to
    /// refuse it. An over-budget program is returned rather than refused — the
    /// signature is total — carrying its unresolved tree, and the first event
    /// refuses.
    pub fn new(tree: Node) -> Self {
        let mut program = BoundedProgram {
            base_tree: tree.clone(),
            store: BindingSources::default(),
            resolved: tree,
            node_cost: 0,
            budget: InteractionBudget::default(),
            effects: EffectPolicy::default(),
            can_dispatch: Box::new(|_| false),
            pending: Vec::new(),
        };
        program.reprice();
        program
    }

    /// Seed the store (builder-style) and re-resolve against it.
    pub fn with_store(mut self, store: BindingSources) -> Self {
        self.store = store;
        self.reprice();
        self
    }

    /// Set the per-interaction resource caps (builder-style).
    pub fn with_budget(mut self, budget: InteractionBudget) -> Self {
        self.budget = budget;
        self.reprice();
        self
    }

    /// Set the client-effect policy (builder-style).
    pub fn with_effect_policy(mut self, effects: EffectPolicy) -> Self {
        self.effects = effects;
        self
    }

    /// Set the dispatch policy gate (builder-style). **The named opt-in away
    /// from deny.**
    pub fn with_dispatch_gate(mut self, gate: impl Fn(&Action) -> bool + 'static) -> Self {
        self.can_dispatch = Box::new(gate);
        self
    }

    fn reprice(&mut self) {
        self.node_cost = tree_cost(self.budget.max_nodes, &self.base_tree);
        self.resolved = if self.node_cost > self.budget.max_nodes {
            self.base_tree.clone()
        } else {
            // §24.4 — the tree's own `Binding.State` declarations seed their
            // slots before any binding resolves. Laid UNDER the store, so a
            // host value and every `SetState` write already folded into it win.
            // Derived here rather than folded into `store` so a seed never
            // outlives the declaration that produced it.
            let seeded = crate::render::with_state_seeds(&self.base_tree, &self.store);
            resolve_tree(&seeded, &self.base_tree)
        };
    }

    /// The current resolved tree — what a host renders.
    pub fn resolved(&self) -> &Node {
        &self.resolved
    }

    /// The current store.
    pub fn store(&self) -> &BindingSources {
        &self.store
    }

    fn refuse(&self, reject: ProgramReject) -> StepOutput {
        StepOutput {
            resolved: self.resolved.clone(),
            effects: Vec::new(),
            denials: Vec::new(),
            rejected: Some(reject),
            diagnostics: Vec::new(),
        }
    }

    /// Step the program with one untrusted inbound event: validate, price,
    /// interpret against the store, re-resolve the **fixed** base tree, and
    /// report what the step reached.
    ///
    /// Re-resolving the base tree rather than the previous step's output is what
    /// keeps the store the only thing carrying state: fold a step's
    /// substitutions into the next step's input and a binding an earlier step
    /// happened to resolve could never be recovered. Specified — §10.5, pinned
    /// by the corpus's `fixed-base-reresolution` scenario.
    ///
    /// **The confirm round trip (Phase 2106).** A gesture folds with every
    /// confirm it reaches addressed, so each asks a question whose token names
    /// the confirm's path in the node's action, and it WITHDRAWS every question
    /// still pending. An answer — the originating event re-delivered with
    /// `confirmToken` / `confirmAccepted` — is admitted only when its token is
    /// pending and still addresses a confirm in the node's action; its
    /// continuation meets the dispatch gate on its own; it then runs as a
    /// selection over the answer and consumes its question. A stale, duplicate or
    /// forged answer is refused as an event, and a refusal leaves the pending
    /// questions as they were. A confirmation is a courtesy to the reader, never
    /// an authorisation.
    ///
    /// The refusal reported here is **event-level** and nothing else (§10.5): a
    /// step is refused when the trust boundary or a budget declined the event
    /// itself. An action that declines inside an admitted event — a reserved
    /// state key, an unsafe destination — leaves `rejected` unset and shows as
    /// an *absent effect*.
    pub fn handle_event(&mut self, event: &LiveEvent) -> StepOutput {
        let action = match validate(&*self.can_dispatch, &self.resolved, event) {
            Err(reason) => return self.refuse(ProgramReject::Gate(reason)),
            // A legitimate event that resolves to no action: nothing to do, and
            // deliberately NOT a refusal. Reporting it as one would tell a host
            // its surface was rejected when it was merely inert.
            Ok(validated) => match validated.action {
                None => {
                    // Not an answer — an answer here addresses nothing — so it
                    // withdraws every pending question.
                    if answer_of(event).is_some() {
                        return self.refuse(ProgramReject::Gate(addresses_nothing(&event.node_id)));
                    }
                    self.pending.clear();
                    return StepOutput {
                        resolved: self.resolved.clone(),
                        effects: Vec::new(),
                        denials: Vec::new(),
                        rejected: None,
                        diagnostics: Vec::new(),
                    };
                }
                Some(action) => action,
            },
        };

        // Which fold this event is: a gesture, or the answer to a pending
        // question — and, for an answer, the confirm it addresses.
        let answer = match answer_of(event) {
            None => None,
            Some((token, accepted)) => {
                if !self.pending.iter().any(|t| t == token) {
                    return self.refuse(ProgramReject::Gate(RejectReason::PayloadOutOfBounds {
                        node_id: event.node_id.clone(),
                        detail: "the confirm answer answers no pending question".to_string(),
                    }));
                }
                let Some((path, confirm)) = addressed_confirm(&event.node_id, token, &action)
                else {
                    return self.refuse(ProgramReject::Gate(addresses_nothing(&event.node_id)));
                };
                if let Some(branch) = answer_branch(confirm, accepted) {
                    if !(self.can_dispatch)(branch) {
                        return self.refuse(ProgramReject::Gate(RejectReason::DispatchDenied {
                            node_id: event.node_id.clone(),
                            action: describe_action(branch).to_string(),
                        }));
                    }
                }
                Some((token.to_string(), path, accepted, confirm.clone()))
            }
        };

        let cost = match &answer {
            None => action_cascade_cost(&action),
            Some((_, _, accepted, confirm)) => answer_branch(confirm, *accepted)
                .map(action_cascade_cost)
                .unwrap_or(0),
        };
        if cost > self.budget.max_actions {
            return self.refuse(ProgramReject::BudgetExceeded {
                detail: format!(
                    "action cascade cost {cost} exceeds the {} permitted",
                    self.budget.max_actions
                ),
            });
        }
        if self.node_cost > self.budget.max_nodes {
            return self.refuse(ProgramReject::BudgetExceeded {
                detail: format!(
                    "tree cost {} exceeds the {} permitted",
                    self.node_cost, self.budget.max_nodes
                ),
            });
        }

        let outcome = match &answer {
            None => {
                self.pending.clear();
                run_gesture(&event.node_id, &action, self.store.clone())
            }
            Some((token, path, accepted, confirm)) => {
                self.pending.retain(|t| t != token);
                run_answer(&event.node_id, path, *accepted, confirm, self.store.clone())
            }
        };
        self.pending
            .extend(outcome.effects.iter().filter_map(|e| match e {
                ClientEffect::Confirm { token, .. } => Some(token.clone()),
                _ => None,
            }));
        self.store = outcome.store;
        self.resolved = resolve_tree(&self.store, &self.base_tree);

        let denials = outcome
            .effects
            .iter()
            .filter_map(|e| self.effects.decide(e))
            .collect();

        StepOutput {
            resolved: self.resolved.clone(),
            effects: outcome.effects,
            denials,
            rejected: None,
            diagnostics: outcome.diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{decode_node, encode_node};
    use std::collections::BTreeMap;

    const TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[{"id":"go","kind":{"$type":"Button","label":"go","onClick":{"$type":"Navigate","route":"/next"},"variant":"Secondary"}},{"id":"set","kind":{"$type":"Button","label":"set","onClick":{"$type":"SetState","key":"msg","value":"updated"},"variant":"Secondary"}},{"id":"readout","kind":{"$type":"Markdown","text":{"$type":"Bound","binding":{"$type":"State","defaultValue":"init","key":"msg"}}}}],"layout":{"$type":"Auto"},"role":"Dashboard"}}"#;

    fn click(node_id: &str) -> LiveEvent {
        LiveEvent {
            node_id: node_id.into(),
            event: "click".into(),
            payload: BTreeMap::new(),
        }
    }

    fn program() -> BoundedProgram {
        BoundedProgram::new(decode_node(TREE).expect("the fixture decodes"))
            .with_dispatch_gate(|_| true)
            .with_effect_policy(EffectPolicy::permissive())
    }

    #[test]
    fn a_state_write_becomes_visible_in_the_resolved_tree() {
        let mut program = program();
        assert!(encode_node(program.resolved()).contains("\"text\":\"init\""));
        let step = program.handle_event(&click("set"));
        assert!(step.rejected.is_none());
        assert!(encode_node(&step.resolved).contains("\"text\":\"updated\""));
    }

    #[test]
    fn a_declined_effect_is_reported_and_the_fold_is_unchanged_by_the_policy() {
        // The same event through a permissive host and a deny-all host reaches
        // the same arm with the same value — only the reporting differs. That
        // equality is what "performance is host-defined" means operationally.
        let permissive = program().handle_event(&click("go"));
        let mut denying = BoundedProgram::new(decode_node(TREE).expect("the fixture decodes"))
            .with_dispatch_gate(|_| true);
        let declined = denying.handle_event(&click("go"));

        assert_eq!(permissive.effects, declined.effects);
        assert!(permissive.denials.is_empty());
        assert_eq!(
            declined.denials,
            vec![Denial::Unregistered {
                capability: "Navigate".into()
            }],
            "a declined effect is reported, never dropped in silence"
        );
    }

    #[test]
    fn an_over_budget_cascade_is_refused_and_mutates_nothing() {
        let mut program = program().with_budget(InteractionBudget {
            max_actions: 0,
            max_nodes: u64::MAX,
        });
        let before = encode_node(program.resolved());
        let step = program.handle_event(&click("set"));
        assert!(matches!(
            step.rejected,
            Some(ProgramReject::BudgetExceeded { .. })
        ));
        assert_eq!(encode_node(&step.resolved), before);
    }

    // ── Phase 2106: the confirm round trip ───────────────────────────────────

    const CONFIRM_TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[{"id":"delete","kind":{"$type":"Button","label":"delete","onClick":{"$type":"Chain","ops":[{"$type":"SetState","key":"msg","value":"asked"},{"$type":"Confirm","onCancel":{"$type":"SetState","key":"msg","value":"kept"},"onConfirm":{"$type":"Navigate","route":"/orders"},"prompt":"Delete?"}]},"variant":"Secondary"}},{"id":"blank","kind":{"$type":"Button","label":"blank","onClick":{"$type":"Confirm","onConfirm":{"$type":"Print"},"prompt":{"$type":"Bound","binding":{"$type":"State","key":"absent"}}},"variant":"Secondary"}},{"id":"readout","kind":{"$type":"Markdown","text":{"$type":"Bound","binding":{"$type":"State","defaultValue":"init","key":"msg"}}}}],"layout":{"$type":"Auto"},"role":"Dashboard"}}"#;

    fn confirm_program(gate: impl Fn(&Action) -> bool + 'static) -> BoundedProgram {
        BoundedProgram::new(decode_node(CONFIRM_TREE).expect("the fixture decodes"))
            .with_dispatch_gate(gate)
            .with_effect_policy(EffectPolicy::permissive())
    }

    fn answer(node_id: &str, token: &str, accepted: bool) -> LiveEvent {
        let mut payload = BTreeMap::new();
        payload.insert(CONFIRM_TOKEN_KEY.to_string(), LiveValue::Str(token.into()));
        payload.insert(CONFIRM_ACCEPTED_KEY.to_string(), LiveValue::Bool(accepted));
        LiveEvent {
            node_id: node_id.into(),
            event: "click".into(),
            payload,
        }
    }

    #[test]
    fn a_gesture_asks_with_the_confirms_address_and_the_answer_runs_one_continuation() {
        let mut program = confirm_program(|_| true);
        let ask = program.handle_event(&click("delete"));
        assert_eq!(
            ask.effects,
            vec![ClientEffect::Confirm {
                prompt: "Delete?".into(),
                token: "delete#1".into()
            }],
            "the question, addressed by its chain position"
        );
        assert!(encode_node(&ask.resolved).contains("\"text\":\"asked\""));

        let no = program.handle_event(&answer("delete", "delete#1", false));
        assert!(no.rejected.is_none());
        assert!(no.effects.is_empty(), "onCancel reaches no effect");
        assert!(encode_node(&no.resolved).contains("\"text\":\"kept\""));

        let again = program.handle_event(&answer("delete", "delete#1", true));
        assert!(again.rejected.is_some(), "the answer consumed its question");
    }

    #[test]
    fn a_withdrawn_or_forged_answer_is_refused_and_a_refusal_leaves_the_question_pending() {
        let mut program = confirm_program(|_| true);
        assert!(
            program
                .handle_event(&answer("delete", "delete#1", true))
                .rejected
                .is_some(),
            "never asked"
        );

        program.handle_event(&click("delete"));
        assert!(
            program
                .handle_event(&answer("delete", "delete#0", true))
                .rejected
                .is_some(),
            "a token addressing no confirm"
        );
        let yes = program.handle_event(&answer("delete", "delete#1", true));
        assert!(
            yes.rejected.is_none(),
            "the refusal left the question standing"
        );
        assert_eq!(
            yes.effects,
            vec![ClientEffect::Navigate {
                route: "/orders".into(),
                target: crate::wire::NavigateTarget::Self_
            }]
        );

        program.handle_event(&click("delete"));
        assert!(
            program.handle_event(&click("blank")).rejected.is_none(),
            "an admitted gesture"
        );
        assert!(
            program
                .handle_event(&answer("delete", "delete#1", true))
                .rejected
                .is_some(),
            "an event that is not the answer withdrew the question"
        );
    }

    #[test]
    fn the_continuation_meets_the_dispatch_gate_on_its_own() {
        let mut program = confirm_program(|a| !matches!(a, Action::Navigate { .. }));
        assert!(
            program.handle_event(&click("delete")).rejected.is_none(),
            "the dialogue is admitted"
        );
        let yes = program.handle_event(&answer("delete", "delete#1", true));
        assert!(
            matches!(
                yes.rejected,
                Some(ProgramReject::Gate(RejectReason::DispatchDenied { .. }))
            ),
            "the navigation behind it is not"
        );
    }

    #[test]
    fn a_prompt_that_resolves_to_nothing_asks_nothing() {
        let mut program = confirm_program(|_| true);
        let step = program.handle_event(&click("blank"));
        assert!(step.effects.is_empty());
        assert!(!step.diagnostics.is_empty(), "the refusal is observable");
        assert!(
            program
                .handle_event(&answer("blank", "blank#", true))
                .rejected
                .is_some(),
            "and nothing is pending"
        );
    }
}
