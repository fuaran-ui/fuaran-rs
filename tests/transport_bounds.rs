//! Server-driven resource bounds and the apply-time §21 guard.
//!
//! Two unbounded collections and one unbounded growth path, each reachable by a
//! client that need do nothing a server would notice as traffic.

use fuaran_rs::ops::{ApplyErrorCode, apply};
use fuaran_rs::serverdriven::{
    Channel, Connection, Event, InMemoryChannel, RejectReason, Session, SseChannel,
};
use fuaran_rs::wire::{Node, NodeKind, TreeOp, decode_node, decode_op};

const BASE_TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[{"id":"btn","kind":{"$type":"Button","label":{"$type":"Literal","text":"Go"},"onClick":{"$type":"Chain","ops":[]},"variant":"Primary"}}],"layout":{"$type":"Auto"},"role":"Group"}}"#;

fn base_tree() -> Node {
    decode_node(BASE_TREE).expect("base tree decodes")
}

fn op(json: &str) -> TreeOp {
    decode_op(json).expect("op decodes")
}

fn click(node_id: &str) -> Event {
    Event {
        conn_id: "c1".into(),
        node_id: node_id.into(),
        event: "click".into(),
        payload: String::new(),
        last_seq: 0,
    }
}

fn recolour_handler() -> fuaran_rs::serverdriven::Handler {
    Box::new(|_tree: &Node, _ev: &Event| {
        Ok(vec![op(
            r#"{"$type":"UpdateStyle","style":{"emphasis":"Loud","tone":"Brand","weight":"Standard"},"target":"btn"}"#,
        )])
    })
}

// ── M-G10 · the reject trail is bounded ────────────────────────────────────

#[test]
fn the_reject_trail_is_capped_and_reports_what_it_dropped() {
    // Rejects are the one thing a hostile or merely broken client produces at
    // will: every refused event appends one and pushes no frame, so the
    // cheapest possible client behaviour grew server memory forever. The replay
    // buffer beside it was bounded for exactly this reason; the trail was not.
    let session = Session::new(base_tree(), recolour_handler());
    let mut conn =
        Connection::new("c1", session, InMemoryChannel::new()).with_reject_trail_capacity(8);

    for _ in 0..50 {
        conn.handle(&click("ghost")).unwrap();
    }

    assert_eq!(conn.rejects().len(), 8, "the trail must stop at its cap");
    assert_eq!(
        conn.rejects_dropped(),
        42,
        "a trail that silently forgets is worse than a short one — the count is \
         what tells an operator 'these are the most recent of very many'"
    );
    assert_eq!(conn.rejects()[0].reason, RejectReason::UnknownNode);
    // No frame was ever pushed: a reject changes nothing.
    assert_eq!(conn.channel().pushed().len(), 0);
}

#[test]
fn a_zero_reject_capacity_is_ignored_not_unbounded() {
    // A zero from an uninitialised config must not silently reinstate the
    // defect this cap closes.
    let session = Session::new(base_tree(), recolour_handler());
    let mut conn =
        Connection::new("c1", session, InMemoryChannel::new()).with_reject_trail_capacity(0);

    for _ in 0..3 {
        conn.handle(&click("ghost")).unwrap();
    }
    assert_eq!(conn.rejects().len(), 3);
    assert_eq!(conn.rejects_dropped(), 0);
}

#[test]
fn an_uncapped_trail_still_retains_an_ordinary_run() {
    // The cap must not cost the working case: a normal session's rejects are
    // all still there.
    let session = Session::new(base_tree(), recolour_handler());
    let mut conn = Connection::new("c1", session, InMemoryChannel::new());
    for _ in 0..5 {
        conn.handle(&click("ghost")).unwrap();
    }
    assert_eq!(conn.rejects().len(), 5);
    assert_eq!(conn.rejects_dropped(), 0);
}

// ── M-G10 · the SSE stream is drainable ────────────────────────────────────

#[test]
fn take_stream_drains_the_pushed_frames() {
    // `stream()` borrows, so it cannot clear what it lends: an hour-long
    // connection pushing a frame a second held every one of those frames' bytes
    // with nothing reading them.
    let mut ch = SseChannel::new();
    let frame = fuaran_rs::serverdriven::Frame {
        seq: 1,
        ops: vec![op(
            r#"{"$type":"UpdateStyle","style":{"emphasis":"Loud","tone":"Brand","weight":"Standard"},"target":"btn"}"#,
        )],
    };

    ch.push(&frame).unwrap();
    assert!(ch.pending_bytes() > 0);

    let taken = ch.take_stream();
    assert!(taken.contains("id: 1"), "the drained bytes are the SSE frame");
    assert_eq!(
        ch.pending_bytes(),
        0,
        "after the drain the channel holds the unwritten tail, not the session"
    );
    assert_eq!(ch.stream(), "");

    // The channel keeps working after a drain.
    ch.push(&frame).unwrap();
    assert!(ch.pending_bytes() > 0);
}

