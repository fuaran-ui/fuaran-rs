//! The capability gate — default-deny dispatch over a marketplace of mounted
//! mini-apps (the Bazaar mechanic). Each `Mount` declares the capabilities its
//! mini-app needs (`MountSpec.capabilities`); the host holds a grant set; the
//! gate decides, per mount and per capability-scoped action, whether dispatch is
//! permitted — **denying by default** anything the host has not explicitly
//! granted. The decode-reject codec is the *structural* gate (a hostile payload
//! never decodes); this is the *authority* gate on top of it (a well-formed
//! mini-app still only reaches the capabilities it was granted).

use std::collections::BTreeSet;

use crate::introspect::all_nodes;
use crate::wire::{Action, MountSpec, Node, NodeKind};

/// The gate's verdict for one mount or action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Dispatch is permitted — every required capability is granted.
    Allow,
    /// Dispatch is refused; the carried list names the ungranted capabilities.
    Deny { missing: Vec<String> },
}

impl GateDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, GateDecision::Allow)
    }
}

/// A host authority context — the set of capability ids the host grants. The
/// default (`CapabilityGate::default`) grants nothing: default-deny.
#[derive(Debug, Clone, Default)]
pub struct CapabilityGate {
    grants: BTreeSet<String>,
}

impl CapabilityGate {
    /// A gate granting exactly the given capability ids.
    pub fn granting<I, S>(caps: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        CapabilityGate {
            grants: caps.into_iter().map(Into::into).collect(),
        }
    }

    /// Grant one more capability (builder-style).
    pub fn grant(mut self, cap: impl Into<String>) -> Self {
        self.grants.insert(cap.into());
        self
    }

    /// `true` when this exact capability id is granted.
    pub fn allows(&self, capability_id: &str) -> bool {
        self.grants.contains(capability_id)
    }

    /// Decide a single capability id.
    pub fn decide_capability(&self, capability_id: &str) -> GateDecision {
        if self.allows(capability_id) {
            GateDecision::Allow
        } else {
            GateDecision::Deny {
                missing: vec![capability_id.to_string()],
            }
        }
    }

    /// Decide a mount: `Allow` iff **every** declared capability is granted;
    /// otherwise `Deny` naming exactly the ungranted ones (deduped, sorted).
    pub fn decide_mount(&self, mount: &MountSpec) -> GateDecision {
        let missing: BTreeSet<String> = mount
            .capabilities
            .iter()
            .filter(|c| !self.allows(c))
            .cloned()
            .collect();
        if missing.is_empty() {
            GateDecision::Allow
        } else {
            GateDecision::Deny {
                missing: missing.into_iter().collect(),
            }
        }
    }

    /// Decide an action. A capability-scoped `Invoke` is gated on its id; a
    /// `Chain` is allowed only if every sub-action is (denied with the union of
    /// missing ids); every non-capability action is outside this gate's scope
    /// (`Allow` — its safety is the codec's inert-decode + the runtime's own
    /// seams, not an authority grant).
    pub fn decide_action(&self, action: &Action) -> GateDecision {
        match action {
            Action::Invoke { capability_id, .. } => self.decide_capability(capability_id),
            Action::Chain(actions) => {
                let mut missing: BTreeSet<String> = BTreeSet::new();
                for a in actions {
                    if let GateDecision::Deny { missing: m } = self.decide_action(a) {
                        missing.extend(m);
                    }
                }
                if missing.is_empty() {
                    GateDecision::Allow
                } else {
                    GateDecision::Deny {
                        missing: missing.into_iter().collect(),
                    }
                }
            }
            _ => GateDecision::Allow,
        }
    }

    /// Audit a whole tree: the decision for every `Mount` node, in document
    /// order — the "which marketplace mini-apps are live vs blocked" view the
    /// Bazaar renders. A blocked mount's mini-app is never mounted.
    pub fn audit_mounts(&self, tree: &Node) -> Vec<(String, GateDecision)> {
        all_nodes(tree)
            .into_iter()
            .filter_map(|n| match &n.kind {
                NodeKind::Mount(spec) => Some((n.id.clone(), self.decide_mount(spec))),
                _ => None,
            })
            .collect()
    }
}
