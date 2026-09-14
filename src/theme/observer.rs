//! The observer implementation — the PURE tier only.
//!
//! **Headless boundary (deliberate, do not regress).** The sibling Python and F#
//! hosts also ship a BROWSER observer that reads LIVE `getComputedStyle` under a
//! live `MutationObserver`. That read-back is a host concern, not a derivation
//! one, and it is deliberately NOT in this crate — the same boundary the Go host
//! records. What ships here is [`InMemoryStyleObserver`]: it consumes SUPPLIED
//! resolved-style facts (a [`StyleInput`] per node), never a live DOM.
//!
//! That is not a reduced surface. Everything a browser observer computes from
//! live style is identical once the resolved colours are supplied — the
//! derivation core in [`crate::theme::flags`] is shared — so a `wasm32` client
//! that reads `getComputedStyle` on the JS side and hands the facts across the
//! C-ABI gets exactly the observations, and exactly the bytes, a Go or Python
//! server would produce for the same facts. The pure tier is the complete
//! substrate-free surface; the read-back is a thin, untestable-in-Rust shim
//! above it.
//!
//! **The parent pointer is a TREE pointer, not a compositing one.** A registered
//! parent decides the [`InMemoryStyleObserver::observe_tree`] walk and nothing
//! else. Background layers compose from the caller-supplied
//! [`StyleInput::background_layers`] stack — element-first, ancestors outward —
//! exactly as in the sibling hosts, where a browser observer walks
//! `parentElement` to BUILD that stack before handing it to the derivation.
//! Deriving the stack from the parent pointer instead would make this host
//! disagree with every other one about the same fixture.

use std::collections::HashMap;

use crate::theme::flags::{
    StyleInput, StyleObservation, StyleObserverOptions, baseline_style_input, flags_equal,
    to_style_observation,
};
use crate::theme::manifest::ThemeManifest;
use crate::theme::manifest_flags::per_node_flags;

/// A subscriber handle, returned by [`InMemoryStyleObserver::subscribe`] and
/// consumed by [`InMemoryStyleObserver::unsubscribe`].
///
/// The sibling hosts return an unsubscribe *closure*; an owned token is the
/// Rust spelling of the same contract — a closure that borrowed the observer
/// mutably would keep it borrowed for as long as the handle lived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

/// The subscriber callback: `(node_id, observation)` on each emission.
pub type Subscriber = Box<dyn FnMut(&str, &StyleObservation)>;

struct FixtureEntry {
    input: StyleInput,
    parent: Option<String>,
}

/// The fixture-driven observer — substrate-free, for tests and non-browser
/// hosts. Registration order is retained, so the tree walk is deterministic
/// rather than hash-ordered.
#[derive(Default)]
pub struct InMemoryStyleObserver {
    options: StyleObserverOptions,
    manifest: Option<ThemeManifest>,
    registry: HashMap<String, FixtureEntry>,
    /// Registration order — the deterministic child order the BFS walks.
    order: Vec<String>,
    last_flags: HashMap<String, Vec<crate::theme::flags::StyleFlag>>,
    subscribers: Vec<(SubscriptionId, Subscriber)>,
    next_sub_id: u64,
}

impl std::fmt::Debug for InMemoryStyleObserver {
    /// Hand-written because a [`Subscriber`] is a boxed closure and cannot
    /// derive `Debug`; the count is the part a reader needs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryStyleObserver")
            .field("options", &self.options)
            .field("has_manifest", &self.manifest.is_some())
            .field("registered", &self.order.len())
            .field("subscribers", &self.subscribers.len())
            .finish()
    }
}

impl InMemoryStyleObserver {
    /// Construct an observer with the given options and an optional manifest
    /// (`None` for the manifest-free tier).
    pub fn new(options: StyleObserverOptions, manifest: Option<ThemeManifest>) -> Self {
        InMemoryStyleObserver {
            options,
            manifest,
            registry: HashMap::new(),
            order: Vec::new(),
            last_flags: HashMap::new(),
            subscribers: Vec::new(),
            next_sub_id: 0,
        }
    }

