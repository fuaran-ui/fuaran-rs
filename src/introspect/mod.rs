//! Structural introspection + search over a decoded tree — "the machine can
//! see the UI". The interface is typed data, so a host reads its structure
//! rather than guessing at pixels: enumerate nodes, get a node's structural
//! facts, run a structural predicate returning the matching node ids, and
//! assert against the living tree. This is the substrate behind the Unit-Test
//! (assertions that survive a restyle because they test structure, not pixels),
//! Grep-Your-Apps (a structural query whose results are the running apps, matched
//! ids glowing), Pattern-Bank (deterministic structural search, no model call),
//! and Kintsugi (diagnose sabotage from typed structural signals) demos.
//!
//! Layout-flag derivation ([`layout`]) is the geometric companion — the machine
//! reads whether something overflows from measured geometry, not from looking.

pub mod layout;

use crate::wire::{
    Binding, Emphasis, FontVoice, Node, NodeCategory, NodeKind, StyleRole, StyleWeight, TextSource,
    ToneVariant,
};

/// The structural facts of one node — what a machine reads instead of pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeFacts {
    pub id: String,
    /// The wire `$type` name (`"Metric"`, `"DataGrid"`, `"Box"`, …).
    pub kind: &'static str,
    pub category: NodeCategory,
    pub tone: ToneVariant,
    pub weight: StyleWeight,
    pub emphasis: Emphasis,
    pub role: StyleRole,
    pub voice: FontVoice,
    /// The ids of this node's immediate structural children (in order).
    pub child_ids: Vec<String>,
    pub has_accessibility: bool,
    /// The static literal text of the node's primary text slot, when it has one
    /// (heading / markdown / badge / callout / link / metric label). `None` for
    /// bound / absent text — a machine reads the authored literal.
    pub text: Option<String>,
}

/// Every immediate sub-node the tree walk descends into (layout children +
/// structural sub-trees + the `state` surfaces).
fn child_nodes(n: &Node) -> Vec<&Node> {
    let mut out: Vec<&Node> = Vec::new();
    match &n.kind {
        NodeKind::Box(s) => out.extend(&s.children),
        NodeKind::SplitPanel(s) => out.extend(&s.children),
        NodeKind::Tabs(s) => out.extend(&s.children),
        NodeKind::Stepper(s) => out.extend(&s.children),
        NodeKind::SummaryList(s) => out.extend(&s.children),
        NodeKind::Disclosure(s) => out.extend(&s.children),
        NodeKind::Modal(s) => out.extend(&s.children),
        NodeKind::ScrollArea(s) => out.extend(&s.children),
        NodeKind::ErrorBoundary(s) => {
            out.push(&s.child);
            out.push(&s.fallback);
        }
        NodeKind::Switch(s) => {
            out.extend(s.cases.iter().map(|c| &c.child));
            out.push(&s.default);
        }
        NodeKind::FragmentDecl(s) => out.push(&s.body),
        _ => {}
    }
    if let Some(b) = &n.state.on_loading {
        out.push(b);
    }
    if let Some(b) = &n.state.on_empty {
        out.push(b);
    }
    out
}

/// The immediate *structural children* whose ids `NodeFacts.child_ids` reports —
/// the layout children only (the addressable structural list).
fn structural_child_ids(n: &Node) -> Vec<String> {
    let kids: &[Node] = match &n.kind {
        NodeKind::Box(s) => &s.children,
        NodeKind::SplitPanel(s) => &s.children,
        NodeKind::Tabs(s) => &s.children,
        NodeKind::Stepper(s) => &s.children,
        NodeKind::SummaryList(s) => &s.children,
        NodeKind::Disclosure(s) => &s.children,
        NodeKind::Modal(s) => &s.children,
        NodeKind::ScrollArea(s) => &s.children,
        _ => &[],
    };
    kids.iter().map(|c| c.id.clone()).collect()
}

fn literal_text(t: &TextSource) -> Option<String> {
    match t {
        TextSource::Literal(s) => Some(s.clone()),
        _ => None,
    }
}

