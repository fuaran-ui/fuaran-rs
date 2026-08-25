//! Edge-session behaviour: the durability protocol a worker-shaped runtime runs
//! the certified core under — one owner, journal-before-adopt, and rehydration
//! from the journal after the activation goes away.
//!
//! Every test here is about a property the session claims rather than about a
//! method returning what it was passed. The eviction cases in particular are
//! written as *kill points*: the session value is dropped mid-life and a fresh
//! one activated over the same store, which is exactly what the runtime does and
//! is the only way the replay path is actually exercised.

use fuaran_rs::edge::{
    ActivationToken, Checkpoint, DurableSessionStore, EdgeError, EdgeSession, InMemoryDurableStore,
    StoreError,
};
use fuaran_rs::opstream::{Actor, OpRecord, OpResult, verify_chain};

const SEED: &str = r#"{"id":"root","kind":{"$type":"Box","children":[
    {"id":"title","kind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"Live"},"variant":"Standard"}}
],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;

fn heading(text: &str) -> String {
    format!(
        r#"{{"$type":"EditNode","target":"title","newKind":{{"$type":"Heading","level":1,"text":{{"$type":"Literal","text":"{text}"}},"variant":"Standard"}}}}"#
    )
}

fn actor() -> Actor {
    Actor::Human {
        id: "operator".to_string(),
    }
}

fn open(store: InMemoryDurableStore, id: &str) -> EdgeSession<InMemoryDurableStore> {
    EdgeSession::activate(store, id, SEED, actor()).expect("session activates")
}

#[test]
fn a_fresh_session_starts_from_its_seed_with_an_empty_journal() {
    let s = open(InMemoryDurableStore::new(), "s1");
    assert_eq!(s.sequence(), 0);
    assert!(s.records().is_empty());
    assert!(s.render().contains(">Live</h1>"));
    assert_eq!(s.activation(), ActivationToken(1));
}

#[test]
fn an_applied_op_is_journaled_and_the_tree_moves() {
    let mut s = open(InMemoryDurableStore::new(), "s1");
    s.apply_op(&heading("Updated"), 1_787_000_000, None)
        .expect("op applies");

    assert_eq!(s.sequence(), 1);
    assert!(s.render().contains(">Updated</h1>"));
    // The journal is a chain from the first record, not merely a list.
    verify_chain(s.records()).expect("chain verifies");
    assert_eq!(s.head(), s.records()[0].hash);
}

#[test]
fn a_refused_op_journals_nothing() {
    // The journal is the APPLIED history. A record whose op does not apply would
    // make the next replay fail on its own evidence, which is why a refusal is
    // not written down.
    let mut s = open(InMemoryDurableStore::new(), "s1");
    let before = s.tree_json();
    match s.apply_op(r#"{"$type":"RemoveNode","target":"ghost"}"#, 1, None) {
        Err(EdgeError::Client(_)) => {}
        other => panic!("expected a client refusal, got {other:?}"),
    }
    assert_eq!(s.sequence(), 0);
    assert_eq!(s.tree_json(), before);

    // Same for an op that never decoded.
    match s.apply_op("{not json", 1, None) {
        Err(EdgeError::Decode(_)) => {}
        other => panic!("expected a decode refusal, got {other:?}"),
    }
    assert_eq!(s.sequence(), 0);
}

#[test]
fn an_evicted_session_rehydrates_to_the_same_tree_from_its_journal() {
    // THE KILL POINT. The activation is dropped with no checkpoint at all, so
    // the recovered tree is the fold of the journal and nothing else.
    let mut s = open(InMemoryDurableStore::new(), "s1");
    s.apply_op(&heading("One"), 1, None).expect("op 1");
    s.apply_op(&heading("Two"), 2, None).expect("op 2");
    let expected = s.tree_json();
    let expected_head = s.head();
    let store = s.into_store();

    let revived = open(store, "s1");
    assert_eq!(revived.tree_json(), expected);
    assert_eq!(revived.head(), expected_head);
    assert_eq!(revived.sequence(), 2);
    assert!(revived.render().contains(">Two</h1>"));
    // A new activation, which is what a fence is for.
    assert_eq!(revived.activation(), ActivationToken(2));
}

