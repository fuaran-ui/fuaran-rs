//! The target-neutral C-ABI: a minimal `extern "C"` export surface over the
//! target-agnostic [`ClientSession`](crate::client::ClientSession), moving UTF-8
//! bytes across the boundary through explicit [`fuaran_alloc`] / [`fuaran_dealloc`]
//! and driving a session held as an opaque handle. It is dependency-free (no
//! `wasm-bindgen` / `web-sys` / `js-sys`, no third-party crate) and compiles for
//! **every** target: the browser-native `wasm32` client (the thin JS loader
//! `js/fuaran-loader.js` instantiates it) *and* native staticlib / cdylib
//! consumers (the Swift XCFramework and Kotlin cargo-ndk `.so` packaging tiers
//! bind these same symbols). See `include/fuaran.h` for the C declarations and
//! the full ownership + threading contract.
//!
//! This module was factored out of `src/client/wasm.rs` (Phase 537): nothing in
//! the surface is `wasm32`-specific — the WASM entry points are these exact
//! symbols, re-exported at their historical path by
//! [`crate::client::wasm`](../client/wasm/index.html) for the browser build.
//!
//! **Memory contract.** Buffers are exact-length boxed byte slices:
//! - *Input* buffers are owned by the caller — `fuaran_alloc(len)` hands the
//!   caller a zeroed buffer to write into; the caller frees it with
//!   `fuaran_dealloc(ptr, len)` after the call. Rust only *borrows* an input
//!   buffer for the duration of a call.
//! - *Output* buffers are owned by Rust — a returning function leaks an exact
//!   boxed slice and returns a [`FuaranBuf`] `(ptr, len)` pair; the caller reads
//!   the UTF-8, then frees it with the same `fuaran_dealloc(ptr, len)`.
//!
//! **The `(ptr, len)` return ABI is pointer-width dependent** (see [`FuaranBuf`]):
//! on `wasm32` (32-bit pointers) it is a packed `u64` — `ptr` in the high 32
//! bits, `len` in the low 32 — so the thin JS loader stays byte-for-byte
//! unchanged. On native 64-bit targets a 64-bit pointer cannot share a `u64`
//! with a length, so the pair is returned as a `#[repr(C)]` two-word struct
//! `{ uint8_t* ptr; size_t len; }` — the shape the Swift / Kotlin native
//! bindings consume. The logical contract ("Rust owns the buffer; free it with
//! `fuaran_dealloc(ptr, len)`") is identical either way; only the register-level
//! packing differs, because the packed-`u64` form is only lossless when pointers
//! fit in 32 bits.
//!
//! All exported functions are `extern "C"` with stable, prefixed names.
//!
//! **Threading / ownership contract.** A [`ClientSession`] handle is
//! **single-owner**: it must be confined to one thread / executor for its whole
//! lifetime (no `Send`/`Sync` guarantee is made across the boundary — a Swift
//! `actor` or a Kotlin single-threaded dispatcher is the intended confinement).
//! The last-error slot ([`fuaran_last_error`]) is a `thread_local`, so it is
//! **per-thread**: read it on the *same* thread that made the failing
//! `fuaran_session_new` call. Under the single-owner confinement this is exactly
//! the right visibility — concurrent sessions on different threads never clobber
//! each other's last error. (The `wasm32` browser host is single-threaded, so
//! the `thread_local` degenerates to a single global there.)

use std::cell::RefCell;

use crate::canonical::{JVal, render_canonical};
use crate::client::{ClientError, ClientSession, RowsOutcome};

/// The bounded loop's conformance-harness entry point — feature-gated and
/// OFF by default, so it imposes no obligation on the stable surface
/// `include/fuaran.h` declares. See the module header for why it exists.
#[cfg(feature = "driver-semantics-abi")]
pub mod bounded;
/// The session-level placement verbs — place / nudge / duplicate / paste over
/// [`crate::ops::placement`]. Part of the default export surface
/// `include/fuaran.h` declares, on every target.
pub mod placement;
pub mod rosetta;

