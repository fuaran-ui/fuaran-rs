//! The **native** leg of the platform-baseline capability wave, driven through
//! the raw `extern "C"` surface a native binding uses — `fuaran_session_new`,
//! `fuaran_session_set_state`, `fuaran_session_render`,
//! `fuaran_session_tree_json`.
//!
//! **The cases live in `tests/fixtures/platform-baseline.json`, and the
//! `wasm32` leg (`js/platform-baseline.mjs`) reads the SAME file** — the
//! arrangement `placement-abi.json` established and `list-param.json` repeated,
//! for the same reason. This crate is TWO hosts (a headless native one and a
//! browser-native `wasm32` client); the vocabulary is owed by both, so it is
//! certified on both by EXECUTION rather than by the fact that one module
//! compiles. Two copies of the cases would drift into certifying two different
//! things while reporting one.
//!
//! **Why the ABI leg matters for this wave in particular.** `fuaran-swift` and
//! `fuaran-kt` are decode-only render PROJECTIONS over this core: they never
//! decode a document or emit markup themselves, so whatever this surface does
//! not expose, they cannot reach. Five capabilities landing in the codec and the
//! server renderer says nothing about whether a native surface can *see* them —
//! this file is where that question is answered, and answered the same way on
//! the target whose pointer width differs (`FuaranBuf` is a packed `u64` on
//! `wasm32` and a two-word `repr(C)` struct natively, and that split is exactly
//! what a native-only gate cannot see).
//!
//! **What the fixture is, and is not.** Every `tree` is a shared-corpus fixture
//! verbatim and `expectTree` is the same bytes, so the round-trip leg is
//! certified against `wire-format-fixtures` rather than against itself. The
//! render expectations are drawn from the NORMATIVE obligations, and whether
//! this host meets them is settled in `tests/render_obligations.rs` against
//! `render-fidelity.json`. Confusing the two would let a wrong expectation
//! certify itself.
//!
//! **No new verb was added for this wave, and that is the finding.** The ABI is
//! generic over the vocabulary — decode a document, write a store slot, render,
//! re-encode — so a kind or a field reaches a native surface the moment the
//! codec and the renderer carry it. What a native surface CANNOT reach is
//! anything the surface has no verb for at all; the `egressNote` in the fixture
//! records the one such gap this wave exposes.

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::client::ClientSession;
use fuaran_rs::ffi::{
    FuaranBuf, fuaran_alloc, fuaran_dealloc, fuaran_session_free, fuaran_session_new,
    fuaran_session_render, fuaran_session_set_state, fuaran_session_tree_json,
};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/platform-baseline.json"
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

fn string_list(j: &JVal, key: &str) -> Vec<String> {
    match j.field(key) {
        Some(JVal::Arr(items)) => items
            .iter()
            .map(|v| match v {
                JVal::Str(s) => s.clone(),
                other => panic!("'{key}' holds a non-string ({other:?})"),
            })
            .collect(),
        None => Vec::new(),
        other => panic!("the fixture's '{key}' is an array (got {other:?})"),
    }
}

fn fixture() -> JVal {
    let raw = std::fs::read_to_string(FIXTURE).expect("the ABI fixture is readable");
    parse(&raw).expect("the ABI fixture parses with this host's own JSON layer")
}

fn cases(doc: &JVal) -> &Vec<JVal> {
    match doc.field("cases") {
        Some(JVal::Arr(cases)) => cases,
        _ => panic!("the fixture carries no 'cases' array"),
    }
}

fn open_session(tree: &str) -> *mut ClientSession {
    let (ptr, len) = input(tree);
    // SAFETY: a live input buffer, freed immediately after the call.
    let session = unsafe { fuaran_session_new(ptr, len) };
    unsafe { fuaran_dealloc(ptr, len) };
    assert!(
        !session.is_null(),
        "the fixture tree decodes to a live handle over the ABI — a null here \
         means this vocabulary does not reach a native surface at all"
    );
    session
}

