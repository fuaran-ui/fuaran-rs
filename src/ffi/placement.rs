//! The session-level placement verbs of the C-ABI: place, nudge, duplicate,
//! paste.
//!
//! # Why these live on the ABI at all
//!
//! The adopted architecture drives all native rendering through a session, and
//! the native surfaces over this core are **decode-only render projections** —
//! every structural edit they perform goes through the session. The placement
//! algebra ([`crate::ops::placement`]) is what turns "put this node there" into
//! the positionless op vocabulary, so a native surface without these entry
//! points would have to reimplement the algebra to author a placed insert. It
//! would then be a *second* implementation of a semantics this crate is the
//! reference for, which is the outcome the C-ABI exists to prevent.
//!
//! # Shape
//!
//! Each verb takes ONE canonical-JSON request document (a `(ptr, len)` UTF-8
//! buffer per the [module memory contract](super)) rather than a widening list
//! of `(ptr, len)` pairs — a placement carries a parent, a placement case, an
//! optional anchor and, for two of the verbs, a whole node. The request is the
//! same document shape across all four verbs, so a binding writes one encoder.
//!
//! ```text
//! place     { "parentId": …, "placement": "Last"|"First"|"Before"|"After",
//!             "anchor"?: …, "child": { …node… } }
//! paste     { …target…, "subtree": { …node… }, "idPrefix"?: … }
//! duplicate { …target…, "source": …, "idPrefix"?: … }
//! nudge     { "target": …, "delta": ±n }
//! ```
//!
//! `anchor` is REQUIRED for `Before` / `After` and REFUSED for `Last` / `First`:
//! a caller that supplied an anchor a verb would silently drop has stated an
//! intent that is not being honoured, and saying so is cheaper than a placement
//! that lands somewhere else.
//!
//! `idPrefix` selects the **deterministic** fresh-id strategy
//! ([`SequentialIds`] — `<prefix>-1`, `-2`, …) for the clone verbs; omitting it
//! takes the derived default (`<oldId>-copy`). It is the replay/test option the
//! algebra's id seam exists for, reachable from a binding rather than only from
//! Rust.
//!
//! # Result
//!
//! On success the session adopts the new tree and the call returns
//! `{"ok":true,"op":<the emitted canonical TreeOp>}`. The op is handed back
//! because it is the artefact the verb computed: a host that journals its
//! op-stream (or replays, or diffs) needs the op, and it cannot re-derive one
//! from the resulting tree. On failure the held tree is UNTOUCHED and the call
//! returns the same error envelope the rest of the surface returns, with a
//! `"placement"` class carrying the [`PlaceError`] code.

use crate::canonical::{JVal, parse, render_canonical};
use crate::client::ClientSession;
use crate::ops::placement::{
    DerivedIds, FreshIds, PlaceError, Placement, SequentialIds, Target, duplicate_op_with,
    nudge_op, paste_op_with, place_op,
};
use crate::wire::{Node, TreeOp, decode_node, encode_op};

use super::{FuaranBuf, borrow_str, pack_string};

/// A failure of one placement call, in the order a caller meets them.
enum VerbError {
    /// The request document could not be read as one.
    Request(String),
    /// The placement algebra refused, naming the apply-side refusal the emitted
    /// op would have met.
    Place(PlaceError),
    /// The emitted op did not apply. Reported rather than swallowed: the whole
    /// claim of the algebra is that its ops pass the standard apply gate, so
    /// this envelope means the claim failed, and a caller must be able to see
    /// that rather than read a generic refusal.
    Apply(crate::ops::ApplyError),
}

fn envelope(class: &str, code: &str, message: &str) -> String {
    let message = render_canonical(&JVal::Str(message.to_string()));
    format!("{{\"error\":{{\"class\":\"{class}\",\"code\":\"{code}\",\"message\":{message}}}}}")
}

impl VerbError {
    fn to_json(&self) -> String {
        match self {
            VerbError::Request(m) => envelope("request", "INVALID_REQUEST", m),
            VerbError::Place(e) => envelope("placement", e.code(), &e.to_string()),
            VerbError::Apply(e) => envelope("apply", e.code.as_str(), &e.message),
        }
    }
}