// The placement verbs are `extern "C"` symbols like every other entry point on
// this surface, so they are reachable at `ffi::…` from Rust too — one import
// path for the whole ABI, whichever file the body happens to live in.
pub use placement::{
    fuaran_session_duplicate, fuaran_session_nudge, fuaran_session_paste, fuaran_session_place,
};

thread_local! {
    /// The last `fuaran_session_new` decode failure, as its JSON envelope —
    /// read once via [`fuaran_last_error`] when `new` returns null. Per-thread:
    /// the failing `new` and the `last_error` read must be on the same thread.
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// A Rust-owned output buffer handed back across the C-ABI as a `(ptr, len)`
/// pair. **The ABI representation is pointer-width dependent** so the packed form
/// is only used where it is lossless:
///
/// - On `wasm32` (32-bit pointers) it is a **packed `u64`** — `ptr` in the high
///   32 bits, `len` in the low 32. This is the exact form the thin JS loader has
///   always read, so the browser ABI is byte-for-byte unchanged.
/// - On native targets (64-bit pointers) a full pointer cannot share a `u64`
///   with a length without truncation, so it is a `#[repr(C)]` two-word struct
///   `{ ptr, len }` — the form the Swift / Kotlin native bindings consume.
///
/// Either way the caller reads `len` UTF-8 bytes at `ptr`, then frees the buffer
/// with `fuaran_dealloc(ptr, len)`.
#[cfg(target_arch = "wasm32")]
pub type FuaranBuf = u64;

/// A Rust-owned output buffer: see the `wasm32` alias above for the full
/// contract. On native targets this is the two-word `{ ptr, len }` struct form.
#[cfg(not(target_arch = "wasm32"))]
#[repr(C)]
pub struct FuaranBuf {
    /// Pointer to `len` UTF-8 bytes, Rust-owned; free with `fuaran_dealloc`.
    pub ptr: *mut u8,
    /// Byte length of the buffer at `ptr`.
    pub len: usize,
}

/// Build a [`FuaranBuf`] from a raw `(ptr, len)` pair — the packed `u64` on
/// `wasm32`, the two-word struct on native.
#[cfg(target_arch = "wasm32")]
fn make_buf(ptr: *mut u8, len: usize) -> FuaranBuf {
    ((ptr as u64) << 32) | (len as u64)
}

/// Build a [`FuaranBuf`] from a raw `(ptr, len)` pair (native two-word struct).
#[cfg(not(target_arch = "wasm32"))]
fn make_buf(ptr: *mut u8, len: usize) -> FuaranBuf {
    FuaranBuf { ptr, len }
}

/// Leak an exact-length boxed byte slice from a `String`, returning the owned
/// `(ptr, len)` [`FuaranBuf`]. The caller reads `len` UTF-8 bytes at `ptr`, then
/// frees via [`fuaran_dealloc`].
pub(crate) fn pack_string(s: String) -> FuaranBuf {
    let boxed = s.into_bytes().into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *mut u8;
    make_buf(ptr, len)
}

/// Borrow an input buffer the caller wrote (via [`fuaran_alloc`]) as a `&str`.
/// Returns `None` on invalid UTF-8.
///
/// # Safety
/// `ptr` must point at `len` initialised bytes that outlive the borrow — true
/// for a caller-owned `fuaran_alloc` buffer that the caller frees only after the
/// call.
pub(crate) unsafe fn borrow_str<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if ptr.is_null() {
        return Some("");
    }
    // SAFETY: caller contract — `ptr`/`len` name a live initialised buffer.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes).ok()
}

/// Allocate a zeroed input buffer of `len` bytes; the caller writes UTF-8 into
/// it and frees it with [`fuaran_dealloc`] after the call that consumes it.
#[unsafe(no_mangle)]
pub extern "C" fn fuaran_alloc(len: usize) -> *mut u8 {
    let boxed = vec![0u8; len].into_boxed_slice();
    Box::into_raw(boxed) as *mut u8
}

