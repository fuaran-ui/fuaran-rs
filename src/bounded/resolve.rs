//! Binding re-resolution — the pass that makes a state write **visible**.
//!
//! A `State` binding encodes identically whatever the store holds: it carries
//! the key and the default, never the resolved value. So the tree a program
//! starts from is byte-stable across every state write, and a consumer diffing
//! or re-rendering the raw tree would see nothing change. This pass substitutes
//! every *resolvable* binding with the concrete value it resolves to, against
//! the store as the interpreter left it — which is what turns a store write into
//! an observable difference in the tree.
//!
//! It is applied to the **fixed** tree the program started from, never to the
//! previous step's output. Resolving a resolved tree would fold each step's
//! substitutions into the next one's input, so a later step could not recover a
//! binding an earlier step happened to resolve — the store would stop being the
//! only thing that carries state. This is now **normative** rather than
//! inherited: the specification's §10.5 states it, and the corpus's
//! `fixed-base-reresolution` scenario is the multi-event trace that tells the
//! two readings apart — every one-event scenario passes under both.
//!
//! ## The coverage floor is deliberate, and it is recorded rather than implied
//!
//! Not every kind's bindings re-resolve. The covered set is the state-reactive
//! display, input and layout slots; a kind outside it passes through with its
//! bindings intact, and a state write inside one is therefore not yet visible.
//! That is a **negative result the conformance corpus pins**, not an accident: a
//! scenario records the kinds the pass does not reach, so widening the floor
//! changes a recorded expectation rather than moving behaviour silently.
//!
//! §10.5 rules that arrangement normative and puts the membership where it
//! already was: a floor exists and is neither everything nor nothing — the part
//! a host cannot derive — while *which* kinds are in it is enumerated by the
//! corpus rather than tabulated in a document that does not own the tree
//! vocabulary. For a kind no scenario reaches, this module's floor is this
//! host's own, and the two paragraphs above are the declaration §10.5 requires
//! of it.
//!
//! Structure is never lost. Children recurse generically through
//! [`structural_children`], so an uncovered container still has its covered
//! descendants resolved.

use crate::canonical::JVal;
use crate::render::BindingSources;
use crate::render::bindings::{Resolution, Value, resolve, try_scalar_string};
use crate::wire::{Binding, Node, NodeKind, StaticValue, TextSource};

/// The child nodes a kind carries as an ordered structural list — the same set
/// the structural tree-ops address. A kind holding child nodes in some other
/// shape (a switch's cases, an error boundary's two arms, a mount's inputs)
/// answers with none, and is right to: those positions are not an ordered list,
/// and treating them as one is how a traversal and an apply engine drift apart.
pub fn structural_children(kind: &NodeKind) -> Vec<&Node> {
    match kind {
        NodeKind::Box(spec) => spec.children.iter().collect(),
        NodeKind::SplitPanel(spec) => spec.children.iter().collect(),
        NodeKind::Tabs(spec) => spec.children.iter().collect(),
        NodeKind::Stepper(spec) => spec.children.iter().collect(),
        NodeKind::SummaryList(spec) => spec.children.iter().collect(),
        NodeKind::Disclosure(spec) => spec.children.iter().collect(),
        NodeKind::Modal(spec) => spec.children.iter().collect(),
        NodeKind::ScrollArea(spec) => spec.children.iter().collect(),
        NodeKind::FragmentDecl(spec) => vec![spec.body.as_ref()],
        _ => Vec::new(),
    }
}

/// Substitute a binding with the concrete value it resolves to; leave it exactly
/// as it was when it does not resolve. An unresolved binding is a real state a
/// renderer distinguishes (still loading, versus resolved-to-nothing), so this
/// pass never fabricates a value to fill it.
fn subst(sources: &BindingSources, binding: &Binding) -> Binding {
    match resolve(sources, binding) {
        Resolution::Resolved(value) => Binding::Static {
            value: to_static(&value),
        },
        Resolution::NotResolved | Resolution::I18nUnresolved(_) => binding.clone(),
    }
}

