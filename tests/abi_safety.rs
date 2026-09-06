//! The C-ABI safety boundary — the panic guard, the input-pair contract, and a
//! deterministic fuzz over the four subsystems a native binding can reach.
//!
//! ## What this certifies that `tests/ffi.rs` does not
//!
//! `tests/ffi.rs` proves the surface WORKS: a well-formed tree round-trips
//! through `fuaran_alloc` / `session_new` / `render` / `apply_op` / `set_state`.
//! This file proves the surface CANNOT TAKE THE HOST DOWN, which is a different
//! claim and the one the two native tiers rest on. A Swift or Kotlin app has no
//! recovery from a Rust panic: before the guard the release profile was
//! `panic = "abort"`, so any reachable `unreachable!` / `expect` / arithmetic
//! overflow inside the core ended the app's process, with a stack trace nobody
//! on that side could read and no way to keep the session.
//!
//! Three legs, in the order the claims depend on each other:
//!
//! 1. **The guard converts** — `ffi::abi_guard` turns a panic into the ordinary
//!    error envelope rather than propagating it. Deliberately proved with an
//!    INJECTED panic, because a mechanism that has never been shown to fire is
//!    not evidence of anything. This is the go-red proof: delete the
//!    `catch_unwind` in `src/ffi/mod.rs` and this test fails (by unwinding out
//!    of the call, which the harness reports as a panicking test).
//! 2. **The input-pair contract holds** — `(NULL, len > 0)` is REFUSED with a
//!    typed envelope rather than read. This is the one caller mistake that used
//!    to reach `slice::from_raw_parts(null, len)`, which is undefined behaviour
//!    whatever the length; a fuzz leg cannot certify its absence, only a
//!    deterministic check can.
//! 3. **Nothing escapes under mutation** — a deterministic fuzz drives APPLY and
//!    RENDER through the C-ABI, and MERGE and TELEPORT through their crate API
//!    (neither is on the C-ABI, so the surface a binding reaches them through is
//!    the Rust one), asserting every call returns rather than unwinds.
//!
//! ## Why hand-rolled, again
//!
//! Same reasons `tests/decoder_fuzz.rs` records at length: this crate declares
//! no third-party dependency, and `cargo-fuzz` needs a nightly toolchain and a
//! sanitizer runtime the CI gate does not carry. A leg the gate cannot run is a
//! leg the gate does not have. What is kept is a deterministic, replayable
//! generator; what is given up is coverage-guided mutation, stated here rather
//! than assumed away.
//!
//! Replay one seed:
//!
//! ```text
//! FUARAN_ABI_FUZZ_SEED=12345 cargo test --test abi_safety -- --nocapture
//! ```

use std::panic::{AssertUnwindSafe, catch_unwind};

use fuaran_rs::dag::merge::merge3_way;
use fuaran_rs::ffi::{
    FuaranBuf, abi_guard, fuaran_alloc, fuaran_dealloc, fuaran_last_error, fuaran_session_apply_op,
    fuaran_session_free, fuaran_session_new, fuaran_session_place, fuaran_session_project_resolved,
    fuaran_session_render, fuaran_session_resolved_rows, fuaran_session_set_filter,
    fuaran_session_set_query, fuaran_session_set_state, fuaran_session_tree_json,
};
use fuaran_rs::teleport;
use fuaran_rs::wire::decode_node;

// ─── Marshalling helpers (the caller-side dance a native binding performs) ───

/// Read a Rust-owned [`FuaranBuf`] into an owned `String`, then free it through
/// `fuaran_dealloc`. Lossy on purpose: a fuzz leg must not assert UTF-8 of a
/// buffer whose contents it is trying to break.
fn take_buf(buf: FuaranBuf) -> String {
    if buf.ptr.is_null() {
        return String::new();
    }
    let out = if buf.len == 0 {
        String::new()
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    unsafe { fuaran_dealloc(buf.ptr, buf.len) };
    out
}

/// Marshal a `&str` into a fresh `fuaran_alloc` input buffer.
fn input(s: &str) -> (*mut u8, usize) {
    let len = s.len();
    let ptr = fuaran_alloc(len);
    assert!(
        !ptr.is_null() || len == 0,
        "fuaran_alloc({len}) returned NULL for an ordinary request"
    );
    if len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), ptr, len) };
    }
    (ptr, len)
}

