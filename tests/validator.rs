//! Pre-emit validator behaviour: each rule fires on its canonical defect and
//! stays silent on the clean shape. Trees are authored as canonical wire JSON
//! through the host's own decoder — the pre-emit posture a driving service has.

use fuaran_rs::validator::{Severity, ValidateOptions, validate, validate_with};
use fuaran_rs::wire::decode_node;

fn findings_for(json: &str) -> Vec<(String, String)> {
    let tree = decode_node(json).expect("test tree decodes");
    validate(&tree)
        .into_iter()
        .map(|f| (f.code.to_string(), f.node_id))
        .collect()
}

fn codes(json: &str) -> Vec<String> {
    findings_for(json).into_iter().map(|(c, _)| c).collect()
}

#[test]
fn clean_tree_passes() {
    let clean = r#"{"id":"root","kind":{"$type":"Box","children":[
        {"id":"h1","kind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"T"},"variant":"Standard"}}
    ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;
    assert!(codes(clean).is_empty());
}

#[test]
fn duplicate_node_id_is_fuaran001() {
    let dup = r#"{"id":"root","kind":{"$type":"Box","children":[
        {"id":"x","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"a"}}},
        {"id":"x","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"b"}}}
    ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;
    assert_eq!(codes(dup), vec!["FUARAN001"]);
}

#[test]
fn tabs_shape_rules() {
    // 2 headers over 1 child → FUARAN047; 2 tags over 1 child → FUARAN048.
    let tabs = r#"{"id":"t","kind":{"$type":"Tabs","activeIndex":{"$type":"State","defaultValue":0,"key":"tab"},"children":[
        {"id":"c1","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"a"}}}
    ],"orientation":"Horizontal","tabHeaders":[
        {"label":{"$type":"Literal","text":"One"}},{"label":{"$type":"Literal","text":"Two"}}
    ],"tabTags":["a","b"]}}"#;
    let found = codes(tabs);
    assert!(found.contains(&"FUARAN047".to_string()), "{found:?}");
    assert!(found.contains(&"FUARAN048".to_string()), "{found:?}");

    // activeTag without tabTags → FUARAN049.
    let tag_only = r#"{"id":"t","kind":{"$type":"Tabs","activeIndex":{"$type":"State","defaultValue":0,"key":"tab"},"activeTag":{"$type":"State","defaultValue":"a","key":"tag"},"children":[
        {"id":"c1","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"a"}}}
    ],"orientation":"Horizontal"}}"#;
    assert!(codes(tag_only).contains(&"FUARAN049".to_string()));
}

#[test]
fn progress_fraction_bounds_is_fuaran050() {
    let over = r#"{"id":"p","kind":{"$type":"Progress","fraction":{"$type":"Static","value":1.5},"indeterminate":false,"tone":"Default"}}"#;
    assert_eq!(codes(over), vec!["FUARAN050"]);
    let ok = r#"{"id":"p","kind":{"$type":"Progress","fraction":{"$type":"Static","value":0.5},"indeterminate":false,"tone":"Default"}}"#;
    assert!(codes(ok).is_empty());
}

#[test]
fn blank_currency_code_is_fuaran061() {
    let blank = r#"{"id":"m","kind":{"$type":"Markdown","text":{"$type":"Bound","binding":{"$type":"Format","format":{"$type":"Currency","isoCode":""},"locale":{"$type":"Ambient"},"source":{"$type":"Static","value":42}}}}}"#;
    assert_eq!(codes(blank), vec!["FUARAN061"]);
}

#[test]
fn blank_link_href_is_fuaran063() {
    let blank = r#"{"id":"l","kind":{"$type":"Link","download":false,"href":{"$type":"Static","value":""},"label":{"$type":"Literal","text":"Docs"}}}"#;
    assert_eq!(codes(blank), vec!["FUARAN063"]);
}

#[test]
fn static_false_disabled_is_fuaran064() {
    let noop = r#"{"id":"b","kind":{"$type":"Button","disabled":{"$type":"Static","value":false},"label":{"$type":"Literal","text":"Go"},"onClick":{"$type":"Navigate","route":"/x"},"variant":"Primary"}}"#;
    assert_eq!(codes(noop), vec!["FUARAN064"]);
    // Static(true) — a permanently-disabled placeholder — is not flagged.
    let disabled = r#"{"id":"b","kind":{"$type":"Button","disabled":{"$type":"Static","value":true},"label":{"$type":"Literal","text":"Go"},"onClick":{"$type":"Navigate","route":"/x"},"variant":"Primary"}}"#;
    assert!(codes(disabled).is_empty());
}

