//! The DAG / branching substrate: the multi-parent op-record wire form
//! ([`record`]) and the facet-refined author-agnostic 3-way tree merge
//! ([`merge`]) — the fork / "parallel universes" / reconcile capability behind
//! the Git-for-Interfaces and Counterfactual demos. Both are certified against
//! the shared `dag/` and `merge-conformance/` corpora.
//!
//! Merge by fold (WIRE_FORMAT §34): the base-aware op [`footprint`] with Core's
//! conflict report over two deltas, and the [`reconcile`] policy that derives a
//! merge from them or refuses it with the §34.5 envelope — certified against the
//! `foldFixtures` family of `merge-conformance/`.

pub mod footprint;
pub mod merge;
pub mod reconcile;
pub mod record;

pub use footprint::{
    ConflictShape, Footprint, OpConflict, conflicts_in, footprint_each_in, footprint_in,
    footprint_of,
};
pub use merge::{
    MergeConflict, MergeConflictClass, MergeResult, MergeSide, encode_envelope, merge3_way,
    sort_canonical,
};
pub use reconcile::{
    FoldFault, FoldReconciled, FoldRefusal, FoldRefusalClass, FoldSide, GatedFoldFault, apply_held,
    encode_fold_envelope, reconcile_fold, reconcile_fold_gated, sort_fold_refusals,
};
pub use record::{DagRecord, decode_record, encode_record};