/// Run one closure with panic reporting silenced, returning `Err` when it
/// unwound. Used ONLY where an escape is the thing under test — a silenced
/// hook anywhere else would hide the diagnosis this suite exists to print.
fn quietly<T>(f: impl FnOnce() -> T) -> Result<T, ()> {
    let prior = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(f));
    std::panic::set_hook(prior);
    outcome.map_err(|_| ())
}

// ─── Leg 1: the guard converts a panic into the error envelope ───────────────

#[test]
fn abi_guard_converts_a_panic_into_the_error_envelope() {
    // The INJECTED panic. `abi_guard` is the exact seam every `extern "C"` body
    // in `src/ffi/` runs inside, so proving it here proves it for all of them —
    // and it needs no new exported symbol, which would widen the very ABI
    // surface `include/fuaran.h` pins.
    let envelope = abi_guard(
        "fuaran_session_apply_op",
        |envelope| envelope,
        || -> String { panic!("injected: a reachable defect on decoded data") },
    );

    assert!(
        envelope.contains("\"class\":\"internal\""),
        "a caught panic is reported under the internal class, never as a \
         statement about the caller's input — got: {envelope}"
    );
    assert!(
        envelope.contains("\"code\":\"PANIC\""),
        "a caught panic carries the PANIC code so a consumer can tell it from a \
         legitimate refusal and report it — got: {envelope}"
    );
    assert!(
        envelope.contains("fuaran_session_apply_op"),
        "the envelope names the entry point — got: {envelope}"
    );
    assert!(
        envelope.contains("injected: a reachable defect on decoded data"),
        "a string panic payload survives into the message — got: {envelope}"
    );

    // The envelope is well-formed canonical JSON: a consumer parses it with the
    // same reader it uses for every other refusal on this surface, so a message
    // carrying a quote or a backslash must not break that.
    let quoted = abi_guard(
        "fuaran_session_render",
        |envelope| envelope,
        || -> String { panic!("quote \" backslash \\ newline \n end") },
    );
    assert!(
        !quoted.contains('\n'),
        "a raw newline inside a JSON string is invalid JSON — the panic message is canonically escaped, not spliced raw. Got: {quoted:?}"
    );
    assert!(
        quoted.contains("\\\""),
        "the message's own quote survives as an ESCAPED quote — got: {quoted:?}"
    );
}

#[test]
fn abi_guard_passes_a_normal_return_through_untouched() {
    // The other half of the go-red proof: a guard that always reported an
    // envelope would pass the test above and break every call. Cheap, and it is
    // the assertion that makes the first one mean something.
    let value = abi_guard("entry", |_| "PANICKED".to_string(), || "ordinary".to_string());
    assert_eq!(value, "ordinary");
}

// ─── Leg 2: the input-pair contract ─────────────────────────────────────────

#[test]
fn a_null_pointer_with_a_non_zero_length_is_refused_not_read() {
    // Reading it would be undefined behaviour whatever the length. There is no
    // fuzz that certifies the absence of UB, so this is a deterministic check.
    let session = unsafe { fuaran_session_new(std::ptr::null(), 7) };
    assert!(
        session.is_null(),
        "(NULL, 7) is a caller mistake, not a tree"
    );
    let err = take_buf(fuaran_last_error());
    assert!(
        err.contains("\"code\":\"INVALID_JSON\"") && err.contains("NULL"),
        "the refusal names the mistake so the caller repairs the right argument \
         — got: {err}"
    );

    // The same pair on a live session, on the entry points that take one.
    let (tp, tl) = input(SEED_TREE);
    let live = unsafe { fuaran_session_new(tp, tl) };
    unsafe { fuaran_dealloc(tp, tl) };
    assert!(!live.is_null());

    for envelope in [
        take_buf(unsafe { fuaran_session_apply_op(live, std::ptr::null(), 3) }),
        take_buf(unsafe { fuaran_session_resolved_rows(live, std::ptr::null(), 3) }),
        take_buf(unsafe { fuaran_session_place(live, std::ptr::null(), 3) }),
    ] {
        assert!(
            envelope.contains("NULL"),
            "every (NULL, len>0) input pair is refused by name — got: {envelope}"
        );
    }

    // (NULL, 0) is the empty string and must NOT be refused as a null pointer —
    // it is how a caller spells "no bytes", and conflating the two would turn a
    // legal call into an error.
    let empty = take_buf(unsafe { fuaran_session_apply_op(live, std::ptr::null(), 0) });
    assert!(
        !empty.contains("NULL"),
        "(NULL, 0) is the empty string, not a null-pointer mistake — got: {empty}"
    );

    // The session survived every refusal — that is the point of refusing rather
    // than aborting.
    let json = take_buf(unsafe { fuaran_session_tree_json(live) });
    assert!(json.contains("\"id\":\"root\""));
    unsafe { fuaran_session_free(live) };
}