fn subst_opt(sources: &BindingSources, binding: &Option<Binding>) -> Option<Binding> {
    binding.as_ref().map(|b| subst(sources, b))
}

/// A resolved value's `Static` payload form.
fn to_static(value: &Value<'_>) -> StaticValue {
    match value {
        Value::Static(payload) => (*payload).clone(),
        Value::Json(json) => StaticValue::Ast((*json).clone()),
        Value::Text(text) => StaticValue::Ast(JVal::Str(text.clone())),
    }
}

/// Resolve a bound text slot to its literal; a literal or an i18n key passes
/// through (neither moves under a state write).
fn resolve_text(sources: &BindingSources, text: &TextSource) -> TextSource {
    match text {
        TextSource::Bound(binding) => match try_scalar_string(sources, binding) {
            Some(resolved) => TextSource::Literal(resolved),
            None => text.clone(),
        },
        TextSource::Literal(_) | TextSource::I18n { .. } => text.clone(),
    }
}

fn resolve_text_opt(sources: &BindingSources, text: &Option<TextSource>) -> Option<TextSource> {
    text.as_ref().map(|t| resolve_text(sources, t))
}

/// Re-resolve a whole tree's bindings against the store. Structure is preserved
/// — no node added, removed or re-identified; only leaf binding and text values
/// change.
pub fn resolve_tree(sources: &BindingSources, node: &Node) -> Node {
    Node {
        id: node.id.clone(),
        kind: resolve_kind(sources, &node.kind),
        state: node.state.clone(),
        style: node.style,
        accessibility: node.accessibility.clone(),
        // Phase 1112 - the hint is CONTENT, so it resolves like every other
        // TextSource slot rather than travelling through unread: a `Bound`
        // tooltip must show the store's value, not its binding.
        tooltip: resolve_text_opt(sources, &node.tooltip),
    }
}

fn resolve_children(sources: &BindingSources, children: &[Node]) -> Vec<Node> {
    children.iter().map(|n| resolve_tree(sources, n)).collect()
}

