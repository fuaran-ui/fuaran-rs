//! The README's quick-start, compiled.
//!
//! # Why this exists
//!
//! `README.md`'s first `rust` block is the first code a reader of this crate
//! ever runs, and nothing compiled it. It had fallen two record widenings
//! behind — `Node` gained `tooltip` (Phase 1112) and `visible` (Phase 1535),
//! `BoxSpec` gained `keep_together` and `break_before` (Phase 1473) — so the
//! documented quick-start did not build, and the only way to find that out was
//! to be a new reader typing it in.
//!
//! Appending the missing fields to the README would have fixed the instance and
//! left the mechanism: the next widening breaks it again, silently, in exactly
//! the same way. So the code lives HERE, where `cargo test` builds it, and the
//! README's copy is held to it by text.
//!
//! # What the parity check claims, and what it does not
//!
//! It compares the two under whitespace normalisation — every run of whitespace
//! collapses to one space before the comparison. That is deliberate: `cargo fmt
//! --check` owns this file's layout and will reflow it, and a check that also
//! pinned line breaks would go red for a formatting decision rather than for a
//! documentation defect. So it claims **the two carry the same tokens in the
//! same order** — which is what catches a field the README does not set — and
//! it claims nothing about their indentation or line wrapping.
//!
//! It also does not claim the README's *other* code blocks compile. Only the
//! quick-start is held here; the egress block further down is an illustrative
//! fragment over values it does not construct.

use fuaran_rs::wire::{
    BoxLayout, BoxRole, BoxSpec, HeadingSpec, HeadingVariant, Node, NodeKind, TextSource,
    encode_node,
};

/// The quick-start body, compiled. Everything between the two markers is the
/// text `README.md`'s first `rust` block must agree with, so an edit here that
/// is not mirrored there — or there and not here — fails the test below.
fn quickstart() -> String {
    // README-QUICKSTART-BEGIN
    let tree = Node {
        id: "root".into(),
        kind: NodeKind::Box(BoxSpec {
            children: vec![Node {
                id: "title".into(),
                kind: NodeKind::Heading(HeadingSpec {
                    level: 2,
                    text: TextSource::Literal("Channel performance".into()),
                    variant: HeadingVariant::Standard,
                }),
                state: Default::default(),
                style: Default::default(),
                accessibility: None,
                tooltip: None,
                visible: None,
            }],
            heading: None,
            layout: BoxLayout::Auto,
            role: BoxRole::Dashboard,
            keep_together: false,
            break_before: false,
        }),
        state: Default::default(),
        style: Default::default(),
        accessibility: None,
        tooltip: None,
        visible: None,
    };

    let wire: String = encode_node(&tree); // canonical wire JSON, byte-identical to every host
    // README-QUICKSTART-END
    wire
}

fn normalise(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The quick-start does what it says: the tree it builds encodes, and the bytes
/// are the canonical ones. A block that merely *compiled* would satisfy the
/// parity check below while emitting anything at all.
#[test]
fn the_quick_start_encodes_the_tree_it_documents() {
    let wire = quickstart();
    assert_eq!(
        wire,
        concat!(
            r#"{"id":"root","kind":{"$type":"Box","children":[{"id":"title","#,
            r#""kind":{"$type":"Heading","level":2,"text":"Channel performance","#,
            r#""variant":"Standard"}}],"layout":{"$type":"Auto"},"role":"Dashboard"}}"#
        ),
        "the documented quick-start no longer encodes to the canonical bytes"
    );
}

#[test]
fn the_readme_quick_start_matches_the_compiled_one() {
    let readme = include_str!("../README.md");
    let source = include_str!("readme_quickstart.rs");

    // The README's first ```rust block.
    let after = readme
        .split_once("```rust\n")
        .expect("README.md carries a ```rust block")
        .1;
    let block = after
        .split_once("\n```")
        .expect("the README's first ```rust block is closed")
        .0;

    // Everything between this file's own markers. The marker LINES are matched
    // by their text rather than by a line number, so inserting a doc paragraph
    // above cannot silently shift the region.
    let after = source
        .split_once("// README-QUICKSTART-BEGIN\n")
        .expect("this file carries a begin marker")
        .1;
    let compiled = after
        .split_once("// README-QUICKSTART-END")
        .expect("this file carries an end marker")
        .0;

    // The README's block opens with the `use` this file hoists to module scope,
    // so the comparison starts where the two are the same document.
    let documented = block
        .split_once("let tree = Node {")
        .expect("the README quick-start builds `let tree = Node {`")
        .1;

    let documented = normalise(documented);
    let compiled = normalise(
        compiled
            .split_once("let tree = Node {")
            .expect("the compiled quick-start builds `let tree = Node {`")
            .1,
    );

    assert!(
        !documented.is_empty(),
        "the README block extracted empty — the extraction, not the documentation, is broken"
    );
    assert_eq!(
        compiled, documented,
        "README.md's quick-start and the compiled one in tests/readme_quickstart.rs have \
         diverged. The compiled one is the source of truth: it builds. Copy it into the README \
         rather than editing this assertion."
    );
}