impl From<PlaceError> for VerbError {
    fn from(e: PlaceError) -> Self {
        VerbError::Place(e)
    }
}

fn required_str(document: &JVal, key: &str) -> Result<String, VerbError> {
    match document.field(key) {
        Some(JVal::Str(s)) => Ok(s.clone()),
        _ => Err(VerbError::Request(format!(
            "the request carries no string '{key}'"
        ))),
    }
}

fn optional_str(document: &JVal, key: &str) -> Result<Option<String>, VerbError> {
    match document.field(key) {
        None | Some(JVal::Null) => Ok(None),
        Some(JVal::Str(s)) => Ok(Some(s.clone())),
        Some(_) => Err(VerbError::Request(format!(
            "the request carries a non-string '{key}'"
        ))),
    }
}

/// A node member, re-rendered canonically and read back through this host's own
/// decoder — so a malformed node is refused by the codec that owns that
/// judgement, never by a second reading here.
fn node_member(document: &JVal, key: &str) -> Result<Node, VerbError> {
    let Some(value) = document.field(key) else {
        return Err(VerbError::Request(format!(
            "the request carries no node '{key}'"
        )));
    };
    decode_node(&render_canonical(value)).map_err(|e| {
        VerbError::Request(format!(
            "the '{key}' node does not decode: {} at {}",
            e.message, e.path
        ))
    })
}

/// The destination, read from `parentId` / `placement` / `anchor`.
fn target_of(document: &JVal) -> Result<Target, VerbError> {
    let parent_id = required_str(document, "parentId")?;
    let case = required_str(document, "placement")?;
    let anchor = optional_str(document, "anchor")?;
    let anchored = |a: Option<String>| -> Result<String, VerbError> {
        a.ok_or_else(|| {
            VerbError::Request(format!("placement '{case}' requires an 'anchor' node id"))
        })
    };
    let placement = match case.as_str() {
        "Last" | "First" => {
            if anchor.is_some() {
                return Err(VerbError::Request(format!(
                    "placement '{case}' takes no 'anchor'"
                )));
            }
            if case == "Last" {
                Placement::Last
            } else {
                Placement::First
            }
        }
        "Before" => Placement::Before(anchored(anchor)?),
        "After" => Placement::After(anchored(anchor)?),
        other => {
            return Err(VerbError::Request(format!(
                "'{other}' is not a placement — expected Last | First | Before | After"
            )));
        }
    };
    Ok(Target {
        parent_id,
        placement,
    })
}

/// The clone verbs' id strategy: the deterministic sequential minter when the
/// request names an `idPrefix`, else the derived default.
fn id_strategy(document: &JVal) -> Result<Box<dyn FreshIds>, VerbError> {
    Ok(match optional_str(document, "idPrefix")? {
        None => Box::new(DerivedIds),
        Some(prefix) => Box::new(SequentialIds::new(prefix)),
    })
}

/// Apply an emitted op to the session and render the success envelope. The op
/// rides back with the verdict — see the module header.
fn adopt(session: &mut ClientSession, op: &TreeOp) -> Result<String, VerbError> {
    session.apply_decoded(op).map_err(VerbError::Apply)?;
    Ok(format!("{{\"ok\":true,\"op\":{}}}", encode_op(op)))
}

/// Run one verb over a borrowed session and a request document.
fn run(
    session: &mut ClientSession,
    request: &str,
    verb: fn(&ClientSession, &JVal) -> Result<TreeOp, VerbError>,
) -> String {
    let document = match parse(request) {
        Ok(d) => d,
        Err(e) => {
            return VerbError::Request(format!("the request does not parse: {}", e.message))
                .to_json();
        }
    };
    match verb(session, &document).and_then(|op| adopt(session, &op)) {
        Ok(ok) => ok,
        Err(e) => e.to_json(),
    }
}

fn place_verb(session: &ClientSession, document: &JVal) -> Result<TreeOp, VerbError> {
    let target = target_of(document)?;
    let child = node_member(document, "child")?;
    Ok(place_op(session.tree(), &child, &target)?)
}

fn paste_verb(session: &ClientSession, document: &JVal) -> Result<TreeOp, VerbError> {
    let target = target_of(document)?;
    let subtree = node_member(document, "subtree")?;
    let mut fresh = id_strategy(document)?;
    Ok(paste_op_with(
        fresh.as_mut(),
        session.tree(),
        &subtree,
        &target,
    )?)
}

