//! Step traces — driving a scripted event list through the loop and comparing
//! the result against a recorded one.
//!
//! This module does **no IO**. Reading a scenario off a disk is the one part of
//! a conformance harness that genuinely differs per host, so it is the one part
//! that stays host-specific; everything that decides whether a loop agrees with
//! a recorded trace is here, and compiles for every target this crate builds
//! for — including `wasm32`, where there is no filesystem to have depended on.
//!
//! ## Two comparison rules, and they are deliberately different
//!
//! **The tree is compared SEMANTICALLY.** A recorded step carries the resolved
//! tree as an embedded *document*, never as a string of one implementation's
//! bytes. This host decodes it with its own decoder and re-encodes it with its
//! own encoder, so both sides of the comparison are this host's bytes and the
//! only thing being compared is the meaning. A host whose canonical form differs
//! from the one that recorded the trace is not thereby wrong — and a comparison
//! that held it to somebody else's bytes would be certifying an encoder under
//! the name of a loop.
//!
//! **The effects are compared BYTE-for-byte.** Their envelope is pinned
//! normatively, and there is no host freedom left to respect. So a recorded
//! effect is a *string whose contents are the document's own bytes*, and it is
//! compared as such. Putting one through a canonical encoder before comparing —
//! the obvious "tidy-up" — would silently correct the very exception this family
//! preserves, in the one place nobody would notice it had happened.
//!
//! ## First divergence is an obligation, not a convenience
//!
//! A comparison reports the **first** step at which two traces differ, naming
//! the step index and the member. Reporting only a final-state mismatch would be
//! wrong even where the verdict happened to be right: a fold that diverges at
//! step 2 and re-converges at step 5 passes a final-state comparison, and that
//! shape is precisely the defect a per-step trace exists to catch.
//!
//! Index 0 is the state **before any event**. A host that resolved the initial
//! tree differently has already diverged, and must be told so at step 0 rather
//! than at the first event, where it would read as a fold bug.

use crate::canonical::{JVal, parse, render_canonical};
use crate::render::BindingSources;
use crate::wire::{decode_node, encode_node};

use super::effect::EffectPolicy;
use super::program::BoundedProgram;
use super::validate::{LiveEvent, LiveValue};

use std::collections::BTreeMap;

/// What one step produced.
///
/// `tree_json` is a wire tree **document**. From a run it is this host's own
/// encoding of the resolved tree; from a recorded trace it is the document the
/// trace carries, which is why it must pass through
/// [`normalise_expectation`] before anything is compared — the two are the same
/// meaning and only accidentally the same bytes.
///
/// `effects` are client-effect documents in their as-emitted form, one per
/// effect, in order. Not merely the arm names: a trace recording only the arm
/// could not tell a navigation to one route from a navigation to another, and
/// computing the wrong route is exactly the fold defect worth catching.
#[derive(Debug, Clone, PartialEq)]
pub struct StepObservation {
    pub tree_json: String,
    pub effects: Vec<String>,
    pub refused: bool,
}

/// Where two traces stopped agreeing.
#[derive(Debug, Clone, PartialEq)]
pub struct Divergence {
    pub scenario: String,
    pub step: usize,
    pub member: &'static str,
    pub expected: String,
    pub actual: String,
}

impl Divergence {
    /// A report naming the first differing step and what differed in it.
    pub fn describe(&self) -> String {
        format!(
            "{}: diverged at step {} on {}\n  expected: {}\n  actual:   {}",
            self.scenario, self.step, self.member, self.expected, self.actual
        )
    }
}

/// Drive a tree through an event script and observe every step.
///
/// The host is constructed **permissive** — every effect arm registered and
/// permitted, every dispatch allowed — because a recorded trace is about whether
/// the fold agrees, not about whether a particular host's policy declines. The
/// policy seam has its own tests, and a declined effect does not change the fold
/// in any case.
pub fn run_scenario(tree_json: &str, events: &[LiveEvent]) -> Result<Vec<StepObservation>, String> {
    let tree =
        decode_node(tree_json).map_err(|e| format!("the scenario's tree does not decode: {e}"))?;
    let mut program = BoundedProgram::new(tree)
        .with_store(BindingSources::default())
        .with_dispatch_gate(|_| true)
        .with_effect_policy(EffectPolicy::permissive());

    // Step 0: the state before any event.
    let mut observations = vec![StepObservation {
        tree_json: encode_node(program.resolved()),
        effects: Vec::new(),
        refused: false,
    }];

    for event in events {
        let step = program.handle_event(event);
        observations.push(StepObservation {
            tree_json: encode_node(&step.resolved),
            effects: step.effects.iter().map(|e| e.encode()).collect(),
            refused: step.rejected.is_some(),
        });
    }
    Ok(observations)
}