fn set_state(session: *mut ClientSession, key: &str, value: &str) {
    let (kp, kl) = input(key);
    let (vp, vl) = input(value);
    // SAFETY: a live handle and two live input buffers, freed below.
    let out = take_buf(unsafe { fuaran_session_set_state(session, kp, kl, vp, vl) });
    unsafe {
        fuaran_dealloc(kp, kl);
        fuaran_dealloc(vp, vl);
    }
    assert_eq!(out, "{\"ok\":true}", "the store write must succeed");
}

/// Assert `needles` appear in `haystack` in the given ORDER.
///
/// Separate from the contains list because two of these obligations are about
/// SEQUENCE — authored track order, and document order of tree rows — and a
/// containment check cannot tell a host that preserved the order from one that
/// sorted it.
fn assert_order(name: &str, haystack: &str, needles: &[String]) {
    let mut last = 0usize;
    for needle in needles {
        let at = haystack
            .find(needle.as_str())
            .unwrap_or_else(|| panic!("{name}: '{needle}' is absent from:\n{haystack}"));
        assert!(
            at >= last,
            "{name}: '{needle}' appears out of the required order in:\n{haystack}"
        );
        last = at;
    }
}

/// One case, start to finish over the ABI: open a session on the document,
/// write any declared store slots, then take BOTH observations — the re-encoded
/// tree and the rendered markup.
///
/// Both, because they answer different questions and a native surface needs
/// each: `tree_json` is what a projection re-serialises (so it is where a
/// dropped field shows up), and `render` is what it paints (so it is where a
/// dropped OBLIGATION shows up). A wave that landed the codec and not the
/// renderer passes the first and fails the second.
fn run_case(case: &JVal) -> (String, String) {
    let session = open_session(&string_member(case, "tree"));
    if let Some(JVal::Arr(writes)) = case.field("state") {
        for write in writes {
            set_state(
                session,
                &string_member(write, "key"),
                &string_member(write, "value"),
            );
        }
    }
    // SAFETY: a live handle in both calls; each buffer freed by `take_buf`.
    let tree = take_buf(unsafe { fuaran_session_tree_json(session) });
    let html = take_buf(unsafe { fuaran_session_render(session) });
    unsafe { fuaran_session_free(session) };
    (tree, html)
}

#[test]
fn the_native_abi_carries_every_platform_baseline_capability() {
    let doc = fixture();
    let cases = cases(&doc);
    assert!(
        cases.len() >= 5,
        "the wave has five capabilities; the fixture holds {}",
        cases.len()
    );
    for case in cases {
        let name = string_member(case, "name");
        let (tree, html) = run_case(case);

        assert_eq!(
            tree,
            string_member(case, "expectTree"),
            "{name}: the tree a native surface re-serialises must be byte-identical \
             to the corpus fixture it was opened on"
        );

        // A pass proves nothing unless the oracle looked at something. A
        // mis-keyed expectation list reads as an EMPTY list and every loop below
        // becomes vacuously green — which is the failure shape a fixture-driven
        // gate is most prone to and least likely to be noticed for.
        let contains = string_list(case, "expectRenderContains");
        assert!(
            contains.len() >= 3,
            "{name}: the case declares only {} render expectations — too few to              certify a capability reached a native surface",
            contains.len()
        );
        for needle in contains {
            assert!(
                html.contains(&needle),
                "{name}: the rendered markup a native surface receives is missing \
                 '{needle}':\n{html}"
            );
        }
        for needle in string_list(case, "expectRenderAbsent") {
            assert!(
                !html.contains(&needle),
                "{name}: the rendered markup must NOT carry '{needle}':\n{html}"
            );
        }
        assert_order(&name, &html, &string_list(case, "expectOrder"));
    }
}

/// A null session is the surface's own refusal shape, and every verb this leg
/// uses honours it — so a binding that lost its handle gets an empty buffer
/// rather than a crash inside the module.
#[test]
fn the_verbs_this_leg_uses_tolerate_a_null_handle() {
    // SAFETY: passing NULL is exactly the contract under test.
    assert_eq!(
        take_buf(unsafe { fuaran_session_render(std::ptr::null_mut()) }),
        ""
    );
    assert_eq!(
        take_buf(unsafe { fuaran_session_tree_json(std::ptr::null_mut()) }),
        ""
    );
}
