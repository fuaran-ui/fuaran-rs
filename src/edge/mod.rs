//! **Edge hosting of the certified core** — the session tier a worker-shaped
//! runtime runs, and the durable-store obligations it needs its platform to
//! meet.
//!
//! The [`client`](crate::client) session already holds a tree, renders it and
//! drives it; what it does not hold is a story about *surviving*. A worker
//! runtime is evicted between requests by design, so a session there is a thing
//! that must be able to disappear and come back. This module is that half:
//!
//!   * **one owner per session**, so two activations can never both be writing;
//!   * **the op-stream journaled before the held tree moves**, so a crash
//!     between the two replays to the same tree rather than to a tree nobody
//!     wrote;
//!   * **rehydration from the journal**, checkpoint plus suffix, verified.
//!
//! ## It names obligations, never a platform
//!
//! [`DurableSessionStore`] is a trait with four methods and no vendor in it.
//! Every worker platform with a per-session durable primitive can meet it, and
//! the crate links nothing to find out — [`InMemoryDurableStore`] is the
//! reference implementation, exercised by `tests/edge.rs`, and a real host
//! writes its own against the same four sentences. That is deliberate rather
//! than diplomatic: the fact this tier is about is *the session had exactly one
//! writer and its journal is intact*, which is a property of the protocol below
//! and not of whose storage it lands in.
//!
//! ## Single-owner is the type system's, not a convention
//!
//! An [`EdgeSession`] **owns** its store — the value is moved in — so a second
//! session over the same handle is not something a reviewer has to notice. What
//! ownership cannot reach is a second *process* opening the same session id
//! somewhere else, which is exactly the failure a distributed runtime has, so
//! the store carries an [`ActivationToken`]: [`activate`](EdgeSession::activate)
//! takes a fresh one, every write presents it, and a write from a superseded
//! activation is refused as [`StoreError::NotOwner`] rather than interleaved.
//! A fence the platform enforces is the only kind worth having; this is the
//! shape a host implements, and the reference store implements it fully so the
//! rule is tested rather than described.
//!
//! ## Write-ahead, and what is deliberately not journaled
//!
//! [`apply_op`](EdgeSession::apply_op) proves the op applies, appends the record
//! **durably**, and only then moves the held tree. A refused op appends nothing:
//! the journal is the applied history, and a record whose op does not apply
//! would make replay fail on its own evidence.
//!
//! Reactive slots (`$state` / `$filters` / `$queries`) are **not** journaled and
//! do not survive an eviction. That is a limit, stated: they are a view's live
//! inputs rather than authored history, a host re-seeds them on activation, and
//! journaling them would put a query result in a provenance chain.
//!
//! ## Determinism
//!
//! Nothing here reads a clock, allocates an id, or touches a filesystem. The
//! timestamp on a record is supplied by the caller, exactly as it is in
//! [`opstream`](crate::opstream) — a session whose replay depended on when it
//! ran would not be a session that can be replayed. The whole module compiles
//! for `wasm32` with no target-specific code, which is what makes the browser
//! module and an edge worker the same session type.

use std::collections::BTreeMap;

use crate::client::{ClientError, ClientSession, RowsOutcome};
use crate::ops::apply;
use crate::opstream::{Actor, OpRecord, OpResult, OpStream, VerificationError, verify_chain};
use crate::wire::{DecodeError, decode_op};

/// Which activation of a session a write belongs to.
///
/// Monotonic per session. A store hands out a strictly greater token on every
/// [`acquire`](DurableSessionStore::acquire) and refuses a write presenting
/// anything but the newest, which is what makes "one owner" survive a runtime
/// that can start a second copy of a session it believed was gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ActivationToken(pub u64);

