//! Native C-ABI smoke test (Phase 537): drive a `ClientSession` through the raw
//! `fuaran_*` export surface exactly as a native binding (Swift / Kotlin) will —
//! `fuaran_alloc` an input buffer, `fuaran_session_new`, `render` / `tree_json` /
//! `apply_op` / `set_state`, then free. This certifies the **native** `(ptr, len)`
//! return ABI (`FuaranBuf` two-word struct) actually round-trips on a 64-bit host
//! — the packing that a packed-`u64` return would silently corrupt. Tests never
//! run on `wasm32`, so `FuaranBuf` here is always the native struct form.

use fuaran_rs::ffi::{
    FuaranBuf, fuaran_alloc, fuaran_dealloc, fuaran_last_error, fuaran_session_apply_op,
    fuaran_session_free, fuaran_session_new, fuaran_session_project_resolved,
    fuaran_session_render, fuaran_session_resolved_rows, fuaran_session_set_state,
    fuaran_session_tree_json,
};

const TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[
    {"id":"metric","kind":{"$type":"Metric","emphasis":"Loud","format":{"$type":"Currency","code":"GBP"},"label":{"$type":"Literal","text":"Revenue"},"value":{"$type":"State","defaultValue":0,"key":"revenue"},"tone":"Brand","weight":"Standard"}}
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

/// A Badge whose label is a scalar Transform (count of a 2-row embedded frame)
/// — the projection folds it to the literal "2"; tree_json keeps the Transform.
const TRANSFORM_TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[
    {"id":"count-badge","kind":{"$type":"Badge","label":{"$type":"Bound","binding":{"$type":"Transform","pipeline":[{"$type":"groupBy","aggs":[{"fn":"count","name":"n","of":"id"}],"keys":[]}],"source":{"columns":{"id":{"values":["A","B"]}},"schema":[{"name":"id","type":"string"}]}}},"variant":"Neutral"}}
],"layout":{"$type":"Auto"},"role":"Group"}}"#;

#[test]
fn native_c_abi_project_resolved_folds_scalar_transform() {
    let (tp, tl) = input(TRANSFORM_TREE);
    let session = unsafe { fuaran_session_new(tp, tl) };
    unsafe { fuaran_dealloc(tp, tl) };
    assert!(!session.is_null(), "a valid tree decodes to a live handle");

    // The raw tree_json still carries the unresolved Transform (additive — the
    // existing entry point is byte-unchanged).
    let raw = take_buf(unsafe { fuaran_session_tree_json(session) });
    assert!(
        raw.contains("\"$type\":\"Transform\""),
        "tree_json keeps the raw Transform: {raw}"
    );

    // The resolved projection folds the scalar Transform to its literal count.
    let projected = take_buf(unsafe { fuaran_session_project_resolved(session) });
    assert!(
        !projected.contains("\"$type\":\"Transform\""),
        "the scalar Transform is folded out of the projection: {projected}"
    );
    // A `TextSource::Literal` encodes as the bare-string shorthand (§3.3), so the
    // folded Badge label rides as `"label":"2"`.
    assert!(
        projected.contains("\"label\":\"2\""),
        "the Badge label becomes the literal count 2: {projected}"
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

/// The Phase 750 declarative pill, driven through the raw ABI exactly as the Swift
/// and Kotlin projections will. Named here rather than left to the codec tests
/// because the projections' whole contract is "the Rust core owns truth, the native
/// surface holds a render projection" — so the case reaching them at all depends on
/// this boundary carrying it, and nothing else in the repo would notice if it did
/// not. Two directions, both load-bearing for a decode-only projection:
/// `tree_json` must re-emit the canonical case for the projections to decode, and
/// `render` must paint the per-row tone the Rust core resolved.
const TONED_PILL_TREE: &str = r#"{"id":"g","kind":{"$type":"DataGrid","columns":[{"field":"status","kind":{"$type":"TonedPill","default":"Subdued","field":"status","map":{"Delayed":"Warning"}},"label":"Status"}],"rowKeyField":"status","source":{"$type":"Transform","pipeline":[],"source":{"columns":{"status":{"validity":[true,true],"values":["Delayed","Other"]}},"schema":[{"name":"status","type":"string"}]}}}}"#;

#[test]
fn native_c_abi_carries_the_toned_pill_case() {
    let (tp, tl) = input(TONED_PILL_TREE);
    let session = unsafe { fuaran_session_new(tp, tl) };
    unsafe { fuaran_dealloc(tp, tl) };
    assert!(
        !session.is_null(),
        "a TonedPill tree decodes to a live handle"
    );

    // tree_json — the canonical case crosses the boundary intact, `default` and all,
    // so a decode-only native projection sees exactly what the corpus specifies.
    let json = take_buf(unsafe { fuaran_session_tree_json(session) });
    assert!(
        json.contains(r#""$type":"TonedPill","default":"Subdued","field":"status","map":{"Delayed":"Warning"}"#),
        "tree_json did not carry the canonical TonedPill:\n{json}"
    );

    // render — the mapped row and the unmapped fallback, resolved core-side.
    let html = take_buf(unsafe { fuaran_session_render(session) });
    assert!(
        html.contains(r#"<span class="fuaran-grid-cell-pill fuaran-pill-warning">Delayed</span>"#),
        "mapped row lost its tone:\n{html}"
    );
    assert!(
        html.contains(r#"<span class="fuaran-grid-cell-pill fuaran-pill-subdued">Other</span>"#),
        "unmapped row lost the default tone:\n{html}"
    );

    unsafe { fuaran_session_free(session) };
}

/// The resolved-rows hand-off across the raw ABI — the call the Swift and Kotlin
/// grid renderers will make once they have a row loop. Driven here exactly as a
/// native binding drives it: `fuaran_alloc` the node id, read the packed buffer,
/// free both.
#[test]
fn native_c_abi_hands_over_resolved_rows() {
    let (tp, tl) = input(TONED_PILL_TREE);
    let session = unsafe { fuaran_session_new(tp, tl) };
    unsafe { fuaran_dealloc(tp, tl) };
    assert!(!session.is_null());

    // The premise, asserted rather than assumed: the tree the consumer decodes
    // carries an UNRESOLVED Transform, so the rows are not in it to be found.
    let json = take_buf(unsafe { fuaran_session_tree_json(session) });
    assert!(json.contains(r#""$type":"Transform""#), "{json}");

    let (np, nl) = input("g");
    let rows = take_buf(unsafe { fuaran_session_resolved_rows(session, np, nl) });
    unsafe { fuaran_dealloc(np, nl) };
    assert_eq!(
        rows, r#"{"resolved":true,"rows":[{"status":"Delayed"},{"status":"Other"}]}"#,
        "the rows the tree could not carry"
    );

    // A caller mistake reports as one, in the same error shape the other entry
    // points use — never as an empty grid.
    let (bp, bl) = input("no-such-node");
    let missing = take_buf(unsafe { fuaran_session_resolved_rows(session, bp, bl) });
    unsafe { fuaran_dealloc(bp, bl) };
    assert!(missing.contains(r#""code":"NO_ROW_SOURCE""#), "{missing}");
    assert!(missing.contains(r#""class":"lookup""#), "{missing}");

    unsafe { fuaran_session_free(session) };
}
