//! Decode-side resource limits for untrusted wire input (`WIRE_FORMAT.md` §21).
//!
//! # Why this exists, and why this host was the worst case
//!
//! §6 promises decoding is *total*: a malformed or hostile input yields a
//! structured, typed error, never a crash. That promise held on **semantics**
//! and was false on **shape**. `parse_value` / `parse_object` / `parse_array`
//! were unbounded mutual recursion, and the structural decoder recursed with no
//! counter — and a Rust stack overflow **aborts the process**. It is not a
//! catchable condition, so no `Result` could ever be returned. §21.2 rule 3
//! names exactly this.
//!
//! Measured before it was fixed, by bisection on the default main-thread stack:
//! the deepest surviving node decode was **7 unoptimised / 102 optimised**, and
//! nested `Batch` **22 / 296**. Against a max node depth of **24** that means
//! the unoptimised build died on a *conformant* document — one rule 1 requires
//! every host to accept — three times under the limit, in the configuration its
//! developers build in.
//!
//! So a depth counter alone did not bring this host into conformance: a guard at
//! 24 is never reached when the process is gone at 8. Per §21.4 the limit is a
//! protocol number and does not move for one host's frame size, so the work was
//! to make the decode walk affordable at 24 levels *and* to guard it. See
//! `decode.rs`'s note on the kind dispatch for the frame-size half.
//!
//! # The figures
//!
//! These are protocol limits, not tuning knobs. Changing one is a protocol
//! change — it moves in `WIRE_FORMAT.md` §21 and across every host, never here
//! alone.
//!
//! The two depth numbers are separate because neither derives from the other:
//! one tree level costs several JSON levels (a `Box` costs three — the node
//! object, its `children` array, the child object), and a rule-12 structured
//! payload nests freely *within* one node and consumes no node depth at all. A
//! host must never report a node-depth breach as a syntax-depth breach, because
//! that diagnosis sends the author to repair the wrong thing.

/// Maximum NODE nesting depth of a wire tree (the root is depth 1).
///
/// The same figure bounds `TreeOp::Batch` nesting in the op decoder — a
/// separate axis, counted on its own, held to the same ceiling.
pub const MAX_NODE_DEPTH: usize = 24;

/// Maximum SYNTACTIC JSON nesting depth (the outermost value is depth 1).
/// Every `{` and `[` counts, whether it carries a node, a spec, or a rule-12
/// payload.
pub const MAX_JSON_DEPTH: usize = 256;

/// Maximum length of a single decoded JSON string, in Unicode CODE POINTS
/// (§21.6).
///
/// Not bytes, and not UTF-16 code units. "Character" is not a unit, and the
/// five hosts counted in three: a 600 000-character CJK string is 600 000 code
/// points, 600 000 UTF-16 units and 1 800 000 UTF-8 bytes, so three hosts
/// accepted it and one refused it, each believing it enforced the same number.
/// Code points are the only candidate that is a property of the TEXT rather
/// than of a host's string representation or of the author's alphabet. A
/// surrogate pair counts as one, which costs nothing here: §20.2 row 6 refuses
/// an unpaired half at the parser, so every code point in a document this host
/// accepts is a Unicode scalar value.
///
/// Enforced in `canonical::json`'s string parser as the literal is built, per
/// §21.2 rule 4 — a bound checked on the finished string has already paid the
/// allocation it exists to refuse.
pub const MAX_STRING_LENGTH: usize = 1_048_576;

/// Maximum elements in a single JSON array, and members in a single JSON
/// object. Enforced in `canonical::json`'s array and object parsers, as the
/// container is built.
pub const MAX_ARRAY_LENGTH: usize = 100_000;

/// Maximum total node count of one document, summed across the whole tree.
///
/// Needed even once depth is bounded, because the depth, string and array
/// limits together still admit a document that is hostile by being **wide** —
/// 24 levels of 100 000 siblings is within every other limit. Its cost is
/// linear in the input, but the constant is not: a decoded tree is far larger
/// in memory than the bytes that produced it.
pub const MAX_NODES: usize = 100_000;