/// The per-kind pass. Exhaustive with **no catch-all arm**, so a kind added to
/// the vocabulary is a build error here until somebody decides whether it is
/// inside the floor or outside it — which is the decision this module exists to
/// keep explicit.
fn resolve_kind(sources: &BindingSources, kind: &NodeKind) -> NodeKind {
    match kind {
        // ── Layout: covered chrome, and children recurse ─────────────────────
        NodeKind::Box(spec) => {
            let mut spec = spec.clone();
            spec.children = resolve_children(sources, &spec.children);
            spec.heading = resolve_text_opt(sources, &spec.heading);
            NodeKind::Box(spec)
        }
        NodeKind::Tabs(spec) => {
            let mut spec = spec.clone();
            spec.children = resolve_children(sources, &spec.children);
            spec.active_index = subst(sources, &spec.active_index);
            spec.active_tag = subst_opt(sources, &spec.active_tag);
            NodeKind::Tabs(spec)
        }
        NodeKind::Stepper(spec) => {
            let mut spec = spec.clone();
            spec.children = resolve_children(sources, &spec.children);
            spec.active_step = subst(sources, &spec.active_step);
            NodeKind::Stepper(spec)
        }
        NodeKind::SummaryList(spec) => {
            let mut spec = spec.clone();
            spec.children = resolve_children(sources, &spec.children);
            spec.heading = resolve_text_opt(sources, &spec.heading);
            NodeKind::SummaryList(spec)
        }
        NodeKind::Disclosure(spec) => {
            let mut spec = spec.clone();
            spec.children = resolve_children(sources, &spec.children);
            spec.open = subst(sources, &spec.open);
            spec.heading = resolve_text(sources, &spec.heading);
            NodeKind::Disclosure(spec)
        }
        // Outside the floor for their own slots, but still containers: their
        // descendants resolve.
        NodeKind::SplitPanel(spec) => {
            let mut spec = spec.clone();
            spec.children = resolve_children(sources, &spec.children);
            NodeKind::SplitPanel(spec)
        }
        NodeKind::Modal(spec) => {
            let mut spec = spec.clone();
            spec.children = resolve_children(sources, &spec.children);
            NodeKind::Modal(spec)
        }
        NodeKind::ScrollArea(spec) => {
            let mut spec = spec.clone();
            spec.children = resolve_children(sources, &spec.children);
            NodeKind::ScrollArea(spec)
        }
        NodeKind::FragmentDecl(spec) => {
            let mut spec = spec.clone();
            spec.body = Box::new(resolve_tree(sources, &spec.body));
            NodeKind::FragmentDecl(spec)
        }

        // ── Display: the state-reactive text and scalar slots ────────────────
        NodeKind::Heading(spec) => {
            let mut spec = spec.clone();
            spec.text = resolve_text(sources, &spec.text);
            NodeKind::Heading(spec)
        }
        NodeKind::Markdown(spec) => {
            let mut spec = spec.clone();
            spec.text = resolve_text(sources, &spec.text);
            NodeKind::Markdown(spec)
        }
        NodeKind::Badge(spec) => {
            let mut spec = spec.clone();
            spec.label = resolve_text(sources, &spec.label);
            NodeKind::Badge(spec)
        }
        NodeKind::Metric(spec) => {
            let mut spec = spec.clone();
            spec.label = resolve_text(sources, &spec.label);
            spec.value = subst(sources, &spec.value);
            spec.trend = subst_opt(sources, &spec.trend);
            spec.subtext = resolve_text_opt(sources, &spec.subtext);
            NodeKind::Metric(spec)
        }
        NodeKind::Callout(spec) => {
            let mut spec = spec.clone();
            spec.heading = resolve_text_opt(sources, &spec.heading);
            spec.body = resolve_text(sources, &spec.body);
            NodeKind::Callout(spec)
        }
        NodeKind::Progress(spec) => {
            let mut spec = spec.clone();
            spec.fraction = subst(sources, &spec.fraction);
            spec.label = resolve_text_opt(sources, &spec.label);
            spec.caveat = resolve_text_opt(sources, &spec.caveat);
            NodeKind::Progress(spec)
        }
        NodeKind::LabelValueRow(spec) => {
            let mut spec = spec.clone();
            spec.label = resolve_text(sources, &spec.label);
            spec.value = subst(sources, &spec.value);
            spec.help = resolve_text_opt(sources, &spec.help);
            NodeKind::LabelValueRow(spec)
        }
        NodeKind::Link(spec) => {
            let mut spec = spec.clone();
            spec.href = subst(sources, &spec.href);
            spec.label = resolve_text(sources, &spec.label);
            NodeKind::Link(spec)
        }
        NodeKind::Sparkline(spec) => {
            let mut spec = spec.clone();
            spec.source = subst(sources, &spec.source);
            NodeKind::Sparkline(spec)
        }

        // ── Input: the two kinds whose own chrome is state-reactive ──────────
        NodeKind::Button(spec) => {
            let mut spec = spec.clone();
            spec.label = resolve_text(sources, &spec.label);
            spec.disabled = subst_opt(sources, &spec.disabled);
            NodeKind::Button(spec)
        }
        NodeKind::Select(spec) => {
            let mut spec = spec.clone();
            spec.label = resolve_text(sources, &spec.label);
            spec.source = subst(sources, &spec.source);
            spec.value = subst(sources, &spec.value);
            spec.placeholder = resolve_text_opt(sources, &spec.placeholder);
            NodeKind::Select(spec)
        }

        // ── Outside the floor, and carrying no structural child list ─────────
        //
        // Each of these is passed through whole. Several DO carry reactive
        // bindings (a grid's or chart's row source, a form's field values, a
        // switch's selector), and a state write inside one is not yet visible —
        // which is exactly the negative result the corpus records. Widening the
        // floor means moving a kind up out of this arm and re-recording the
        // expectation that pins it.
        NodeKind::Skeleton(_)
        | NodeKind::Icon(_)
        | NodeKind::Fact(_)
        | NodeKind::Image(_)
        // Phase 1076 — `Media` sits beside `Image` for the same reason: it
        // carries reactive bindings (`src`, and `Video`'s `poster`) and no
        // structural child list, so a state write inside one is not yet
        // visible. Same recorded negative result, same remedy if it moves.
        | NodeKind::Media(_)
        // Phase 1111 / 1120 — beside `Media` for the same reason, and recorded
        // rather than implied. `Embed` carries a reactive `src` and no
        // structural child list. `Tree`'s rows are `TreeItem` RECORDS, not
        // `Node`s, so `structural_children` does not reach them at all and its
        // labels are the only reactive surface it has; its two State slots are
        // read by the RENDERER from the store, never substituted into the tree,
        // so there is nothing here for this pass to make visible either way.
        // A state write inside either is therefore not yet visible — the same
        // negative result, and the same remedy if the floor widens.
        | NodeKind::Embed(_)
        | NodeKind::Tree(_)
        | NodeKind::List(_)
        | NodeKind::Toast(_)
        | NodeKind::CodeBlock(_)
        | NodeKind::Math(_)
        | NodeKind::Drawing(_)
        | NodeKind::Form(_)
        | NodeKind::Filters(_)
        | NodeKind::FileUpload(_)
        | NodeKind::DataGrid(_)
        | NodeKind::Chart(_)
        | NodeKind::Map(_)
        | NodeKind::Custom(_)
        | NodeKind::ErrorBoundary(_)
        | NodeKind::Switch(_)
        | NodeKind::FragmentRef(_)
        | NodeKind::Mount(_) => kind.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{decode_node, encode_node};

    const TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[{"id":"readout","kind":{"$type":"Markdown","text":{"$type":"Bound","binding":{"$type":"State","defaultValue":"init","key":"msg"}}}}],"layout":{"$type":"Auto"},"role":"Dashboard"}}"#;

    #[test]
    fn a_bound_text_slot_resolves_to_the_store_value_and_falls_back_to_its_default() {
        let tree = decode_node(TREE).expect("the fixture decodes");

        let empty = BindingSources::default();
        assert!(encode_node(&resolve_tree(&empty, &tree)).contains("\"text\":\"init\""));

        let mut sources = BindingSources::default();
        sources
            .state
            .insert("msg".into(), JVal::Str("updated".into()));
        assert!(encode_node(&resolve_tree(&sources, &tree)).contains("\"text\":\"updated\""));
    }

    #[test]
    fn an_unresolvable_binding_is_left_exactly_as_it_was() {
        let tree = decode_node(
            r#"{"id":"m","kind":{"$type":"Metric","label":"l","value":{"$type":"Query","name":"__absent__"}}}"#,
        )
        .expect("the fixture decodes");
        let resolved = resolve_tree(&BindingSources::default(), &tree);
        assert_eq!(encode_node(&resolved), encode_node(&tree));
    }

    #[test]
    fn the_pass_reaches_a_covered_slot_inside_an_uncovered_container() {
        // A modal is outside the floor for its own chrome and still recurses,
        // so structure never hides a covered descendant.
        let tree = decode_node(
            r#"{"id":"root","kind":{"$type":"Modal","children":[{"id":"readout","kind":{"$type":"Markdown","text":{"$type":"Bound","binding":{"$type":"State","defaultValue":"init","key":"msg"}}}}],"dismissable":true,"open":{"$type":"Static","value":true}}}"#,
        )
        .expect("the fixture decodes");
        let mut sources = BindingSources::default();
        sources.state.insert("msg".into(), JVal::Str("deep".into()));
        assert!(encode_node(&resolve_tree(&sources, &tree)).contains("\"text\":\"deep\""));
    }
}
