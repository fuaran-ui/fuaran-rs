//! LIST-valued `Binding.Transform` params — the Rust host's adoption of the
//! rule `WIRE_FORMAT.md` §3.3 states under "LIST-valued `Binding.Transform`
//! params (Phase 610)", and `fuaran-core#91` specifies.
//!
//! Three behaviours, and each is asserted here rather than inferred from the
//! other two:
//!
//! 1. **Resolution is by SUBSTITUTION, before evaluation.** A bound list param
//!    rewrites `InParam` to the literal `InList` form; the evaluator never sees
//!    a list env, and one that reaches it names an unbound param.
//! 2. **An EMPTY selection is UNBOUND, never `items: []`.** The dependent
//!    `filter` step prunes, so deselecting everything shows the UNFILTERED
//!    table — the acceptance criterion Phase 610 pinned as output.
//! 3. **A kind mismatch reaches the strict unbound-param refusal** — a list
//!    bound to a name the pipeline reads as a scalar `param`, or a scalar bound
//!    to one it reads through `in`/`param`. Never a silent wrong scoping.
//!
//! The oracle is the shared corpus fixture `nodes/multiselect-chip-list-param.json`
//! (a `multiple` Select whose `values` binding names a filter, beside a DataGrid
//! whose Transform scopes rows off the same name). The corpus is a sibling
//! checkout, so the fixture-driven legs report and skip when it is absent — the
//! substitution legs below are self-contained and always run.

use std::collections::HashMap;
use std::path::PathBuf;

use fuaran_rs::canonical::{JVal, parse};
use fuaran_rs::render::BindingSources;
use fuaran_rs::render::bindings::{ResolvedRows, resolve_rows};
use fuaran_rs::render::render_to_html;
use fuaran_rs::transform::{self, ListEnv};
use fuaran_rs::wire::{Binding, Cell, ColExpr, Node, NodeKind, TransformStep, decode_node};

// ─── the substitution itself (behaviour 1, in isolation) ─────────────────────

fn in_param(column: &str, name: &str) -> ColExpr {
    ColExpr::InParam {
        subject: Box::new(ColExpr::Col {
            name: column.to_string(),
        }),
        name: name.to_string(),
    }
}

fn list_env_of(name: &str, items: &[&str]) -> ListEnv {
    let mut env = ListEnv::new();
    env.insert(
        name.to_string(),
        items.iter().map(|s| Cell::Str((*s).to_string())).collect(),
    );
    env
}

#[test]
fn a_bound_list_param_is_rewritten_to_the_literal_membership_form() {
    let pipeline = vec![TransformStep::Filter {
        pred: in_param("dept", "depts"),
    }];
    let out = transform::substitute_list_params(&list_env_of("depts", &["eng", "ops"]), &pipeline);

    let TransformStep::Filter { pred } = &out[0] else {
        panic!("the step kind must survive substitution");
    };
    let ColExpr::InList { subject, items } = pred else {
        panic!(
            "a BOUND list param must become the literal InList form, not stay InParam: {pred:?}"
        );
    };
    assert_eq!(
        **subject,
        ColExpr::Col {
            name: "dept".into()
        }
    );
    assert_eq!(
        items,
        &vec![
            ColExpr::Lit {
                cell: Cell::Str("eng".into())
            },
            ColExpr::Lit {
                cell: Cell::Str("ops".into())
            },
        ],
        "the selection's order and cell coercion must ride through as literals"
    );
}

#[test]
fn an_unbound_list_param_survives_intact_so_the_prune_still_sees_its_name() {
    let pipeline = vec![TransformStep::Filter {
        pred: in_param("dept", "depts"),
    }];
    // A list env binding some OTHER name must leave this one alone.
    let out = transform::substitute_list_params(&list_env_of("regions", &["emea"]), &pipeline);

    let TransformStep::Filter { pred } = &out[0] else {
        panic!("step kind survives");
    };
    assert_eq!(
        pred,
        &in_param("dept", "depts"),
        "an unbound list param must survive intact — the caller's prune reads its name off it"
    );
    assert_eq!(
        transform::step_params(&out[0]),
        vec!["depts".to_string()],
        "and it must still NAME its own param, which is what makes one prune cover both kinds"
    );
}

