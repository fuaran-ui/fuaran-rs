//! `WIRE_FORMAT.md` §21 resource limits.
//!
//! This host was the worst case of the five, and the tests are shaped by that.
//! Its hand-rolled parser and its structural decoder were unbounded recursion,
//! and a Rust stack overflow **aborts the process** — not a catchable condition,
//! so no `Result` could ever be returned (§21.2 rule 3's exact prohibition).
//!
//! Worse, it could not decode a document rule 1 requires every conformant host
//! to ACCEPT: measured on the default main-thread stack, the deepest surviving
//! node decode was **7** unoptimised against a limit of 24, and nested `Batch`
//! **22**. A depth guard alone could not fix that — a guard at 24 is never
//! reached when the process is already gone at 9 — so the dispatch was split
//! into small sequential groups first (see `decode_node_kind`'s note) and
//! guarded second. After both, the same measurement clears 90 levels on both
//! axes in an unoptimised build.
//!
//! Every bound is therefore asserted from BOTH sides of its boundary. The
//! at-the-limit cases are not padding: they are the half this host actually
//! failed, and a refusal-only suite would have passed throughout.

use fuaran_rs::limits::{MAX_JSON_DEPTH, MAX_NODE_DEPTH};
use fuaran_rs::wire::{decode_node, decode_op};

const BOX_OPEN: &str = r#"{"id":"n","kind":{"$type":"Box","role":"Group","layout":{"$type":"Flex","direction":"Vertical","wrap":false},"children":["#;
const BOX_LEAF: &str = r#"{"id":"leaf","kind":{"$type":"Box","role":"Group","layout":{"$type":"Flex","direction":"Vertical","wrap":false},"children":[]}}"#;

/// A chain of `n` nested Box nodes, innermost an empty Box.
fn nested_nodes(n: usize) -> String {
    let mut s = String::new();
    for _ in 0..n - 1 {
        s.push_str(BOX_OPEN);
    }
    s.push_str(BOX_LEAF);
    for _ in 0..n - 1 {
        s.push_str("]}}");
    }
    s
}