// ── M-B28 · the apply-time §21 guard ───────────────────────────────────────

/// An empty Box with the given id.
fn empty_box(id: &str) -> Node {
    decode_node(&format!(
        r#"{{"id":"{id}","kind":{{"$type":"Box","children":[],"layout":{{"$type":"Auto"}},"role":"Group"}}}}"#
    ))
    .expect("box decodes")
}

/// Wrap `child` in a new Box, deepening the tree by one.
///
/// Built by MUTATION rather than by decoding deeper JSON, deliberately: the
/// decoder already refuses a tree past MAX_NODE_DEPTH, so an over-limit fixture
/// cannot be decoded at all. That is the point — the decoder's bound holds, and
/// the gap this test covers is a tree ASSEMBLED past it, which is exactly what
/// this construction models.
fn deeper(id: &str, child: Node) -> Node {
    let mut outer = empty_box(id);
    if let NodeKind::Box(spec) = &mut outer.kind {
        spec.children.push(child);
    }
    outer
}

/// A Box nested `depth` levels (the root is depth 1); the deepest node is "n1".
fn chain(depth: usize) -> Node {
    let mut n = empty_box("n1");
    for i in 2..=depth {
        n = deeper(&format!("n{i}"), n);
    }
    n
}

fn insert_child(parent_id: &str, child_id: &str) -> TreeOp {
    op(&format!(
        r#"{{"$type":"InsertChild","child":{{"id":"{child_id}","kind":{{"$type":"Box","children":[],"layout":{{"$type":"Auto"}},"role":"Group"}}}},"parentId":"{parent_id}"}}"#
    ))
}

#[test]
fn insert_child_refuses_past_the_node_depth_limit() {
    // The decoder bounds what ARRIVES; nothing bounded what an apply produces.
    // A tree assembled op by op could grow past the limit without any single op
    // looking unusual, and the result was a tree this host held happily and no
    // host could decode — itself included, on the next round trip.
    let tree = chain(fuaran_rs::limits::MAX_NODE_DEPTH);

    let err = apply(&tree, &insert_child("n1", "over")).expect_err("must refuse");
    assert_eq!(err.code, ApplyErrorCode::LimitExceeded);
    assert_eq!(
        err.code.as_str(),
        "LimitExceeded",
        "the outcome code is the same string on every host"
    );
}

#[test]
fn insert_child_at_the_depth_limit_is_accepted() {
    // A guard that refused the boundary case would be a liveness bug wearing a
    // safety fix's clothes.
    let tree = chain(fuaran_rs::limits::MAX_NODE_DEPTH - 1);
    apply(&tree, &insert_child("n1", "at-the-limit")).expect("exactly at the limit is fine");
}

#[test]
fn replace_root_refuses_an_over_deep_tree() {
    // ReplaceRoot swaps the whole tree, so it is the one op that can breach the
    // limit in a single step from any starting point.
    let deep = chain(fuaran_rs::limits::MAX_NODE_DEPTH + 1);
    let op = TreeOp::ReplaceRoot { node: deep };

    let err = apply(&base_tree(), &op).expect_err("must refuse");
    assert_eq!(err.code, ApplyErrorCode::LimitExceeded);
}

#[test]
fn a_batch_is_refused_when_its_result_breaches() {
    // The check is on the RESULT, so a Batch whose individual ops each look fine
    // but whose composition crosses the line is refused — and nothing is
    // applied, since a Batch is all-or-nothing.
    let tree = chain(fuaran_rs::limits::MAX_NODE_DEPTH - 1);
    let batch = TreeOp::Batch(vec![insert_child("n1", "a"), insert_child("a", "b")]);

    let err = apply(&tree, &batch).expect_err("must refuse");
    assert_eq!(err.code, ApplyErrorCode::LimitExceeded);
}

#[test]
fn non_growing_ops_are_not_charged_for_the_guard() {
    // The seven ops that cannot grow the tree must not pay a walk to establish
    // what their own semantics already guarantee. Observable as behaviour: a
    // rewrite on an ALREADY-over-limit tree still applies, because it is not the
    // op that put it there and refusing would strand the tree.
    let over = chain(fuaran_rs::limits::MAX_NODE_DEPTH + 5);
    let recolour = op(
        r#"{"$type":"UpdateStyle","style":{"emphasis":"Loud","tone":"Brand","weight":"Standard"},"target":"n1"}"#,
    );

    match apply(&over, &recolour) {
        Ok(_) => {}
        Err(e) => assert_ne!(
            e.code,
            ApplyErrorCode::LimitExceeded,
            "a non-growing op was refused by the limit guard, stranding an over-limit tree"
        ),
    }
}