#[test]
fn fuaran_alloc_returns_null_rather_than_aborting_on_an_impossible_request() {
    // `include/fuaran.h` has always told the caller to check for NULL. Before
    // the fallible path it could not happen: the infallible allocation aborted
    // the host instead, on a length that arrives from a decoded document.
    assert!(
        fuaran_alloc(usize::MAX).is_null(),
        "an unsatisfiable allocation returns NULL"
    );

    // Zero is not a failure: it returns a non-NULL, aligned pointer owning no
    // bytes, freed by the same `fuaran_dealloc` path as any other buffer.
    let zero = fuaran_alloc(0);
    assert!(!zero.is_null(), "fuaran_alloc(0) is not a failure");
    unsafe { fuaran_dealloc(zero, 0) };
}

// ─── Leg 3: deterministic fuzz through apply / render / merge / teleport ─────

const SEED_TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[{"id":"m1","kind":{"$type":"Markdown","text":"# h\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n[l]: http://e.example \"t\"\n\n```rs\nx\n```\n"}},{"id":"m2","kind":{"$type":"Heading","level":2,"text":{"$type":"Literal","text":"h"}}}],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;

const SEED_OPS: &[&str] = &[
    r#"{"$type":"RemoveNode","target":"m1"}"#,
    r#"{"$type":"ReorderChildren","parentId":"root","newOrder":["m2","m1"]}"#,
    r#"{"$type":"InsertChild","parentId":"root","child":{"id":"m3","kind":{"$type":"Markdown","text":"x"}}}"#,
    r#"{"$type":"UpdateState","target":"root","state":{}}"#,
    r#"{"$type":"Batch","ops":[{"$type":"RemoveNode","target":"m2"}]}"#,
];

const SEED_PLACEMENTS: &[&str] = &[
    r#"{"parentId":"root","placement":"First","child":{"id":"p1","kind":{"$type":"Markdown","text":"p"}}}"#,
    r#"{"parentId":"root","placement":"After","anchor":"m1","child":{"id":"p2","kind":{"$type":"Markdown","text":"p"}}}"#,
    r#"{"target":"m2","delta":-1}"#,
];

/// The structural bytes a splice mutation inserts — the characters that make a
/// JSON document mean something different rather than merely be malformed.
const SPLICE_BYTES: &[u8] = b"{}[]\",:\\ntue0-.eE";