#[test]
fn inert_control_is_fuaran069() {
    // Declarative field (no onChange) over a Static value: write-back cannot arm.
    let inert = r#"{"id":"f","kind":{"$type":"Form","fields":[
        {"id":"name","kind":{"$type":"Text","value":{"$type":"Static","value":"x"}},"label":{"$type":"Literal","text":"Name"},"required":false}
    ],"onSubmit":{"$type":"Chain","ops":[]},"submitLabel":{"$type":"Literal","text":"Save"}}}"#;
    assert_eq!(codes(inert), vec!["FUARAN069"]);

    // The same field over a writable State slot arms the write-back — silent.
    let writable = r#"{"id":"f","kind":{"$type":"Form","fields":[
        {"id":"name","kind":{"$type":"Text","value":{"$type":"State","defaultValue":"","key":"name"}},"label":{"$type":"Literal","text":"Name"},"required":false}
    ],"onSubmit":{"$type":"Chain","ops":[]},"submitLabel":{"$type":"Literal","text":"Save"}}}"#;
    assert!(codes(writable).is_empty());

    // A present closure handler wins — silent even over a Static value.
    let closured = r#"{"id":"f","kind":{"$type":"Form","fields":[
        {"id":"name","kind":{"$type":"Text","onChange":"<closure>","value":{"$type":"Static","value":"x"}},"label":{"$type":"Literal","text":"Name"},"required":false}
    ],"onSubmit":{"$type":"Chain","ops":[]},"submitLabel":{"$type":"Literal","text":"Save"}}}"#;
    assert!(codes(closured).is_empty());
}

#[test]
fn fire_and_forget_call_is_fuaran073() {
    let faf = r#"{"id":"b","kind":{"$type":"Button","label":{"$type":"Literal","text":"Go"},"onClick":{"$type":"Call","endpoint":"api/refresh"},"variant":"Primary"}}"#;
    assert_eq!(codes(faf), vec!["FUARAN073"]);
    let into = r#"{"id":"b","kind":{"$type":"Button","label":{"$type":"Literal","text":"Go"},"onClick":{"$type":"Call","endpoint":"api/refresh","into":{"$type":"State","key":"result"}},"variant":"Primary"}}"#;
    assert!(codes(into).is_empty());
}