/// Free a buffer (an input buffer, or an output buffer returned packed). `ptr`
/// / `len` must be a pair this module produced.
///
/// # Safety
/// `ptr`/`len` must originate from `fuaran_alloc` or a packed return of this
/// module, freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: reconstruct the exact boxed slice we leaked, then drop it.
    let slice = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
    drop(unsafe { Box::from_raw(slice as *mut [u8]) });
}

/// Decode a canonical wire `Node` JSON into a new session. Returns an opaque
/// session handle, or null on a decode failure — in which case
/// [`fuaran_last_error`] returns the failure's JSON envelope.
///
/// # Safety
/// `ptr`/`len` must name a live UTF-8 buffer per the memory contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_new(ptr: *const u8, len: usize) -> *mut ClientSession {
    // SAFETY: caller contract.
    let Some(json) = (unsafe { borrow_str(ptr, len) }) else {
        LAST_ERROR.with(|e| {
            *e.borrow_mut() = Some(
                "{\"error\":{\"class\":\"decode\",\"code\":\"INVALID_JSON\",\"message\":\"input is not valid UTF-8\",\"path\":\"$\"}}"
                    .to_string(),
            );
        });
        return std::ptr::null_mut();
    };
    match ClientSession::new(json) {
        Ok(session) => {
            LAST_ERROR.with(|e| *e.borrow_mut() = None);
            Box::into_raw(Box::new(session))
        }
        Err(err) => {
            let envelope = ClientError::Decode(err).to_json();
            LAST_ERROR.with(|e| *e.borrow_mut() = Some(envelope));
            std::ptr::null_mut()
        }
    }
}

/// The last `fuaran_session_new` failure envelope (packed), or an empty string
/// when the last `new` succeeded. Per-thread — read on the thread that called
/// the failing `new`.
#[unsafe(no_mangle)]
pub extern "C" fn fuaran_last_error() -> FuaranBuf {
    LAST_ERROR.with(|e| pack_string(e.borrow().clone().unwrap_or_default()))
}

/// Free a session handle.
///
/// # Safety
/// `session` must be a handle from `fuaran_session_new`, freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_free(session: *mut ClientSession) {
    if !session.is_null() {
        // SAFETY: reconstruct the box we leaked, then drop it.
        drop(unsafe { Box::from_raw(session) });
    }
}

/// Render the session's current tree to a body-fragment HTML string (packed).
///
/// # Safety
/// `session` must be a live handle from `fuaran_session_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_render(session: *mut ClientSession) -> FuaranBuf {
    if session.is_null() {
        return pack_string(String::new());
    }
    // SAFETY: caller contract — live handle, single-owner confinement.
    let session = unsafe { &*session };
    pack_string(session.render())
}

/// The session's current tree, re-encoded to canonical wire JSON (packed).
///
/// # Safety
/// `session` must be a live handle from `fuaran_session_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_tree_json(session: *mut ClientSession) -> FuaranBuf {
    if session.is_null() {
        return pack_string(String::new());
    }
    // SAFETY: caller contract.
    let session = unsafe { &*session };
    pack_string(session.tree_json())
}

/// The session's current tree as a **resolved projection**, re-encoded to
/// canonical wire JSON (packed) — Phase 650. Identical to
/// [`fuaran_session_tree_json`] except every scalar-slot `Binding.Transform` is
/// folded to the concrete value it evaluates to against the live sources, so a
/// decode-only consumer renders already-resolved compute values without an
/// evaluator. Additive — `tree_json` is unchanged; call this instead when the
/// consumer cannot itself resolve `Transform`.
///
/// # Safety
/// `session` must be a live handle from `fuaran_session_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_project_resolved(session: *mut ClientSession) -> FuaranBuf {
    if session.is_null() {
        return pack_string(String::new());
    }
    // SAFETY: caller contract.
    let session = unsafe { &*session };
    pack_string(session.project_resolved())
}

