//! The op-stream provenance substrate: a dependency-free SHA-256 ([`sha256`])
//! and the hash-chained op-stream ([`chain`]) — the integrity engine behind the
//! notarisation / provenance / relay capabilities. A chain a Rust host computes
//! is byte-identical to an F#/TS host's, so a chain is verifiable across
//! runtimes, and a tamper to any historical record breaks the chain at that
//! record.

pub mod chain;
pub mod replay;
pub mod sha256;
pub mod sink;

pub use chain::{
    Actor, OpRecord, OpResult, OpStream, VerificationError, compute_hash, encode_actor,
    genesis_previous_hash, verify_chain,
};
pub use replay::{ReplayError, replay, replay_stream};
pub use sha256::{sha256, sha256_hex};
pub use sink::{FileSink, InMemorySink, OpStreamSink, SinkError};
