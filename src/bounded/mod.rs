//! The bounded program loop — behaviour carried as data, run at the client
//! placement.
//!
//! A program tree that arrived over the wire is data, not code. This module is
//! what runs it: the closed action walk, the per-interaction resource budget,
//! the default-deny effect vocabulary, the binding re-resolution pass that makes
//! a state write visible, and the loop that puts them in order —
//! **validate → interpret → effects → re-resolve**.
//!
//! # The bounded-path declaration
//!
//! The program wire specification makes its driver-semantics family **opt-in by
//! declaration**: the family asserts what a bounded program *loop* does, which
//! is a stronger claim than round-tripping a document, and a host that only
//! decodes, encodes, records, relays or validates those documents is out of
//! scope for it rather than failing it. The specification fixes no encoding for
//! the declaration, because a conformance claim is a sentence somebody writes
//! down. Here is this host's, and it names both halves:
//!
//! > **`fuaran-rs` implements the bounded path, and it reproduces the
//! > driver-semantics family of the program wire specification's conformance
//! > corpus.** Every scenario the corpus enumerates is driven through this loop
//! > and compared step by step — the resolved tree semantically, through this
//! > host's own decoder and encoder; the client effects byte-for-byte in their
//! > as-emitted envelope; the refusal exactly. The first divergence is reported
//! > with its step index and the member that differed.
//!
//! The claim covers the **client** placement. Running host-registered handlers
//! is a separate obligation this host does not declare, and the loop below has
//! no handler registry to consult: a call action therefore resolves to the
//! documented no-op it has always been where nothing is registered.
//!
//! The second half of the claim is executable rather than asserted. The check is
//! `tests/driver_semantics.rs`, and it runs against the corpus when the corpus is
//! present locally — see that file's header for how it is invoked, and why it is
//! a local gate rather than one this repository's public workflow could run.
//!
//! # What a claim over this loop does and does not assert
//!
//! It asserts that the loop folds the same way: same resolved tree, same effects
//! reached with the same values in the same order, same refusals, at every step.
//!
//! It asserts **nothing** about what a host did with an effect it reached.
//! Performance is host-defined and a host that performs an effect differently is
//! conformant; a host that declines every effect is conformant too, because both
//! vocabularies default to deny and a gate whose refusal counted as failure
//! would be a mandatory capability list wearing a gate's name. What is *not*
//! conformant is silence: a declined effect is reported as a denial carrying the
//! derived capability, never dropped, because in a record of what a program did
//! a silently-dropped effect and a performed one are indistinguishable.
//!
//! **That last sentence is now falsifiable rather than merely stated.** A step
//! trace can record what the performer seam declined, so the two hosts it
//! distinguishes — one that drops an effect in silence, one that declines it
//! audibly — no longer produce the same trace. A scenario asking that question
//! names the policy it presumes, by name; this host constructs what the name
//! denotes ([`EffectPolicy::named`]) and refuses one it does not recognise,
//! because falling back to its own default would report a scenario it could not
//! evaluate as one it passed.
//!
//! # Two invariants, and neither is a matter of care
//!
//! **No foreign code.** The wire cannot carry a closure and the decoder erases
//! every closure slot, so there is nothing in the decoded model for this fold to
//! invoke. The only store mutation is the state write; the only outward reach is
//! the closed effect vocabulary.
//!
//! **No arbitrary cost.** Bounded code is not bounded cost. The budget caps the
//! action cascade and the tree's render cost per interaction, deterministically
//! and without reading a clock, and a breach is a structured refusal — never a
//! hang, and never a partial mutation.
//!
//! Bounded code plus bounded cost is what makes a program safe to run untrusted.

pub mod actions;
pub mod budget;
pub mod effect;
pub mod program;
pub mod resolve;
pub mod trace;
pub mod validate;

pub use actions::{BoundedDiagnostic, BoundedOutcome, run_bounded_action};
pub use budget::InteractionBudget;
pub use effect::{CLIENT_EFFECT_ARMS, ClientEffect, Denial, EffectPolicy, EgressFloor};
pub use program::{BoundedProgram, ProgramReject, StepOutput};
pub use resolve::resolve_tree;
pub use trace::{
    Divergence, StepObservation, first_divergence, normalise_expectation, parse_events,
    parse_expectation, run_scenario,
};
pub use validate::{LiveEvent, LiveValue, RejectReason, validate};
