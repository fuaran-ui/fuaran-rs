//! What a node's children are — the ONE enumeration of the parent/child
//! relation in this host, with the reach a caller wants stated rather than
//! re-derived (Phase 2662).
//!
//! The relation used to be written by hand in every walk that needed it, and
//! the copies disagreed about reach: the server renderer's fragment registry
//! and its expansion namespacing stopped at the kind, so a `FragmentDecl` in a
//! `state.onLoading` alternative never resolved and a namespaced body kept the
//! ids of its state alternatives; the island walk read an `ErrorBoundary`'s
//! `child` but not its `fallback` and no `Switch` case at all, so an island
//! there got no hydrate payload; the validator and the introspection walk
//! skipped slot arguments and the envelope `fallback`, so a node the apply
//! engine finds and edits there was invisible to both. Each copy answered a
//! slightly different question, and nothing said which.
//!
//! So this module names the questions. Every position a node holds another
//! node in has a CLASS ([`Position`]), and a caller asks for the classes it
//! means ([`Reach`]). The `held!` match below is the only place a [`NodeKind`]
//! is taken apart to find its children, and every row of it is pinned against
//! the generated slot table ([`NODE_SLOT_MEMBERS`](crate::wire::NODE_SLOT_MEMBERS)),
//! so a node slot the vocabulary gains fails the suite until it is classed here.
//!
//! The classes and the named reaches are the reference host's (`NodeChildren`),
//! so "structural", "kind-held", "fragment scope" and "keyed" mean the same set
//! of positions on every host.

use crate::canonical::ordinal_cmp;
use crate::wire::{FragmentArg, Node, NodeKind, StateBehaviour};

/// The kind of place one node holds another in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Position {
    /// A container's ordered child list (`Box`, `SplitPanel`, `Tabs`, `Stepper`,
    /// `SummaryList`, `Disclosure`, `Modal`, `ScrollArea`), and a `FragmentDecl`'s
    /// body, its one ordered child. The surface the structural ops edit — though
    /// only the containers' lists are resizable ([`ordered_list`]).
    Ordered,
    /// A node the KIND holds in a named arm: `ErrorBoundary.child` /
    /// `ErrorBoundary.fallback`, every `Switch` case child and its default.
    Arm,
    /// The node envelope's `state.onLoading` / `state.onEmpty` alternatives,
    /// which render INSTEAD of the node.
    StateArm,
    /// The node envelope's author-declared `fallback`, which a reader that
    /// cannot render the node renders instead.
    Fallback,
    /// A subtree passed as an argument: a `FragmentRef`'s slot arguments and a
    /// `Mount`'s slot inputs.
    Argument,
}

impl Position {
    const fn bit(self) -> u8 {
        match self {
            Position::Ordered => 1,
            Position::Arm => 2,
            Position::StateArm => 4,
            Position::Fallback => 8,
            Position::Argument => 16,
        }
    }
}

/// The position classes a walk descends through. A walk passes one explicitly;
/// there is no default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reach(u8);

impl Reach {
    /// The ordered child lists alone — what the structural ops edit, and what
    /// the bounded loop's re-resolution and budget walks descend.
    pub const STRUCTURAL: Reach = Reach::of(&[Position::Ordered]);
    /// Every node the node's own KIND holds: the ordered lists plus the arms.
    /// The tree as the vocabulary shapes it — the island walk's relation. It
    /// leaves out the envelope alternatives (which render INSTEAD of the node)
    /// and slot arguments (which belong to the scope they are passed into).
    pub const KIND_HELD: Reach = Reach::of(&[Position::Ordered, Position::Arm]);
    /// The kind-held positions plus the two `state` alternatives: what the
    /// fragment registry and expansion namespacing descend through. The envelope
    /// `fallback` and slot arguments are outside it.
    pub const FRAGMENT_SCOPE: Reach =
        Reach::of(&[Position::Ordered, Position::Arm, Position::StateArm]);
    /// Every position a node holds another node in — the surface node lookup, id
    /// uniqueness, the §21 bounds, the footprint and the validator quantify over
    /// (WIRE_FORMAT §34.2).
    pub const KEYED: Reach = Reach::of(&[
        Position::Ordered,
        Position::Arm,
        Position::StateArm,
        Position::Fallback,
        Position::Argument,
    ]);

    /// The reach naming exactly `positions`.
    pub const fn of(positions: &[Position]) -> Reach {
        let mut bits = 0;
        let mut i = 0;
        while i < positions.len() {
            bits |= positions[i].bit();
            i += 1;
        }
        Reach(bits)
    }