/// SplitMix64 — replayability is the whole point of the seed, and the standard
/// library carries no seedable PRNG at all.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, n)`; `0` for an empty range so no caller has to guard.
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// Mutate `seed` in one of five ways. Byte-level on purpose: the point is to
/// reach the decoder's refusal paths and the subsystems behind them, not to
/// stay well-formed.
fn mutate(rng: &mut Rng, seed: &str) -> String {
    let mut bytes = seed.as_bytes().to_vec();
    if bytes.is_empty() {
        return String::new();
    }
    match rng.below(5) {
        // Flip a byte.
        0 => {
            let at = rng.below(bytes.len());
            bytes[at] ^= 1u8 << rng.below(8);
        }
        // Truncate — the classic way to reach an "unterminated" path.
        1 => bytes.truncate(rng.below(bytes.len())),
        // Splice a structural character.
        2 => {
            let at = rng.below(bytes.len());
            bytes.insert(at, SPLICE_BYTES[rng.below(SPLICE_BYTES.len())]);
        }
        // Delete a run.
        3 => {
            let at = rng.below(bytes.len());
            let n = 1 + rng.below(8);
            let end = (at + n).min(bytes.len());
            bytes.drain(at..end);
        }
        // Repeat a run — grows nesting and string length toward the limits.
        _ => {
            let at = rng.below(bytes.len());
            let n = 1 + rng.below(16);
            let end = (at + n).min(bytes.len());
            let run: Vec<u8> = bytes[at..end].to_vec();
            for _ in 0..(1 + rng.below(4)) {
                bytes.extend_from_slice(&run);
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Drive one mutated input through the C-ABI (apply + every read-back) and
/// through merge + teleport, asserting each call RETURNS.
fn drive(payload: &str) -> Result<(), String> {
    // --- APPLY + RENDER, through the C-ABI exactly as a native binding does ---
    let abi = quietly(|| {
        let (tp, tl) = input(SEED_TREE);
        let session = unsafe { fuaran_session_new(tp, tl) };
        unsafe { fuaran_dealloc(tp, tl) };
        if session.is_null() {
            return;
        }

        let (op, ol) = input(payload);
        let _ = take_buf(unsafe { fuaran_session_apply_op(session, op, ol) });
        let _ = take_buf(unsafe { fuaran_session_place(session, op, ol) });
        let _ = take_buf(unsafe { fuaran_session_resolved_rows(session, op, ol) });
        unsafe { fuaran_dealloc(op, ol) };

        // The mutated payload also reaches the three store channels, which is
        // how a hostile value gets into a binding a render then resolves.
        let (k, kl) = input("revenue");
        let (v, vl) = input(payload);
        let _ = take_buf(unsafe { fuaran_session_set_state(session, k, kl, v, vl) });
        let _ = take_buf(unsafe { fuaran_session_set_filter(session, k, kl, v, vl) });
        let _ = take_buf(unsafe { fuaran_session_set_query(session, k, kl, v, vl) });
        unsafe { fuaran_dealloc(k, kl) };
        unsafe { fuaran_dealloc(v, vl) };

        // Every read-back, after the writes — this is where the render path,
        // the markdown reader and the sanitizer are reached.
        let _ = take_buf(unsafe { fuaran_session_render(session) });
        let _ = take_buf(unsafe { fuaran_session_tree_json(session) });
        let _ = take_buf(unsafe { fuaran_session_project_resolved(session) });
        unsafe { fuaran_session_free(session) };
    });
    if abi.is_err() {
        return Err("a panic escaped the C-ABI boundary".to_string());
    }

    // --- MERGE, through its crate API (it is not on the C-ABI) ---------------
    let merged = quietly(|| {
        let base = decode_node(SEED_TREE).ok();
        let side = decode_node(payload).ok();
        if let (Some(base), Some(side)) = (base, side)
            // `merge3_way` documents its inputs as three trees sharing a root
            // id, so the leg stays inside that contract: what it probes is the
            // CHILDREN a mutation reshapes, which is where the vanished-child
            // arm lived. Driving it outside its documented domain would be
            // measuring an undefined case, not a defect.
            && base.id == side.id
        {
            let _ = merge3_way(&base, &side, &base);
            let _ = merge3_way(&base, &base, &side);
            let _ = merge3_way(&side, &side, &side);
        }
    });
    if merged.is_err() {
        return Err("a panic escaped merge3_way".to_string());
    }

    // --- TELEPORT, through its crate API ------------------------------------
    let ported = quietly(|| {
        let _ = teleport::decode(payload);
        let _ = teleport::decode(&format!("FT1.{payload}"));
    });
    if ported.is_err() {
        return Err("a panic escaped teleport::decode".to_string());
    }

    Ok(())
}

#[test]
fn no_input_escapes_apply_render_merge_or_teleport_as_a_panic() {
    let seed: u64 = std::env::var("FUARAN_ABI_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5EED_0AB1_u64);
    let iterations: usize = std::env::var("FUARAN_ABI_FUZZ_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);

    let mut rng = Rng(seed);
    let corpus: Vec<&str> = SEED_OPS
        .iter()
        .chain(SEED_PLACEMENTS.iter())
        .chain(std::iter::once(&SEED_TREE))
        .copied()
        .collect();

    // Every unmutated seed first: a fuzz that has never driven a VALID input
    // through the surface is measuring the refusal path only.
    for seed_input in &corpus {
        if let Err(why) = drive(seed_input) {
            panic!("{why} on the unmutated seed:\n{seed_input}");
        }
    }

    for i in 0..iterations {
        let base = corpus[rng.below(corpus.len())];
        let payload = mutate(&mut rng, base);
        if let Err(why) = drive(&payload) {
            panic!(
                "{why}\n  seed:       {seed}\n  iteration:  {i}\n  replay:     \
                 FUARAN_ABI_FUZZ_SEED={seed} cargo test --test abi_safety\n  \
                 input:      {payload}"
            );
        }
    }

    println!("  [abi-safety] {iterations} mutated inputs, seed {seed}, no escape");
}