#[test]
fn a_checkpoint_changes_how_far_replay_runs_and_not_what_it_produces() {
    // A checkpoint is an optimisation. Two stores, identical op sequences, one
    // checkpointed halfway — the recovered trees and chain heads must agree, or
    // the snapshot is a second source of truth rather than a shortcut.
    let build = |checkpoint_after: Option<u64>| {
        let mut s = open(InMemoryDurableStore::new(), "s1");
        for (i, text) in ["One", "Two", "Three"].iter().enumerate() {
            s.apply_op(&heading(text), i as i64 + 1, None).expect("op");
            if checkpoint_after == Some(i as u64 + 1) {
                s.checkpoint().expect("checkpoint");
            }
        }
        s.into_store()
    };

    let plain = open(build(None), "s1");
    let snapshotted = open(build(Some(2)), "s1");

    assert_eq!(plain.tree_json(), snapshotted.tree_json());
    assert_eq!(plain.head(), snapshotted.head());
    assert!(snapshotted.render().contains(">Three</h1>"));
}

#[test]
fn the_checkpoint_is_the_base_the_suffix_replays_onto() {
    // The test above passes on a host that IGNORES checkpoints entirely — both
    // sides fold the whole journal and agree. This is the pair that separates
    // the two: the store is handed a snapshot that is deliberately NOT what the
    // first two records fold to, and the suffix is an op whose effect survives
    // either base. Honouring the checkpoint yields "Snapshot"; ignoring it
    // yields "Two", which is what the journal alone says.
    let mut s = open(InMemoryDurableStore::new(), "s1");
    s.apply_op(&heading("One"), 1, None).expect("op 1");
    s.apply_op(&heading("Two"), 2, None).expect("op 2");
    s.checkpoint().expect("checkpoint at 2");
    s.apply_op(
        r#"{"$type":"InsertChild","child":{"id":"added","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"tail"}}},"parentId":"root"}"#,
        3,
        None,
    )
    .expect("op 3");
    let mut store = s.into_store();

    let token = store.acquire("s1").expect("activation");
    store
        .checkpoint(
            "s1",
            token,
            &Checkpoint {
                through_sequence: 2,
                tree_json: SEED.replace("Live", "Snapshot"),
            },
        )
        .expect("substituted snapshot");

    let revived = open(store, "s1");
    let html = revived.render();
    assert!(
        html.contains(">Snapshot</h1>"),
        "the checkpoint is the base"
    );
    assert!(
        !html.contains(">Two</h1>"),
        "the journal prefix is not re-folded"
    );
    assert!(
        html.contains("data-fuaran-node-id=\"added\""),
        "the suffix ran"
    );
}

#[test]
fn a_checkpoint_records_the_sequence_it_includes() {
    let mut s = open(InMemoryDurableStore::new(), "s1");
    s.apply_op(&heading("One"), 1, None).expect("op");
    let cp = s.checkpoint().expect("checkpoint");
    assert_eq!(cp.through_sequence, 1);
    assert_eq!(cp.tree_json, s.tree_json());
    assert_eq!(
        s.store().latest_checkpoint("s1").expect("read"),
        Some(cp.clone())
    );
    // The snapshot is the tree AT that sequence — a later op does not rewrite it.
    s.apply_op(&heading("Two"), 2, None).expect("op");
    assert_eq!(s.store().latest_checkpoint("s1").expect("read"), Some(cp));
}