/// Maximum `ColExpr` nodes in ONE expression (§21.8).
///
/// The other limits bound the SIZE of a document; this one bounds what a host
/// must EVALUATE, which is why it exists beside them rather than being derived
/// from them. Counted per expression rather than per document — a tree may
/// carry many bounded expressions, with the whole still bounded by `MAX_NODES`.
///
/// Its scope is EVERY expression a decoded document can name (Phase 1662): a
/// `Binding::Expr`'s expression, and the `ColExpr` a `Binding::Transform`
/// pipeline embeds — a `derive`'s expression, a `filter`'s predicate. Those two
/// are the whole surface: `Filter` and `Derive` are the only `TransformStep`
/// arms carrying a `ColExpr`, and a `join` / `union` operand is a `DataSource`,
/// never another pipeline.
///
/// Until 1662 the pipeline surface was deliberately not covered, on the reading
/// that a pipeline's cost is already bounded by its own rows and steps. That
/// reading was wrong about the expression: a `derive`'s expression is evaluated
/// PER ROW and its own size is bounded by nothing, so saying the pipeline was
/// out of scope made this limit bypassable by wrapping the expression in a
/// Transform — the one shape from which a decoded document could still name an
/// unbounded evaluation. Closing it changes what an already-shipped decoder
/// accepts, so the refusal is stated in §21.8 rather than left to be read off
/// the code: a document past the bound is refused OUTRIGHT, with no profile
/// boundary and no grandfathering, because §21.2 rules 1 and 2 admit no second
/// acceptance class.
///
/// ONE budget for both surfaces, not two. The thing bounded is identical — the
/// evaluation named by one `ColExpr` — so a second constant would be one more
/// figure to keep in step across five hosts while refusing nothing this one
/// does not. And PER EXPRESSION rather than per pipeline: twenty `derive` steps
/// of ten nodes each are twenty cheap evaluations, not one expensive one.
pub const MAX_EXPR_NODES: usize = 512;

/// Maximum UTF-8 **byte** length of one whole input document (§21.7).
///
/// The five structural limits compose MULTIPLICATIVELY, and a document that
/// respects every one of them can still be arbitrarily large: 100 000 array
/// elements each holding a 1 048 576-code-point string is inside every bound
/// above and is a hundred gigabytes. Each individual check refuses nothing,
/// because each individual check is satisfied — the structural limits bound the
/// SHAPE of the walk, and nothing bounded its TOTAL.
///
/// Three things about it are normative and easy to get subtly wrong:
///
///  * **Bytes, not code points, and not this host's string length.** §21.6
///    bounds a VALUE the author wrote, so it is measured in units of text; this
///    bounds the CARRIAGE, which is what an attacker sends and what a host
///    allocates. Rust's `str::len()` is already UTF-8 bytes, so this host
///    measures it directly and has nothing to convert — the trap the rule warns
///    about is a UTF-16 host substituting its native length, which under-counts
///    a CJK document threefold, in the direction that ADMITS.
///  * **Before parsing.** It is one comparison on the input's length, so a host
///    that defers it has chosen to allocate the document twice for no benefit.
///  * **The path is `$`.** The breach is a property of the document, not of a
///    position inside it.
///
/// The figure is constrained from BELOW by [`MAX_NODES`]: a document at exactly
/// 100 000 nodes is about 8 MB of small nodes, so an 8 MiB ceiling — which
/// looks generous beside a 1 MiB string bound — would refuse a document rule 1
/// requires every host to ACCEPT, quietly lowering the node ceiling while
/// leaving its stated value in the table. 32 MiB leaves about 335 bytes per
/// node at that ceiling.
///
/// **The vector is HOST-LOCAL and deliberately not a corpus fixture** —
/// committing 32 MiB of padding to a shared repository to assert one integer
/// comparison is a poor trade, and unlike the depth bounds this is not a
/// recursion hazard. `tests/limits.rs` asserts the pair this host owes: one
/// byte over is refused, and a document at the node ceiling still decodes.
pub const MAX_DOCUMENT_BYTES: usize = 33_554_432;

use std::cell::Cell;