fn duplicate_verb(session: &ClientSession, document: &JVal) -> Result<TreeOp, VerbError> {
    let target = target_of(document)?;
    let source = required_str(document, "source")?;
    let mut fresh = id_strategy(document)?;
    Ok(duplicate_op_with(
        fresh.as_mut(),
        session.tree(),
        &source,
        &target,
    )?)
}

fn nudge_verb(session: &ClientSession, document: &JVal) -> Result<TreeOp, VerbError> {
    let node_id = required_str(document, "target")?;
    let delta = match document.field("delta") {
        Some(JVal::Num(n)) if n.fract() == 0.0 && n.abs() <= f64::from(i32::MAX) => *n as i32,
        Some(JVal::Num(_)) => {
            return Err(VerbError::Request(
                "'delta' must be a whole number of sibling positions".to_string(),
            ));
        }
        _ => {
            return Err(VerbError::Request(
                "the request carries no number 'delta'".to_string(),
            ));
        }
    };
    Ok(nudge_op(session.tree(), &node_id, delta)?)
}

/// # Safety
/// `session` must be a live handle; `ptr`/`len` a live UTF-8 buffer.
unsafe fn dispatch(
    session: *mut ClientSession,
    ptr: *const u8,
    len: usize,
    verb: fn(&ClientSession, &JVal) -> Result<TreeOp, VerbError>,
) -> FuaranBuf {
    if session.is_null() {
        return pack_string(String::new());
    }
    // SAFETY: caller contract — a live handle under single-owner confinement.
    let session = unsafe { &mut *session };
    let Some(request) = (unsafe { borrow_str(ptr, len) }) else {
        return pack_string(envelope(
            "request",
            "INVALID_REQUEST",
            "the request is not valid UTF-8",
        ));
    };
    pack_string(run(session, request, verb))
}

/// Place a node among a parent's children: `{"parentId":…,"placement":…,
/// "anchor"?:…,"child":{…node…}}`. Returns `{"ok":true,"op":…}` — the session
/// adopts the new tree — or an error envelope, the held tree untouched. NULL
/// session → empty buffer.
///
/// # Safety
/// `session` must be a live handle from `fuaran_session_new`; `ptr`/`len` a live
/// UTF-8 buffer per the memory contract in [`crate::ffi`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_place(
    session: *mut ClientSession,
    ptr: *const u8,
    len: usize,
) -> FuaranBuf {
    // SAFETY: caller contract.
    unsafe { dispatch(session, ptr, len, place_verb) }
}

/// Move a node one or more sibling positions: `{"target":…,"delta":±n}`. Emits
/// the full sibling permutation as one `ReorderChildren`.
///
/// # Safety
/// As [`fuaran_session_place`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_nudge(
    session: *mut ClientSession,
    ptr: *const u8,
    len: usize,
) -> FuaranBuf {
    // SAFETY: caller contract.
    unsafe { dispatch(session, ptr, len, nudge_verb) }
}

/// Duplicate a subtree already in the session's tree and place the clone:
/// `{"source":…,"parentId":…,"placement":…,"anchor"?:…,"idPrefix"?:…}`. Every
/// id in the clone is remapped to a fresh, collision-free one.
///
/// # Safety
/// As [`fuaran_session_place`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_duplicate(
    session: *mut ClientSession,
    ptr: *const u8,
    len: usize,
) -> FuaranBuf {
    // SAFETY: caller contract.
    unsafe { dispatch(session, ptr, len, duplicate_verb) }
}

/// Place a subtree lifted from ANOTHER tree: `{"subtree":{…node…},"parentId":…,
/// "placement":…,"anchor"?:…,"idPrefix"?:…}`. Ids that collide with the target
/// tree are remapped; ids that do not are preserved.
///
/// # Safety
/// As [`fuaran_session_place`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_paste(
    session: *mut ClientSession,
    ptr: *const u8,
    len: usize,
) -> FuaranBuf {
    // SAFETY: caller contract.
    unsafe { dispatch(session, ptr, len, paste_verb) }
}
