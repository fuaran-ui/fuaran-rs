//! The DAG / branching substrate: the multi-parent op-record wire form
//! ([`record`]) and the facet-refined author-agnostic 3-way tree merge
//! ([`merge`]) — the fork / "parallel universes" / reconcile capability behind
//! the Git-for-Interfaces and Counterfactual demos. Both are certified against
//! the shared `dag/` and `merge-conformance/` corpora.

pub mod merge;
pub mod record;

pub use merge::{MergeConflict, MergeResult, merge3_way};
pub use record::{DagRecord, decode_record, encode_record};