/// A session's tree at a sequence — the fold everything after it replays onto.
///
/// A checkpoint is an optimisation and never a source of truth: a store holding
/// only the journal rehydrates identically, just more slowly. That is why
/// [`EdgeSession::activate`] verifies the chain either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// The sequence this snapshot already includes. `0` means "before the first
    /// record", which is how a seeded session with no history is expressed.
    pub through_sequence: u64,
    /// The snapshot, as canonical wire `Node` JSON.
    pub tree_json: String,
}

/// A durable-store failure. Three cases, and they are not interchangeable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The presented activation is not the session's current owner. The write
    /// did **not** land, and the caller must stop rather than retry: it is
    /// holding a tree some other activation has already moved past.
    NotOwner {
        held: ActivationToken,
        presented: ActivationToken,
    },
    /// The store could not answer. Transient by assumption — the caller may
    /// retry, and `apply_op` has left the held tree untouched so a retry is
    /// safe.
    Unavailable(String),
    /// The store answered with something that is not a session's history. Not
    /// transient, and never silently repaired here.
    Corrupt(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NotOwner { held, presented } => write!(
                f,
                "activation {} is no longer this session's owner (the store holds {})",
                presented.0, held.0
            ),
            StoreError::Unavailable(m) => write!(f, "durable store unavailable: {m}"),
            StoreError::Corrupt(m) => write!(f, "durable store holds a corrupt session: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// **What a worker platform must provide** for a session hosted here to be
/// durable. Four methods, no vendor, no async.
///
/// Synchronous by the same argument [`OpStreamSink`](crate::opstream::OpStreamSink)
/// is: the contract is about *ordering* — the record is durable before the tree
/// moves — and a platform whose storage is asynchronous meets it by awaiting
/// inside its own implementation of these methods. Putting the await in the
/// trait would make the ordering a caller's responsibility, which is the one
/// thing this tier exists to take away.
pub trait DurableSessionStore {
    /// Take ownership of `session`, returning a token strictly greater than any
    /// previously issued for it. Every later write presents it.
    fn acquire(&mut self, session: &str) -> Result<ActivationToken, StoreError>;

    /// The session's journal, ascending from sequence 1. Empty for a session
    /// that has never been written.
    fn journal(&self, session: &str) -> Result<Vec<OpRecord>, StoreError>;

    /// Durably append one record. Returns only once the record will survive the
    /// loss of this activation — that is the whole obligation, and a store that
    /// returns before it holds has broken the ordering everything above rests
    /// on. Refuses a superseded activation with [`StoreError::NotOwner`].
    fn append(
        &mut self,
        session: &str,
        activation: ActivationToken,
        record: &OpRecord,
    ) -> Result<(), StoreError>;

    /// Record a snapshot so a later activation replays a suffix instead of the
    /// whole journal. Refuses a superseded activation.
    fn checkpoint(
        &mut self,
        session: &str,
        activation: ActivationToken,
        checkpoint: &Checkpoint,
    ) -> Result<(), StoreError>;

    /// The newest snapshot, if the store holds one. `None` is a session that
    /// replays from its seed, which is correct and not a fault.
    fn latest_checkpoint(&self, session: &str) -> Result<Option<Checkpoint>, StoreError>;
}

/// An edge-session failure, keeping the layers apart so a caller can tell a
/// malformed request from a lost store from a broken history.
#[derive(Debug, Clone)]
pub enum EdgeError {
    /// The op or the seed tree is not valid canonical wire JSON. Nothing was
    /// journaled.
    Decode(DecodeError),
    /// The op does not apply to the held tree. Nothing was journaled — see the
    /// module note on why a refusal is not history.
    Client(ClientError),
    /// The durable store failed. On an append failure the held tree is
    /// untouched, so the session is still exactly its journal.
    Store(StoreError),
    /// The journal read back does not verify as a chain. The session is not
    /// rehydrated and nothing here repairs it: a host that re-chained an
    /// unverified history would be blessing it rather than recovering it.
    Chain(VerificationError),
    /// A journaled op failed to apply during replay, naming the sequence. A
    /// journal only holds ops that applied once, so this is either a corrupt
    /// store or a checkpoint that does not belong to this journal.
    Replay {
        sequence: u64,
        error: Box<ClientError>,
    },
}

impl std::fmt::Display for EdgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EdgeError::Decode(e) => write!(f, "{e}"),
            EdgeError::Client(e) => write!(f, "{e}"),
            EdgeError::Store(e) => write!(f, "{e}"),
            // `VerificationError` names the break structurally and carries no
            // `Display` of its own, so the debug rendering is what there is.
            EdgeError::Chain(e) => write!(f, "session journal does not verify: {e:?}"),
            EdgeError::Replay { sequence, error } => {
                write!(f, "replay failed at sequence {sequence}: {error}")
            }
        }
    }
}