/// The node's primary literal text, when it has an authored one.
fn primary_text(n: &Node) -> Option<String> {
    match &n.kind {
        NodeKind::Heading(s) => literal_text(&s.text),
        NodeKind::Markdown(s) => literal_text(&s.text),
        NodeKind::Badge(s) => literal_text(&s.label),
        NodeKind::Callout(s) => literal_text(&s.body),
        NodeKind::Link(s) => literal_text(&s.label),
        NodeKind::Metric(s) => literal_text(&s.label),
        NodeKind::LabelValueRow(s) => literal_text(&s.label),
        NodeKind::Button(s) => literal_text(&s.label),
        _ => None,
    }
}

/// The structural facts of a single node.
pub fn facts_of(n: &Node) -> NodeFacts {
    NodeFacts {
        id: n.id.clone(),
        kind: n.kind.type_name(),
        category: n.kind.category(),
        tone: n.style.tone,
        weight: n.style.weight,
        emphasis: n.style.emphasis,
        role: n.style.role,
        voice: n.style.voice,
        child_ids: structural_child_ids(n),
        has_accessibility: n.accessibility.is_some(),
        text: primary_text(n),
    }
}

/// Locate a node by id anywhere in the tree.
pub fn get_node<'a>(tree: &'a Node, id: &str) -> Option<&'a Node> {
    if tree.id == id {
        return Some(tree);
    }
    child_nodes(tree).into_iter().find_map(|c| get_node(c, id))
}

/// The structural facts of the node with `id`, when present.
pub fn get_node_facts(tree: &Node, id: &str) -> Option<NodeFacts> {
    get_node(tree, id).map(facts_of)
}

/// Every node's facts, in DFS document order — the "living UI as structured
/// data" surface an assertion suite or a query walks.
pub fn all_facts(tree: &Node) -> Vec<NodeFacts> {
    let mut out = vec![facts_of(tree)];
    for child in child_nodes(tree) {
        out.extend(all_facts(child));
    }
    out
}

/// Every node in the tree in document (pre-order) order, references into the
/// tree — the structural walk other capabilities (gate, digest, diff) key off.
pub fn all_nodes(tree: &Node) -> Vec<&Node> {
    let mut out = vec![tree];
    for child in child_nodes(tree) {
        out.extend(all_nodes(child));
    }
    out
}

// ─── Structural query ────────────────────────────────────────────────────────

/// A structural predicate over a node's facts — the compiled form of a "query
/// chip" (Grep) or a "shape query" (Pattern Bank). Composable with `Any` (OR)
/// and `All` (AND); `Not` negates. Deterministic, no model call.
#[derive(Debug, Clone)]
pub enum Query {
    /// The node's id equals this.
    Id(String),
    /// The node's kind `$type` name equals this (e.g. `"Metric"`).
    Kind(String),
    /// The node's behavioural category.
    Category(NodeCategory),
    /// The node's semantic tone.
    Tone(ToneVariant),
    /// The node has a bound (non-`Static`) reactive value binding somewhere in
    /// its spec — a "live" node.
    HasReactiveBinding,
    /// The node carries an accessibility trait.
    HasAccessibility,
    /// The node's primary literal text contains this substring (case-sensitive).
    TextContains(String),
    /// Any of the sub-queries match (OR).
    Any(Vec<Query>),
    /// All of the sub-queries match (AND).
    All(Vec<Query>),
    /// The sub-query does not match.
    Not(Box<Query>),
}

fn is_reactive(b: &Binding) -> bool {
    matches!(
        b,
        Binding::Query { .. }
            | Binding::Filter { .. }
            | Binding::Selection { .. }
            | Binding::State { .. }
            | Binding::Computed
            | Binding::Transform { .. }
            | Binding::Invoke { .. }
    )
}

/// Whether the node carries a reactive value binding in a primary value slot.
fn has_reactive_binding(n: &Node) -> bool {
    let check = |b: &Binding| is_reactive(b);
    match &n.kind {
        NodeKind::Metric(s) => check(&s.value) || s.trend.as_ref().is_some_and(check),
        NodeKind::LabelValueRow(s) => check(&s.value),
        NodeKind::Sparkline(s) => check(&s.source),
        NodeKind::Progress(s) => check(&s.fraction),
        NodeKind::DataGrid(s) => check(&s.source),
        NodeKind::Chart(s) => check(&s.source),
        NodeKind::Map(s) => check(&s.source),
        NodeKind::Toast(s) => check(&s.open),
        NodeKind::Image(s) => check(&s.src),
        NodeKind::Link(s) => check(&s.href),
        _ => false,
    }
}

