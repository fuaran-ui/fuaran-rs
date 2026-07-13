//! Certifies the op-stream hash chain against the shared cross-host golden
//! (`wire-format-fixtures/chain/chain-corpus.json`): each record's hash must
//! reproduce the byte-identical value the F#/TS hosts compute, and the chain
//! must verify from genesis. Plus behavioural coverage of append, tamper
//! detection, and the SHA-256 vectors (in the module's own unit tests).

use std::path::{Path, PathBuf};

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::opstream::{
    Actor, OpRecord, OpResult, OpStream, VerificationError, compute_hash, genesis_previous_hash,
    verify_chain,
};
use fuaran_rs::wire::{TreeOp, decode_op};

fn find_corpus() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        if dir
            .join("wire-format-fixtures")
            .join("manifest.json")
            .is_file()
        {
            return Some(dir.join("wire-format-fixtures"));
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn str_field(v: &JVal, key: &str) -> Option<String> {
    match v.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn num_field(v: &JVal, key: &str) -> Option<f64> {
    match v.field(key) {
        Some(JVal::Num(n)) => Some(*n),
        _ => None,
    }
}

fn decode_actor(v: &JVal) -> Actor {
    match str_field(v, "kind").as_deref() {
        Some("agent") => Actor::Agent {
            model: str_field(v, "model").unwrap_or_default(),
            version: str_field(v, "version").unwrap_or_default(),
            id: str_field(v, "id").unwrap_or_default(),
        },
        _ => Actor::Human {
            id: str_field(v, "id").unwrap_or_default(),
        },
    }
}

fn decode_result(v: &JVal) -> OpResult {
    match str_field(v, "kind").as_deref() {
        Some("failure") => OpResult::Failure {
            code: str_field(v, "code").unwrap_or_default(),
            message: str_field(v, "message").unwrap_or_default(),
        },
        _ => OpResult::Success,
    }
}

/// The op each corpus record references (already round-trip-certified fixtures).
fn read_op(corpus: &Path, rel: &str) -> TreeOp {
    let json = std::fs::read_to_string(corpus.join(rel)).expect("reading op fixture");
    decode_op(&json).expect("op fixture decodes")
}

#[test]
fn chain_corpus_hashes_are_byte_identical_cross_host() {
    let Some(corpus) = find_corpus() else {
        eprintln!("corpus not found; skipping (standalone checkout)");
        return;
    };
    let raw = std::fs::read_to_string(corpus.join("chain").join("chain-corpus.json"))
        .expect("reading chain corpus");
    let doc = parse(&raw).expect("chain corpus parses");
    let genesis = str_field(&doc, "genesisPreviousHash").expect("genesis hash");
    assert_eq!(genesis, genesis_previous_hash(), "genesis sentinel matches");

    let Some(JVal::Arr(records)) = doc.field("records") else {
        panic!("chain corpus declares no records");
    };

    // Recompute each record's hash from its fields and assert it equals the
    // golden — this is the cross-host byte-identity proof.
    let mut built: Vec<OpRecord> = Vec::new();
    for rec in records {
        let op_fixture = str_field(rec, "opFixture").expect("opFixture");
        let op = read_op(&corpus, &op_fixture);
        let sequence = num_field(rec, "sequence").expect("sequence") as u64;
        let ts = num_field(rec, "timestampUnixSeconds").expect("ts") as i64;
        let actor = decode_actor(rec.field("actor").expect("actor"));
        let prompt_id = str_field(rec, "promptId");
        let result = decode_result(rec.field("result").expect("result"));
        let previous_hash = str_field(rec, "previousHash").expect("previousHash");
        let golden_hash = str_field(rec, "hash").expect("hash");

        let computed = compute_hash(
            &previous_hash,
            &op,
            sequence,
            ts,
            &actor,
            prompt_id.as_deref(),
            &result,
        );
        assert_eq!(
            computed, golden_hash,
            "record {sequence} hash must match the cross-host golden"
        );

        built.push(OpRecord {
            sequence,
            op,
            timestamp_unix_seconds: ts,
            actor,
            prompt_id,
            result,
            previous_hash,
            hash: golden_hash,
        });
    }

    // The reconstructed chain verifies clean from genesis.
    assert_eq!(verify_chain(&built), Ok(()));
    eprintln!(
        "chain corpus: {} records byte-identical + verified",
        built.len()
    );
}

#[test]
fn append_builds_a_verifiable_chain() {
    let op1 = decode_op(r#"{"$type":"RemoveNode","target":"a"}"#).unwrap();
    let op2 = decode_op(
        r#"{"$type":"UpdateStyle","style":{"emphasis":"Loud","tone":"Brand","weight":"Standard"},"target":"b"}"#,
    )
    .unwrap();

    let mut stream = OpStream::new();
    assert_eq!(stream.head(), genesis_previous_hash());
    let h1 = stream.append(
        op1,
        1700000000,
        Actor::Human { id: "u".into() },
        None,
        OpResult::Success,
    );
    let h2 = stream.append(
        op2,
        1700000001,
        Actor::Agent {
            model: "claude".into(),
            version: "4.8".into(),
            id: "planner".into(),
        },
        Some("p-1".into()),
        OpResult::Success,
    );
    assert_eq!(stream.len(), 2);
    assert_eq!(stream.head(), h2);
    assert_ne!(h1, h2);
    assert_eq!(stream.records()[1].previous_hash, h1);
    assert_eq!(stream.verify(), Ok(()));
}

#[test]
fn tampering_a_historical_op_breaks_the_chain() {
    let mut stream = OpStream::new();
    stream.append(
        decode_op(r#"{"$type":"RemoveNode","target":"a"}"#).unwrap(),
        1700000000,
        Actor::Human { id: "u".into() },
        None,
        OpResult::Success,
    );
    stream.append(
        decode_op(r#"{"$type":"RemoveNode","target":"b"}"#).unwrap(),
        1700000001,
        Actor::Human { id: "u".into() },
        None,
        OpResult::Success,
    );
    let mut records = stream.records().to_vec();

    // Rewrite the FIRST record's op payload (the "notarised" tamper): the stored
    // hash no longer recomputes → a HashMismatch at that record.
    records[0].op = decode_op(r#"{"$type":"RemoveNode","target":"EVIL"}"#).unwrap();
    match verify_chain(&records) {
        Err(VerificationError::HashMismatch { sequence, .. }) => assert_eq!(sequence, 1),
        other => panic!("expected a HashMismatch at record 1, got {other:?}"),
    }
}

#[test]
fn a_reordered_or_relinked_record_is_caught() {
    let mut stream = OpStream::new();
    stream.append(
        decode_op(r#"{"$type":"RemoveNode","target":"a"}"#).unwrap(),
        1,
        Actor::Human { id: "u".into() },
        None,
        OpResult::Success,
    );
    stream.append(
        decode_op(r#"{"$type":"RemoveNode","target":"b"}"#).unwrap(),
        2,
        Actor::Human { id: "u".into() },
        None,
        OpResult::Success,
    );
    let mut records = stream.records().to_vec();
    // Break the previous-hash link on the second record.
    records[1].previous_hash = genesis_previous_hash();
    match verify_chain(&records) {
        Err(VerificationError::PreviousHashMismatch { sequence, .. }) => assert_eq!(sequence, 2),
        other => panic!("expected a PreviousHashMismatch at record 2, got {other:?}"),
    }
}