/// The **resolved rows** of one row-bearing node (`DataGrid` / `Chart` / `Map` /
/// `Sparkline`), addressed by node id and evaluated against the live sources —
/// packed, canonical JSON.
///
/// The out-of-band companion to [`fuaran_session_project_resolved`], and it
/// exists because that projection *cannot* carry this: a row-context `Transform`
/// resolves to a *collection*, and the wire's `Static` slot erases a COMPUTED
/// collection to `"<opaque>"` (§2 rule 11), so resolved rows cannot ride the
/// tree. A decode-only consumer therefore cannot obtain them from `tree_json`
/// however much of the tree it understands, and renders every data-bound grid
/// empty. This hands them over directly, keeping the division the tiers rest on:
/// this core evaluates, the native surface renders.
///
/// fuaran#665 narrowed *which* sources that argument covers, and did not retire
/// it: an AUTHORED rows feed (`Static` / `State`) is now typed on the wire and
/// does ride the tree, but a `Transform`- or `Query`-sourced feed still resolves
/// only here. Both arrive through this one call, so a consumer needs no
/// per-source knowledge — which is why the call is addressed by node id rather
/// than by binding case.
///
/// Three distinct results, and a consumer MUST tell them apart:
///
/// * `{"resolved":true,"rows":[…]}` — resolved. A zero-length `rows` is an empty
///   state; render it as one.
/// * `{"resolved":false}` — the source did not resolve (a `Transform` that
///   errored, or a store not yet fed). Render the LOADING surface, **not** an
///   empty table. Flattening this to zero rows shows "no data" for "not yet".
/// * `{"error":{"class":"lookup","code":"NO_ROW_SOURCE","message":…}}` — no node
///   with that id, or a kind that carries no row source. A caller mistake.
///
/// # Safety
/// `session` must be a live handle; `ptr`/`len` a live UTF-8 buffer naming the
/// node id.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_resolved_rows(
    session: *mut ClientSession,
    ptr: *const u8,
    len: usize,
) -> FuaranBuf {
    if session.is_null() {
        return pack_string(String::new());
    }
    // SAFETY: caller contract.
    let session = unsafe { &*session };
    let Some(node_id) = (unsafe { borrow_str(ptr, len) }) else {
        return pack_string(invalid_utf8_envelope());
    };
    let json = match session.resolved_rows(node_id) {
        RowsOutcome::Rows(rows) => render_canonical(&JVal::Obj(vec![
            ("resolved".to_string(), JVal::Bool(true)),
            ("rows".to_string(), JVal::Arr(rows)),
        ])),
        RowsOutcome::NotResolved => render_canonical(&JVal::Obj(vec![(
            "resolved".to_string(),
            JVal::Bool(false),
        )])),
        RowsOutcome::NoRowSource => no_row_source_envelope(node_id),
    };
    pack_string(json)
}

/// The lookup-failure envelope, shaped like the decode/apply ones the other
/// entry points return so a consumer parses one error shape, not three.
fn no_row_source_envelope(node_id: &str) -> String {
    let message = render_canonical(&JVal::Str(format!(
        "no node '{node_id}', or its kind carries no row source \
         (DataGrid | Chart | Map | Sparkline)"
    )));
    format!(
        "{{\"error\":{{\"class\":\"lookup\",\"code\":\"NO_ROW_SOURCE\",\"message\":{message}}}}}"
    )
}

/// The success result JSON both mutating entry points return on `Ok`.
const OK_RESULT: &str = "{\"ok\":true}";

/// Apply a canonical wire `TreeOp` JSON. Returns `{"ok":true}` on success (the
/// session adopts the new tree) or the structured error envelope on failure
/// (the held tree is untouched) — both packed.
///
/// # Safety
/// `session` must be a live handle; `ptr`/`len` a live UTF-8 buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_apply_op(
    session: *mut ClientSession,
    ptr: *const u8,
    len: usize,
) -> FuaranBuf {
    if session.is_null() {
        return pack_string(String::new());
    }
    // SAFETY: caller contract.
    let session = unsafe { &mut *session };
    let Some(op_json) = (unsafe { borrow_str(ptr, len) }) else {
        return pack_string(invalid_utf8_envelope());
    };
    match session.apply_op(op_json) {
        Ok(()) => pack_string(OK_RESULT.to_string()),
        Err(err) => pack_string(err.to_json()),
    }
}

