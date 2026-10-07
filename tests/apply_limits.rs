//! The apply-time §21 tree limits (Phase 2141): every op that can grow the
//! tree is checked on its RESULT, and one that takes it past `MAX_NODE_DEPTH`
//! or `MAX_NODES` is refused with `LimitExceeded`.
//!
//! `EditNode` can give a node a kind that holds children and `UpdateState` can
//! attach `onLoading` / `onEmpty` subtrees, so both put nodes into the tree;
//! `MoveNode` adds none, but moving one legal branch under the leaf of another
//! stacks two depths that each passed. All three were accepted past the limit.
//! The last test certifies this host against the shared
//! `apply/limits-apply.json` corpus family.

use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::limits::{MAX_NODE_DEPTH, MAX_NODES};
use fuaran_rs::ops::{ApplyErrorCode, apply};
use fuaran_rs::wire::{Node, NodeKind, StateBehaviour, TreeOp, decode_node, decode_op};

const FLEX: &str =
    r#""layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group""#;

fn box_json(id: &str, children: &[String]) -> String {
    format!(
        r#"{{"id":"{id}","kind":{{"$type":"Box","children":[{}],{FLEX}}}}}"#,
        children.join(",")
    )
}

/// A Box chain `depth` levels deep: top `<prefix><depth>`, deepest `<prefix>1`.
fn chain_json(prefix: &str, depth: usize) -> String {
    let mut node = box_json(&format!("{prefix}1"), &[]);
    for d in 2..=depth {
        node = box_json(&format!("{prefix}{d}"), &[node]);
    }
    node
}

fn node(json: &str) -> Node {
    decode_node(json).expect("test tree decodes")
}

fn chain(depth: usize) -> Node {
    node(&chain_json("n", depth))
}

/// The kind of a Box holding `children` (wire JSON) — what an EditNode installs.
fn box_kind(children: &[String]) -> NodeKind {
    node(&box_json("carrier", children)).kind
}

/// A Box holding `n` leaves, built in memory: a document this wide would be
/// refused by the decoder before it reached apply.
fn wide(id: &str, n: usize) -> Node {
    let mut wide = node(&box_json(id, &[]));
    let leaf = node(&box_json("leaf", &[]));
    if let NodeKind::Box(spec) = &mut wide.kind {
        spec.children = (0..n)
            .map(|i| {
                let mut l = leaf.clone();
                l.id = format!("m{i}");
                l
            })
            .collect();
    }
    wide
}

fn edit(target: &str, new_kind: NodeKind) -> TreeOp {
    TreeOp::EditNode {
        target: target.into(),
        new_kind,
    }
}

fn loading(target: &str, n: Node) -> TreeOp {
    TreeOp::UpdateState {
        target: target.into(),
        state: StateBehaviour {
            on_loading: Some(Box::new(n)),
            on_empty: None,
            on_error: None,
        },
    }
}

fn mv(target: &str, new_parent_id: &str) -> TreeOp {
    TreeOp::MoveNode {
        target: target.into(),
        new_parent_id: new_parent_id.into(),
    }
}

fn forked(a: usize, b: usize) -> Node {
    node(&box_json("root", &[chain_json("a", a), chain_json("b", b)]))
}

fn assert_limit_exceeded(tree: &Node, op: &TreeOp, what: &str) {
    match apply(tree, op) {
        Ok(_) => panic!("{what}: applied past the limit"),
        Err(e) => assert_eq!(
            e.code,
            ApplyErrorCode::LimitExceeded,
            "{what}: {}",
            e.message
        ),
    }
}

#[test]
fn growing_ops_are_refused_past_the_limits() {
    let half = MAX_NODE_DEPTH / 2;
    let over = vec![box_json("over", &[])];
    let cases: Vec<(&str, Node, TreeOp)> = vec![
        (
            "EditNode past MAX_NODE_DEPTH",
            chain(MAX_NODE_DEPTH),
            edit("n1", box_kind(&over)),
        ),
        (
            "EditNode past MAX_NODES",
            chain(2),
            edit("n1", wide("carrier", MAX_NODES).kind),
        ),
        (
            "UpdateState past MAX_NODE_DEPTH",
            chain(MAX_NODE_DEPTH),
            loading("n1", node(&box_json("loading", &[]))),
        ),
        (
            "UpdateState past MAX_NODES",
            chain(2),
            loading("n1", wide("loading", MAX_NODES)),
        ),
        (
            "MoveNode stacking two legal branches",
            forked(half, half),
            mv(&format!("b{half}"), "a1"),
        ),
        (
            "Batch carrying a growing EditNode",
            chain(MAX_NODE_DEPTH),
            TreeOp::Batch(vec![edit("n1", box_kind(&over))]),
        ),
    ];
    for (what, tree, op) in &cases {
        assert_limit_exceeded(tree, op, what);
    }
}

