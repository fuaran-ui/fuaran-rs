//! The signature-searchable function registry (Phase 558) — the Rust host of the
//! F# reference `Fuaran.Core.FunctionRegistry.findBySignature` (Phase 50/512)
//! plus deterministic compose-path resolution (the twin of the Python
//! `fuaran_py.function` registry, Phase 523).
//!
//! Composition-by-lookup, not composition-by-generation: register functions by
//! the node-kind they *produce* and the typed *holes* they require, then ask the
//! registry "what can I run to produce X with the context I have?" — a total,
//! in-memory structural search, no model call, no server — and compose a result
//! by chaining matched functions rather than prompting. This is the Pattern
//! Bank's deterministic no-model-call fast path.
//!
//! Reference semantics (canonical = F#):
//! - a query is `(result_type, available)` — the node-kind to produce (`None` =
//!   any) plus the context holes on offer; only a function's REQUIRED holes gate
//!   a match; matching is by absolute address.
//! - [`MatchMode::Subsumes`] — result type matches (or wildcard) and every
//!   required hole is satisfiable from context (`available ⊆ required` for value
//!   spaces, a slot-kind match for slots).
//! - [`MatchMode::Exact`] — the required-hole address set equals the context set
//!   and each pair is shape-equal (kind + space + slot).
//! - candidates return in deterministic lexicographic id order (no ranking).
//! - a compose that cannot reach the target returns a typed [`ComposeResult::NoPath`],
//!   never a guess — the closed wire outcomes modelled as a native, exhaustive
//!   `enum`.
//!
//! Certified against the shared `wire-format-fixtures/function-registry` goldens —
//! shape-identical resolution across the F#, py, ts, go, rs hosts. NOTE on the one
//! host divergence: the F# reference `spaceSubsumes` treats an `AnyString`
//! required space as subsuming an `Enum` available; the Python host does not. This
//! host follows the F# reference (the canonical semantics); the shared goldens
//! deliberately avoid that single edge so every host agrees on every fixture.

use std::collections::{BTreeMap, BTreeSet};

/// A hole's value-space — the type domain of a value argument.
#[derive(Clone, Debug, PartialEq)]
pub enum Space {
    /// A bounded integer range.
    IntRange { min: i64, max: i64 },
    /// A bounded floating-point range.
    FloatRange { min: f64, max: f64 },
    /// A bounded string-length range.
    StringLen { min: i64, max: i64 },
    /// A closed set of string choices.
    Enum { choices: Vec<String> },
    /// Any string (the only unbounded space).
    AnyString,
}

/// The role a hole plays in a function's signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoleKind {
    /// A typed scalar value.
    Value,
    /// A tree-typed slot (higher-order), carrying an optional node-kind constraint.
    Slot,
    /// A bounded repeat.
    Repeat,
    /// A dispatch/handler slot (the behaviour axis).
    Action,
}

/// One hole in a function signature — matched by absolute [`SigEntry::addr`]
/// (hygiene). A value/repeat hole carries a [`Space`]; a slot hole carries a
/// node-kind [`SigEntry::slot`] constraint. Twin of F# `SigEntry`.
#[derive(Clone, Debug, PartialEq)]
pub struct SigEntry {
    /// The absolute lexical address (id-path) the hole binds by.
    pub addr: String,
    /// The human label.
    pub name: String,
    /// The hole role.
    pub kind: HoleKind,
    /// The value-space (value/repeat holes); `None` for slot/action holes.
    pub space: Option<Space>,
    /// The slot node-kind constraint (slot holes); `None` when unconstrained / not a slot.
    pub slot: Option<String>,
    /// Whether the hole gates a match (only required holes do).
    pub required: bool,
}

/// A registered function: an id, the node-kind it *produces*
/// ([`FunctionEntry::result_type`]), and its required-hole shape. Twin of F#
/// `FunctionEntry`.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionEntry {
    /// The function id (stable identity).
    pub id: String,
    /// The node-kind the function produces.
    pub result_type: String,
    /// The declared holes.
    pub holes: Vec<SigEntry>,
}

/// How strictly an entry's signature must match a query. Twin of F# `MatchMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchMode {
    /// "Everything I can run with this context" — structural subsumption.
    Subsumes,
    /// "The function with precisely these holes" — exact shape match.
    Exact,
}

/// A signature search: the node-kind to produce (`None` = any, a produce-axis
/// wildcard) plus the context holes on offer. Twin of F# `SignatureQuery`.
#[derive(Clone, Debug, PartialEq)]
pub struct SignatureQuery {
    /// The desired result type, or `None` for a wildcard.
    pub result_type: Option<String>,
    /// The context holes the caller can fill.
    pub available: Vec<SigEntry>,
}

/// One function applied in a composition — its id + the slot it fills (`None` at
/// the root).
#[derive(Clone, Debug, PartialEq)]
pub struct ComposeStep {
    /// The applied function's id.
    pub function_id: String,
    /// The slot address this step fills (`None` at the composition root).
    pub fills_slot: Option<String>,
}