/// Write a `$state.<key>` slot from a JSON value. `{"ok":true}` or an error
/// envelope, packed. Re-render to observe the change.
///
/// # Safety
/// `session` must be live; the four `ptr`/`len` pairs live UTF-8 buffers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_set_state(
    session: *mut ClientSession,
    key_ptr: *const u8,
    key_len: usize,
    val_ptr: *const u8,
    val_len: usize,
) -> FuaranBuf {
    // SAFETY: caller contract.
    unsafe {
        store_write(
            session,
            key_ptr,
            key_len,
            val_ptr,
            val_len,
            StoreKind::State,
        )
    }
}

/// Write a `$filters.<name>` slot from a JSON value.
///
/// # Safety
/// As [`fuaran_session_set_state`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_set_filter(
    session: *mut ClientSession,
    key_ptr: *const u8,
    key_len: usize,
    val_ptr: *const u8,
    val_len: usize,
) -> FuaranBuf {
    // SAFETY: caller contract.
    unsafe {
        store_write(
            session,
            key_ptr,
            key_len,
            val_ptr,
            val_len,
            StoreKind::Filter,
        )
    }
}

/// Seed a `$queries.<name>` result slot from a JSON value (host-fed data).
///
/// # Safety
/// As [`fuaran_session_set_state`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_set_query(
    session: *mut ClientSession,
    key_ptr: *const u8,
    key_len: usize,
    val_ptr: *const u8,
    val_len: usize,
) -> FuaranBuf {
    // SAFETY: caller contract.
    unsafe {
        store_write(
            session,
            key_ptr,
            key_len,
            val_ptr,
            val_len,
            StoreKind::Query,
        )
    }
}

enum StoreKind {
    State,
    Filter,
    Query,
}

/// # Safety
/// `session` must be live; the two `ptr`/`len` pairs live UTF-8 buffers.
unsafe fn store_write(
    session: *mut ClientSession,
    key_ptr: *const u8,
    key_len: usize,
    val_ptr: *const u8,
    val_len: usize,
    kind: StoreKind,
) -> FuaranBuf {
    if session.is_null() {
        return pack_string(String::new());
    }
    // SAFETY: caller contract.
    let session = unsafe { &mut *session };
    let (Some(key), Some(value)) = (unsafe { borrow_str(key_ptr, key_len) }, unsafe {
        borrow_str(val_ptr, val_len)
    }) else {
        return pack_string(invalid_utf8_envelope());
    };
    let result = match kind {
        StoreKind::State => session.set_state(key, value),
        StoreKind::Filter => session.set_filter(key, value),
        StoreKind::Query => session.set_query(key, value),
    };
    match result {
        Ok(()) => pack_string(OK_RESULT.to_string()),
        Err(err) => pack_string(err.to_json()),
    }
}

/// Encode the public Rosetta parity demo's exemplar tree from its six scalar
/// holes (Phase 656). The `(ptr, len)` input is a small JSON object
/// `{"labelA":…,"valueA":…,"labelB":…,"valueB":…,"labelC":…,"valueC":…}`; the
/// return is the canonical wire bytes (packed), or an empty buffer when the
/// holes JSON is malformed. Additive example entry — this host builds its own
/// tree from the six scalars and runs the corpus-certified canonical encoder, so
/// the parity strip's "independent implementations converge on one hash" claim
/// stays honest (see [`rosetta`]). The codec and session surface are unchanged.
///
/// # Safety
/// `ptr`/`len` must name a live UTF-8 buffer per the memory contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_rosetta_encode(ptr: *const u8, len: usize) -> FuaranBuf {
    // SAFETY: caller contract.
    let Some(json) = (unsafe { borrow_str(ptr, len) }) else {
        return pack_string(String::new());
    };
    pack_string(rosetta::encode_from_holes(json).unwrap_or_default())
}

fn invalid_utf8_envelope() -> String {
    "{\"error\":{\"class\":\"decode\",\"code\":\"INVALID_JSON\",\"message\":\"input is not valid UTF-8\",\"path\":\"$\"}}"
        .to_string()
}
