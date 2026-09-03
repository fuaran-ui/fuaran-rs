//! The **native** leg of the list-param behaviour, driven through the raw
//! `extern "C"` surface a native binding uses — `fuaran_session_new`,
//! `fuaran_session_set_filter`, `fuaran_session_resolved_rows`.
//!
//! **The scenarios live in `tests/fixtures/list-param.json`, and the `wasm32`
//! leg (`js/list-param.mjs`) reads the SAME file** — the arrangement
//! `placement-abi.json` established, for the same reason. This crate is TWO
//! hosts (a headless native one and a browser-native `wasm32` client) and the
//! rule is owed by both, so it is certified on both by EXECUTION rather than by
//! the fact that one module compiles. Two copies of the scenarios would drift
//! into certifying two different things while reporting one.
//!
//! **What the fixture is, and is not.** It records the exact envelope each
//! selection returns, so the two targets are held to identical bytes for
//! identical inputs. It is not the semantic oracle: whether the *rule* is right
//! is settled in `tests/transform_list_param.rs` against the shared corpus
//! fixture `nodes/multiselect-chip-list-param.json`. Confusing the two would
//! let a wrong expectation certify itself.
//!
//! The `wasm32` client and the server renderer resolve through the very same
//! `render::bindings::eval_transform_frame`, so this leg is not a second
//! implementation under test — it is proof that the shared seam is what the
//! client path actually reaches, over the ABI, with its own marshalling.

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::client::ClientSession;
use fuaran_rs::ffi::{
    FuaranBuf, fuaran_alloc, fuaran_dealloc, fuaran_session_free, fuaran_session_new,
    fuaran_session_resolved_rows, fuaran_session_set_filter,
};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/list-param.json"
);

fn take_buf(buf: FuaranBuf) -> String {
    if buf.ptr.is_null() {
        return String::new();
    }
    // SAFETY: the surface's contract — `len` initialised UTF-8 bytes at `ptr`,
    // Rust-owned, freed exactly once.
    let bytes = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) };
    let s = std::str::from_utf8(bytes)
        .expect("output is UTF-8")
        .to_owned();
    unsafe { fuaran_dealloc(buf.ptr, buf.len) };
    s
}

fn input(s: &str) -> (*mut u8, usize) {
    let len = s.len();
    let ptr = fuaran_alloc(len);
    // SAFETY: `fuaran_alloc` returned `len` writable bytes.
    unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), ptr, len) };
    (ptr, len)
}

fn string_member(j: &JVal, key: &str) -> String {
    match j.field(key) {
        Some(JVal::Str(s)) => s.clone(),
        other => panic!("the fixture carries no string '{key}' (got {other:?})"),
    }
}

fn fixture() -> JVal {
    let raw = std::fs::read_to_string(FIXTURE).expect("the ABI fixture is readable");
    parse(&raw).expect("the ABI fixture parses")
}

fn cases(doc: &JVal) -> &Vec<JVal> {
    match doc.field("cases") {
        Some(JVal::Arr(cases)) => cases,
        _ => panic!("the fixture carries no 'cases' array"),
    }
}

fn open_session(doc: &JVal) -> *mut ClientSession {
    let (ptr, len) = input(&string_member(doc, "tree"));
    // SAFETY: a live input buffer, freed immediately after the call.
    let session = unsafe { fuaran_session_new(ptr, len) };
    unsafe { fuaran_dealloc(ptr, len) };
    assert!(
        !session.is_null(),
        "the fixture tree decodes to a live handle"
    );
    session
}

fn set_filter(session: *mut ClientSession, name: &str, value: &str) {
    let (kp, kl) = input(name);
    let (vp, vl) = input(value);
    // SAFETY: a live handle and two live input buffers, freed below.
    let out = take_buf(unsafe { fuaran_session_set_filter(session, kp, kl, vp, vl) });
    unsafe {
        fuaran_dealloc(kp, kl);
        fuaran_dealloc(vp, vl);
    }
    assert_eq!(out, "{\"ok\":true}", "the store write must succeed");
}