/// Bring a recorded trace into this host's own terms.
///
/// Each step's tree document is decoded with this host's decoder and re-encoded
/// with its encoder. The effects beside it are deliberately left untouched —
/// see the module header.
pub fn normalise_expectation(
    scenario: &str,
    recorded: &[StepObservation],
) -> Result<Vec<StepObservation>, String> {
    recorded
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let tree = decode_node(&step.tree_json).map_err(|e| {
                format!("{scenario}: the recorded tree at step {index} does not decode: {e}")
            })?;
            Ok(StepObservation {
                tree_json: encode_node(&tree),
                ..step.clone()
            })
        })
        .collect()
}

/// Compare two traces step by step and return the **first** divergence.
///
/// Member order matters to the report's usefulness: the tree first (the
/// semantics), then the effects, then the refusal. A divergence in the tree
/// explains a later divergence in the effects, so reporting the effects first
/// would name the symptom and hide the cause.
pub fn first_divergence(
    scenario: &str,
    expected: &[StepObservation],
    actual: &[StepObservation],
) -> Option<Divergence> {
    if expected.len() != actual.len() {
        return Some(Divergence {
            scenario: scenario.to_string(),
            step: expected.len().min(actual.len()),
            member: "step count",
            expected: expected.len().to_string(),
            actual: actual.len().to_string(),
        });
    }
    expected
        .iter()
        .zip(actual.iter())
        .enumerate()
        .find_map(|(step, (want, got))| {
            let at = |member, expected: String, actual: String| {
                Some(Divergence {
                    scenario: scenario.to_string(),
                    step,
                    member,
                    expected,
                    actual,
                })
            };
            if want.tree_json != got.tree_json {
                at("tree", want.tree_json.clone(), got.tree_json.clone())
            } else if want.effects != got.effects {
                at(
                    "effects",
                    format!("[{}]", want.effects.join(", ")),
                    format!("[{}]", got.effects.join(", ")),
                )
            } else if want.refused != got.refused {
                at("refused", want.refused.to_string(), got.refused.to_string())
            } else {
                None
            }
        })
}

// ─── Reading a scenario's two authored documents ─────────────────────────────
//
// Parsing is not IO, so it lives here with everything else a host needs and
// compiles everywhere. Both parsers go through this host's own JSON layer.

/// Parse an event script: an ordered array of `{nodeId, event, payload}`.
pub fn parse_events(json: &str) -> Result<Vec<LiveEvent>, String> {
    let JVal::Arr(entries) =
        parse(json).map_err(|e| format!("the event script does not parse: {}", e.message))?
    else {
        return Err("the event script is not an array".to_string());
    };
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let string_at = |key: &str| match entry.field(key) {
                Some(JVal::Str(s)) => Ok(s.clone()),
                _ => Err(format!("event {index} carries no string '{key}'")),
            };
            let payload = match entry.field("payload") {
                None => BTreeMap::new(),
                Some(JVal::Obj(fields)) => fields
                    .iter()
                    .map(|(k, v)| (k.clone(), live_value(v)))
                    .collect(),
                Some(_) => return Err(format!("event {index} carries a non-object payload")),
            };
            Ok(LiveEvent {
                node_id: string_at("nodeId")?,
                event: string_at("event")?,
                payload,
            })
        })
        .collect()
}

fn live_value(value: &JVal) -> LiveValue {
    match value {
        JVal::Str(s) => LiveValue::Str(s.clone()),
        JVal::Num(n) => LiveValue::Num(*n),
        JVal::Bool(b) => LiveValue::Bool(*b),
        // An array or object has no form in the portable payload subset; it
        // reads as the absent value rather than being coerced into a shape a
        // bounds check would then trust.
        JVal::Null | JVal::Arr(_) | JVal::Obj(_) => LiveValue::Null,
    }
}