    /// This reach with `position` removed.
    pub const fn without(self, position: Position) -> Reach {
        Reach(self.0 & !position.bit())
    }

    /// Whether a walk at this reach descends through `position`.
    pub const fn contains(self, position: Position) -> bool {
        self.0 & position.bit() != 0
    }
}

/// One position a node holds another node in.
#[derive(Debug, Clone, Copy)]
pub struct Slot<'a> {
    /// The member that declares the position, as its construct token — exactly
    /// a row of [`NODE_SLOT_MEMBERS`](crate::wire::NODE_SLOT_MEMBERS).
    pub member: &'static str,
    pub position: Position,
    pub node: &'a Node,
}

/// Every row of the slot list: the member each position is read from and its
/// class. The test suite holds it equal to the generated
/// [`NODE_SLOT_MEMBERS`](crate::wire::NODE_SLOT_MEMBERS), and holds `held!`
/// to reaching every row.
pub const SLOT_MEMBERS: &[(&str, Position)] = &[
    ("FragmentArg.SlotArg.tree", Position::Argument),
    ("Kind.Box.children", Position::Ordered),
    ("Kind.Disclosure.children", Position::Ordered),
    ("Kind.ErrorBoundary.child", Position::Arm),
    ("Kind.ErrorBoundary.fallback", Position::Arm),
    ("Kind.FragmentDecl.body", Position::Ordered),
    ("Kind.Modal.children", Position::Ordered),
    ("Kind.ScrollArea.children", Position::Ordered),
    ("Kind.SplitPanel.children", Position::Ordered),
    ("Kind.Stepper.children", Position::Ordered),
    ("Kind.SummaryList.children", Position::Ordered),
    ("Kind.Switch.default", Position::Arm),
    ("Kind.Tabs.children", Position::Ordered),
    ("Node.fallback", Position::Fallback),
    ("StateBehaviour.onEmpty", Position::StateArm),
    ("StateBehaviour.onLoading", Position::StateArm),
    ("SwitchCase.child", Position::Arm),
];

const SLOT_ARG: &str = "FragmentArg.SlotArg.tree";

// The one match over the kinds that hold nodes, written once and expanded for a
// shared and for a mutable borrow (`held!(kind)` / `held!(kind, mut)`). It
// yields the container's resizable list apart, because the structural ops edit
// that list as a list, then every other kind-held node with its member and
// class, in the kind's own order. Exhaustive with no catch-all, so a kind the
// vocabulary gains is a build error here until it is classed.
macro_rules! held {
    ($kind:expr $(, $m:ident)?) => {{
        let mut list = None;
        let mut nodes = Vec::new();
        let mut args = None;
        match $kind {
            NodeKind::Box(s) => list = Some(("Kind.Box.children", &$($m)? s.children)),
            NodeKind::SplitPanel(s) => list = Some(("Kind.SplitPanel.children", &$($m)? s.children)),
            NodeKind::Tabs(s) => list = Some(("Kind.Tabs.children", &$($m)? s.children)),
            NodeKind::Stepper(s) => list = Some(("Kind.Stepper.children", &$($m)? s.children)),
            NodeKind::SummaryList(s) => {
                list = Some(("Kind.SummaryList.children", &$($m)? s.children))
            }
            NodeKind::Disclosure(s) => {
                list = Some(("Kind.Disclosure.children", &$($m)? s.children))
            }
            NodeKind::Modal(s) => list = Some(("Kind.Modal.children", &$($m)? s.children)),
            NodeKind::ScrollArea(s) => {
                list = Some(("Kind.ScrollArea.children", &$($m)? s.children))
            }
            NodeKind::FragmentDecl(s) => {
                nodes.push(("Kind.FragmentDecl.body", Position::Ordered, &$($m)? *s.body))
            }
            NodeKind::ErrorBoundary(s) => {
                nodes.push(("Kind.ErrorBoundary.child", Position::Arm, &$($m)? *s.child));
                nodes.push(("Kind.ErrorBoundary.fallback", Position::Arm, &$($m)? *s.fallback));
            }
            NodeKind::Switch(s) => {
                for case in &$($m)? s.cases {
                    nodes.push(("SwitchCase.child", Position::Arm, &$($m)? case.child));
                }
                nodes.push(("Kind.Switch.default", Position::Arm, &$($m)? *s.default));
            }
            NodeKind::FragmentRef(s) => args = Some(&$($m)? s.args),
            NodeKind::Mount(s) => args = Some(&$($m)? s.inputs),
            NodeKind::Heading(_)
            | NodeKind::Markdown(_)
            | NodeKind::Text(_)
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
            | NodeKind::Media(_)
            // A frame's document is somebody else's tree, and a `TreeItem` is a
            // record rather than a `Node`: neither holds a node.
            | NodeKind::Embed(_)
            | NodeKind::Tree(_)
            | NodeKind::List(_)
            | NodeKind::Toast(_)
            | NodeKind::CodeBlock(_)
            | NodeKind::Math(_)
            | NodeKind::Form(_)
            | NodeKind::Filters(_)
            | NodeKind::Button(_)
            | NodeKind::FileUpload(_)
            | NodeKind::Select(_)
            | NodeKind::DataGrid(_)
            | NodeKind::Chart(_)
            | NodeKind::Map(_)
            | NodeKind::Custom(_)
            | NodeKind::Drawing(_) => {}
        }
        (list, nodes, args)
    }};
}

