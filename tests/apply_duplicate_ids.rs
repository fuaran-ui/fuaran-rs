//! Apply refuses a tree that would hold duplicate node ids (Phase 2172,
//! WIRE_FORMAT §8.1).
//!
//! Every op addresses its target by id alone, so a tree that holds one id
//! twice makes every later id-addressed op ambiguous. The decoder accepts a
//! repeated id, and an apply could BUILD one from parts that each decoded
//! cleanly. The check reads the op's RESULT and charges it only for the ids it
//! installed.

use fuaran_rs::limits::MAX_NODE_DEPTH;
use fuaran_rs::ops::{ApplyErrorCode, apply};
use fuaran_rs::wire::{Node, decode_node, decode_op};

const FLEX: &str =
    r#""layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group""#;

fn bx(id: &str, children: &[&str]) -> String {
    format!(
        r#"{{"id":"{id}","kind":{{"$type":"Box","children":[{}],{FLEX}}}}}"#,
        children.join(",")
    )
}

fn kind(children: &[&str]) -> String {
    format!(
        r#"{{"$type":"Box","children":[{}],{FLEX}}}"#,
        children.join(",")
    )
}

fn edit(target: &str, new_kind: &str) -> String {
    format!(r#"{{"$type":"EditNode","newKind":{new_kind},"target":"{target}"}}"#)
}

fn node(json: &str) -> Node {
    decode_node(json).expect("test tree decodes")
}

fn outcome(tree: &str, op: &str) -> Result<(), ApplyErrorCode> {
    let op = decode_op(op).expect("test op decodes");
    apply(&node(tree), &op).map(|_| ()).map_err(|e| e.code)
}

fn base() -> String {
    bx("r", &[&bx("a", &[]), &bx("b", &[])])
}

#[test]
fn an_installed_duplicate_is_refused() {
    let x = bx("x", &[]);
    let cases: Vec<(&str, String)> = vec![
        (
            "ReplaceRoot repeats an id",
            format!(
                r#"{{"$type":"ReplaceRoot","node":{}}}"#,
                bx("r2", &[&x, &x])
            ),
        ),
        (
            "ReplaceRoot repeats its root id below",
            format!(
                r#"{{"$type":"ReplaceRoot","node":{}}}"#,
                bx("r", &[&bx("r", &[])])
            ),
        ),
        (
            "EditNode collides with the tree",
            edit("a", &kind(&[&bx("b", &[])])),
        ),
        (
            "EditNode repeats an id within its kind",
            edit("a", &kind(&[&x, &x])),
        ),
        (
            "EditNode repeats the edited node's id",
            edit("a", &kind(&[&bx("a", &[])])),
        ),
        (
            "UpdateState collides with the tree",
            format!(
                r#"{{"$type":"UpdateState","state":{{"onLoading":{}}},"target":"a"}}"#,
                bx("b", &[])
            ),
        ),
        (
            "InsertChild repeats an id within itself",
            format!(
                r#"{{"$type":"InsertChild","child":{},"parentId":"r"}}"#,
                bx("c", &[&x, &x])
            ),
        ),
        (
            "InsertChild collides with the tree",
            format!(
                r#"{{"$type":"InsertChild","child":{},"parentId":"r"}}"#,
                bx("b", &[])
            ),
        ),
        (
            "a Batch whose result repeats an installed id",
            format!(
                r#"{{"$type":"Batch","ops":[{}]}}"#,
                edit("a", &kind(&[&bx("b", &[])]))
            ),
        ),
    ];
    for (what, op) in cases {
        assert_eq!(
            outcome(&base(), &op),
            Err(ApplyErrorCode::DuplicateNodeId),
            "{what}"
        );
    }
}

#[test]
fn what_only_looks_like_a_duplicate_applies() {
    let cases: Vec<(&str, String, String)> = vec![
        (
            "ReplaceRoot reuses the replaced tree's ids",
            base(),
            format!(
                r#"{{"$type":"ReplaceRoot","node":{}}}"#,
                bx("r", &[&bx("a", &[]), &bx("b", &[])])
            ),
        ),
        (
            "EditNode restates the children it replaces",
            bx("r", &[&bx("a", &[&bx("c", &[])])]),
            edit("a", &kind(&[&bx("c", &[]), &bx("d", &[])])),
        ),
        (
            "UpdateState replaces its own alternative",
            format!(
                r#"{{"id":"r","kind":{{"$type":"Box","children":[{{"id":"a","kind":{{"$type":"Markdown","text":"a"}},"state":{{"onLoading":{}}}}}],{FLEX}}}}}"#,
                bx("l", &[])
            ),
            format!(
                r#"{{"$type":"UpdateState","state":{{"onLoading":{}}},"target":"a"}}"#,
                bx("l", &[&bx("l2", &[])])
            ),
        ),
        (
            "a duplicate the op did not install is not charged to it",
            bx(
                "r",
                &[&bx("x", &[]), &bx("y", &[&bx("x", &[])]), &bx("z", &[])],
            ),
            edit("z", &kind(&[&bx("w", &[])])),
        ),
        (
            "a Batch whose intermediate state duplicates but whose result does not",
            bx("r", &[&bx("a", &[]), &bx("s", &[&bx("b", &[])])]),
            format!(
                r#"{{"$type":"Batch","ops":[{},{}]}}"#,
                edit("a", &kind(&[&bx("b", &[])])),
                edit("s", &kind(&[]))
            ),
        ),
    ];
    for (what, tree, op) in cases {
        assert_eq!(outcome(&tree, &op), Ok(()), "{what}");
    }
}

#[test]
fn limit_exceeded_takes_precedence_over_a_duplicate() {
    // limitsApply's `editnode-repeated-id-past-maxdepth` pins this order.
    let mut chain = bx("n1", &[]);
    for d in 2..=MAX_NODE_DEPTH {
        chain = bx(&format!("n{d}"), &[&chain]);
    }
    assert_eq!(
        outcome(&chain, &edit("n1", &kind(&[&bx("n1", &[])]))),
        Err(ApplyErrorCode::LimitExceeded)
    );
}