    /// Derive an observation, appending the manifest-aware flags when a manifest
    /// is wired. The manifest-free flags always come first — the list is
    /// compared and encoded as a sequence.
    fn to_obs(&self, node_id: &str, input: &StyleInput) -> StyleObservation {
        let mut obs = to_style_observation(&self.options, node_id, input);
        if let Some(manifest) = &self.manifest {
            obs.flags.extend(per_node_flags(manifest, &obs));
        }
        obs
    }

    fn emit(&mut self, node_id: &str, obs: &StyleObservation) {
        for (_, handler) in self.subscribers.iter_mut() {
            handler(node_id, obs);
        }
    }

    /// Register or replace a fixture; fires an initial emission
    /// unconditionally (the first observation of a node is never a "change").
    /// `None` as the parent registers a root.
    pub fn register_fixture(&mut self, node_id: &str, input: StyleInput, parent: Option<&str>) {
        if !self.registry.contains_key(node_id) {
            self.order.push(node_id.to_string());
        }
        let obs = self.to_obs(node_id, &input);
        self.registry.insert(
            node_id.to_string(),
            FixtureEntry {
                input,
                parent: parent.map(str::to_string),
            },
        );
        self.last_flags
            .insert(node_id.to_string(), obs.flags.clone());
        self.emit(node_id, &obs);
    }

    /// Replace a registered node's input, honouring
    /// [`StyleObserverOptions::emit_on_flag_change_only`]. A no-op when the node
    /// is not registered.
    pub fn update(&mut self, node_id: &str, input: StyleInput) {
        let Some(existing) = self.registry.get(node_id) else {
            return;
        };
        let parent = existing.parent.clone();
        let obs = self.to_obs(node_id, &input);
        self.registry
            .insert(node_id.to_string(), FixtureEntry { input, parent });
        let previous = self
            .last_flags
            .insert(node_id.to_string(), obs.flags.clone())
            .unwrap_or_default();
        let should_emit =
            !self.options.emit_on_flag_change_only || !flags_equal(&obs.flags, &previous);
        if should_emit {
            self.emit(node_id, &obs);
        }
    }

    /// The current observation for a node, or `None` when it is not registered.
    pub fn observe(&self, node_id: &str) -> Option<StyleObservation> {
        let entry = self.registry.get(node_id)?;
        Some(self.to_obs(node_id, &entry.input))
    }

    /// The root and every descendant, breadth-first, children in registration
    /// order. Empty when the root is not registered.
    pub fn observe_tree(&self, root_id: &str) -> Vec<StyleObservation> {
        if !self.registry.contains_key(root_id) {
            return Vec::new();
        }
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
        for node_id in &self.order {
            if let Some(parent) = self.registry[node_id].parent.as_deref() {
                children.entry(parent).or_default().push(node_id);
            }
        }
        let mut acc = Vec::new();
        let mut queue: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
        queue.push_back(root_id);
        while let Some(node_id) = queue.pop_front() {
            if let Some(entry) = self.registry.get(node_id) {
                acc.push(self.to_obs(node_id, &entry.input));
            }
            if let Some(kids) = children.get(node_id) {
                queue.extend(kids.iter().copied());
            }
        }
        acc
    }

    /// Register a handler; the returned [`SubscriptionId`] removes it again.
    pub fn subscribe(&mut self, handler: Subscriber) -> SubscriptionId {
        let id = SubscriptionId(self.next_sub_id);
        self.next_sub_id += 1;
        self.subscribers.push((id, handler));
        id
    }

    /// Remove a handler. Returns whether one was removed, so an
    /// already-unsubscribed handle is reported rather than silently accepted.
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        let before = self.subscribers.len();
        self.subscribers.retain(|(existing, _)| *existing != id);
        self.subscribers.len() != before
    }

    /// Create a baseline entry with no fixture, so a mount hook that registers
    /// before any style is resolved does not fail. A no-op when the node is
    /// already registered.
    pub fn register(&mut self, node_id: &str) {
        if !self.registry.contains_key(node_id) {
            self.register_fixture(node_id, baseline_style_input(), None);
        }
    }

    /// Remove a node and its cached flags. Its children keep pointing at it, so
    /// they drop out of the walk with it — the same shape the sibling hosts have.
    pub fn unregister(&mut self, node_id: &str) {
        self.registry.remove(node_id);
        self.last_flags.remove(node_id);
        self.order.retain(|id| id != node_id);
    }
}
