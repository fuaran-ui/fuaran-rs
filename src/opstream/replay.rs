//! Op-stream replay — folding a verified op-record sequence into a tree via the
//! shared apply engine (the "what state is the UI in?" / Time Machine capability,
//! FGP 5). Replay is the orthogonal concern to verification: [`replay`] drives
//! apply, while [`super::verify_chain`] proves the stream was not tampered with —
//! a caller verifies first, then folds. Snapshot + suffix is expressed by passing
//! a checkpoint snapshot as `initial` and the records after it as the suffix.
//! Mirrors the `fuaran-go` `ApplyTo` / `ReplayStream` semantics.

use crate::ops::{ApplyError, apply};
use crate::wire::Node;

use super::chain::OpRecord;
use super::sink::OpStreamSink;

/// A replay stopped at the first op that failed to apply — carrying the
/// offending record's sequence and the underlying apply error.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayError {
    pub sequence: u64,
    pub error: ApplyError,
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "replay failed at sequence {}: {}",
            self.sequence, self.error
        )
    }
}

impl std::error::Error for ReplayError {}

/// Apply every record to `initial` in order, returning the final tree or the
/// first apply failure (with the offending record's sequence). Resume from a
/// checkpoint by passing its snapshot as `initial` and only the suffix records.
pub fn replay(initial: &Node, records: &[OpRecord]) -> Result<Node, ReplayError> {
    let mut tree = initial.clone();
    for record in records {
        match apply(&tree, &record.op) {
            Ok(outcome) => tree = outcome.new_tree,
            Err(error) => {
                return Err(ReplayError {
                    sequence: record.sequence,
                    error,
                });
            }
        }
    }
    Ok(tree)
}

/// Read records in `[from, to]` from the sink and fold them through the apply
/// engine starting at `initial`. Resume from a checkpoint by passing its snapshot
/// as `initial` and `checkpoint.sequence + 1` as `from`; a `to` of `0` means
/// "up to the sink's latest".
pub fn replay_stream(
    sink: &dyn OpStreamSink,
    initial: &Node,
    from: u64,
    to: u64,
) -> Result<Node, ReplayError> {
    let up_to = if to == 0 { sink.latest_sequence() } else { to };
    replay(initial, &sink.replay(from, up_to))
}
