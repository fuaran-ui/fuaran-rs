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