/// A chain of `n` nested `Batch` ops, innermost a `RemoveNode`.
fn nested_batch(n: usize) -> String {
    let mut s = String::new();
    for _ in 0..n - 1 {
        s.push_str(r#"{"$type":"Batch","ops":["#);
    }
    s.push_str(r#"{"$type":"RemoveNode","target":"x"}"#);
    for _ in 0..n - 1 {
        s.push_str("]}");
    }
    s
}

// ── the node-depth bound ────────────────────────────────────────────────────

#[test]
fn accepts_a_tree_at_exactly_max_node_depth() {
    // Rule 1 — refusing a conformant document is non-conformance, not caution.
    // This is the assertion this host used to fail by ABORTING, three times
    // under the limit.
    let r = decode_node(&nested_nodes(MAX_NODE_DEPTH));
    assert!(r.is_ok(), "a tree at exactly the limit must decode: {r:?}");
}

#[test]
fn refuses_a_tree_one_level_past_max_node_depth() {
    let e = decode_node(&nested_nodes(MAX_NODE_DEPTH + 1)).unwrap_err();
    assert_eq!(e.code.as_str(), "LIMIT_EXCEEDED");
    // Rule 2 — a limit breach is not a syntax error.
    assert_ne!(e.code.as_str(), "INVALID_JSON");
    assert!(
        e.message.contains(&MAX_NODE_DEPTH.to_string()),
        "the message should name the bound: {}",
        e.message
    );
}

#[test]
fn a_far_over_limit_tree_returns_rather_than_aborting() {
    // The original defect in one line. Reaching this assertion at all is the
    // result: before the fix the process died and no assertion ran.
    let e = decode_node(&nested_nodes(400)).unwrap_err();
    assert_eq!(e.code.as_str(), "LIMIT_EXCEEDED");
}

// ── the op-decoder axis ─────────────────────────────────────────────────────

#[test]
fn accepts_nested_batch_at_exactly_max_node_depth() {
    let r = decode_op(&nested_batch(MAX_NODE_DEPTH));
    assert!(r.is_ok(), "nested Batch at the limit must decode: {r:?}");
}

#[test]
fn refuses_nested_batch_one_level_past_the_limit() {
    let e = decode_op(&nested_batch(MAX_NODE_DEPTH + 1)).unwrap_err();
    assert_eq!(e.code.as_str(), "LIMIT_EXCEEDED");
}

#[test]
fn the_op_axis_is_counted_separately_from_the_node_axis() {
    // A Batch chain at the op limit whose payload node is at the node limit
    // must decode. If the two shared one counter this would breach at the sum —
    // the plausible wrong implementation every other assertion here still
    // passes.
    let inner = format!(
        r#"{{"$type":"ReplaceRoot","node":{}}}"#,
        nested_nodes(MAX_NODE_DEPTH)
    );
    let mut doc = String::new();
    for _ in 0..MAX_NODE_DEPTH - 1 {
        doc.push_str(r#"{"$type":"Batch","ops":["#);
    }
    doc.push_str(&inner);
    for _ in 0..MAX_NODE_DEPTH - 1 {
        doc.push_str("]}");
    }
    let r = decode_op(&doc);
    assert!(r.is_ok(), "the two axes must be counted separately: {r:?}");
}

// ── the syntactic bound ─────────────────────────────────────────────────────

#[test]
fn refuses_bare_nesting_past_max_json_depth() {
    let n = MAX_JSON_DEPTH + 1;
    let doc = "[".repeat(n) + &"]".repeat(n);
    let e = decode_node(&doc).unwrap_err();
    assert_eq!(e.code.as_str(), "LIMIT_EXCEEDED");
}

#[test]
fn bare_nesting_at_exactly_max_json_depth_fails_on_shape_not_on_the_limit() {
    // Not a valid node, so it must fail — but on SHAPE. This is what stops the
    // syntactic guard sitting one level too tight, which a refusal-only test
    // could never detect.
    let n = MAX_JSON_DEPTH;
    let doc = "[".repeat(n) + &"]".repeat(n);
    let e = decode_node(&doc).unwrap_err();
    assert_ne!(e.code.as_str(), "LIMIT_EXCEEDED");
}

#[test]
fn still_calls_genuinely_malformed_input_invalid_json() {
    // Non-vacuity for the classification: it must distinguish, not relabel.
    let e = decode_node("}{ not json").unwrap_err();
    assert_eq!(e.code.as_str(), "INVALID_JSON");
}

// ── the counters do not leak between calls ──────────────────────────────────

#[test]
fn a_refused_decode_does_not_poison_the_next() {
    // The counters are thread-locals popped in `Drop`, which is what makes them
    // correct on the error paths — and the error paths are most of them, since
    // the decoder is a long chain of `?` early returns.
    assert!(decode_node(&nested_nodes(MAX_NODE_DEPTH + 1)).is_err());
    assert!(decode_node(&nested_nodes(MAX_NODE_DEPTH)).is_ok());
}

#[test]
fn a_shape_failure_does_not_poison_the_next_decode() {
    assert!(decode_node(r#"{"id":"x","kind":{"$type":"NoSuchKind"}}"#).is_err());
    assert!(decode_node(&nested_nodes(MAX_NODE_DEPTH)).is_ok());
}

#[test]
fn concurrent_decodes_do_not_share_counters() {
    // `decode_node` is public API and nothing stops two threads calling it at
    // once, which is why the counters are thread-locals rather than statics.
    // Half the threads decode a conformant tree and must succeed; half decode an
    // over-limit one and must be refused. Shared state makes the two interfere
    // in both directions, so this asserts on the RESULTS rather than relying on
    // a race detector being available.
    let handles: Vec<_> = (0..32)
        .map(|i| {
            std::thread::spawn(move || {
                if i % 2 == 0 {
                    decode_node(&nested_nodes(MAX_NODE_DEPTH)).is_ok()
                } else {
                    decode_node(&nested_nodes(MAX_NODE_DEPTH + 1)).is_err()
                }
            })
        })
        .collect();
    for h in handles {
        assert!(h.join().unwrap(), "a concurrent decode saw the wrong bound");
    }
}
