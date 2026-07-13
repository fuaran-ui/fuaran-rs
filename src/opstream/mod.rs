//! The op-stream provenance substrate: a dependency-free SHA-256 ([`sha256`])
//! and the hash-chained op-stream ([`chain`]) — the integrity engine behind the
//! notarisation / provenance / relay capabilities. A chain a Rust host computes
//! is byte-identical to an F#/TS host's, so a chain is verifiable across
//! runtimes, and a tamper to any historical record breaks the chain at that
//! record.

pub mod chain;
pub mod sha256;

pub use chain::{
    Actor, OpRecord, OpResult, OpStream, VerificationError, compute_hash, genesis_previous_hash,
    verify_chain,
};
pub use sha256::{sha256, sha256_hex};