#[test]
fn duplicate_switch_match_is_fuaran082() {
    let dup = r#"{"id":"sw","kind":{"$type":"Switch","cases":[
        {"child":{"id":"a","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"a"}}},"match":"x"},
        {"child":{"id":"b","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"b"}}},"match":"x"}
    ],"default":{"id":"d","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"d"}}},"stateKey":"mode"}}"#;
    assert!(codes(dup).contains(&"FUARAN082".to_string()));
}

#[test]
fn ungrounded_switch_state_key_is_fuaran083() {
    // An empty-key State selector can never resolve a case — the switch is
    // stuck on its default.
    let ungrounded = r#"{"id":"sw","kind":{"$type":"Switch","cases":[
        {"child":{"id":"a","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"a"}}},"match":"x"}
    ],"default":{"id":"d","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"d"}}},"stateKey":""}}"#;
    let tree = decode_node(ungrounded).expect("decodes");
    let found = validate(&tree);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "FUARAN083");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(found[0].node_id, "sw");

    // A named state key grounds the selector — silent.
    let named = r#"{"id":"sw","kind":{"$type":"Switch","cases":[
        {"child":{"id":"a","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"a"}}},"match":"x"}
    ],"default":{"id":"d","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"d"}}},"stateKey":"mode"}}"#;
    assert!(codes(named).is_empty());

    // The widened rule: any non-State selector names its source and is
    // grounded by construction — a Selection selector is silent.
    let selection = r#"{"id":"sw","kind":{"$type":"Switch","cases":[
        {"child":{"id":"a","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"a"}}},"match":"x"}
    ],"default":{"id":"d","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"d"}}},"on":{"$type":"Selection","nodeId":"grid"}}}"#;
    assert!(codes(selection).is_empty());
}

#[test]
fn computed_binding_is_fuaran084_and_hardens_when_orchestrated() {
    let computed = r#"{"id":"m","kind":{"$type":"Metric","emphasis":"Normal","format":{"$type":"None"},"label":{"$type":"Literal","text":"x"},"value":{"$type":"Computed","fn":"<closure>"},"tone":"Default","weight":"Standard"}}"#;
    let tree = decode_node(computed).expect("decodes");

    let advisory = validate(&tree);
    assert_eq!(advisory.len(), 1);
    assert_eq!(advisory[0].code, "FUARAN084");
    assert_eq!(advisory[0].severity, Severity::Warning);

    let orchestrated = validate_with(&tree, ValidateOptions { orchestrated: true });
    assert_eq!(orchestrated[0].severity, Severity::Error);
}

#[test]
fn findings_anchor_to_the_offending_node() {
    let nested = r#"{"id":"root","kind":{"$type":"Box","children":[
        {"id":"deep","kind":{"$type":"Progress","fraction":{"$type":"Static","value":2},"indeterminate":false,"tone":"Default"}}
    ],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;
    let found = findings_for(nested);
    assert_eq!(found, vec![("FUARAN050".to_string(), "deep".to_string())]);
}

#[test]
fn chart_schema_grounding_family_fuaran086_089() {
    // A helper: a Chart over an Embedded two-column table (dept: string,
    // amount: int) with an empty pipeline — the statically-known schema case.
    let chart = |kind: &str, extra: &str, x: &str, y: &str| {
        format!(
            r#"{{"id":"c","kind":{{"$type":"Chart","kind":"{kind}","source":{{"$type":"Transform","pipeline":[],"source":{{"columns":{{"amount":{{"validity":[true],"values":[1]}},"dept":{{"validity":[true],"values":["ops"]}}}},"schema":[{{"name":"amount","type":"int"}},{{"name":"dept","type":"string"}}]}}}},"stacked":{extra},"xField":"{x}","yFields":[{y}]}}}}"#
        )
    };

    // Clean: a grounded bar chart passes.
    assert!(codes(&chart("Bar", "false", "dept", "\"amount\"")).is_empty());

    // FUARAN086 — an ungrounded field name (x and y).
    let found = codes(&chart("Bar", "false", "ghost", "\"amount\""));
    assert_eq!(found, vec!["FUARAN086"], "{found:?}");
    let found = codes(&chart("Bar", "false", "dept", "\"ghost\""));
    assert_eq!(found, vec!["FUARAN086"], "{found:?}");

    // FUARAN087 — a grounded but non-numeric value field; and Scatter's
    // numeric x requirement.
    let found = codes(&chart("Bar", "false", "dept", "\"dept\""));
    assert_eq!(found, vec!["FUARAN087"], "{found:?}");
    let found = codes(&chart("Scatter", "false", "dept", "\"amount\""));
    assert_eq!(found, vec!["FUARAN087"], "{found:?}");

    // FUARAN088 — pie with other than exactly one series (Error).
    let found = codes(&chart("Pie", "false", "dept", "\"amount\",\"amount\""));
    assert!(found.contains(&"FUARAN088".to_string()), "{found:?}");

    // FUARAN089 — stacked on a kind where stacking is meaningless (Warning).
    let found = codes(&chart("Line", "true", "dept", "\"amount\""));
    assert_eq!(found, vec!["FUARAN089"], "{found:?}");

    // An unknowable source (a Query) deliberately passes ungrounded.
    let query = r#"{"id":"c","kind":{"$type":"Chart","kind":"Bar","source":{"$type":"Query","name":"rows"},"stacked":false,"xField":"ghost","yFields":["also-ghost"]}}"#;
    assert!(codes(query).is_empty());
}

/// FUARAN108 (Phase 1076) — `ImageSpec.alt`'s a11y floor WITHOUT the decorative
/// escape, and the absence of that escape is the whole of the rule. An image
/// can honestly declare `alt=""`; a media element is a TRANSPORT a reader
/// focuses, plays, pauses and seeks, so it is never decorative, and an unnamed
/// one is announced as "video" or "audio" and nothing more.
///
/// Error rather than Warning because there is no legitimate shape it refuses.
#[test]
fn empty_media_label_is_fuaran108() {
    let unnamed = r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Video"},"label":"","src":{"$type":"Static","value":"/w.mp4"}}}"#;
    assert_eq!(codes(unnamed), vec!["FUARAN108"]);
    assert!(matches!(
        validate(&decode_node(unnamed).expect("decodes")).as_slice(),
        [f] if f.severity == Severity::Error
    ));

    // Whitespace is not a name either — an author who typed a space has not
    // told a listener what the recording is.
    let blank = r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Audio"},"label":"   ","src":{"$type":"Static","value":"/c.mp3"}}}"#;
    assert_eq!(codes(blank), vec!["FUARAN108"]);

    // A named transport passes, and so does one whose name is DEFERRED: a
    // `Bound` or `I18n` label resolves at render time, and refusing one on the
    // evidence available pre-emit would accuse a document that names its
    // transport perfectly well.
    let named = r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Video"},"label":"Studio walkthrough","src":{"$type":"Static","value":"/w.mp4"}}}"#;
    assert!(codes(named).is_empty());
    let deferred = r#"{"id":"m","kind":{"$type":"Media","kind":{"$type":"Audio"},"label":{"$type":"I18n","args":{},"key":"clip.name"},"src":{"$type":"Static","value":"/c.mp3"}}}"#;
    assert!(codes(deferred).is_empty());
}

// ── FUARAN075 — the dangling-filter-reference rule (Phase 1836) ──────────────
//
// Hand-built trees, so every arm can be driven in the direction that FAILS. The
// corpus-bound twins (`nodes/filters-{dependson,param-source}-{declared,undeclared}`)
// are asserted in `tests/conformance.rs` against the corpus's own bytes, beside
// the check that `validator-coverage.json` declares what this host does.

const CHIPS_REGION: &str = r#"{"id":"chips","kind":{"$type":"Filters","items":[{"kind":{"$type":"Choice","options":{"$type":"Static","value":[{"label":"EMEA","value":"emea"}]}},"label":"Region","name":"region"}]}}"#;

const CHIPS_REGION_GENRE: &str = r#"{"id":"chips","kind":{"$type":"Filters","items":[{"kind":{"$type":"Choice","options":{"$type":"Static","value":[{"label":"EMEA","value":"emea"}]}},"label":"Region","name":"region"},{"kind":{"$type":"Choice","options":{"$type":"Static","value":[{"label":"Drama","value":"drama"}]}},"label":"Genre","name":"genre"}]}}"#;

fn metric(id: &str, value: &str) -> String {
    format!(r#"{{"id":"{id}","kind":{{"$type":"Metric","label":"Revenue","value":{value}}}}}"#)
}

fn query_depending_on(names: &[&str]) -> String {
    let deps = names
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"$type":"Query","dependsOn":[{deps}],"name":"orders"}}"#)
}

fn boxed(id: &str, children: &[&str]) -> String {
    format!(
        r#"{{"id":"{id}","kind":{{"$type":"Box","children":[{}],"layout":{{"$type":"Flex","direction":"Vertical","wrap":false}},"role":"Group"}}}}"#,
        children.join(",")
    )
}

fn fuaran075(json: &str) -> Vec<(String, String)> {
    let tree = decode_node(json).expect("test tree decodes");
    validate(&tree)
        .into_iter()
        .filter(|f| f.code == "FUARAN075")
        .map(|f| {
            assert_eq!(f.severity, Severity::Error, "FUARAN075 is an Error");
            (f.node_id, f.message)
        })
        .collect()
}

#[test]
fn dependson_naming_an_undeclared_chip_is_fuaran075() {
    let m = metric("scoped", &query_depending_on(&["region", "genre"]));
    let undeclared = boxed("root", &[CHIPS_REGION, &m]);
    let found = fuaran075(&undeclared);
    assert_eq!(
        found.len(),
        1,
        "exactly the undeclared name is reported: {found:?}"
    );
    let (reader, message) = &found[0];
    assert_eq!(
        reader, "scoped",
        "attributed to the edge's reader, not the root"
    );
    assert!(
        message.contains("'genre'") && message.contains("'scoped'"),
        "the message names the reader and the undeclared name: {message}"
    );
    assert!(
        !message.contains("'region'"),
        "the declared name is not reported"
    );

    let declared = boxed("root", &[CHIPS_REGION_GENRE, &m]);
    assert!(fuaran075(&declared).is_empty(), "the declared twin passes");
}

#[test]
fn transform_param_sourced_from_an_undeclared_chip_is_fuaran075() {
    let transform = r#"{"$type":"Transform","params":[{"from":{"$type":"Filter","name":"region"},"name":"region"},{"from":{"$type":"Filter","name":"genre"},"name":"genre"}],"pipeline":[{"$type":"filter","pred":{"$type":"binary","left":{"$type":"col","name":"region"},"op":"eq","right":{"$type":"param","name":"region"}}},{"$type":"filter","pred":{"$type":"binary","left":{"$type":"col","name":"genre"},"op":"eq","right":{"$type":"param","name":"genre"}}}],"source":{"columns":{"genre":{"validity":[true],"values":["drama"]},"region":{"validity":[true],"values":["emea"]}},"schema":[{"name":"region","type":"string"},{"name":"genre","type":"string"}]}}"#;
    let grid = format!(
        r#"{{"id":"grid","kind":{{"$type":"DataGrid","columns":[{{"field":"region","kind":{{"$type":"Text"}},"label":"Region"}}],"rowKeyField":"region","source":{transform}}}}}"#
    );
    let undeclared = boxed("root", &[CHIPS_REGION, &grid]);
    let found = fuaran075(&undeclared);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "grid");
    assert!(found[0].1.contains("'genre'"));

    let declared = boxed("root", &[CHIPS_REGION_GENRE, &grid]);
    assert!(fuaran075(&declared).is_empty());
}

#[test]
fn expr_param_sourced_from_an_undeclared_chip_is_fuaran075() {
    let expr = r#"{"$type":"Expr","expr":{"$type":"binary","left":{"$type":"param","name":"g"},"op":"eq","right":{"$type":"lit","cell":{"$type":"Str","value":"drama"}}},"params":[{"from":{"$type":"Filter","name":"genre"},"name":"g"}]}"#;
    let m = metric("flag", expr);
    let found = fuaran075(&boxed("root", &[CHIPS_REGION, &m]));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "flag");
    assert!(found[0].1.contains("'genre'"));
    assert!(fuaran075(&boxed("root", &[CHIPS_REGION_GENRE, &m])).is_empty());
}

#[test]
fn a_tree_with_no_filters_node_declares_nothing() {
    // The reference judges against the chips the TREE declares; a tree with none
    // declares none, so every declared edge dangles.
    let m = metric("scoped", &query_depending_on(&["region"]));
    let found = fuaran075(&boxed("root", &[&m]));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, "scoped");
}

#[test]
fn a_plain_filter_value_read_is_not_an_edge() {
    // A host may feed filter values without chips; only a DECLARED edge is judged.
    let m = metric("plain", r#"{"$type":"Filter","name":"genre"}"#);
    assert!(fuaran075(&boxed("root", &[&m])).is_empty());
}

#[test]
fn a_chip_declared_after_its_consumer_still_grounds_it() {
    let m = metric("scoped", &query_depending_on(&["region", "genre"]));
    assert!(fuaran075(&boxed("root", &[&m, CHIPS_REGION_GENRE])).is_empty());
}

#[test]
fn a_chip_declared_anywhere_in_the_tree_grounds_the_edge() {
    let m = metric("scoped", &query_depending_on(&["genre"]));
    let deep_chips = boxed("inner", &[CHIPS_REGION_GENRE]);
    assert!(fuaran075(&boxed("root", &[&m, &deep_chips])).is_empty());
}

#[test]
fn edges_attribute_to_their_nearest_enclosing_node() {
    // Nested two deep, and inside a node-level `state.onEmpty` child: each edge
    // is reported against the node that carries it, never an ancestor.
    let deep = metric("deep", &query_depending_on(&["genre"]));
    let placeholder = metric("placeholder", &query_depending_on(&["season"]));
    let with_empty = format!(
        r#"{{"id":"host","kind":{{"$type":"Metric","label":"Revenue","value":{}}},"state":{{"onEmpty":{placeholder}}}}}"#,
        query_depending_on(&["region"])
    );
    let tree = boxed(
        "root",
        &[CHIPS_REGION, &boxed("mid", &[&deep]), &with_empty],
    );
    let mut readers: Vec<String> = fuaran075(&tree).into_iter().map(|(r, _)| r).collect();
    readers.sort();
    assert_eq!(readers, vec!["deep".to_string(), "placeholder".to_string()]);
}

#[test]
fn an_inserted_fragment_is_not_judged_against_chips_it_cannot_see() {
    // An InsertChild subtree is grounded by the tree it lands in, which this call
    // does not have: judging the fragment alone would accuse every idiomatic
    // consumer inserted under a Filters-declaring page.
    let m = metric("inserted", &query_depending_on(&["region"]));
    let insert = format!(r#"{{"$type":"InsertChild","child":{m},"parentId":"root"}}"#);
    let op = fuaran_rs::wire::decode_op(&insert).expect("insert decodes");
    assert!(
        fuaran_rs::validator::validate_op(&op)
            .iter()
            .all(|f| f.code != "FUARAN075")
    );

    // A ReplaceRoot carries a WHOLE tree, so the rule applies to it.
    let replace = format!(
        r#"{{"$type":"ReplaceRoot","node":{}}}"#,
        boxed("root", &[&m])
    );
    let op = fuaran_rs::wire::decode_op(&replace).expect("replace decodes");
    assert!(
        fuaran_rs::validator::validate_op(&op)
            .iter()
            .any(|f| f.code == "FUARAN075" && f.node_id == "inserted")
    );
}