#[test]
fn a_superseded_activation_is_fenced_out_of_its_own_session() {
    // The failure a distributed runtime actually has: the platform decides a
    // session is gone and starts another copy, while the first is still holding
    // a tree. The second write must be REFUSED, never interleaved.
    let mut store = InMemoryDurableStore::new();
    let first = store.acquire("s1").expect("first activation");
    let second = store.acquire("s1").expect("second activation");
    assert!(second > first);

    let record = OpRecord {
        sequence: 1,
        op: fuaran_rs::wire::decode_op(&heading("Ghost")).expect("op decodes"),
        timestamp_unix_seconds: 1,
        actor: actor(),
        prompt_id: None,
        result: OpResult::Success,
        previous_hash: fuaran_rs::opstream::genesis_previous_hash(),
        hash: String::new(),
    };
    match store.append("s1", first, &record) {
        Err(StoreError::NotOwner { held, presented }) => {
            assert_eq!(held, second);
            assert_eq!(presented, first);
        }
        other => panic!("expected the stale activation to be fenced, got {other:?}"),
    }
    // And the fence covers checkpoints too, not only appends.
    let cp = Checkpoint {
        through_sequence: 0,
        tree_json: SEED.to_string(),
    };
    assert!(matches!(
        store.checkpoint("s1", first, &cp),
        Err(StoreError::NotOwner { .. })
    ));
    // The current owner is unaffected.
    assert!(store.checkpoint("s1", second, &cp).is_ok());
}

#[test]
fn a_session_whose_journal_does_not_verify_is_refused_rather_than_repaired() {
    // Nothing here re-chains a broken history: a host that did would be blessing
    // it, and afterwards nothing could tell the difference.
    let mut s = open(InMemoryDurableStore::new(), "s1");
    s.apply_op(&heading("One"), 1, None).expect("op");
    let mut store = s.into_store();

    let activation = store.acquire("s1").expect("activation");
    let mut tampered = store.journal("s1").expect("journal");
    tampered[0].timestamp_unix_seconds = 999; // the hash no longer recomputes
    // Re-seat the tampered record by writing to a second session id, which the
    // store will accept because it verifies nothing — verification is the
    // session's obligation, and that is the point of this test.
    let fresh = store.acquire("s2").expect("activation");
    store
        .append("s2", fresh, &tampered[0])
        .expect("the store stores what it is given");
    let _ = activation;

    match EdgeSession::activate(store, "s2", SEED, actor()) {
        Err(EdgeError::Chain(_)) => {}
        other => panic!("expected a chain refusal, got {}", describe(other)),
    }
}

#[test]
fn reactive_slots_are_view_state_and_do_not_survive_an_eviction() {
    // A stated limit rather than a silent one: `$state` is a view's live input,
    // not authored history, so it is not journaled and a host re-seeds it on
    // activation. The tree, which IS history, comes back.
    let tree = r#"{"id":"root","kind":{"$type":"Box","children":[
        {"id":"metric","kind":{"$type":"Metric","emphasis":"Loud","format":{"$type":"Currency","code":"GBP"},"label":{"$type":"Literal","text":"Revenue"},"value":{"$type":"State","defaultValue":0,"key":"revenue"},"tone":"Brand","weight":"Standard"}}
    ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;

    let mut s =
        EdgeSession::activate(InMemoryDurableStore::new(), "s1", tree, actor()).expect("activates");
    s.set_state("revenue", "425.5").expect("state write");
    assert!(s.render().contains("GBP 425.50"));

    let revived = EdgeSession::activate(s.into_store(), "s1", tree, actor()).expect("reactivates");
    assert!(revived.render().contains("GBP 0.00"));
}

#[test]
fn two_sessions_in_one_store_do_not_see_each_others_history() {
    let mut store = InMemoryDurableStore::new();
    {
        let mut a = EdgeSession::activate(store, "a", SEED, actor()).expect("a");
        a.apply_op(&heading("A"), 1, None).expect("op");
        store = a.into_store();
    }
    let b = EdgeSession::activate(store, "b", SEED, actor()).expect("b");
    assert_eq!(b.sequence(), 0);
    assert!(b.render().contains(">Live</h1>"));
    assert_eq!(
        b.store().session_ids(),
        vec!["a".to_string(), "b".to_string()]
    );
}

/// `EdgeSession` is deliberately not `Debug` (it holds a whole tree), so a
/// panic message about an unexpected `Ok` renders the outcome by hand.
fn describe(outcome: Result<EdgeSession<InMemoryDurableStore>, EdgeError>) -> String {
    match outcome {
        Ok(_) => "a successfully activated session".to_string(),
        Err(e) => e.to_string(),
    }
}