impl std::error::Error for EdgeError {}

impl From<StoreError> for EdgeError {
    fn from(e: StoreError) -> Self {
        EdgeError::Store(e)
    }
}

/// One activation of one durable session: the held tree, its journal, and the
/// store that outlives both.
///
/// The store is **moved in**, so there is exactly one `EdgeSession` per store
/// handle by construction and the compiler is what enforces it. Recover the
/// handle at the end of an activation with [`into_store`](Self::into_store).
pub struct EdgeSession<S: DurableSessionStore> {
    session_id: String,
    activation: ActivationToken,
    store: S,
    stream: OpStream,
    client: ClientSession,
    actor: Actor,
}

impl<S: DurableSessionStore> EdgeSession<S> {
    /// Activate `session_id` against `store`, rehydrating from whatever the
    /// store holds.
    ///
    /// The order is the point. The activation token is taken **first**, so a
    /// previous activation's in-flight writes are already fenced off before
    /// anything is read; the journal is then verified as a chain before a single
    /// op is applied; and the fold starts from the newest checkpoint the store
    /// holds, or from `seed_tree_json` when it holds none. `seed_tree_json` is
    /// the tree a session begins life with — for a session that has run before
    /// it is unused, which is why a caller may pass the same seed on every
    /// activation without having to know whether this is the first.
    pub fn activate(
        mut store: S,
        session_id: &str,
        seed_tree_json: &str,
        actor: Actor,
    ) -> Result<Self, EdgeError> {
        let activation = store.acquire(session_id)?;
        let records = store.journal(session_id)?;
        verify_chain(&records).map_err(EdgeError::Chain)?;

        let checkpoint = store.latest_checkpoint(session_id)?;
        let (base_json, from_sequence) = match &checkpoint {
            Some(c) => (c.tree_json.as_str(), c.through_sequence),
            None => (seed_tree_json, 0),
        };

        let mut client = ClientSession::new(base_json).map_err(EdgeError::Decode)?;
        for record in records.iter().filter(|r| r.sequence > from_sequence) {
            client
                .apply_decoded(&record.op)
                .map_err(|error| EdgeError::Replay {
                    sequence: record.sequence,
                    error: Box::new(ClientError::Apply(error)),
                })?;
        }

        Ok(EdgeSession {
            session_id: session_id.to_string(),
            activation,
            store,
            stream: OpStream::from_records(records),
            client,
            actor,
        })
    }

    /// The session id this activation owns.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// This activation's fencing token.
    pub fn activation(&self) -> ActivationToken {
        self.activation
    }

    /// The highest journaled sequence; `0` for a session with no history.
    pub fn sequence(&self) -> u64 {
        self.stream.len() as u64
    }

    /// The journal's chain head — the value a peer compares to decide whether it
    /// has seen this session's whole history.
    pub fn head(&self) -> String {
        self.stream.head()
    }

    /// The held tree as canonical wire JSON.
    pub fn tree_json(&self) -> String {
        self.client.tree_json()
    }

    /// The held tree as a resolved projection (see
    /// [`ClientSession::project_resolved`]).
    pub fn project_resolved(&self) -> String {
        self.client.project_resolved()
    }