/// The outcome of a compose — a native, exhaustive sum of the closed wire cases
/// (the Rust host recovers the compile-time exhaustiveness a sum-type-free host
/// trades away). Twin of the Python `ComposePath` / `NoPath`.
#[derive(Clone, Debug, PartialEq)]
pub enum ComposeResult {
    /// A deterministic composition reaching the target — the ordered steps, root last.
    ComposePath(Vec<ComposeStep>),
    /// No deterministic chain reaches the target (typed, not a guess) — a reason.
    NoPath(String),
}

/// A signature-typed function registry — the artifact-function catalogue, queried
/// by signature. The `by_result` index maps a produced node-kind to the ids
/// producing it, so a "produces a Box" query narrows before the hole-shape filter
/// runs. Twin of F# `FunctionRegistry`.
#[derive(Clone, Debug, Default)]
pub struct FunctionRegistry {
    entries: BTreeMap<String, FunctionEntry>,
    by_result: BTreeMap<String, BTreeSet<String>>,
}

impl FunctionRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an entry — additive, no silent overwrite (a duplicate id is a
    /// named `Err`). Maintains both the id map and the result-type index. Twin of
    /// F# `FunctionRegistry::register`.
    ///
    /// # Errors
    /// Returns the id when an entry with the same id is already registered.
    pub fn register(&mut self, entry: FunctionEntry) -> Result<(), String> {
        if self.entries.contains_key(&entry.id) {
            return Err(format!("function '{}' is already registered", entry.id));
        }
        self.by_result
            .entry(entry.result_type.clone())
            .or_default()
            .insert(entry.id.clone());
        self.entries.insert(entry.id.clone(), entry);
        Ok(())
    }

    /// Look up a registered entry by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&FunctionEntry> {
        self.entries.get(id)
    }

    /// Find every registered function whose signature matches the query under
    /// `mode`. A `Some` result type narrows via the `by_result` index first; a
    /// `None` result type scans all entries. Survivors are returned id-stable
    /// (lexicographic — `BTreeMap`/`BTreeSet` iterate sorted). Twin of F#
    /// `FunctionRegistry::findBySignature`.
    #[must_use]
    pub fn find_by_signature(
        &self,
        mode: MatchMode,
        query: &SignatureQuery,
    ) -> Vec<&FunctionEntry> {
        let candidates: Vec<&FunctionEntry> = match &query.result_type {
            Some(rt) => match self.by_result.get(rt) {
                Some(ids) => ids.iter().filter_map(|id| self.entries.get(id)).collect(),
                None => Vec::new(),
            },
            None => self.entries.values().collect(),
        };
        candidates
            .into_iter()
            .filter(|e| matches_query(mode, query, e))
            .collect()
    }

    /// Chain functions to produce `output` from the `inputs` context
    /// deterministically ([`ComposeResult::ComposePath`], root last), or return a
    /// typed [`ComposeResult::NoPath`]. A direct signature match is a single step;
    /// an unfilled slot hole is recursively composed from the same context. No
    /// model call, no guess. Twin of the Python `FunctionRegistry.compose`.
    #[must_use]
    pub fn compose(
        &self,
        output: &str,
        inputs: &[SigEntry],
        mode: MatchMode,
        max_depth: i32,
    ) -> ComposeResult {
        match self.compose_steps(output, inputs, mode, max_depth, &BTreeSet::new()) {
            Some(steps) => ComposeResult::ComposePath(steps),
            None => ComposeResult::NoPath(format!(
                "no deterministic function chain reaches '{output}' from the given context"
            )),
        }
    }

    fn compose_steps(
        &self,
        output: &str,
        available: &[SigEntry],
        mode: MatchMode,
        depth: i32,
        seen: &BTreeSet<String>,
    ) -> Option<Vec<ComposeStep>> {
        if depth <= 0 || seen.contains(output) {
            return None;
        }

        // Direct match: a function producing `output` whose every required hole is in context.
        let query = SignatureQuery {
            result_type: Some(output.to_string()),
            available: available.to_vec(),
        };
        if let Some(first) = self.find_by_signature(mode, &query).first() {
            return Some(vec![ComposeStep {
                function_id: first.id.clone(),
                fills_slot: None,
            }]);
        }

        let mut seen_next = seen.clone();
        seen_next.insert(output.to_string());
        let by_addr: BTreeMap<&str, &SigEntry> =
            available.iter().map(|e| (e.addr.as_str(), e)).collect();

        // Otherwise: a producer whose only unmet required holes are slots we can compose.
        let producers = self.by_result.get(output)?;
        for id in producers {
            let entry = &self.entries[id];
            let mut sub: Vec<ComposeStep> = Vec::new();
            let mut satisfiable = true;
            for hole in entry.holes.iter().filter(|h| h.required) {
                if let Some(av) = by_addr.get(hole.addr.as_str()).copied() {
                    if hole_satisfied(hole, av) {
                        continue;
                    }
                }
                match (&hole.kind, &hole.slot) {
                    (HoleKind::Slot, Some(slot)) => {
                        match self.compose_steps(slot, available, mode, depth - 1, &seen_next) {
                            Some(mut child) => {
                                if let Some(root) = child.last_mut() {
                                    root.fills_slot = Some(hole.addr.clone());
                                }
                                sub.extend(child);
                            }
                            None => {
                                satisfiable = false;
                                break;
                            }
                        }
                    }
                    _ => {
                        satisfiable = false;
                        break;
                    }
                }
            }
            if satisfiable {
                sub.push(ComposeStep {
                    function_id: id.clone(),
                    fills_slot: None,
                });
                return Some(sub);
            }
        }
        None
    }

    /// The canonical per-entry shape descriptors of the registry, sorted — the
    /// 548-style attestation surface. A host whose registry model drops a hole
    /// field, reorders holes, or mistypes a space produces a divergent
    /// descriptor, so a cross-host shape drift fails the conformance gate with the
    /// entry named, rather than silently diverging.
    #[must_use]
    pub fn registry_signature_shape(&self) -> Vec<String> {
        let mut out: Vec<String> = self.entries.values().map(entry_desc).collect();
        out.sort();
        out
    }
}