#[test]
fn substitution_walks_nested_positions_and_non_filter_steps() {
    let pipeline = vec![
        TransformStep::Derive {
            name: "hit".into(),
            expr: ColExpr::Not {
                expr: Box::new(in_param("dept", "depts")),
            },
        },
        TransformStep::Distinct,
    ];
    let out = transform::substitute_list_params(&list_env_of("depts", &["eng"]), &pipeline);

    let TransformStep::Derive { expr, .. } = &out[0] else {
        panic!("a Derive carries a ColExpr and is substituted too");
    };
    let ColExpr::Not { expr } = expr else {
        panic!("the surrounding expression is preserved");
    };
    assert!(
        matches!(**expr, ColExpr::InList { .. }),
        "substitution must reach a NESTED InParam, not only a top-level one"
    );
    assert_eq!(
        out[1],
        TransformStep::Distinct,
        "a step carrying no ColExpr is returned unchanged"
    );
}

// ─── the corpus fixture (behaviours 1–3, end to end on the server path) ──────

/// Walks up from the crate directory looking for the shared corpus, matching
/// `tests/conformance.rs`.
fn corpus_nodes_dir() -> Option<PathBuf> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let root = dir.join("wire-format-fixtures");
        if root.join("manifest.json").is_file() {
            return Some(root.join("nodes"));
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// The oracle fixture's canonical bytes, or `None` on a standalone checkout.
pub fn fixture_json() -> Option<String> {
    let path = corpus_nodes_dir()?.join("multiselect-chip-list-param.json");
    Some(std::fs::read_to_string(path).expect("the fixture file reads"))
}

fn find_node<'a>(node: &'a Node, id: &str) -> Option<&'a Node> {
    if node.id == id {
        return Some(node);
    }
    if let NodeKind::Box(spec) = &node.kind {
        for child in &spec.children {
            if let Some(found) = find_node(child, id) {
                return Some(found);
            }
        }
    }
    None
}

fn grid_source(tree: &Node) -> Binding {
    let grid = find_node(tree, "dept-grid").expect("the fixture carries the dept-grid node");
    let NodeKind::DataGrid(spec) = &grid.kind else {
        panic!("dept-grid is a DataGrid");
    };
    spec.source.clone()
}

fn sources_with_filter(value: Option<&str>) -> BindingSources {
    let mut filters: HashMap<String, JVal> = HashMap::new();
    if let Some(json) = value {
        filters.insert(
            "depts".to_string(),
            parse(json).expect("the filter value parses"),
        );
    }
    BindingSources {
        filters,
        ..BindingSources::default()
    }
}

/// The `dept` column of the rows the grid resolves to, or `None` when the
/// source did not resolve (the loading surface — a refused pipeline).
fn resolved_depts(tree: &Node, filter: Option<&str>) -> Option<Vec<String>> {
    match resolve_rows(&sources_with_filter(filter), &grid_source(tree)) {
        ResolvedRows::Rows(rows) => Some(
            rows.iter()
                .map(|r| match r.field("dept") {
                    Some(JVal::Str(s)) => s.clone(),
                    other => panic!("every row carries a string dept, got {other:?}"),
                })
                .collect(),
        ),
        ResolvedRows::NotResolved => None,
    }
}

fn oracle_tree() -> Option<Node> {
    let raw = fixture_json()?;
    Some(decode_node(&raw).expect("the fixture decodes with this host's codec"))
}