/// The slot trees of an argument bag in ordinal key order (the canonical
/// encoding's order); a value argument holds no node.
fn slot_trees(args: &[(String, FragmentArg)]) -> Vec<&Node> {
    let mut keyed: Vec<(&str, &Node)> = args
        .iter()
        .filter_map(|(k, a)| match a {
            FragmentArg::Slot { tree } => Some((k.as_str(), &**tree)),
            FragmentArg::Value(_) => None,
        })
        .collect();
    keyed.sort_by(|x, y| ordinal_cmp(x.0, y.0));
    keyed.into_iter().map(|(_, t)| t).collect()
}

fn slot_trees_mut(args: &mut [(String, FragmentArg)]) -> Vec<&mut Node> {
    let mut keyed: Vec<(&str, &mut Node)> = args
        .iter_mut()
        .filter_map(|(k, a)| match a {
            FragmentArg::Slot { tree } => Some((k.as_str(), &mut **tree)),
            FragmentArg::Value(_) => None,
        })
        .collect();
    keyed.sort_by(|x, y| ordinal_cmp(x.0, y.0));
    keyed.into_iter().map(|(_, t)| t).collect()
}

/// The positions a KIND holds at `reach`, in the kind's order: the ordered list
/// (or a fragment body), the arms, then the slot arguments in key order.
pub fn kind_slots(kind: &NodeKind, reach: Reach) -> Vec<Slot<'_>> {
    let (list, nodes, args) = held!(kind);
    let mut out = Vec::new();
    if reach.contains(Position::Ordered)
        && let Some((member, children)) = list
    {
        out.extend(children.iter().map(|node| Slot {
            member,
            position: Position::Ordered,
            node,
        }));
    }
    out.extend(
        nodes
            .into_iter()
            .filter(|(_, position, _)| reach.contains(*position))
            .map(|(member, position, node)| Slot {
                member,
                position,
                node,
            }),
    );
    if reach.contains(Position::Argument)
        && let Some(args) = args
    {
        out.extend(slot_trees(args).into_iter().map(|node| Slot {
            member: SLOT_ARG,
            position: Position::Argument,
            node,
        }));
    }
    out
}

/// Every position `node` holds another node in at `reach`, in ONE canonical
/// order: what the kind holds ([`kind_slots`]), then `state.onLoading`,
/// `state.onEmpty`, then the envelope `fallback`.
pub fn slots(node: &Node, reach: Reach) -> Vec<Slot<'_>> {
    let mut out = kind_slots(&node.kind, reach);
    if reach.contains(Position::StateArm) {
        out.extend(state_slots(&node.state));
    }
    if reach.contains(Position::Fallback)
        && let Some(n) = node.fallback.as_deref()
    {
        out.push(Slot {
            member: "Node.fallback",
            position: Position::Fallback,
            node: n,
        });
    }
    out
}

/// The alternatives a `state` block holds, `onLoading` then `onEmpty` — on a
/// node's envelope, or in an `UpdateState` op's new block.
pub fn state_slots(state: &StateBehaviour) -> Vec<Slot<'_>> {
    [
        ("StateBehaviour.onLoading", &state.on_loading),
        ("StateBehaviour.onEmpty", &state.on_empty),
    ]
    .into_iter()
    .filter_map(|(member, alternative)| {
        alternative.as_deref().map(|node| Slot {
            member,
            position: Position::StateArm,
            node,
        })
    })
    .collect()
}

