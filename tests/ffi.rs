//! Native C-ABI smoke test (Phase 537): drive a `ClientSession` through the raw
//! `fuaran_*` export surface exactly as a native binding (Swift / Kotlin) will —
//! `fuaran_alloc` an input buffer, `fuaran_session_new`, `render` / `tree_json` /
//! `apply_op` / `set_state`, then free. This certifies the **native** `(ptr, len)`
//! return ABI (`FuaranBuf` two-word struct) actually round-trips on a 64-bit host
//! — the packing that a packed-`u64` return would silently corrupt. Tests never
//! run on `wasm32`, so `FuaranBuf` here is always the native struct form.

use fuaran_rs::ffi::{
    FuaranBuf, fuaran_alloc, fuaran_dealloc, fuaran_last_error, fuaran_session_apply_op,
    fuaran_session_free, fuaran_session_new, fuaran_session_render, fuaran_session_set_state,
    fuaran_session_tree_json,
};

const TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[
    {"id":"metric","kind":{"$type":"Metric","emphasis":"Loud","format":{"$type":"Currency","code":"GBP"},"label":{"$type":"Literal","text":"Revenue"},"source":{"$type":"State","defaultValue":0,"key":"revenue"},"tone":"Brand","weight":"Standard"}}
],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;

/// Read a Rust-owned [`FuaranBuf`] into an owned `String`, then free it through
/// the C-ABI `fuaran_dealloc` — the exact caller-side dance a native binding does.
fn take_buf(buf: FuaranBuf) -> String {
    if buf.ptr.is_null() || buf.len == 0 {
        // An empty output still owns an allocation; free it if the length is
        // zero but the pointer is a live (dangling-but-valid) empty box.
        if !buf.ptr.is_null() {
            unsafe { fuaran_dealloc(buf.ptr, buf.len) };
        }
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) };
    let s = std::str::from_utf8(bytes)
        .expect("output is UTF-8")
        .to_owned();
    unsafe { fuaran_dealloc(buf.ptr, buf.len) };
    s
}

/// Marshal a `&str` into a fresh `fuaran_alloc` input buffer; returns the raw
/// `(ptr, len)` the caller passes in and frees after the consuming call.
fn input(s: &str) -> (*mut u8, usize) {
    let len = s.len();
    let ptr = fuaran_alloc(len);
    unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), ptr, len) };
    (ptr, len)
}

#[test]
fn native_c_abi_session_round_trips() {
    // new(tree)
    let (tp, tl) = input(TREE);
    let session = unsafe { fuaran_session_new(tp, tl) };
    unsafe { fuaran_dealloc(tp, tl) };
    assert!(!session.is_null(), "a valid tree decodes to a live handle");

    // render() — the native FuaranBuf must carry a recoverable ptr + len.
    let html = take_buf(unsafe { fuaran_session_render(session) });
    assert!(html.contains("data-fuaran-node-id=\"root\""));
    assert!(html.contains("fuaran-metric-value"));

    // tree_json() re-encodes canonically and round-trips the node id.
    let json = take_buf(unsafe { fuaran_session_tree_json(session) });
    assert!(json.contains("\"id\":\"root\""));

    // Before any write the State binding falls to its carried default (0).
    assert!(html.contains("fuaran-metric-value\">GBP 0.00<"));
    // set_state(revenue = 1000) then re-render observes the write-back.
    let (kp, kl) = input("revenue");
    let (vp, vl) = input("1000");
    let ok = take_buf(unsafe { fuaran_session_set_state(session, kp, kl, vp, vl) });
    unsafe { fuaran_dealloc(kp, kl) };
    unsafe { fuaran_dealloc(vp, vl) };
    assert_eq!(ok, "{\"ok\":true}");
    let rendered = take_buf(unsafe { fuaran_session_render(session) });
    assert!(
        rendered.contains("fuaran-metric-value\">GBP 1000.00<"),
        "the state write drives the metric"
    );

    unsafe { fuaran_session_free(session) };
}

#[test]
fn native_c_abi_apply_op_reports_structured_error() {
    let (tp, tl) = input(TREE);
    let session = unsafe { fuaran_session_new(tp, tl) };
    unsafe { fuaran_dealloc(tp, tl) };
    assert!(!session.is_null());

    // A malformed op JSON surfaces the structured error envelope (packed), and
    // the held tree is untouched.
    let (op, ol) = input(r#"{"$type":"Nonsense"}"#);
    let err = take_buf(unsafe { fuaran_session_apply_op(session, op, ol) });
    unsafe { fuaran_dealloc(op, ol) };
    assert!(
        err.contains("\"error\""),
        "a bad op returns an error envelope"
    );

    unsafe { fuaran_session_free(session) };
}

#[test]
fn native_c_abi_bad_tree_yields_null_and_last_error() {
    // An EMPTY_NODE_ID tree fails to decode: null handle + a last-error envelope
    // readable on this thread.
    let (bp, bl) = input(r#"{"id":"","kind":{"$type":"Markdown","text":"x"}}"#);
    let session = unsafe { fuaran_session_new(bp, bl) };
    unsafe { fuaran_dealloc(bp, bl) };
    assert!(session.is_null(), "a bad tree returns a null handle");

    let envelope = take_buf(fuaran_last_error());
    assert!(
        envelope.contains("EMPTY_NODE_ID"),
        "last_error carries the code"
    );
}