    /// Render the held tree against the live binding sources.
    pub fn render(&self) -> String {
        self.client.render()
    }

    /// The resolved rows of one row-bearing node (see
    /// [`ClientSession::resolved_rows`]).
    pub fn resolved_rows(&self, node_id: &str) -> RowsOutcome {
        self.client.resolved_rows(node_id)
    }

    /// Write a `$state.<key>` slot. **Not journaled** — see the module note.
    pub fn set_state(&mut self, key: &str, value_json: &str) -> Result<(), EdgeError> {
        self.client
            .set_state(key, value_json)
            .map_err(EdgeError::Client)
    }

    /// Write a `$filters.<name>` slot. **Not journaled.**
    pub fn set_filter(&mut self, name: &str, value_json: &str) -> Result<(), EdgeError> {
        self.client
            .set_filter(name, value_json)
            .map_err(EdgeError::Client)
    }

    /// Seed a `$queries.<name>` result slot. **Not journaled.**
    pub fn set_query(&mut self, name: &str, value_json: &str) -> Result<(), EdgeError> {
        self.client
            .set_query(name, value_json)
            .map_err(EdgeError::Client)
    }

    /// Apply a canonical wire `TreeOp`, journaling it durably first.
    ///
    /// Four steps, and the order is the durability claim:
    ///
    /// 1. decode the op — a malformed op journals nothing;
    /// 2. apply it against the held tree to learn whether it applies at all, and
    ///    keep the result;
    /// 3. append the record to the durable store — this is the acknowledgement
    ///    boundary, and a failure here leaves the held tree exactly where the
    ///    journal says it is;
    /// 4. adopt the tree step 2 computed.
    ///
    /// A crash between 3 and 4 loses nothing: the next activation replays the
    /// record and arrives at the same tree, because apply is a total function of
    /// the tree and the op and reads nothing else.
    pub fn apply_op(
        &mut self,
        op_json: &str,
        timestamp_unix_seconds: i64,
        prompt_id: Option<String>,
    ) -> Result<(), EdgeError> {
        let op = decode_op(op_json).map_err(EdgeError::Decode)?;
        let outcome =
            apply(self.client.tree(), &op).map_err(|e| EdgeError::Client(ClientError::Apply(e)))?;

        // The record is built against the CURRENT head, appended, and only then
        // adopted into the stream — so a store refusal leaves the in-memory
        // chain unmoved as well as the tree.
        let previous_hash = self.stream.head();
        let sequence = self.stream.len() as u64 + 1;
        let hash = crate::opstream::compute_hash(
            &previous_hash,
            &op,
            sequence,
            timestamp_unix_seconds,
            &self.actor,
            prompt_id.as_deref(),
            &OpResult::Success,
        );
        let record = OpRecord {
            sequence,
            op,
            timestamp_unix_seconds,
            actor: self.actor.clone(),
            prompt_id,
            result: OpResult::Success,
            previous_hash,
            hash,
        };

        self.store
            .append(&self.session_id, self.activation, &record)?;

        self.stream = OpStream::from_records(
            self.stream
                .records()
                .iter()
                .cloned()
                .chain(std::iter::once(record))
                .collect(),
        );
        self.client.adopt_tree(outcome.new_tree);
        Ok(())
    }

    /// Snapshot the held tree at the current sequence, so the next activation
    /// replays a suffix.
    ///
    /// Purely an optimisation: dropping every checkpoint a store holds changes
    /// no rehydrated tree, only how long rehydration takes. A host calls it on
    /// whatever policy suits it — every N ops, or before an eviction it can see
    /// coming.
    pub fn checkpoint(&mut self) -> Result<Checkpoint, EdgeError> {
        let checkpoint = Checkpoint {
            through_sequence: self.sequence(),
            tree_json: self.client.tree_json(),
        };
        self.store
            .checkpoint(&self.session_id, self.activation, &checkpoint)?;
        Ok(checkpoint)
    }

