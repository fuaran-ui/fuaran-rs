//! The `wasm32` client ABI: a minimal C-ABI export surface over WASM linear
//! memory, dependency-free (no `wasm-bindgen` / `web-sys` / `js-sys`). The thin
//! generic JS loader (`js/fuaran-loader.js`) instantiates the module, moves
//! UTF-8 bytes across the boundary through [`fuaran_alloc`] / [`fuaran_dealloc`],
//! and drives a [`ClientSession`](super::ClientSession) held as an opaque
//! handle.
//!
//! **Memory contract.** Buffers are exact-length boxed byte slices:
//! - *Input* buffers are owned by JS — `fuaran_alloc(len)` hands JS a zeroed
//!   buffer to write into; JS frees it with `fuaran_dealloc(ptr, len)` after
//!   the call. Rust only *borrows* an input buffer for the duration of a call.
//! - *Output* buffers are owned by Rust — a returning function leaks an exact
//!   boxed slice and returns a packed `u64` (`ptr` in the high 32 bits, `len`
//!   in the low 32); JS reads the UTF-8, then frees it with the same
//!   `fuaran_dealloc`.
//!
//! All exported functions are `extern "C"` with stable, prefixed names. The
//! host is single-threaded (browser WASM), so the last-error slot is a
//! `thread_local`.

use std::cell::RefCell;

use super::ClientSession;

thread_local! {
    /// The last `fuaran_session_new` decode failure, as its JSON envelope —
    /// read once via [`fuaran_last_error`] when `new` returns null.
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Pack a `(ptr, len)` pair into a `u64` for return across the ABI.
fn pack(ptr: *const u8, len: usize) -> u64 {
    ((ptr as u64) << 32) | (len as u64)
}

/// Leak an exact-length boxed byte slice from a `String`, returning the packed
/// `(ptr, len)`. JS reads `len` UTF-8 bytes at `ptr`, then frees via
/// [`fuaran_dealloc`].
fn pack_string(s: String) -> u64 {
    let boxed = s.into_bytes().into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *const u8;
    pack(ptr, len)
}

/// Borrow an input buffer JS wrote (via [`fuaran_alloc`]) as a `&str`. Returns
/// `None` on invalid UTF-8.
///
/// # Safety
/// `ptr` must point at `len` initialised bytes that outlive the borrow — true
/// for a JS-owned `fuaran_alloc` buffer that JS frees only after the call.
unsafe fn borrow_str<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if ptr.is_null() {
        return Some("");
    }
    // SAFETY: caller contract — `ptr`/`len` name a live initialised buffer.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes).ok()
}

/// Allocate a zeroed input buffer of `len` bytes; JS writes UTF-8 into it and
/// frees it with [`fuaran_dealloc`] after the call that consumes it.
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
            let envelope = super::ClientError::Decode(err).to_json();
            LAST_ERROR.with(|e| *e.borrow_mut() = Some(envelope));
            std::ptr::null_mut()
        }
    }
}

/// The last `fuaran_session_new` failure envelope (packed), or an empty string
/// when the last `new` succeeded.
#[unsafe(no_mangle)]
pub extern "C" fn fuaran_last_error() -> u64 {
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
pub unsafe extern "C" fn fuaran_session_render(session: *mut ClientSession) -> u64 {
    if session.is_null() {
        return pack_string(String::new());
    }
    // SAFETY: caller contract — live handle, single-threaded host.
    let session = unsafe { &*session };
    pack_string(session.render())
}

/// The session's current tree, re-encoded to canonical wire JSON (packed).
///
/// # Safety
/// `session` must be a live handle from `fuaran_session_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_session_tree_json(session: *mut ClientSession) -> u64 {
    if session.is_null() {
        return pack_string(String::new());
    }
    // SAFETY: caller contract.
    let session = unsafe { &*session };
    pack_string(session.tree_json())
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
) -> u64 {
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
) -> u64 {
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
) -> u64 {
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
) -> u64 {
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
) -> u64 {
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

fn invalid_utf8_envelope() -> String {
    "{\"error\":{\"class\":\"decode\",\"code\":\"INVALID_JSON\",\"message\":\"input is not valid UTF-8\",\"path\":\"$\"}}"
        .to_string()
}
