//! Client-session behaviour: the decode → render → drive loop the browser
//! (`wasm32`) client runs, exercised natively over the target-agnostic
//! [`ClientSession`]. The WASM ABI is a thin marshalling shim over exactly
//! this type, so covering it here covers the client tier's logic.

use fuaran_rs::canonical::JVal;
use fuaran_rs::client::{ClientError, ClientSession, RowsOutcome};

const TREE: &str = r#"{"id":"root","kind":{"$type":"Box","children":[
    {"id":"title","kind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"Live"},"variant":"Standard"}},
    {"id":"metric","kind":{"$type":"Metric","emphasis":"Loud","format":{"$type":"Currency","code":"GBP"},"label":{"$type":"Literal","text":"Revenue"},"value":{"$type":"State","defaultValue":0,"key":"revenue"},"tone":"Brand","weight":"Standard"}}
],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;

fn session() -> ClientSession {
    ClientSession::new(TREE).expect("tree decodes")
}

#[test]
fn new_decodes_and_renders_server_parity_html() {
    let s = session();
    let html = s.render();
    // The same reference class vocabulary the server renderer emits.
    assert!(html.contains("data-fuaran-node-id=\"root\""));
    assert!(html.contains("fuaran-kind-heading"));
    assert!(html.contains("fuaran-metric-value"));
}

#[test]
fn new_surfaces_a_decode_error_for_a_bad_tree() {
    // `ClientSession` is intentionally not `Debug` (it holds the whole tree), so
    // match rather than `unwrap_err`.
    match ClientSession::new(r#"{"id":"","kind":{"$type":"Markdown","text":"x"}}"#) {
        Ok(_) => panic!("expected an EMPTY_NODE_ID decode error"),
        Err(err) => assert_eq!(err.code.as_str(), "EMPTY_NODE_ID"),
    }
}

#[test]
fn state_write_back_drives_a_reactive_binding() {
    let mut s = session();
    // Before any write, the State binding falls to its carried default (0).
    assert!(s.render().contains("fuaran-metric-value\">GBP 0.00<"));
    // The write-back: an omitted-handler control writes its value to the slot.
    s.set_state("revenue", "425.5").expect("state write");
    assert!(s.render().contains("fuaran-metric-value\">GBP 425.50<"));
}

#[test]
fn apply_op_mutates_the_held_tree_and_re_renders() {
    let mut s = session();
    assert!(s.render().contains(">Live</h1>"));
    s.apply_op(
        r#"{"$type":"EditNode","target":"title","newKind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"Updated"},"variant":"Standard"}}"#,
    )
    .expect("op applies");
    let html = s.render();
    assert!(html.contains(">Updated</h1>"));
    assert!(!html.contains(">Live</h1>"));
    // The tree round-trips out through the codec, reflecting the edit.
    assert!(s.tree_json().contains("Updated"));
}

#[test]
fn a_failed_op_leaves_the_tree_untouched_with_a_structured_error() {
    let mut s = session();
    let before = s.tree_json();
    let err = s
        .apply_op(r#"{"$type":"RemoveNode","target":"ghost"}"#)
        .unwrap_err();
    match err {
        ClientError::Apply(e) => assert_eq!(e.code.as_str(), "NodeNotFound"),
        ClientError::Decode(_) => panic!("expected an apply error"),
    }
    // The held tree is unchanged.
    assert_eq!(s.tree_json(), before);
}

#[test]
fn error_envelopes_are_stable_json_for_the_js_boundary() {
    let mut s = session();
    // An apply failure.
    let apply_err = s
        .apply_op(r#"{"$type":"RemoveNode","target":"ghost"}"#)
        .unwrap_err();
    let json = apply_err.to_json();
    assert!(json.contains("\"class\":\"apply\""));
    assert!(json.contains("\"code\":\"NodeNotFound\""));

    // A malformed state value.
    let state_err = s.set_state("revenue", "not json").unwrap_err();
    let json = state_err.to_json();
    assert!(json.contains("\"class\":\"decode\""));
    assert!(json.contains("\"code\":\"INVALID_JSON\""));
}

#[test]
fn filter_and_query_stores_resolve_their_bindings() {
    // A grid consuming a host-fed query result renders its rows.
    let tree = r#"{"id":"g","kind":{"$type":"DataGrid","columns":[
        {"field":"name","format":{"$type":"None"},"kind":{"$type":"Text"},"label":"Name","width":{"$type":"Auto"}}
    ],"editable":false,"source":{"$type":"Query","accessor":"<closure>","name":"rows"}}}"#;
    let mut s = ClientSession::new(tree).expect("decodes");
    s.set_query("rows", r#"[{"name":"Widgets"},{"name":"Gadgets"}]"#)
        .expect("query seed");
    let html = s.render();
    assert!(html.contains(">Widgets<"));
    assert!(html.contains(">Gadgets<"));
}