    /// Verify the in-memory journal. A host runs it on rehydration for free
    /// (activation already does), and may run it again before handing a head to
    /// a peer.
    pub fn verify(&self) -> Result<(), VerificationError> {
        self.stream.verify()
    }

    /// The journal this activation holds.
    pub fn records(&self) -> &[OpRecord] {
        self.stream.records()
    }

    /// Borrow the store — for a host that reads its own bookkeeping.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// End the activation and recover the store handle.
    pub fn into_store(self) -> S {
        self.store
    }
}

// ─── The reference store ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct SessionState {
    activation: u64,
    records: Vec<OpRecord>,
    checkpoint: Option<Checkpoint>,
}

/// The in-memory reference implementation of [`DurableSessionStore`].
///
/// It is the *reference*, not a toy: it implements the fence, refuses a
/// superseded activation, and keeps the journal append-only, so the protocol
/// above is exercised by `tests/edge.rs` rather than merely specified. What it
/// deliberately is not is durable — it lives in the process it is created in, so
/// a host uses it for tests and conformance and writes its own for anything
/// whose survival matters.
///
/// A `BTreeMap` rather than a hash map so an iteration order exists at all,
/// which is the same reason everything else in this crate that can be ordered
/// is.
#[derive(Debug, Clone, Default)]
pub struct InMemoryDurableStore {
    sessions: BTreeMap<String, SessionState>,
}

impl InMemoryDurableStore {
    pub fn new() -> Self {
        InMemoryDurableStore::default()
    }

    /// The session ids this store holds.
    pub fn session_ids(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }

    fn owner(&self, session: &str) -> ActivationToken {
        ActivationToken(
            self.sessions
                .get(session)
                .map(|s| s.activation)
                .unwrap_or(0),
        )
    }

    fn check_owner(&self, session: &str, presented: ActivationToken) -> Result<(), StoreError> {
        let held = self.owner(session);
        if held == presented {
            Ok(())
        } else {
            Err(StoreError::NotOwner { held, presented })
        }
    }
}

impl DurableSessionStore for InMemoryDurableStore {
    fn acquire(&mut self, session: &str) -> Result<ActivationToken, StoreError> {
        let state = self.sessions.entry(session.to_string()).or_default();
        state.activation += 1;
        Ok(ActivationToken(state.activation))
    }

    fn journal(&self, session: &str) -> Result<Vec<OpRecord>, StoreError> {
        Ok(self
            .sessions
            .get(session)
            .map(|s| s.records.clone())
            .unwrap_or_default())
    }

    fn append(
        &mut self,
        session: &str,
        activation: ActivationToken,
        record: &OpRecord,
    ) -> Result<(), StoreError> {
        self.check_owner(session, activation)?;
        let state = self
            .sessions
            .get_mut(session)
            .ok_or_else(|| StoreError::Corrupt(format!("no session '{session}'")))?;
        if state.records.iter().any(|r| r.sequence == record.sequence) {
            return Err(StoreError::Corrupt(format!(
                "sequence {} is already journaled",
                record.sequence
            )));
        }
        state.records.push(record.clone());
        Ok(())
    }

    fn checkpoint(
        &mut self,
        session: &str,
        activation: ActivationToken,
        checkpoint: &Checkpoint,
    ) -> Result<(), StoreError> {
        self.check_owner(session, activation)?;
        let state = self
            .sessions
            .get_mut(session)
            .ok_or_else(|| StoreError::Corrupt(format!("no session '{session}'")))?;
        state.checkpoint = Some(checkpoint.clone());
        Ok(())
    }

    fn latest_checkpoint(&self, session: &str) -> Result<Option<Checkpoint>, StoreError> {
        Ok(self
            .sessions
            .get(session)
            .and_then(|s| s.checkpoint.clone()))
    }
}