fn resolved_rows(session: *mut ClientSession, node: &str) -> String {
    let (ptr, len) = input(node);
    // SAFETY: a live handle and a live input buffer, freed below.
    let out = take_buf(unsafe { fuaran_session_resolved_rows(session, ptr, len) });
    unsafe { fuaran_dealloc(ptr, len) };
    out
}

/// One case: a fresh session, an optional selection written to the filter
/// store, then the grid's resolved rows. Fresh per case deliberately — a
/// carried-over selection would make the cases order-dependent, and "nothing
/// selected" is a state a shared session could never return to.
fn run_case(doc: &JVal, selection: Option<&str>) -> String {
    let session = open_session(doc);
    if let Some(value) = selection {
        set_filter(session, &string_member(doc, "filter"), value);
    }
    let out = resolved_rows(session, &string_member(doc, "node"));
    unsafe { fuaran_session_free(session) };
    out
}

fn selection_of(case: &JVal) -> Option<String> {
    match case.field("selection") {
        Some(JVal::Str(s)) => Some(s.clone()),
        Some(JVal::Null) | None => None,
        other => panic!("a case's 'selection' is a JSON-text string or null (got {other:?})"),
    }
}

#[test]
fn the_native_abi_reproduces_every_recorded_envelope() {
    let doc = fixture();
    for case in cases(&doc) {
        let name = string_member(case, "name");
        let observed = run_case(&doc, selection_of(case).as_deref());
        assert_eq!(
            observed,
            string_member(case, "expect"),
            "case '{name}' returned an envelope the fixture does not record"
        );
    }
}

#[test]
fn the_fixture_carries_all_three_behaviours_rather_than_only_happy_paths() {
    // A green run means nothing without this. The three the phase names:
    // an unfiltered baseline + a scoping selection (substitution), an EMPTY
    // selection that must equal the unfiltered answer (the prune), and a kind
    // mismatch that must refuse.
    let doc = fixture();
    let by_name = |n: &str| -> String {
        cases(&doc)
            .iter()
            .find(|c| string_member(c, "name") == n)
            .map(|c| string_member(c, "expect"))
            .unwrap_or_else(|| panic!("the fixture carries no case '{n}'"))
    };
    let unfiltered = by_name("nothing-selected-shows-the-unfiltered-table");
    assert_eq!(
        by_name("an-empty-selection-prunes-to-the-unfiltered-table"),
        unfiltered,
        "the empty selection must record the UNFILTERED answer, or this fixture pins the bug"
    );
    let scoped = by_name("a-selection-scopes-the-grid");
    assert_ne!(
        scoped, unfiltered,
        "a scoping selection that recorded the unfiltered answer would certify no substitution"
    );
    assert_eq!(
        by_name("a-scalar-where-a-list-is-read-is-refused"),
        "{\"resolved\":false}",
        "a mismatch must record the loading surface — never rows"
    );
}

#[test]
fn a_perturbed_selection_does_not_reproduce_its_recorded_envelope() {
    // The go-red probe, on this target, every run: every envelope above is
    // produced by the code it certifies, so the comparison is worth something
    // only if a changed selection changes the answer.
    let doc = fixture();
    let probe = doc.field("probe").expect("the fixture carries a probe");
    let case = string_member(probe, "case");
    let recorded = cases(&doc)
        .iter()
        .find(|c| string_member(c, "name") == case)
        .map(|c| string_member(c, "expect"))
        .expect("the probe names a real case");
    let selection = string_member(probe, "selection");
    let observed = run_case(&doc, Some(&selection));
    assert_ne!(
        observed, recorded,
        "a selection naming no department returned the recorded envelope"
    );
    assert_eq!(
        observed,
        string_member(probe, "expect"),
        "and it must be an EMPTY table — a selection that matches nothing is a constraint no row \
         satisfies, which is exactly what an empty selection is NOT"
    );
}
