//! The WASM demo page's tree, decoded by the host it is a demo of.
//!
//! # Why this exists
//!
//! `js/index.html` is the browser-native demo: it loads the `wasm32` module and
//! mounts a small tree. Its tree had carried a retired Metric spelling
//! (`source` where the 0.2.0 rename law says `value` for a scalar displayed
//! value), so `createSession` threw `MISSING_FIELD` at `$.kind.value` and the
//! page never mounted — and no gate anywhere read the file, so the demo was
//! broken for as long as it took someone to open it.
//!
//! Correcting the spelling fixes the instance. This fixes the mechanism: the
//! page's tree is written as strict JSON between two markers, and this test
//! decodes it with the same decoder the module ships. A future wire change that
//! invalidates the demo now goes red here, in the commit that makes it.
//!
//! # What it claims
//!
//! That the demo's tree DECODES, and that its own extraction is not vacuous
//! (the perturbation probe below). It does not claim the page renders, mounts,
//! or that the loader's URLs resolve — those need a browser and a built module,
//! and a test that pretended otherwise would be the sort of green this crate's
//! corpus resolvers are being tightened to avoid.

use fuaran_rs::wire::decode_node;

const PAGE: &str = include_str!("../js/index.html");

/// The strict-JSON object literal between the page's two markers.
fn demo_tree_json() -> String {
    let after = PAGE
        .split_once("// DEMO-TREE-BEGIN\n")
        .expect("js/index.html carries a DEMO-TREE-BEGIN marker")
        .1;
    let block = after
        .split_once("// DEMO-TREE-END")
        .expect("js/index.html carries a DEMO-TREE-END marker")
        .0;
    let body = block
        .split_once("const tree =")
        .expect("the marked region declares `const tree =`")
        .1;
    body.trim().trim_end_matches(';').trim().to_string()
}

#[test]
fn the_demo_pages_tree_decodes() {
    let json = demo_tree_json();
    assert!(
        json.starts_with('{') && json.ends_with('}'),
        "the extraction did not yield an object literal — the extraction is broken, not the page:\n{json}"
    );
    match decode_node(&json) {
        Ok(node) => assert_eq!(node.id.as_str(), "root"),
        Err(e) => panic!(
            "js/index.html's demo tree does not decode, so the page throws before it mounts: {e:?}"
        ),
    }
}

#[test]
fn the_extraction_would_notice_a_broken_tree() {
    // Verify the probe, not just the verdict: an extraction that silently
    // yielded something the decoder happens to accept would report the same
    // green over a page that never mounts. Put the retired spelling back — the
    // exact defect this test was written for — and require a refusal.
    let perturbed = demo_tree_json().replace("\"value\":", "\"source\":");
    assert!(
        perturbed != demo_tree_json(),
        "the perturbation changed nothing, so this probe proves nothing"
    );
    assert!(
        decode_node(&perturbed).is_err(),
        "the retired `Metric.source` spelling decoded — this test cannot see the defect it exists for"
    );
}
