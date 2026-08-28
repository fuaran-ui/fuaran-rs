//! `Binding.State` slot seeding — `WIRE_FORMAT.md` §24.4, and its §24.6
//! conformance leg.
//!
//! §24.6 is a RENDER-parity obligation, not a codec one: the bytes round-trip
//! identically with or without the rule, so every codec family in
//! `tests/conformance.rs` passes on a host that has not adopted it — this host
//! decoded `nodes/shared-source-seeded-pair` green throughout and rendered the
//! wrong value. That is why this file asserts a DERIVED VALUE rather than
//! bytes: it is the only leg that can tell an adopting host from a
//! non-adopting one.
//!
//! Measured here before the pass landed: the badge rendered EMPTY, because its
//! own `Transform` source declares `"defaultValue": []` and nothing filled the
//! slot the grid beside it carries the rows for. After: `2`, the value the two
//! reference tiers pin.
//!
//! Every rule carries its deliberately mis-seeded case alongside the correct
//! one: a rule asserted only in its passing direction cannot tell a working
//! implementation from an absent one.

use std::collections::HashMap;
use std::path::PathBuf;

use fuaran_rs::canonical::JVal;
use fuaran_rs::client::ClientSession;
use fuaran_rs::render::{
    BindingSources, collect_state_seeds, render_to_html, render_with_islands, with_state_seeds,
};
use fuaran_rs::wire::{Node, decode_node};

const BADGE_OPEN: &str = "class=\"fuaran-badge fuaran-badge-info\">";

