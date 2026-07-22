//! Client-session behaviour: the decode → render → drive loop the browser
//! (`wasm32`) client runs, exercised natively over the target-agnostic
//! [`ClientSession`]. The WASM ABI is a thin marshalling shim over exactly
//! this type, so covering it here covers the client tier's logic.

use fuaran_rs::client::{ClientError, ClientSession};

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