/// The nodes a KIND holds at `reach` — [`kind_slots`] without the labels.
pub fn kind_children(kind: &NodeKind, reach: Reach) -> Vec<&Node> {
    kind_slots(kind, reach)
        .into_iter()
        .map(|s| s.node)
        .collect()
}

/// `node`'s children at `reach` — [`slots`] without the labels.
pub fn children(node: &Node, reach: Reach) -> Vec<&Node> {
    slots(node, reach).into_iter().map(|s| s.node).collect()
}

/// `node`'s children at `reach`, mutably, in [`slots`]'s order.
pub fn children_mut(node: &mut Node, reach: Reach) -> Vec<&mut Node> {
    let (list, nodes, args) = held!(&mut node.kind, mut);
    let mut out: Vec<&mut Node> = Vec::new();
    if reach.contains(Position::Ordered)
        && let Some((_, children)) = list
    {
        out.extend(children.iter_mut());
    }
    out.extend(
        nodes
            .into_iter()
            .filter(|(_, position, _)| reach.contains(*position))
            .map(|(_, _, n)| n),
    );
    if reach.contains(Position::Argument)
        && let Some(args) = args
    {
        out.extend(slot_trees_mut(args));
    }
    // `state` and `fallback` are fields distinct from `kind`; the borrows are
    // disjoint.
    if reach.contains(Position::StateArm) {
        out.extend(node.state.on_loading.as_deref_mut());
        out.extend(node.state.on_empty.as_deref_mut());
    }
    if reach.contains(Position::Fallback) {
        out.extend(node.fallback.as_deref_mut());
    }
    out
}

/// `node` with every child at `reach` replaced by `f` of it; every other
/// position, and the node's own slots, as they were.
pub fn map_children(node: &Node, reach: Reach, mut f: impl FnMut(&Node) -> Node) -> Node {
    let mut out = node.clone();
    for child in children_mut(&mut out, reach) {
        *child = f(child);
    }
    out
}

/// `node` with its children at `reach` replaced, in [`children`]'s order, by
/// `replacements`. Arity-preserving: a list of any other length answers `node`
/// unchanged, since a rebuild is only ever handed the list it read.
pub fn replace_children(node: &Node, reach: Reach, replacements: Vec<Node>) -> Node {
    let mut out = node.clone();
    let slots = children_mut(&mut out, reach);
    if slots.len() != replacements.len() {
        return node.clone();
    }
    for (slot, replacement) in slots.into_iter().zip(replacements) {
        *slot = replacement;
    }
    out
}

/// A container's resizable ordered list — the list the structural ops
/// (`InsertChild`, `RemoveNode`, `MoveNode`, `ReorderChildren`) insert into,
/// filter and permute — or `None` for a kind holding none. A `FragmentDecl`'s
/// body is [`Position::Ordered`] but is no list: it holds exactly one node.
pub fn ordered_list(kind: &NodeKind) -> Option<&Vec<Node>> {
    held!(kind).0.map(|(_, list)| list)
}

/// [`ordered_list`], mutably.
pub fn ordered_list_mut(kind: &mut NodeKind) -> Option<&mut Vec<Node>> {
    held!(kind, mut).0.map(|(_, list)| list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::NODE_SLOT_MEMBERS;

    #[test]
    fn the_slot_list_is_the_generated_slot_table() {
        let mine: Vec<&str> = SLOT_MEMBERS.iter().map(|(m, _)| *m).collect();
        assert_eq!(
            mine, NODE_SLOT_MEMBERS,
            "a node slot the IDL declares must be classed in SLOT_MEMBERS, and only those"
        );
    }

    #[test]
    fn the_named_reaches_nest() {
        for p in [
            Position::Ordered,
            Position::Arm,
            Position::StateArm,
            Position::Fallback,
            Position::Argument,
        ] {
            assert!(Reach::KEYED.contains(p));
            if Reach::STRUCTURAL.contains(p) {
                assert!(Reach::KIND_HELD.contains(p));
            }
            if Reach::KIND_HELD.contains(p) {
                assert!(Reach::FRAGMENT_SCOPE.contains(p));
            }
        }
        assert!(!Reach::FRAGMENT_SCOPE.contains(Position::Fallback));
        assert!(!Reach::FRAGMENT_SCOPE.contains(Position::Argument));
        assert_eq!(
            Reach::KEYED
                .without(Position::StateArm)
                .without(Position::Fallback),
            Reach::of(&[Position::Ordered, Position::Arm, Position::Argument])
        );
    }
}