fn matches(n: &Node, q: &Query) -> bool {
    match q {
        Query::Id(id) => n.id == *id,
        Query::Kind(k) => n.kind.type_name() == k,
        Query::Category(c) => n.kind.category() == *c,
        Query::Tone(t) => n.style.tone == *t,
        Query::HasReactiveBinding => has_reactive_binding(n),
        Query::HasAccessibility => n.accessibility.is_some(),
        Query::TextContains(sub) => primary_text(n).is_some_and(|t| t.contains(sub)),
        Query::Any(qs) => qs.iter().any(|q| matches(n, q)),
        Query::All(qs) => qs.iter().all(|q| matches(n, q)),
        Query::Not(q) => !matches(n, q),
    }
}

/// The ids of every node matching the structural query, in DFS document order —
/// "the search results are the running apps, matched nodes glowing".
pub fn find(tree: &Node, query: &Query) -> Vec<String> {
    let mut out = Vec::new();
    find_into(tree, query, &mut out);
    out
}

fn find_into(n: &Node, query: &Query, out: &mut Vec<String>) {
    if matches(n, query) {
        out.push(n.id.clone());
    }
    for child in child_nodes(n) {
        find_into(child, query, out);
    }
}

/// Whether any node in the tree matches — a fast structural existence check.
pub fn any_match(tree: &Node, query: &Query) -> bool {
    matches(tree, query) || child_nodes(tree).iter().any(|c| any_match(c, query))
}

// ─── Assertions (the Unit-Test surface) ──────────────────────────────────────

/// A structural assertion against the living tree. Restyle-proof: it tests
/// structure, not pixels, so it stays green across a theme swap.
#[derive(Debug, Clone)]
pub enum Assertion {
    /// A node with this id exists.
    NodeExists(String),
    /// The node with this id has this kind `$type` name.
    NodeHasKind { id: String, kind: String },
    /// At least `count` nodes match the query.
    AtLeast { query: Query, count: usize },
    /// Exactly `count` nodes match the query.
    Exactly { query: Query, count: usize },
    /// No node matches the query (a negative structural guarantee).
    None(Query),
}

/// The outcome of an assertion: pass, or a human/AI-readable reason it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionOutcome {
    Pass,
    Fail(String),
}

/// Run one assertion against the tree.
pub fn assert_tree(tree: &Node, assertion: &Assertion) -> AssertionOutcome {
    use AssertionOutcome::{Fail, Pass};
    match assertion {
        Assertion::NodeExists(id) => {
            if get_node(tree, id).is_some() {
                Pass
            } else {
                Fail(format!("no node with id '{id}'"))
            }
        }
        Assertion::NodeHasKind { id, kind } => match get_node(tree, id) {
            None => Fail(format!("no node with id '{id}'")),
            Some(n) if n.kind.type_name() == kind => Pass,
            Some(n) => Fail(format!(
                "node '{id}' is a {}, expected {kind}",
                n.kind.type_name()
            )),
        },
        Assertion::AtLeast { query, count } => {
            let n = find(tree, query).len();
            if n >= *count {
                Pass
            } else {
                Fail(format!("matched {n} nodes, expected at least {count}"))
            }
        }
        Assertion::Exactly { query, count } => {
            let n = find(tree, query).len();
            if n == *count {
                Pass
            } else {
                Fail(format!("matched {n} nodes, expected exactly {count}"))
            }
        }
        Assertion::None(query) => {
            let matched = find(tree, query);
            if matched.is_empty() {
                Pass
            } else {
                Fail(format!("expected no match, found {matched:?}"))
            }
        }
    }
}

/// Run a suite of assertions; returns each outcome in order (the Unit-Test
/// panel's green/red run).
pub fn assert_all(tree: &Node, assertions: &[Assertion]) -> Vec<AssertionOutcome> {
    assertions.iter().map(|a| assert_tree(tree, a)).collect()
}
