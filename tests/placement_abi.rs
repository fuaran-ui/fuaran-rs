//! The **native** leg of the placement C-ABI: drive `fuaran_session_place` /
//! `_nudge` / `_duplicate` / `_paste` through the raw `extern "C"` surface,
//! exactly as a native binding does — `fuaran_alloc` an input buffer, call,
//! read the returned [`FuaranBuf`], `fuaran_dealloc` both. This certifies the
//! native two-word `{ ptr, len }` return form that a packed-`u64` return would
//! silently corrupt.
//!
//! **The scenarios live in `tests/fixtures/placement-abi.json`, and the `wasm32`
//! leg (`js/placement-abi.mjs`) reads the SAME file.** That is the point of the
//! file existing at all: this crate ships the placement verbs for two targets,
//! so it certifies them on two targets, and two copies of the scenarios would
//! drift into certifying two different things while reporting one.
//!
//! **What the fixture is, and is not.** It records the exact result envelope
//! each request returns, so the two targets are held to *identical bytes for
//! identical requests*. It is not the semantic oracle — whether a placement is
//! *correct* is settled in `tests/placement.rs` against the real apply engine,
//! over an exhaustive enumeration. Confusing the two would let a wrong
//! expectation certify itself.

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::ffi::{
    FuaranBuf, fuaran_alloc, fuaran_dealloc, fuaran_session_duplicate, fuaran_session_free,
    fuaran_session_new, fuaran_session_nudge, fuaran_session_paste, fuaran_session_place,
    fuaran_session_tree_json,
};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/placement-abi.json"
);

/// Read a Rust-owned [`FuaranBuf`] into an owned `String`, then free it through
/// the C-ABI — the exact caller-side dance a native binding performs.
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

/// Marshal a `&str` into a fresh `fuaran_alloc` input buffer; the caller frees
/// it after the consuming call.
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
        _ => panic!("the fixture carries no string '{key}'"),
    }
}

/// Call one verb by its fixture name, over the raw ABI.
fn call(session: *mut fuaran_rs::client::ClientSession, verb: &str, request: &str) -> String {
    let (ptr, len) = input(request);
    // SAFETY: a live handle and a live input buffer, freed below.
    let out = take_buf(unsafe {
        match verb {
            "place" => fuaran_session_place(session, ptr, len),
            "nudge" => fuaran_session_nudge(session, ptr, len),
            "duplicate" => fuaran_session_duplicate(session, ptr, len),
            "paste" => fuaran_session_paste(session, ptr, len),
            other => panic!("the fixture names an unknown verb '{other}'"),
        }
    });
    unsafe { fuaran_dealloc(ptr, len) };
    out
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

fn open_session(doc: &JVal) -> *mut fuaran_rs::client::ClientSession {
    let tree = string_member(doc, "tree");
    let (ptr, len) = input(&tree);
    // SAFETY: a live input buffer, freed immediately after the call.
    let session = unsafe { fuaran_session_new(ptr, len) };
    unsafe { fuaran_dealloc(ptr, len) };
    assert!(
        !session.is_null(),
        "the fixture tree decodes to a live handle"
    );
    session
}

#[test]
fn the_native_abi_reproduces_every_recorded_envelope() {
    let doc = fixture();
    let session = open_session(&doc);
    for case in cases(&doc) {
        let name = string_member(case, "name");
        let observed = call(
            session,
            &string_member(case, "verb"),
            &string_member(case, "request"),
        );
        assert_eq!(
            observed,
            string_member(case, "expect"),
            "case '{name}' returned an envelope the fixture does not record"
        );
    }
    // The session ADOPTED every successful edit, and refused ones left the held
    // tree untouched — one recorded blob rather than one per case, because it is
    // the accumulated effect that is worth pinning.
    let final_tree = take_buf(unsafe { fuaran_session_tree_json(session) });
    assert_eq!(final_tree, string_member(&doc, "finalTree"));
    unsafe { fuaran_session_free(session) };
}

#[test]
fn the_fixture_exercises_every_verb_and_both_refusal_classes() {
    // A green run means nothing without this: a fixture of only happy paths, or
    // one that never reaches a verb, would certify the ABI it does not call.
    let doc = fixture();
    let mut verbs: Vec<String> = cases(&doc)
        .iter()
        .map(|c| string_member(c, "verb"))
        .collect();
    verbs.sort();
    verbs.dedup();
    assert_eq!(verbs, ["duplicate", "nudge", "paste", "place"]);
    let expects: Vec<String> = cases(&doc)
        .iter()
        .map(|c| string_member(c, "expect"))
        .collect();
    assert!(expects.iter().any(|e| e.starts_with("{\"ok\":true")));
    assert!(
        expects
            .iter()
            .any(|e| e.contains("\"class\":\"placement\""))
    );
    assert!(expects.iter().any(|e| e.contains("\"class\":\"request\"")));
}

#[test]
fn a_perturbed_request_does_not_reproduce_its_recorded_envelope() {
    // The go-red probe. Every recorded envelope above is produced by the code it
    // certifies, so the comparison is only worth something if a changed request
    // changes the answer.
    let doc = fixture();
    let session = open_session(&doc);
    let case = &cases(&doc)[0];
    let perturbed = string_member(case, "request").replace("\"left\"", "\"ghost\"");
    assert_ne!(
        perturbed,
        string_member(case, "request"),
        "the perturbation changed nothing, so this probe proves nothing"
    );
    let observed = call(session, &string_member(case, "verb"), &perturbed);
    assert_ne!(
        observed,
        string_member(case, "expect"),
        "a request naming an absent parent returned the recorded envelope"
    );
    assert!(
        observed.contains("\"code\":\"ParentNotFound\""),
        "{observed}"
    );
    unsafe { fuaran_session_free(session) };
}

#[test]
fn a_null_session_handle_yields_an_empty_buffer_rather_than_a_crash() {
    // The surface-wide null contract, restated for the new entry points: a
    // binding that lost its handle must get nothing back, not undefined
    // behaviour.
    for verb in ["place", "nudge", "duplicate", "paste"] {
        assert_eq!(call(std::ptr::null_mut(), verb, "{}"), "");
    }
}
