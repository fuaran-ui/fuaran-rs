//! Server-driven driver coverage (Phase 554): the happy path (event → applied
//! ops → pushed frame + advancing server tree), reconnect-replay (a bounded
//! buffer re-pushes frames newer than the client's last seq), the reject
//! vocabulary (unknown node / illegitimate event / capability-gate denial), and
//! the frame wire shape (byte-interoperable with the `fuaran-go` driver).

use fuaran_rs::gate::CapabilityGate;
use fuaran_rs::ops::all_node_ids;
use fuaran_rs::serverdriven::{
    Connection, Event, Frame, InMemoryChannel, RejectReason, Session, decode_event,
    encode_frame_json, encode_sse,
};
use fuaran_rs::wire::{Node, TreeOp, decode_node, decode_op};

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

/// A handler that recolours the button on every legitimate click.
fn recolour_handler() -> fuaran_rs::serverdriven::Handler {
    Box::new(|_tree: &Node, _ev: &Event| {
        Ok(vec![op(
            r#"{"$type":"UpdateStyle","style":{"emphasis":"Loud","tone":"Brand","weight":"Standard"},"target":"btn"}"#,
        )])
    })
}

#[test]
fn happy_path_pushes_a_frame_and_advances_the_tree() {
    let session = Session::new(base_tree(), recolour_handler());
    let mut conn = Connection::new("c1", session, InMemoryChannel::new());

    conn.handle(&click("btn")).unwrap();
    conn.handle(&click("btn")).unwrap();

    assert_eq!(conn.sequence(), 2);
    let pushed = conn.channel().pushed();
    assert_eq!(pushed.len(), 2);
    assert_eq!(pushed[0].seq, 1);
    assert_eq!(pushed[1].seq, 2);
    assert_eq!(pushed[0].ops.len(), 1);
    // The server tree advanced (the button's style is now Loud/Brand).
    assert!(conn.rejects().is_empty());
}

#[test]
fn an_unknown_node_is_rejected_with_no_frame() {
    let session = Session::new(base_tree(), recolour_handler());
    let mut conn = Connection::new("c1", session, InMemoryChannel::new());
    conn.handle(&click("ghost")).unwrap();
    assert_eq!(conn.channel().pushed().len(), 0);
    assert_eq!(conn.sequence(), 0);
    assert_eq!(conn.rejects().len(), 1);
    assert_eq!(conn.rejects()[0].reason, RejectReason::UnknownNode);
}

#[test]
fn an_illegitimate_event_is_rejected() {
    let session = Session::new(base_tree(), recolour_handler());
    let mut conn = Connection::new("c1", session, InMemoryChannel::new());
    let ev = Event {
        event: "submit".into(),
        ..click("btn")
    };
    conn.handle(&ev).unwrap();
    assert_eq!(conn.channel().pushed().len(), 0);
    assert_eq!(conn.rejects()[0].reason, RejectReason::IllegitimateEvent);
}

/// A handler that mounts an ungranted-capability mini-app on click.
fn mount_handler() -> fuaran_rs::serverdriven::Handler {
    Box::new(|_tree: &Node, _ev: &Event| {
        Ok(vec![op(
            r#"{"$type":"InsertChild","child":{"id":"m1","kind":{"$type":"Mount","capabilities":["fs.write"],"channel":{"direction":"OutOnly"},"scopeId":"s1"}},"parentId":"root","position":1}"#,
        )])
    })
}

#[test]
fn a_driven_op_is_refused_by_the_capability_gate() {
    // Default gate grants nothing → the mount is denied, nothing is pushed, and
    // the server tree is untouched.
    let session = Session::new(base_tree(), mount_handler());
    let mut conn = Connection::new("c1", session, InMemoryChannel::new());
    conn.handle(&click("btn")).unwrap();

    assert_eq!(conn.channel().pushed().len(), 0);
    assert_eq!(conn.rejects().len(), 1);
    let reject = &conn.rejects()[0];
    assert_eq!(reject.reason, RejectReason::CapabilityDenied);
    assert_eq!(reject.missing_capabilities, vec!["fs.write".to_string()]);
    assert!(!all_node_ids(conn.session().tree()).contains(&"m1".to_string()));
}

#[test]
fn a_granted_capability_lets_the_driven_mount_through() {
    let session = Session::new(base_tree(), mount_handler())
        .with_gate(CapabilityGate::granting(["fs.write"]));
    let mut conn = Connection::new("c1", session, InMemoryChannel::new());
    conn.handle(&click("btn")).unwrap();

    assert!(conn.rejects().is_empty());
    assert_eq!(conn.channel().pushed().len(), 1);
    assert!(all_node_ids(conn.session().tree()).contains(&"m1".to_string()));
}

#[test]
fn reconnect_replay_re_pushes_frames_newer_than_the_last_seq() {
    let session = Session::new(base_tree(), recolour_handler());
    let mut conn = Connection::new("c1", session, InMemoryChannel::new());
    conn.handle(&click("btn")).unwrap();
    conn.handle(&click("btn")).unwrap();
    assert_eq!(conn.channel().pushed().len(), 2);

    // The client reconnected having applied up to seq 1 → only frame 2 replays.
    let replayed = conn.resync(1).unwrap();
    assert_eq!(replayed, 1);
    let pushed = conn.channel().pushed();
    assert_eq!(pushed.len(), 3);
    assert_eq!(pushed[2].seq, 2);

    // A client that applied nothing gets both frames re-pushed.
    let replayed_all = conn.resync(0).unwrap();
    assert_eq!(replayed_all, 2);
    assert_eq!(conn.channel().pushed().len(), 5);
}

#[test]
fn an_event_for_another_connection_is_ignored() {
    let session = Session::new(base_tree(), recolour_handler());
    let mut conn = Connection::new("c1", session, InMemoryChannel::new());
    let ev = Event {
        conn_id: "other".into(),
        ..click("btn")
    };
    conn.handle(&ev).unwrap();
    assert_eq!(conn.channel().pushed().len(), 0);
    assert!(conn.rejects().is_empty());
}

#[test]
fn frame_wire_shape_is_interoperable_with_the_go_driver() {
    let frame = Frame {
        seq: 3,
        ops: vec![op(r#"{"$type":"RemoveNode","target":"x"}"#)],
    };
    assert_eq!(
        encode_frame_json(&frame),
        r#"{"ops":[{"$type":"RemoveNode","target":"x"}],"seq":3}"#
    );
    assert_eq!(
        encode_sse(&frame),
        "id: 3\nevent: patch\ndata: {\"ops\":[{\"$type\":\"RemoveNode\",\"target\":\"x\"}],\"seq\":3}\n\n"
    );
}

#[test]
fn client_events_decode_from_the_control_wire() {
    let ev =
        decode_event(r#"{"connId":"c1","nodeId":"btn","event":"click","payload":"","lastSeq":4}"#)
            .expect("event decodes");
    assert_eq!(ev.conn_id, "c1");
    assert_eq!(ev.node_id, "btn");
    assert_eq!(ev.event, "click");
    assert_eq!(ev.last_seq, 4);
    assert!(decode_event("not json").is_err());
}