thread_local! {
    static NODE_DEPTH: Cell<usize> = const { Cell::new(0) };
    static NODE_COUNT: Cell<usize> = const { Cell::new(0) };
    static OP_DEPTH: Cell<usize> = const { Cell::new(0) };
    static TREE_ITEM_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Reset the walk counters. Called by each public decode entry point, so a walk
/// that returned early on an error never leaves a counter poisoned for the next
/// caller on this thread.
pub(crate) fn reset_walk() {
    NODE_DEPTH.with(|d| d.set(0));
    NODE_COUNT.with(|c| c.set(0));
    OP_DEPTH.with(|d| d.set(0));
    TREE_ITEM_DEPTH.with(|d| d.set(0));
}

/// Why `thread_local!` rather than a threaded `&mut` parameter or a plain
/// static.
///
/// A plain `static mut` would be a data race: `decode_node` is public API and
/// nothing stops two threads calling it at once. Threading a `&mut Walk` through
/// the decoder would be correct but touches every one of the ~100 per-kind spec
/// decoders and the whole dispatch, for state that all of them ignore.
///
/// A thread-local is race-free by construction — each thread has its own — and
/// costs no signature change. Re-entrancy on one thread is the only way it could
/// go wrong, and decoding never calls back into a public entry point, so the
/// reset at entry is sufficient rather than merely convenient.
pub(crate) struct NodeGuard;

impl NodeGuard {
    /// Enter one node level, refusing on the way DOWN (§21.2 rule 4) — before
    /// the recursion that would breach the bound, never afterwards by measuring
    /// the tree that was built. A check that runs after the walk it is meant to
    /// bound has already paid the cost it exists to refuse, and on a host whose
    /// overflow is fatal it never runs at all.
    ///
    /// Returns `Err(())` when a bound is breached; the caller turns that into a
    /// typed `LIMIT_EXCEEDED` with its own path.
    pub(crate) fn enter() -> Result<NodeGuard, LimitBreach> {
        let depth = NODE_DEPTH.with(|d| d.get());
        if depth >= MAX_NODE_DEPTH {
            return Err(LimitBreach::NodeDepth);
        }
        let count = NODE_COUNT.with(|c| c.get()) + 1;
        if count > MAX_NODES {
            return Err(LimitBreach::NodeCount);
        }
        NODE_COUNT.with(|c| c.set(count));
        NODE_DEPTH.with(|d| d.set(depth + 1));
        Ok(NodeGuard)
    }
}

impl Drop for NodeGuard {
    /// Popping in `Drop` rather than at each return is what makes the counter
    /// correct on the ERROR paths, which is most of them: the decoder is a long
    /// chain of `?` early-returns, and a hand-written decrement would have to be
    /// right at every one of them.
    fn drop(&mut self) {
        NODE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// The op axis, counted separately from the node axis.
///
/// §21.5's note for implementers: bounding the node decoder is **not**
/// sufficient. `Batch` makes the op decoder self-recursive on its own axis, and
/// the syntactic bound only *looks* like adequate cover for it — on the
/// reference host, 2.6 KB of nested Batches killed the process with every
/// node-side guard already in place. This host measured 22 levels unoptimised,
/// i.e. it aborted below the §21 limit on the op axis too.
pub(crate) struct OpGuard;

impl OpGuard {
    pub(crate) fn enter() -> Result<OpGuard, LimitBreach> {
        let depth = OP_DEPTH.with(|d| d.get());
        if depth >= MAX_NODE_DEPTH {
            return Err(LimitBreach::OpDepth);
        }
        OP_DEPTH.with(|d| d.set(depth + 1));
        Ok(OpGuard)
    }
}

impl Drop for OpGuard {
    fn drop(&mut self) {
        OP_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// The `Tree` ITEM axis, counted separately from both the node and the op axes.
///
/// WIRE_FORMAT.md §21.5, on the `TreeOp::Batch` precedent: a whole hierarchy of
/// `TreeItem` rows lives inside ONE node, so it consumes no node depth at all —
/// `NodeGuard` is entered once for the `Tree` node and never again however deep
/// the rows go. At roughly two JSON levels per row it is nowhere near the
/// syntactic bound either, so neither existing guard reaches it and a
/// self-referential shape with no counter of its own is unbounded recursion in
/// a decoder whose overflow ABORTS THE PROCESS.
///
/// Held to the same `MAX_NODE_DEPTH` ceiling, because that is a protocol number
/// rather than a per-host tuning knob.
pub(crate) struct TreeItemGuard;

impl TreeItemGuard {
    pub(crate) fn enter() -> Result<TreeItemGuard, LimitBreach> {
        let depth = TREE_ITEM_DEPTH.with(|d| d.get());
        if depth >= MAX_NODE_DEPTH {
            return Err(LimitBreach::TreeItemDepth);
        }
        TREE_ITEM_DEPTH.with(|d| d.set(depth + 1));
        Ok(TreeItemGuard)
    }
}

impl Drop for TreeItemGuard {
    fn drop(&mut self) {
        TREE_ITEM_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Which bound was breached. Carried separately from the error type so this
/// module takes no dependency on the wire crate's error shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LimitBreach {
    NodeDepth,
    NodeCount,
    OpDepth,
    TreeItemDepth,
}

impl LimitBreach {
    /// The message names the limit AND the observed bound, per §21.2 rule 2, so
    /// an author repairing the document knows which ceiling to come back under.
    pub(crate) fn message(self) -> String {
        match self {
            LimitBreach::NodeDepth => {
                format!("node nesting deeper than the wire limit MAX_NODE_DEPTH = {MAX_NODE_DEPTH}")
            }
            LimitBreach::NodeCount => {
                format!("the document holds more than the wire limit MAX_NODES = {MAX_NODES} nodes")
            }
            LimitBreach::OpDepth => {
                format!("op nesting deeper than the wire limit MAX_NODE_DEPTH = {MAX_NODE_DEPTH}")
            }
            LimitBreach::TreeItemDepth => format!(
                "Tree item nesting deeper than the wire limit MAX_NODE_DEPTH = {MAX_NODE_DEPTH}"
            ),
        }
    }

    pub(crate) fn expected(self) -> String {
        match self {
            LimitBreach::NodeDepth => {
                format!("a tree nesting nodes no more than {MAX_NODE_DEPTH} levels deep")
            }
            LimitBreach::NodeCount => {
                format!("a tree of no more than {MAX_NODES} nodes in total")
            }
            LimitBreach::OpDepth => {
                format!("a Batch nesting ops no more than {MAX_NODE_DEPTH} levels deep")
            }
            LimitBreach::TreeItemDepth => {
                format!("a Tree nesting items no more than {MAX_NODE_DEPTH} levels deep")
            }
        }
    }
}