// ── value-space + slot subsumption (available ⊆ required) ─────────────────────

/// Does `required` value-space subsume `available` — is every value the context
/// can supply acceptable to the function? (`available ⊆ required`.) Twin of F#
/// `spaceSubsumes` (the canonical reference).
#[must_use]
pub fn space_subsumes(required: &Space, available: &Space) -> bool {
    match (required, available) {
        (Space::IntRange { min: rl, max: rh }, Space::IntRange { min: al, max: ah })
        | (Space::StringLen { min: rl, max: rh }, Space::StringLen { min: al, max: ah }) => {
            rl <= al && ah <= rh
        }
        (Space::FloatRange { min: rl, max: rh }, Space::FloatRange { min: al, max: ah }) => {
            rl <= al && ah <= rh
        }
        (Space::Enum { choices: rs }, Space::Enum { choices: als }) => {
            als.iter().all(|v| rs.contains(v))
        }
        (Space::AnyString, Space::StringLen { .. } | Space::Enum { .. } | Space::AnyString) => true,
        _ => false,
    }
}

fn slot_subsumes(required: &Option<String>, available: &Option<String>) -> bool {
    match required {
        None => true,
        Some(rk) => available.as_deref() == Some(rk.as_str()),
    }
}

fn hole_satisfied(req: &SigEntry, av: &SigEntry) -> bool {
    if req.kind != av.kind {
        return false;
    }
    if req.kind == HoleKind::Slot {
        return slot_subsumes(&req.slot, &av.slot);
    }
    match (&req.space, &av.space) {
        (Some(rs), Some(avs)) => space_subsumes(rs, avs),
        _ => false,
    }
}

fn matches_query(mode: MatchMode, query: &SignatureQuery, entry: &FunctionEntry) -> bool {
    if let Some(rt) = &query.result_type {
        if rt != &entry.result_type {
            return false;
        }
    }
    let avail_by_addr: BTreeMap<&str, &SigEntry> = query
        .available
        .iter()
        .map(|e| (e.addr.as_str(), e))
        .collect();
    let required: Vec<&SigEntry> = entry.holes.iter().filter(|h| h.required).collect();

    match mode {
        MatchMode::Subsumes => required.iter().all(|req| {
            avail_by_addr
                .get(req.addr.as_str())
                .copied()
                .is_some_and(|av| hole_satisfied(req, av))
        }),
        MatchMode::Exact => {
            if required.len() != query.available.len() {
                return false;
            }
            required.iter().all(|req| {
                avail_by_addr
                    .get(req.addr.as_str())
                    .copied()
                    .is_some_and(|av| {
                        req.kind == av.kind && req.space == av.space && req.slot == av.slot
                    })
            })
        }
    }
}

// ── registry-shape attestation descriptors ────────────────────────────────────

fn space_desc(space: &Option<Space>) -> String {
    match space {
        None => "-".to_string(),
        Some(Space::IntRange { min, max }) => format!("intRange({min},{max})"),
        Some(Space::FloatRange { min, max }) => format!("floatRange({min},{max})"),
        Some(Space::StringLen { min, max }) => format!("stringLen({min},{max})"),
        Some(Space::Enum { choices }) => format!("enum({})", choices.join("|")),
        Some(Space::AnyString) => "anyString".to_string(),
    }
}

fn kind_str(kind: HoleKind) -> &'static str {
    match kind {
        HoleKind::Value => "value",
        HoleKind::Slot => "slot",
        HoleKind::Repeat => "repeat",
        HoleKind::Action => "action",
    }
}

fn hole_desc(h: &SigEntry) -> String {
    let slot = h.slot.as_deref().unwrap_or("-");
    let req = if h.required { "req" } else { "opt" };
    format!(
        "{}:{}:{}:{}:{}",
        h.addr,
        kind_str(h.kind),
        space_desc(&h.space),
        slot,
        req
    )
}

fn entry_desc(e: &FunctionEntry) -> String {
    let holes: Vec<String> = e.holes.iter().map(hole_desc).collect();
    format!("{}|{}|{}", e.id, e.result_type, holes.join(";"))
}