/// Parse a recorded trace: an ordered array of `{tree, effects, refused}`, one
/// entry per step.
///
/// A step entry declares exactly those three members and no others; a fourth is
/// refused rather than ignored, because a trace carrying a member somebody
/// expected to be honoured is worse than one that fails to load.
pub fn parse_expectation(json: &str) -> Result<Vec<StepObservation>, String> {
    let JVal::Arr(entries) =
        parse(json).map_err(|e| format!("the trace does not parse: {}", e.message))?
    else {
        return Err("the trace is not an array".to_string());
    };
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let JVal::Obj(fields) = entry else {
                return Err(format!("step {index} is not an object"));
            };
            for (key, _) in fields {
                if !matches!(key.as_str(), "tree" | "effects" | "refused") {
                    return Err(format!("step {index} carries an undeclared member '{key}'"));
                }
            }
            let Some(tree) = entry.field("tree") else {
                return Err(format!("step {index} carries no 'tree'"));
            };
            let Some(JVal::Arr(effects)) = entry.field("effects") else {
                return Err(format!("step {index} carries no 'effects' array"));
            };
            let Some(JVal::Bool(refused)) = entry.field("refused") else {
                return Err(format!("step {index} carries no 'refused' boolean"));
            };
            let effects = effects
                .iter()
                .map(|e| match e {
                    // As emitted: the recorded effect IS the document's bytes,
                    // carried as a string precisely so no JSON writer can
                    // reorder its members on the way through.
                    JVal::Str(s) => Ok(s.clone()),
                    _ => Err(format!(
                        "step {index} records an effect that is not a string of its own bytes"
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(StepObservation {
                // The tree is an embedded document. Rendering it back through
                // this host's own writer hands the decoder a document with the
                // right MEANING; normalise_expectation is what puts it into this
                // host's bytes before anything is compared.
                tree_json: render_canonical(tree),
                effects,
                refused: *refused,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[{"id":"set","kind":{"$type":"Button","label":"set","onClick":{"$type":"SetState","key":"msg","value":"updated"},"variant":"Secondary"}},{"id":"readout","kind":{"$type":"Markdown","text":{"$type":"Bound","binding":{"$type":"State","defaultValue":"init","key":"msg"}}}}],"layout":{"$type":"Auto"},"role":"Dashboard"}}"#;

    fn run() -> Vec<StepObservation> {
        let events = parse_events(r#"[{"nodeId":"set","event":"click","payload":{}}]"#)
            .expect("the script parses");
        run_scenario(TREE, &events).expect("the scenario runs")
    }

    #[test]
    fn a_trace_carries_one_entry_per_step_with_index_zero_before_any_event() {
        let observed = run();
        assert_eq!(observed.len(), 2);
        assert!(observed[0].tree_json.contains("\"text\":\"init\""));
        assert!(observed[1].tree_json.contains("\"text\":\"updated\""));
    }

    #[test]
    fn the_comparison_names_the_first_differing_step_not_the_last() {
        let observed = run();
        let mut perturbed = observed.clone();
        perturbed[0].tree_json = perturbed[0].tree_json.replace("\"init\"", "\"drifted\"");
        perturbed[1].tree_json = perturbed[1].tree_json.replace("\"updated\"", "\"also\"");
        let divergence =
            first_divergence("probe", &observed, &perturbed).expect("a divergence is found");
        assert_eq!(divergence.step, 0);
        assert_eq!(divergence.member, "tree");
    }

    #[test]
    fn a_fold_that_re_converges_still_diverges() {
        // The shape a final-state comparison would pass: differ in the middle,
        // agree at the end.
        let same = StepObservation {
            tree_json: "{}".into(),
            effects: vec![],
            refused: false,
        };
        let mut middle = same.clone();
        middle.tree_json = "{\"a\":1}".into();
        let a = vec![same.clone(), same.clone(), same.clone()];
        let b = vec![same.clone(), middle, same];
        assert_eq!(
            first_divergence("probe", &a, &b).map(|d| d.step),
            Some(1),
            "a mid-trace divergence must be caught even where the final states agree"
        );
    }

    #[test]
    fn the_comparison_is_semantic_on_the_tree_and_byte_exact_on_the_effects() {
        let observed = run();
        // Re-ordering a tree document's members changes every byte and no
        // meaning; normalisation must erase the difference.
        let reordered: Vec<StepObservation> = observed
            .iter()
            .map(|s| StepObservation {
                tree_json: reorder_members(&s.tree_json),
                ..s.clone()
            })
            .collect();
        assert_ne!(
            reordered[0].tree_json, observed[0].tree_json,
            "the reshaping actually changed the bytes"
        );
        let normalised = normalise_expectation("probe", &reordered).expect("it normalises");
        assert!(first_divergence("probe", &normalised, &observed).is_none());

        // And the other half: a normalisation that could not tell two DIFFERENT
        // trees apart would pass the above and be worthless.
        let altered: Vec<StepObservation> = observed
            .iter()
            .map(|s| StepObservation {
                tree_json: s.tree_json.replace("\"id\":\"root\"", "\"id\":\"rooted\""),
                ..s.clone()
            })
            .collect();
        let normalised = normalise_expectation("probe", &altered).expect("it normalises");
        assert!(first_divergence("probe", &normalised, &observed).is_some());
    }

    /// Reverse every object's member order — a different document, the same
    /// meaning. Deliberately not a canonical re-render, which would be a no-op
    /// against a canonical input and prove nothing.
    fn reorder_members(json: &str) -> String {
        fn flip(value: &JVal) -> JVal {
            match value {
                JVal::Obj(fields) => {
                    let mut flipped: Vec<(String, JVal)> =
                        fields.iter().map(|(k, v)| (k.clone(), flip(v))).collect();
                    flipped.reverse();
                    JVal::Obj(flipped)
                }
                JVal::Arr(items) => JVal::Arr(items.iter().map(flip).collect()),
                other => other.clone(),
            }
        }
        render_reversed(&flip(&parse(json).expect("it parses")))
    }

    /// Render preserving the member order given, rather than sorting it — the
    /// canonical renderer would put it straight back.
    fn render_reversed(value: &JVal) -> String {
        match value {
            JVal::Obj(fields) => {
                let members: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}:{}",
                            render_canonical(&JVal::Str(k.clone())),
                            render_reversed(v)
                        )
                    })
                    .collect();
                format!("{{{}}}", members.join(","))
            }
            JVal::Arr(items) => {
                let rendered: Vec<String> = items.iter().map(render_reversed).collect();
                format!("[{}]", rendered.join(","))
            }
            other => render_canonical(other),
        }
    }
}
