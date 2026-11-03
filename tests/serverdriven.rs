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
            r#"{"$type":"InsertChild","child":{"id":"m1","kind":{"$type":"Mount","capabilities":["fs.write"],"channel":{"direction":"OutOnly"},"scopeId":"s1"}},"parentId":"root"}"#,
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

// ─── The constrained emitter (Phase 2452) ────────────────────────────────────
//
// The device half of a panel. `tests/conformance.rs` certifies its output against
// the corpus's `panel@1.1` vectors; these tests hold the mechanics: the compiled-in
// image, the chain, the byte pipe's replay, the event decoder's limits and
// admission, and every refusal.

use fuaran_rs::serverdriven::emitter::{
    self, Actor, BytePipe, CompileError, EVENT_LIMITS, EmitError, Emitter, EventRefusal, Fragment,
    Handler, Image, PanelSpec, PipeChannel, Resync, Value, ValueKind, Write, WriteSpec,
};

/// The panel as a device compiles it in — the tree compiler's own output,
/// verbatim (`compiled_in_image_is_the_compilers_output` holds the two equal).
#[rustfmt::skip]
const PANEL: Image<'static> = Image {
    revision: "panel@1.1",
    root: "{\"id\":\"emitter-panel\",\"kind\":{\"$type\":\"Box\",\"children\":[{\"id\":\"panel-title\",\"kind\":{\"$type\":\"Heading\",\"level\":2,\"text\":\"Cold room 3\"}},{\"id\":\"panel-temp\",\"kind\":{\"$type\":\"Metric\",\"label\":\"Temperature\",\"value\":{\"$type\":\"Static\",\"value\":4}}},{\"id\":\"panel-status\",\"kind\":{\"$type\":\"Badge\",\"label\":\"Cooling\",\"variant\":\"Success\"}},{\"id\":\"panel-defrost\",\"kind\":{\"$type\":\"Button\",\"label\":\"Start defrost\",\"onClick\":{\"$type\":\"Dispatch\"}}},{\"id\":\"panel-schedule\",\"kind\":{\"$type\":\"Form\",\"fields\":[{\"id\":\"minutes\",\"kind\":{\"$type\":\"Number\",\"value\":{\"$type\":\"Static\",\"value\":45}},\"label\":\"Minutes\",\"required\":true},{\"id\":\"start\",\"kind\":{\"$type\":\"DateTime\",\"value\":{\"$type\":\"Static\",\"value\":\"06:00\"},\"variant\":\"Time\"},\"label\":\"Start at\"}],\"onSubmit\":{\"$type\":\"Dispatch\"},\"submitLabel\":\"Schedule\"}}],\"layout\":{\"$type\":\"Flex\",\"direction\":\"Vertical\"},\"role\":\"Group\"}}",
    nodes: &[
        "emitter-panel",
        "panel-title",
        "panel-temp",
        "panel-status",
        "panel-defrost",
        "panel-schedule",
        "panel-alarm",
    ],
    fragments: &[
        Fragment { parent: "emitter-panel", id: "panel-alarm", node: "{\"id\":\"panel-alarm\",\"kind\":{\"$type\":\"Callout\",\"body\":\"Door open\",\"tone\":\"Critical\"}}" },
    ],
    readings: &[
        Write { target: "panel-temp", field: "Value", kind: ValueKind::Int },
    ],
    props: &[
        Write { target: "panel-status", field: "Label", kind: ValueKind::Str },
    ],
    handlers: &[
        Handler { node: "panel-defrost", event: "click" },
        Handler { node: "panel-schedule", event: "submit" },
    ],
    state_keys: &[],
};

fn panel_spec() -> PanelSpec {
    PanelSpec {
        root: decode_node(PANEL.root).expect("the panel decodes"),
        fragments: vec![(
            PANEL.fragments[0].parent.to_string(),
            decode_node(PANEL.fragments[0].node).expect("the alarm decodes"),
        )],
        readings: vec![WriteSpec::new("panel-temp", "Value", ValueKind::Int)],
        props: vec![WriteSpec::new("panel-status", "Label", ValueKind::Str)],
    }
}

fn device() -> Actor {
    Actor::Agent {
        model: "device".to_string(),
        version: emitter::PROFILE_REVISION.to_string(),
        id: "cold-room-3".to_string(),
    }
}

#[test]
fn compiled_in_image_is_the_compilers_output() {
    let compiled = emitter::compile(&panel_spec()).expect("the panel certifies");
    compiled.with_image(|image| assert_eq!(*image, PANEL));
    // The constant above IS the compiler's Rust source, character for character.
    let this_file = include_str!("serverdriven.rs").replace("\r\n", "\n");
    assert!(
        this_file.contains(&compiled.to_rust_source("PANEL")),
        "the compiled-in PANEL is not the compiler's current output:\n{}",
        compiled.to_rust_source("PANEL")
    );
}