// ─── Resolved rows (the out-of-band row hand-off) ────────────────────────────

/// A grid whose rows come from an embedded `Transform` — the shape a decode-only
/// consumer cannot resolve for itself, and the reason this entry point exists.
const GRID: &str = r#"{"id":"root","kind":{"$type":"Box","children":[
    {"id":"shipments","kind":{"$type":"DataGrid","columns":[{"field":"status","kind":{"$type":"TonedPill","default":"Subdued","field":"status","map":{"Delayed":"Warning"}},"label":"Status"}],"rowKeyField":"status","source":{"$type":"Transform","pipeline":[],"source":{"columns":{"status":{"validity":[true,true],"values":["Delayed","Other"]}},"schema":[{"name":"status","type":"string"}]}}}},
    {"id":"heading","kind":{"$type":"Heading","level":1,"text":"Shipments","variant":"Standard"}}
],"layout":{"$type":"Auto"},"role":"Group"}}"#;

#[test]
fn resolved_rows_hands_over_what_the_tree_cannot_carry() {
    let s = ClientSession::new(GRID).expect("grid decodes");
    // The tree JSON still carries the UNRESOLVED Transform — that is the whole
    // problem this entry point answers, so assert it rather than assume it.
    assert!(s.tree_json().contains("\"$type\":\"Transform\""));
    // …and `project_resolved` does not fix it either: a row-context Transform
    // resolves to a collection, which cannot ride a `Static` slot (§2 rule 11).
    assert!(s.project_resolved().contains("\"$type\":\"Transform\""));

    let RowsOutcome::Rows(rows) = s.resolved_rows("shipments") else {
        panic!("expected resolved rows");
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].field("status"), Some(&JVal::Str("Delayed".into())));
    assert_eq!(rows[1].field("status"), Some(&JVal::Str("Other".into())));
}

#[test]
fn a_node_with_no_row_source_is_distinguishable_from_empty() {
    let s = ClientSession::new(GRID).expect("grid decodes");
    // A real node of a kind that has no row source…
    assert_eq!(s.resolved_rows("heading"), RowsOutcome::NoRowSource);
    // …and an id that names nothing at all. Both are caller mistakes, and
    // neither may masquerade as "this grid has no rows".
    assert_eq!(s.resolved_rows("nope"), RowsOutcome::NoRowSource);
}

#[test]
fn an_unresolvable_source_is_not_flattened_to_zero_rows() {
    // A grid bound to a Query the host has not fed. A consumer must render its
    // LOADING surface here — reporting zero rows would show "no data" for "not
    // yet", and the two are not the same claim.
    let json = r#"{"id":"g","kind":{"$type":"DataGrid","columns":[],"rowKeyField":"id","source":{"$type":"Query","dependsOn":[],"name":"shipments"}}}"#;
    let s = ClientSession::new(json).expect("decodes");
    assert_eq!(s.resolved_rows("g"), RowsOutcome::NotResolved);

    // Once the host feeds it, the SAME call resolves — including to genuinely
    // zero rows, which is the empty state and reads differently.
    let mut s = s;
    s.set_query("shipments", "[]").expect("seeds");
    assert_eq!(s.resolved_rows("g"), RowsOutcome::Rows(vec![]));
}