/// Walks up from the crate directory looking for the shared corpus (a sibling
/// checkout). `None` keeps the repo standalone-testable — legs skip, not fail.
fn find_corpus() -> Option<PathBuf> {
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

/// The Info badge's rendered text. Narrow on purpose: it matches the emitted
/// element and nothing else, so a renderer that stopped emitting the badge
/// panics here rather than matching some other span.
fn badge_text(html: &str) -> String {
    let i = html
        .find(BADGE_OPEN)
        .unwrap_or_else(|| panic!("no Info badge in the rendered fragment: {html}"));
    let rest = &html[i + BADGE_OPEN.len()..];
    rest[..rest.find('<').expect("unterminated badge element")].to_string()
}

fn tree(doc: &str) -> Node {
    decode_node(doc).expect("fixture should decode")
}

/// A DECLARING reader: a `Metric` whose value carries a `defaultValue`.
fn metric(id: &str, key: &str, declaration: &str) -> String {
    format!(
        r#"{{"id":"{id}","kind":{{"$type":"Metric","label":"L","value":{{"$type":"State",{declaration}"key":"{key}"}}}}}}"#
    )
}

/// A reader that declares NOTHING: a `Badge` label bound to a bare
/// `{"$type":"State","key":k}`.
///
/// A `Badge` label rather than the obvious `Metric` value, and the choice is
/// forced rather than stylistic. This host's decoder cannot represent an ABSENT
/// `State.defaultValue`: `Binding::State` holds a plain `StaticValue`, so an
/// absent one decodes to the slot's typed placeholder, and at a NUMERIC slot
/// that placeholder (`0`) is re-encoded as a real `"defaultValue":0` — which is
/// a declaration to anything reading the document, including this walk. See
/// `a_bare_state_at_a_numeric_slot_is_re_encoded_with_a_fabricated_default`,
/// which pins that as a named pre-existing defect rather than leaving it to be
/// rediscovered. A text-shaped slot's placeholder IS the absent sentinel, so
/// the encoder omits it and the bare spelling survives the round trip intact.
fn badge_bound(id: &str, key: &str) -> String {
    format!(
        r#"{{"id":"{id}","kind":{{"$type":"Badge","label":{{"$type":"Bound","binding":{{"$type":"State","key":"{key}"}}}},"variant":"Info"}}}}"#
    )
}

fn boxed(children: &[String]) -> String {
    format!(
        r#"{{"id":"root","kind":{{"$type":"Box","children":[{}],"layout":{{"$type":"Auto"}},"role":"Dashboard"}}}}"#,
        children.join(",")
    )
}

fn seeds_of(doc: &str) -> HashMap<String, JVal> {
    collect_state_seeds(&tree(doc))
}

fn rows(teams: &[&str]) -> JVal {
    JVal::Arr(
        teams
            .iter()
            .map(|t| JVal::Obj(vec![("team".to_string(), JVal::Str((*t).to_string()))]))
            .collect(),
    )
}

fn seeded_pair() -> Option<Node> {
    let corpus = find_corpus()?;
    let raw = std::fs::read_to_string(corpus.join("nodes/shared-source-seeded-pair.json")).ok()?;
    Some(tree(&raw))
}

// ── §24.6 — the render-parity assertion ──────────────────────────────────────

/// One declared table under `$state.members`, read by a grid's `source` and by
/// a badge's `Transform`, resolves the badge's derivation over the grid's two
/// rows.
///
/// The VALUE is the assertion, not the markup: `2` is what the reference tiers
/// render for this fixture, so a host that agrees on the bytes and disagrees
/// here is exactly the divergence §24.4 was written to close.
#[test]
fn the_seeded_pair_renders_the_declared_count() {
    let Some(pair) = seeded_pair() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let sources = BindingSources::default();
    assert_eq!(badge_text(&render_to_html(&pair, &sources)), "2");

    // The islands surface must not differ: one document would otherwise render
    // two values depending only on whether a region was marked an island.
    assert_eq!(
        badge_text(&render_with_islands(&pair, &sources, &["member-grid"])),
        "2"
    );
}

/// The go-red half of the assertion above. An assertion nobody has watched fail
/// is a claim about the author's confidence, not about the renderer — so the
/// same badge is measured under a HOST value that makes the derivation say
/// something else, and it must move. Both perturbations are legitimate
/// documents; neither changes a byte of the tree.
#[test]
fn the_parity_assertion_is_sensitive_to_the_derived_value() {
    let Some(pair) = seeded_pair() else {
        return;
    };
    let mut one = BindingSources::default();
    one.state.insert("members".to_string(), rows(&["Solo"]));
    assert_eq!(badge_text(&render_to_html(&pair, &one)), "1");

    let mut none = BindingSources::default();
    none.state.insert("members".to_string(), JVal::Arr(vec![]));
    assert_ne!(
        badge_text(&render_to_html(&pair, &none)),
        "2",
        "an EMPTY host value still derived 2 — the assertion above would pass on a host that ignores the slot"
    );
}

/// The native surfaces over this core (`fuaran-swift`, `fuaran-kt`) obtain a
/// derived value ONLY through the session's projections — they carry no
/// evaluator of their own — so their §24.4 adoption is inherited from here.
/// This is the assertion that makes the inheritance a fact rather than an
/// assumption: it exercises the same two entry points those bindings call.
#[test]
fn the_session_projections_carry_the_seed_to_a_decode_only_consumer() {
    let Some(corpus) = find_corpus() else { return };
    let raw = std::fs::read_to_string(corpus.join("nodes/shared-source-seeded-pair.json"))
        .expect("fixture should be readable");
    let session = ClientSession::new(&raw).expect("fixture should decode");

    // `project_resolved` folds the badge's scalar Transform to a literal.
    let projected = session.project_resolved();
    assert!(
        projected.contains("\"label\":\"2\""),
        "the resolved projection did not carry the seeded derivation: {projected}"
    );

    // `resolved_rows` hands over the grid's rows, which come from the SAME slot.
    match session.resolved_rows("member-grid") {
        fuaran_rs::client::RowsOutcome::Rows(rows) => assert_eq!(rows.len(), 2),
        other => panic!("expected the grid's two declared rows, got {other:?}"),
    }

    // And a WRITE still wins over the seed, through the session's own channel.
    let mut written = ClientSession::new(&raw).expect("fixture should decode");
    written
        .set_state("members", r#"[{"team":"Only"}]"#)
        .expect("a write to an ordinary key should be accepted");
    assert!(
        written.project_resolved().contains("\"label\":\"1\""),
        "a written value did not override the seed"
    );
}

// ── §24.4 — the five rules ───────────────────────────────────────────────────

/// Rule 1 — WHO DECLARES: any `Binding.State` with a PRESENT `defaultValue`, in
/// any slot.
#[test]
fn rule1_a_present_default_declares_and_an_absent_one_does_not() {
    let declared = seeds_of(&metric("m", "users", r#""defaultValue":7,"#));
    assert_eq!(declared.get("users"), Some(&JVal::Num(7.0)));

    let silent = seeds_of(&badge_bound("b", "users"));
    assert!(
        !silent.contains_key("users"),
        "a State carrying NO defaultValue seeded its slot"
    );
}

/// Rule 2 — PRECEDENCE: host value > written value > seed. A seed is the value
/// before anything else has said anything, never an override.
#[test]
fn rule2_the_host_value_wins_over_the_seed() {
    let t = tree(&metric("m", "users", r#""defaultValue":7,"#));

    let mut host = BindingSources::default();
    host.state.insert("users".to_string(), JVal::Num(99.0));
    let merged = with_state_seeds(&t, &host);
    assert_eq!(
        merged.state.get("users"),
        Some(&JVal::Num(99.0)),
        "the seed overrode the host's own value"
    );

    let empty = BindingSources::default();
    assert_eq!(
        with_state_seeds(&t, &empty).state.get("users"),
        Some(&JVal::Num(7.0)),
        "the seed did not reach a caller that named nothing"
    );

    // The caller's own sources are never mutated: a host may reuse one across
    // renders, and a pass that wrote into it would leak the first tree's
    // declarations into the second tree's render. The borrow checker enforces
    // this here, which is why the assertion is on the ABSENCE of a clone
    // instead: an unseeded tree must not pay for one.
    let bare = tree(&badge_bound("b", "users"));
    assert!(
        matches!(
            with_state_seeds(&bare, &empty),
            std::borrow::Cow::Borrowed(_)
        ),
        "a tree that declares nothing still cloned the caller's sources"
    );
}

/// Rule 3 — ORDER-INDEPENDENCE: seeding runs over the WHOLE tree before any
/// binding resolves, so a reader that appears before the declaration is not a
/// special case.
#[test]
fn rule3_document_order_carries_no_meaning() {
    let declaring = metric("declares", "users", r#""defaultValue":7,"#);
    let reading = badge_bound("reads", "users");

    let after = seeds_of(&boxed(&[declaring.clone(), reading.clone()]));
    let before = seeds_of(&boxed(&[reading, declaring]));
    assert_eq!(after, before, "the seed depended on document order");
    assert_eq!(before.get("users"), Some(&JVal::Num(7.0)));
}

/// Rule 4 — TWO DECLARATIONS OF ONE KEY. A disagreement is `FUARAN106`'s to
/// name; a renderer must still be deterministic and takes the FIRST in tree
/// order.
#[test]
fn rule4_the_first_declaration_wins() {
    let first = metric("first", "k", r#""defaultValue":1,"#);
    let second = metric("second", "k", r#""defaultValue":2,"#);

    assert_eq!(
        seeds_of(&boxed(&[first.clone(), second.clone()])).get("k"),
        Some(&JVal::Num(1.0))
    );
    assert_eq!(
        seeds_of(&boxed(&[second, first])).get("k"),
        Some(&JVal::Num(2.0)),
        "reversing the pair did not reverse the winner — the walk is not order-following"
    );
}

/// Rule 4, second half. The empty declaration must not WIN the race, or a badge
/// spelling `"defaultValue": []` before the grid that carries the rows would
/// seed the slot EMPTY and make rule 3 false — and it must not CONFLICT, or
/// that same pair would raise `FUARAN106` on the very document the seeding rule
/// exists to make work.
#[test]
fn rule4_an_empty_declaration_declares_nothing() {
    // Both readers are ROW-shaped, which is where the empty declaration is
    // actually written: `"defaultValue": []` is how a rows slot says "I read
    // this key and carry no data of my own".
    let grid = |id: &str, default: &str| {
        format!(
            r#"{{"id":"{id}","kind":{{"$type":"DataGrid","columns":[{{"field":"team","kind":{{"$type":"Text"}},"label":"Team"}}],"rowKeyField":"team","source":{{"$type":"State","defaultValue":{default},"key":"rows"}}}}}}"#
        )
    };
    let empty = grid("empty", "[]");
    let carrying = grid("carrying", r#"[{"team":"Ops"}]"#);

    let both = seeds_of(&boxed(&[empty.clone(), carrying]));
    assert_eq!(
        both.get("rows"),
        Some(&rows(&["Ops"])),
        "an empty declaration ahead of a carrying one won a race it must not enter"
    );

    assert!(
        !seeds_of(&boxed(&[empty])).contains_key("rows"),
        "an empty declaration seeded its slot on its own"
    );
}

/// Rule 5 — a seed is a tree-originated write, and §12's reserved `host.`
/// namespace refuses those on every path; the wire must not gain a way around a
/// deliberate floor.
#[test]
fn rule5_a_host_reserved_key_is_never_seeded() {
    assert!(
        !seeds_of(&metric("m", "host.users", r#""defaultValue":7,"#)).contains_key("host.users"),
        "a host-reserved key was seeded from the tree"
    );

    // The identical declaration on an ordinary key DOES seed, so the assertion
    // above is measuring the prefix rather than a broken walk.
    assert!(
        seeds_of(&metric("m", "users", r#""defaultValue":7,"#)).contains_key("users"),
        "the control declaration did not seed — rule 5's evidence is vacuous"
    );
}

// ── A named pre-existing defect this pass surfaced ───────────────────────────

/// This host cannot represent an ABSENT `State.defaultValue`, and at a NUMERIC
/// slot that loses information the wire carried.
///
/// `Binding::State` holds a plain `StaticValue`, so an absent `defaultValue`
/// decodes to the slot's typed placeholder. Where that placeholder is the
/// absent sentinel (`StringOpt(None)` / `Ast(Null)`) the encoder omits it again
/// and the round trip is faithful, which is why the corpus is green: its five
/// bare-`State` occurrences all sit at such slots. At a `Float` / `Int` slot the
/// placeholder is `0`, and the encoder emits it — so
/// `{"$type":"State","key":k}` re-encodes as `{"$type":"State","defaultValue":0,
/// "key":k}`, a byte-level round-trip failure no fixture exercises.
///
/// Pinned rather than fixed, and pinned rather than left silent. It PREDATES
/// §24.4 — the seeding pass surfaced it, having to ask "did this reader
/// declare?" of every binding in the tree — and closing it means giving
/// `Binding::State` an optional default, which changes what `resolve` yields for
/// every unwritten numeric state slot on this host. That is a decoder change
/// with its own parity argument, not a footnote to a renderer rule.
///
/// The seeding consequence is stated plainly: a bare `State` at a numeric slot
/// seeds `0` here and seeds nothing on the hosts that model absence. This test
/// is what makes that a known number rather than a surprise.
#[test]
fn a_bare_state_at_a_numeric_slot_is_re_encoded_with_a_fabricated_default() {
    let doc = metric("m", "users", "");
    let t = tree(&doc);
    assert_ne!(
        fuaran_rs::wire::encode_node(&t),
        doc,
        "the round trip is faithful now — delete this pin and the comment above it"
    );
    assert_eq!(
        seeds_of(&doc).get("users"),
        Some(&JVal::Num(0.0)),
        "the fabricated default is what this walk sees; if it no longer does, the decoder was fixed"
    );

    // The text-shaped slot the rule tests use is faithful, which is what makes
    // the divergence a slot-typing question rather than a walk defect.
    let bare = badge_bound("b", "users");
    assert_eq!(fuaran_rs::wire::encode_node(&tree(&bare)), bare);
}