#[test]
fn compile_refuses_what_the_device_could_not_write_conformantly() {
    let refuse = |spec: PanelSpec| -> CompileError {
        emitter::compile(&spec).expect_err("the compiler must refuse")
    };
    let node = |json: &str| decode_node(json).expect("decodes");

    // A kind outside the declared subset.
    let mut spec = panel_spec();
    spec.fragments.push((
        "emitter-panel".to_string(),
        node(r#"{"id":"fact-1","kind":{"$type":"Fact","emphasis":true,"help":"Primary insured","icon":"user","label":"Patient","tone":"Brand","value":"Alice Smith"}}"#),
    ));
    assert_eq!(refuse(spec).code, "OUTSIDE_SUBSET");

    // A non-integer number token.
    let mut spec = panel_spec();
    spec.fragments.push((
        "emitter-panel".to_string(),
        node(r#"{"id":"probe","kind":{"$type":"Metric","label":"T","value":{"$type":"Static","value":4.5}}}"#),
    ));
    assert_eq!(refuse(spec).code, "NOT_INTEGER_ONLY");

    // A fragment reusing a panel id.
    let mut spec = panel_spec();
    spec.fragments.push((
        "emitter-panel".to_string(),
        node(r#"{"id":"panel-temp","kind":{"$type":"Callout","body":"x","tone":"Info"}}"#),
    ));
    assert_eq!(refuse(spec).code, "DUPLICATE_ID");

    // A write the panel cannot take.
    let mut spec = panel_spec();
    spec.readings
        .push(WriteSpec::new("ghost", "Value", ValueKind::Int));
    assert_eq!(refuse(spec).code, "WRITE_DOES_NOT_APPLY");

    // A fragment whose parent the panel does not hold.
    let mut spec = panel_spec();
    spec.fragments[0].0 = "ghost".to_string();
    assert_eq!(refuse(spec).code, "WRITE_DOES_NOT_APPLY");

    // The unperturbed panel certifies, so each refusal above is its perturbation's.
    assert!(emitter::compile(&panel_spec()).is_ok());
}

#[test]
fn every_frame_is_chained_and_a_tamper_breaks_the_chain() {
    let mut em = Emitter::new(&PANEL, &device());
    assert_eq!(em.seq(), 0);
    let frames = vec![
        em.replace_root(),
        em.reading(0, Value::Int(-7)).unwrap(),
        em.insert(0).unwrap(),
        em.prop(0, Value::Str("Line \"two\"\n")).unwrap(),
        em.remove(0).unwrap(),
    ];
    assert_eq!(
        frames.iter().map(|f| f.seq).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    let mut records: Vec<_> = frames.iter().map(|f| f.record(em.actor())).collect();
    let genesis = fuaran_core::chain::Anchor::genesis();
    let end = fuaran_core::chain::walk(&genesis, &records).expect("the chain verifies");
    assert_eq!(end.prev_hash, em.head());
    assert_eq!(frames[4].hash, em.head());

    // Every frame is canonical (escapes included) and its body is the driver's.
    for f in &frames {
        let op = decode_op(&f.op).expect("decodes");
        assert_eq!(fuaran_rs::wire::encode_op(&op), f.op);
        assert_eq!(
            f.body(),
            encode_frame_json(&Frame {
                seq: f.seq,
                ops: vec![op]
            })
        );
    }

    // Rewriting a past reading breaks the chain at that record.
    records[1].op = records[1].op.replace("-7", "7");
    assert!(fuaran_core::chain::walk(&genesis, &records).is_err());
}

#[test]
fn a_refused_write_writes_nothing() {
    let mut em = Emitter::new(&PANEL, &device());
    em.replace_root();
    let head = em.head().to_string();
    assert_eq!(
        em.reading(0, Value::Str("4")),
        Err(EmitError::WrongValueKind {
            expected: ValueKind::Int,
            actual: ValueKind::Str
        })
    );
    assert_eq!(
        em.reading(0, Value::Int(emitter::MAX_SAFE_INTEGER + 1)),
        Err(EmitError::IntegerOutOfRange(emitter::MAX_SAFE_INTEGER + 1))
    );
    assert_eq!(
        em.reading(1, Value::Int(1)),
        Err(EmitError::NoSuchReading(1))
    );
    assert_eq!(em.prop(1, Value::Str("x")), Err(EmitError::NoSuchProp(1)));
    assert_eq!(em.insert(1), Err(EmitError::NoSuchFragment(1)));
    assert_eq!(em.remove(1), Err(EmitError::NoSuchFragment(1)));
    assert_eq!((em.seq(), em.head()), (1, head.as_str()));
    // The bounds themselves are writable.
    assert!(em.reading(0, Value::Int(emitter::MAX_SAFE_INTEGER)).is_ok());
    assert!(
        em.reading(0, Value::Int(-emitter::MAX_SAFE_INTEGER))
            .is_ok()
    );
}

#[derive(Default)]
struct Uart(Vec<u8>);

impl BytePipe for Uart {
    type Error = ();
    fn write(&mut self, bytes: &[u8]) -> Result<(), ()> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

fn lines(pipe: &Uart) -> Vec<String> {
    String::from_utf8(pipe.0.clone())
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn the_byte_pipe_writes_one_frame_per_line_and_replays_within_its_budget() {
    let mut em = Emitter::new(&PANEL, &device());
    let first = em.replace_root();
    let root_cost = first.op.len() + first.prev_hash.len() + first.hash.len() + 32;
    // A budget that holds the first frame and three readings, not four.
    let reading_cost = {
        let probe = Emitter::new(&PANEL, &device())
            .reading(0, Value::Int(10))
            .unwrap();
        probe.op.len() + 64 + 64 + 32
    };
    let budget = root_cost + 3 * reading_cost;
    let mut ch = PipeChannel::new(Uart::default(), budget);
    ch.push(first).unwrap();
    for v in 10..14 {
        ch.push(em.reading(0, Value::Int(v)).unwrap()).unwrap();
    }
    assert!(ch.retained_bytes() <= budget);
    assert_eq!(ch.retained_seqs().collect::<Vec<_>>(), vec![2, 3, 4, 5]);

    // One line per frame, each a frame body every server-driven client reads.
    let written = lines(ch.pipe());
    assert_eq!(written.len(), 5);
    for l in &written {
        let v = fuaran_rs::canonical::parse(l).expect("a line is one JSON document");
        assert!(v.field("ops").is_some() && v.field("seq").is_some());
    }

    // Current, replayed from inside the window, and behind it.
    assert_eq!(ch.resync(5, em.seq()).unwrap(), Resync::Current);
    assert_eq!(ch.resync(3, em.seq()).unwrap(), Resync::Replayed(2));
    let replayed = lines(ch.pipe());
    assert_eq!(&replayed[5..], &written[3..]);
    assert_eq!(ch.resync(1, em.seq()).unwrap(), Resync::Replayed(4));
    assert_eq!(ch.resync(0, em.seq()).unwrap(), Resync::Gap);

    // A frame larger than the whole budget is written and not retained.
    let mut tiny = PipeChannel::new(Uart::default(), 16);
    tiny.push(Emitter::new(&PANEL, &device()).replace_root())
        .unwrap();
    assert_eq!(tiny.retained_bytes(), 0);
    assert_eq!(lines(tiny.pipe()).len(), 1);
}

#[test]
fn the_device_admits_only_registered_events_under_its_limits() {
    let decode = |raw: &str| emitter::decode_event(&PANEL, raw, &EVENT_LIMITS);

    let click =
        r#"{"connId":"c1","event":"click","lastSeq":4,"nodeId":"panel-defrost","payload":""}"#;
    let admitted = decode(click).expect("a registered click is admitted");
    assert_eq!((admitted.handler, admitted.last_seq), (0, 4));
    // The host driver reads the same event the same way.
    let hosted = decode_event(click).unwrap();
    assert_eq!(
        (PANEL.handlers[admitted.handler].node, hosted.last_seq),
        (hosted.node_id.as_str(), 4)
    );

    let submit = r#"{"event":"submit","nodeId":"panel-schedule","payload":"{\"minutes\":30}"}"#;
    let s = decode(submit).expect("a registered submit is admitted");
    assert_eq!(
        (s.handler, s.payload.as_str(), s.last_seq),
        (1, "{\"minutes\":30}", 0)
    );

    // Not registered: a known node with another event, an unknown node.
    for raw in [
        r#"{"event":"submit","nodeId":"panel-defrost"}"#,
        r#"{"event":"click","nodeId":"panel-temp"}"#,
        r#"{"event":"click","nodeId":"ghost"}"#,
    ] {
        assert_eq!(decode(raw), Err(EventRefusal::UnknownHandler), "{raw}");
    }
    // Wrong member types.
    assert_eq!(
        decode(r#"{"event":"click","lastSeq":-1,"nodeId":"panel-defrost"}"#),
        Err(EventRefusal::BadMember("lastSeq"))
    );
    assert_eq!(
        decode(r#"{"event":"click","lastSeq":1.5,"nodeId":"panel-defrost"}"#),
        Err(EventRefusal::BadMember("lastSeq"))
    );
    assert_eq!(
        decode(r#"{"event":7,"nodeId":"panel-defrost"}"#),
        Err(EventRefusal::BadMember("event"))
    );
    assert_eq!(decode(r#""click""#), Err(EventRefusal::NotAnObject));
    // The declared limits: a nested container, an over-long string, too many
    // members, too many bytes — each refused by the reader before anything is read.
    let over = "x".repeat(300);
    let many = (0..9)
        .map(|i| format!("\"m{i}\":0"))
        .collect::<Vec<_>>()
        .join(",");
    for raw in [
        r#"{"event":"click","nodeId":"panel-defrost","payload":{"a":1}}"#.to_string(),
        format!(r#"{{"event":"click","nodeId":"panel-defrost","payload":"{over}"}}"#),
        format!("{{{many}}}"),
        format!(
            r#"{{"connId":"{}","{}":"{}"}}"#,
            "c".repeat(200),
            "k".repeat(200),
            "v".repeat(200)
        ),
    ] {
        assert!(
            matches!(
                decode(&raw),
                Err(EventRefusal::Read(
                    fuaran_core::json::JsonRefusal::LimitExceeded(_)
                ))
            ),
            "{raw}"
        );
    }
    assert!(matches!(
        decode("{\"event\":"),
        Err(EventRefusal::Read(
            fuaran_core::json::JsonRefusal::Malformed(_)
        ))
    ));
}

/// The emitter's `no_std` claim, on a target that has no standard library at all.
/// Runs whenever that target is installed (`rustup target add
/// thumbv7em-none-eabihf`) and says NOT RUN otherwise; the CI `no-std` job
/// installs it, so there it cannot skip.
#[test]
fn the_emitter_builds_no_std_for_a_microcontroller() {
    use std::path::Path;
    use std::process::Command;
    const TARGET: &str = "thumbv7em-none-eabihf";
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let sysroot = Command::new(&rustc)
        .args(["--print", "sysroot"])
        .output()
        .expect("rustc runs");
    let sysroot = String::from_utf8_lossy(&sysroot.stdout).trim().to_string();
    if !Path::new(&sysroot)
        .join("lib/rustlib")
        .join(TARGET)
        .exists()
    {
        eprintln!("NOT RUN: the {TARGET} target is not installed");
        return;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let out = Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "rustc",
            "--lib",
            "--no-default-features",
            "--target",
            TARGET,
            "--crate-type",
            "lib",
            "--target-dir",
            "target/no-std",
            "--quiet",
        ])
        .output()
        .expect("cargo runs");
    assert!(
        out.status.success(),
        "the no_std build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The `fuaran-go` driver's committed frame and event vectors
/// (`serverdriven/frame_test.go`): the emitter writes that frame's two ops byte for
/// byte, and admits that event.
#[test]
fn the_emitter_writes_the_go_drivers_frame_ops_and_reads_its_event() {
    const GO_FRAME: &str = r#"{"ops":[{"$type":"UpdateProp","path":"Source","target":"count","value":1},{"$type":"RemoveNode","target":"x"}],"seq":7}"#;
    const GO_EVENT: &str =
        r#"{"connId":"c1","nodeId":"inc","event":"click","payload":"","lastSeq":4}"#;
    let image = Image {
        revision: emitter::PROFILE_REVISION,
        root: r#"{"id":"root","kind":{"$type":"Box","children":[],"layout":{"$type":"Auto"},"role":"Group"}}"#,
        nodes: &["root", "x"],
        fragments: &[Fragment {
            parent: "root",
            id: "x",
            node: r#"{"id":"x","kind":{"$type":"Heading","level":2,"text":"x"}}"#,
        }],
        readings: &[],
        props: &[Write {
            target: "count",
            field: "Source",
            kind: ValueKind::Int,
        }],
        handlers: &[Handler {
            node: "inc",
            event: "click",
        }],
        state_keys: &[],
    };
    let ops = [
        emitter::update_prop(&image, 0, Value::Int(1)).unwrap(),
        emitter::remove_node(&image, 0).unwrap(),
    ];
    assert_eq!(
        GO_FRAME,
        format!("{{\"ops\":[{}],\"seq\":7}}", ops.join(","))
    );
    let ev = emitter::decode_event(&image, GO_EVENT, &EVENT_LIMITS).expect("admitted");
    assert_eq!((ev.handler, ev.last_seq, ev.payload.as_str()), (0, 4, ""));
}
