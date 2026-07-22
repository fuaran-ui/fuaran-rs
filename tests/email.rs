//! Email-safe digest projection (Send Me) — a tree renders to plain text that
//! survives any inbox: content nodes only, no HTML/JS, no injection surface.

use fuaran_rs::render::email::email_digest;
use fuaran_rs::wire::decode_node;

const REPORT: &str = r#"{"id":"report","kind":{"$type":"Box","children":[
    {"id":"h","kind":{"$type":"Heading","level":1,"text":{"$type":"Literal","text":"Weekly Report"},"variant":"Standard"}},
    {"id":"intro","kind":{"$type":"Markdown","text":{"$type":"Literal","text":"Revenue is up."}}},
    {"id":"rev","kind":{"$type":"Metric","emphasis":"Loud","format":{"$type":"Currency","code":"GBP"},"label":{"$type":"Literal","text":"Revenue"},"value":{"$type":"State","defaultValue":0,"key":"rev"},"tone":"Brand","weight":"Standard"}},
    {"id":"note","kind":{"$type":"Callout","body":{"$type":"Literal","text":"Ends Friday."},"dismissable":false,"heading":{"$type":"Literal","text":"Deadline"},"tone":"Warning"}},
    {"id":"todo","kind":{"$type":"List","items":[{"$type":"Literal","text":"Ship it"},{"$type":"Literal","text":"Tell the team"}],"ordered":false}},
    {"id":"cta","kind":{"$type":"Button","label":{"$type":"Literal","text":"Open dashboard"},"onClick":{"$type":"Dispatch","msg":"<closure>"},"variant":"Primary"}}
],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Card"}}"#;

fn tree() -> fuaran_rs::wire::Node {
    decode_node(REPORT).expect("report decodes")
}

#[test]
fn digest_carries_the_reading_content_in_order() {
    let digest = email_digest(&tree());
    let expected = "# Weekly Report\nRevenue is up.\nRevenue: (live value)\n! Deadline\n  Ends Friday.\n- Ship it\n- Tell the team";
    assert_eq!(digest, expected);
}

#[test]
fn digest_omits_interactive_and_structural_nodes() {
    let digest = email_digest(&tree());
    // The Button (interactive) contributes nothing to an inbox digest.
    assert!(!digest.contains("Open dashboard"));
    // No HTML markup — pure text, safe in any mail client.
    assert!(!digest.contains('<'));
    assert!(!digest.contains("&lt;"));
}

#[test]
fn an_empty_layout_tree_digests_to_the_empty_string() {
    let empty = decode_node(
        r#"{"id":"root","kind":{"$type":"Box","children":[],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#,
    )
    .unwrap();
    assert_eq!(email_digest(&empty), "");
}