#[test]
fn a_selection_scopes_the_grid_through_substitution() {
    let Some(tree) = oracle_tree() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    assert_eq!(
        resolved_depts(&tree, Some(r#"["eng","ops"]"#)),
        Some(vec!["eng".to_string(), "ops".to_string()]),
        "a two-element selection must scope the grid to those two departments"
    );
}

#[test]
fn an_empty_selection_prunes_to_the_unfiltered_table() {
    let Some(tree) = oracle_tree() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    // The acceptance criterion Phase 610 pinned as OUTPUT: deselecting
    // everything shows the unfiltered table, NOT an empty one. `items: []`
    // would give zero rows, which is the answer this rule exists to refuse.
    assert_eq!(
        resolved_depts(&tree, Some("[]")),
        Some(vec![
            "eng".to_string(),
            "sales".to_string(),
            "ops".to_string()
        ]),
        "an EMPTY selection is unbound, so the filter step prunes and every row shows"
    );
}

#[test]
fn an_unwritten_filter_prunes_the_same_way_an_empty_selection_does() {
    let Some(tree) = oracle_tree() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    assert_eq!(
        resolved_depts(&tree, None),
        resolved_depts(&tree, Some("[]")),
        "never-written and deselected-to-empty are the same absence of a constraint"
    );
}

#[test]
fn a_scalar_bound_where_the_pipeline_reads_a_list_is_refused() {
    let Some(tree) = oracle_tree() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    // The pipeline reads `depts` through `in`/`param`; a SCALAR binds the
    // scalar env, substitutes nothing, and the surviving `InParam` reaches the
    // evaluator — which refuses it. The wrong answers this pins out are "all
    // three rows" (silently unfiltered) and "one row" (silently coerced).
    assert_eq!(
        resolved_depts(&tree, Some(r#""eng""#)),
        None,
        "a kind mismatch must refuse, never silently scope"
    );
}

#[test]
fn a_non_scalar_list_element_is_refused_rather_than_coerced() {
    let Some(tree) = oracle_tree() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    assert_eq!(
        resolved_depts(&tree, Some(r#"["eng",{"dept":"ops"}]"#)),
        None,
        "an array holding a non-scalar is the loud non-scalar-value error, not a partial selection"
    );
}

#[test]
fn a_list_bound_where_the_pipeline_reads_a_scalar_is_refused() {
    // The mismatch in the OTHER direction, which the corpus fixture cannot
    // express (its pipeline reads a list). A `filter` step comparing a column
    // to a scalar `param` is handed a list: nothing substitutes it, the name is
    // not unbound so the step is not pruned, and evaluation refuses.
    let Some(raw) = fixture_json() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    // Rewrite ONLY the membership test into the scalar `param` spelling; every
    // other byte of the oracle — the params entry, the source, the chip — is
    // the fixture's own.
    let scalar_read = raw.replace(
        r#"{"$type":"in","expr":{"$type":"col","name":"dept"},"param":"depts"}"#,
        r#"{"$type":"binary","left":{"$type":"col","name":"dept"},"op":"eq","right":{"$type":"param","name":"depts"}}"#,
    );
    assert_ne!(
        scalar_read, raw,
        "the rewrite matched nothing, so this test proves nothing about the fixture"
    );
    let tree = decode_node(&scalar_read).expect("the rewritten tree decodes");

    assert_eq!(
        resolved_depts(&tree, Some(r#"["eng"]"#)),
        None,
        "a LIST bound to a name read as a scalar param must refuse, never silently scope"
    );
    // ... and the same tree with a scalar DOES resolve, so the refusal above is
    // the mismatch and not the rewrite.
    assert_eq!(
        resolved_depts(&tree, Some(r#""eng""#)),
        Some(vec!["eng".to_string()]),
        "the rewritten pipeline is otherwise sound — a scalar scopes it"
    );
}

#[test]
fn substitution_happens_before_evaluation_not_merely_before_the_prune() {
    // The sharpest statement of behaviour 1, and the one the prune could
    // otherwise counterfeit: only a `filter` step is prunable, so a `derive`
    // reading a list param has nowhere to hide. Bound, it must EVALUATE (proof
    // the rewrite reached the evaluator); unbound, it must REFUSE (proof the
    // evaluator never grew a list lookup of its own).
    let Some(raw) = fixture_json() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let derived = raw.replace(
        r#"{"$type":"filter","pred":{"$type":"in","expr":{"$type":"col","name":"dept"},"param":"depts"}}"#,
        r#"{"$type":"derive","expr":{"$type":"in","expr":{"$type":"col","name":"dept"},"param":"depts"},"name":"picked"}"#,
    );
    assert_ne!(
        derived, raw,
        "the rewrite matched nothing, so this test proves nothing about the fixture"
    );
    let tree = decode_node(&derived).expect("the rewritten tree decodes");

    let sources = sources_with_filter(Some(r#"["eng"]"#));
    let source = grid_source(&tree);
    let bound = resolve_rows(&sources, &source);
    let ResolvedRows::Rows(rows) = bound else {
        panic!(
            "a BOUND list param in a non-prunable step must evaluate, which it can only do if the substitution ran BEFORE evaluation"
        );
    };
    assert_eq!(
        rows.iter()
            .map(|r| r.field("picked").cloned())
            .collect::<Vec<_>>(),
        vec![
            Some(JVal::Bool(true)),
            Some(JVal::Bool(false)),
            Some(JVal::Bool(false))
        ],
        "and the derived membership must be the substituted literal list's answer"
    );

    assert!(
        matches!(
            resolve_rows(&sources_with_filter(None), &grid_source(&tree)),
            ResolvedRows::NotResolved
        ),
        "an UNSUBSTITUTED list param in a non-prunable step must reach the strict refusal"
    );
}

#[test]
fn the_rendered_grid_shows_every_row_while_nothing_is_selected() {
    let Some(tree) = oracle_tree() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    // The criterion as OUTPUT rather than as an assertion about output: the
    // server-rendered markup of the unselected fixture carries all three
    // departments.
    let html = render_to_html(&tree, &BindingSources::default());
    for dept in ["eng", "sales", "ops"] {
        assert!(
            html.contains(&format!(">{dept}<")),
            "the unfiltered table must render '{dept}': {html}"
        );
    }
}

#[test]
fn the_two_target_abi_fixture_carries_the_oracle_tree_verbatim() {
    // `tests/fixtures/list-param.json` embeds the corpus fixture's bytes so the
    // `wasm32` leg needs no corpus checkout (the corpus is a sibling clone, and
    // a node script has no walk-up logic). That copy must not drift from the
    // oracle: without this, the two ABI legs could agree perfectly with each
    // other about a tree the corpus no longer contains.
    let Some(oracle) = fixture_json() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let abi = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/list-param.json"
    ))
    .expect("the ABI fixture is readable");
    let embedded = match parse(&abi).expect("the ABI fixture parses").field("tree") {
        Some(JVal::Str(s)) => s.clone(),
        other => panic!("the ABI fixture carries no string 'tree' (got {other:?})"),
    };
    assert_eq!(
        embedded,
        oracle.trim(),
        "the ABI fixture's embedded tree has drifted from the corpus oracle — re-copy \
         wire-format-fixtures/nodes/multiselect-chip-list-param.json into its 'tree' member"
    );
}

#[test]
fn membership_over_an_empty_literal_list_matches_nothing() {
    // The fact that makes `an_empty_selection_prunes_to_the_unfiltered_table`
    // DISCRIMINATING rather than vacuous, pinned here so it cannot quietly
    // change underneath that test.
    //
    // "Empty selection is UNBOUND, never `items: []`" is only a visible rule if
    // the two readings produce different output. They do here: `in` over an
    // empty item list is FALSE for every row (SQL three-valued membership with
    // nothing to match), so substituting `items: []` yields the EMPTY table
    // where the unbound reading yields the unfiltered one. A host whose `in`
    // returned true or null over an empty list would pass the empty-selection
    // test while implementing the rule backwards — same output, wrong mechanism.
    let pipeline = vec![TransformStep::Filter {
        pred: ColExpr::InList {
            subject: Box::new(ColExpr::Col {
                name: "dept".to_string(),
            }),
            items: vec![],
        },
    }];
    let Some(tree) = oracle_tree() else {
        eprintln!("wire-format-fixtures corpus not found; skipping (standalone checkout)");
        return;
    };
    let Binding::Transform { source, .. } = grid_source(&tree) else {
        panic!("the fixture's grid source is a Transform");
    };
    let fuaran_rs::wire::TransformSource::Data(data) = source else {
        panic!("the fixture's Transform carries an embedded data source");
    };
    let table = transform::eval_transform(
        &transform::no_resolve,
        &transform::EvalEnv::new(),
        &data,
        &pipeline,
    )
    .expect("an empty membership test evaluates rather than erroring");
    let rows = table.columns.first().map(|c| c.cells.len()).unwrap_or(0);
    assert_eq!(
        rows, 0,
        "`in` over an EMPTY item list must match nothing — if it matched everything, the \
         empty-selection test above would be green on a host that substitutes `items: []`"
    );
}