#[test]
fn repeated_ids_do_not_hide_an_over_deep_payload() {
    // The guard reads the NODES an op carries, not their ids: a chain whose
    // every id is "x" has one id and many levels. Built in memory — the decoder
    // would refuse a document this deep.
    let mut deep = node(&box_json("x", &[]));
    for _ in 1..(MAX_NODE_DEPTH + 6) {
        let mut parent = node(&box_json("x", &[]));
        if let NodeKind::Box(spec) = &mut parent.kind {
            spec.children = vec![deep];
        }
        deep = parent;
    }
    assert_limit_exceeded(
        &node(&box_json("root", &[])),
        &TreeOp::ReplaceRoot { node: deep.clone() },
        "ReplaceRoot",
    );
    let mut carrier = node(&box_json("carrier", &[]));
    if let NodeKind::Box(spec) = &mut carrier.kind {
        spec.children = vec![deep];
    }
    assert_limit_exceeded(&chain(2), &edit("n1", carrier.kind), "EditNode");
    // The decodable shape - a child repeating the edited id, one level past the
    // limit - is pinned by the corpus family below.
}

#[test]
fn growing_ops_landing_exactly_at_the_limit_apply() {
    let half = MAX_NODE_DEPTH / 2;
    let at = vec![box_json("at-the-limit", &[])];
    let cases: Vec<(&str, Node, TreeOp)> = vec![
        (
            "EditNode",
            chain(MAX_NODE_DEPTH - 1),
            edit("n1", box_kind(&at)),
        ),
        (
            "UpdateState",
            chain(MAX_NODE_DEPTH - 1),
            loading("n1", node(&box_json("loading", &[]))),
        ),
        (
            "MoveNode",
            forked(half, half - 1),
            mv(&format!("b{}", half - 1), "a1"),
        ),
    ];
    for (what, tree, op) in &cases {
        if let Err(e) = apply(tree, op) {
            panic!(
                "{what} reaching exactly MAX_NODE_DEPTH was refused: {}",
                e.message
            );
        }
    }
}

#[test]
fn an_edit_to_a_childless_kind_on_an_over_limit_tree_applies() {
    // An edit that puts no subtree in cannot grow the tree, so it is not
    // charged the walk, and refusing it would strand a tree it did not create.
    let mut over = chain(MAX_NODE_DEPTH);
    // Deepen past the decoder's ceiling in memory.
    for d in 0..5 {
        let mut parent = node(&box_json(&format!("top{d}"), &[]));
        if let NodeKind::Box(spec) = &mut parent.kind {
            spec.children = vec![over];
        }
        over = parent;
    }
    if let Err(e) = apply(&over, &edit("n1", box_kind(&[]))) {
        panic!("a non-growing EditNode was refused: {}", e.message);
    }
}

// ── The shared corpus family ────────────────────────────────────────────────

const CORPUS_ENV: &str = "FUARAN_WIRE_FIXTURES";

fn find_corpus() -> Option<PathBuf> {
    if let Ok(declared) = std::env::var(CORPUS_ENV) {
        let root = PathBuf::from(&declared);
        assert!(
            root.join("manifest.json").is_file(),
            "{CORPUS_ENV} names '{declared}', which holds no manifest.json"
        );
        return Some(root);
    }
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let root = dir.join("wire-format-fixtures");
        if root.join("manifest.json").is_file() {
            return Some(root);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn field<'a>(v: &'a JVal, key: &str) -> &'a JVal {
    match v {
        JVal::Obj(members) => members
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("missing member {key}")),
        _ => panic!("expected an object holding {key}"),
    }
}

fn text(v: &JVal) -> &str {
    match v {
        JVal::Str(s) => s,
        _ => panic!("expected a string"),
    }
}

#[test]
fn limits_apply_corpus_holds_on_this_host() {
    let Some(root) = find_corpus() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let read = |rel: &str| {
        let raw = std::fs::read_to_string(root.join(rel)).expect("corpus file reads");
        parse(&raw).expect("corpus file parses")
    };
    let manifest = read("apply/manifest.json");
    let JVal::Arr(families) = field(&manifest, "families") else {
        panic!("families is not an array")
    };
    let family = families
        .iter()
        .find(|f| text(field(f, "id")) == "limitsApply")
        .expect("apply/manifest.json declares limitsApply");
    let declared = match field(family, "vectors") {
        JVal::Num(n) => *n as usize,
        _ => panic!("vectors is not a number"),
    };
    let doc = read(&format!("apply/{}", text(field(family, "file"))));
    let JVal::Arr(vectors) = field(&doc, "vectors") else {
        panic!("vectors is not an array")
    };
    assert_eq!(
        vectors.len(),
        declared,
        "the manifest's vector count matches the family file"
    );

    for v in vectors {
        let id = text(field(v, "id"));
        let input = field(v, "input");
        let expected = field(v, "expected");
        let tree = decode_node(text(field(input, "tree")))
            .unwrap_or_else(|e| panic!("{id}: the tree did not decode: {e:?}"));
        let op = decode_op(text(field(input, "op")))
            .unwrap_or_else(|e| panic!("{id}: the op did not decode: {e:?}"));
        match (text(field(expected, "verdict")), apply(&tree, &op)) {
            ("accept", Ok(_)) => {}
            ("accept", Err(e)) => panic!("{id}: expected accept, refused with {:?}", e.code),
            ("reject", Err(e)) => {
                assert_eq!(
                    e.code.as_str(),
                    text(field(expected, "code")),
                    "{id}: the refusal code"
                )
            }
            ("reject", Ok(_)) => panic!("{id}: expected a refusal, the op applied"),
            (other, _) => panic!("{id}: unknown verdict {other}"),
        }
    }
}
