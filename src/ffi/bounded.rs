//! A conformance-harness entry point for the bounded loop, behind the
//! `driver-semantics-abi` feature.
//!
//! # Why this exists, and why it is not part of the default ABI
//!
//! The bounded loop's conformance claim is about a *loop*, and this crate ships
//! that loop for two targets: a native host, and the browser-native `wasm32`
//! client. Certifying it on the native target alone would leave the `wasm32`
//! claim resting on the fact that the code compiles — which is worth something
//! and is not the same as running it.
//!
//! There is no filesystem inside a `wasm32` module, so a harness there cannot
//! read a corpus; what it can do is take a scenario's three documents in and
//! hand a verdict back. That is all this entry point is. It runs **exactly** the
//! comparison the native harness runs — the same
//! [`crate::bounded::trace`](crate::bounded::trace) code, compiled for a
//! different target — because a second comparison written for the second target
//! would be certifying two things and reporting one.
//!
//! It is **feature-gated and off by default**, so it is not part of the stable
//! export surface `include/fuaran.h` declares and imposes no compatibility
//! obligation on a consumer binding that surface. A host wanting it opts in:
//!
//! ```text
//! cargo build --target wasm32-unknown-unknown --release --features driver-semantics-abi
//! ```

use crate::bounded::{
    first_divergence, normalise_expectation, parse_events, parse_expectation, run_scenario,
};
use crate::canonical::{JVal, parse, render_canonical};
use crate::ffi::{FuaranBuf, borrow_str, pack_string};

fn envelope(key: &str, value: &str) -> String {
    format!(
        "{{{}:{}}}",
        render_canonical(&JVal::Str(key.to_string())),
        render_canonical(&JVal::Str(value.to_string()))
    )
}

fn string_member(document: &JVal, key: &str) -> Result<String, String> {
    match document.field(key) {
        Some(JVal::Str(s)) => Ok(s.clone()),
        _ => Err(format!("the request carries no string '{key}'")),
    }
}

fn check(request: &str) -> Result<Option<String>, String> {
    let document =
        parse(request).map_err(|e| format!("the request does not parse: {}", e.message))?;
    let name = string_member(&document, "name")?;
    let tree = string_member(&document, "tree")?;
    let events = string_member(&document, "events")?;
    let expectation = string_member(&document, "expectation")?;
    // OPTIONAL: the §10.3 host-policy NAME a scenario declares, absent for a
    // scenario that presumes nothing about the performer seam. A name rather
    // than a policy — the corpus never carries a policy as data — and an
    // unrecognised one is refused by `run_scenario` rather than defaulted.
    let host_policy = match document.field("hostPolicy") {
        None => None,
        Some(JVal::Str(s)) => Some(s.clone()),
        Some(_) => return Err("the request carries a non-string 'hostPolicy'".to_string()),
    };

    let events = parse_events(&events)?;
    let recorded = parse_expectation(&expectation)?;
    if recorded.len() != events.len() + 1 {
        return Err(format!(
            "{name}: a trace carries one entry per step, index 0 being the state before any event"
        ));
    }
    let observed = run_scenario(&tree, &events, host_policy.as_deref())?;
    let expected = normalise_expectation(&name, &recorded)?;
    Ok(first_divergence(&name, &expected, &observed).map(|d| d.describe()))
}

/// Check one scenario, given its three documents as the strings
/// `{"name":…,"tree":…,"events":…,"expectation":…}`, plus an optional
/// `"hostPolicy"` naming the performer-seam policy the scenario presumes.
///
/// Returns `{"ok":""}` when the loop reproduces the recorded trace,
/// `{"divergence":"…"}` naming the first divergence when it does not, or
/// `{"error":"…"}` when the request itself could not be read. The caller frees
/// the returned buffer with `fuaran_dealloc`, per the module's memory contract.
///
/// # Safety
/// `ptr`/`len` must name a live UTF-8 buffer per the memory contract in
/// [`crate::ffi`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fuaran_bounded_check_scenario(ptr: *const u8, len: usize) -> FuaranBuf {
    // SAFETY: caller contract.
    let Some(request) = (unsafe { borrow_str(ptr, len) }) else {
        return pack_string(envelope("error", "the request is not valid UTF-8"));
    };
    pack_string(match check(request) {
        Ok(None) => envelope("ok", ""),
        Ok(Some(report)) => envelope("divergence", &report),
        Err(message) => envelope("error", &message),
    })
}
