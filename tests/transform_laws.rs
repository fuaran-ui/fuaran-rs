//! `laws/transformLaws` — this host's evaluator against the reference's drawn
//! answers (Phase 2001).
//!
//! The family is read THROUGH `laws/manifest.json`: the file must agree with its
//! manifest row (family id, kit version, seed, iterations, vector count), and no
//! count is written here. Every `evalPipeline` vector runs: both inputs decode
//! with this host's codec (carried, as the `dataframeParity` leg carries them, on
//! a DataGrid whose source is a `Transform` binding) and round-trip byte for
//! byte; an `ok` verdict requires the evaluated table to encode byte for byte to
//! `expected.table`, and an `error` verdict requires this host to refuse — the
//! error class is never compared, because the file carries none.
//!
//! The `-unit-` vectors run like every other (Phase 2208): the evaluator carries
//! field metadata through every verb and checks units, so a refusal among them
//! is the unit check's and an answer carries its schema's units and labels.
//!
//! The `columnOp` vectors run (Phase 2500 gave this host the surface): the op
//! decodes through `decode_column_op_json`, re-encodes byte for byte to
//! `expected.op`, and applies through `apply_column_op`, compared as an
//! `evalPipeline` vector is.
//!
//! What is NOT run is reported by name and counted, never silent:
//! - the `delta` case kind: a vector carries two tables and asks for their diff,
//!   and this host's `grid_edit_delta` derives a row delta from an op over the
//!   before table. The set of case kinds in the file must be exactly the three
//!   this leg knows, so a new kind fails loudly.
//!
//! A vector carrying `expected.tolerance` (Phase 2400) names the columns the
//! reference marks host-rounded, each with a bound in ulps. Those columns are
//! compared cell by cell — one type, one length, null in the same rows, two
//! present nonzero finite floats of one sign within the bound (the distance being
//! the difference of their IEEE-754 binary64 bit patterns), every other cell (a
//! zero of either sign, a non-finite float, a non-float) exactly — and the rest
//! of the table byte for byte: this host's answer with every named column
//! replaced by the recorded one must encode to `expected.table` exactly. The
//! bound is the vector's own, never a constant here; the worst distance per
//! named column is reported, so a host drifting toward the bound is visible.

mod common;

use std::collections::BTreeSet;

use fuaran_rs::canonical::{JVal, parse, render_canonical};
use fuaran_rs::transform::{Table, eval_pipeline};
use fuaran_rs::wire::grid_edit::{
    apply_column_op, decode_column_op_json, decode_table_json, encode_column_op,
    encode_table as encode_embedded_table,
};
use fuaran_rs::wire::{
    Binding, Cell, DataColumn, DataSource, NodeKind, TransformSource, decode_node, encode_node,
};

fn str_field(v: &JVal, key: &str) -> Option<String> {
    match v.field(key) {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn num_field(v: &JVal, key: &str) -> Option<f64> {
    match v.field(key) {
        Some(JVal::Num(n)) => Some(*n),
        _ => None,
    }
}

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn grid_json(source: &str, pipeline: &str) -> String {
    format!(
        r#"{{"id":"laws","kind":{{"$type":"DataGrid","columns":[],"editable":false,"rowKey":"<closure>","source":{{"$type":"Transform","pipeline":{pipeline},"source":{source}}}}}}}"#
    )
}

/// The canonical bytes at `kind.source.<member>` of an encoded carrier grid.
fn grid_member(encoded: &str, member: &str) -> String {
    let doc = parse(encoded).expect("the encoded grid parses");
    let value = doc
        .field("kind")
        .and_then(|k| k.field("source"))
        .and_then(|s| s.field(member))
        .unwrap_or_else(|| panic!("the encoded grid carries no kind.source.{member}"));
    render_canonical(value)
}

/// The table the carrier grid's embedded source holds, re-encoded canonically.
fn encode_table(carrier: &fuaran_rs::wire::Node, out: Table) -> String {
    let mut carrier = carrier.clone();
    if let NodeKind::DataGrid(spec) = &mut carrier.kind {
        spec.source = Binding::Transform {
            params: None,
            pipeline: vec![],
            source: TransformSource::Data(DataSource::Embedded {
                schema: out.schema,
                columns: out.columns,
            }),
        };
    }
    grid_member(&encode_node(&carrier), "source")
}

/// One `columnOp` vector: `Ok`, or the first disagreement with the reference.
fn run_column_op_vector(input: &JVal, expected: &JVal) -> Result<(), String> {
    let text = str_field(input, "op").ok_or("the vector carries no input.op")?;
    let doc = parse(&text).map_err(|e| format!("input.op does not parse: {}", e.message))?;
    let op = decode_column_op_json(&doc).map_err(|e| format!("input.op did not decode: {e}"))?;
    let encoded = encode_column_op(&op);
    let want_op = str_field(expected, "op").ok_or("the vector carries no expected.op")?;
    if encoded != want_op {
        return Err(format!("op round-trip\n  got  {encoded}\n  want {want_op}"));
    }
    let source = str_field(input, "source").ok_or("the vector carries no input.source")?;
    let doc = parse(&source).map_err(|e| format!("input.source does not parse: {}", e.message))?;
    let table = decode_table_json(&doc).map_err(|e| format!("input.source: {e}"))?;
    let verdict = str_field(expected, "verdict").ok_or("the vector carries no verdict")?;
    match (apply_column_op(&op, &table), verdict.as_str()) {
        (Err(_), "error") => Ok(()),
        (Ok(_), "error") => Err("the reference refused; this host applied".to_string()),
        (Err(e), _) => Err(format!("the reference applied; this host refused: {e:?}")),
        (Ok(out), _) => {
            let got = encode_embedded_table(&out);
            let want =
                str_field(expected, "table").ok_or("the vector carries no expected.table")?;
            if got == want {
                Ok(())
            } else {
                Err(format!("applied table\n  got  {got}\n  want {want}"))
            }
        }
    }
}

/// The distance between two doubles of one sign: their bit patterns' difference.
fn ulp_distance(a: f64, b: f64) -> u64 {
    a.to_bits().abs_diff(b.to_bits())
}

/// The family's tolerance rule (see the module header): `Ok` with the worst
/// distance per named column, or the first disagreement.
fn within_tolerance(
    tolerance: &[(String, u64)],
    expected_table: &str,
    out: &mut Table,
) -> Result<Vec<(String, u64)>, String> {
    let node = decode_node(&grid_json(expected_table, "[]"))
        .map_err(|e| format!("expected.table does not decode: {e:?}"))?;
    let NodeKind::DataGrid(spec) = &node.kind else {
        return Err("expected.table decoded to another kind".to_string());
    };
    let Binding::Transform {
        source: TransformSource::Data(DataSource::Embedded { columns: want, .. }),
        ..
    } = &spec.source
    else {
        return Err("expected.table is not an embedded table".to_string());
    };
    let mut worst = Vec::new();
    for (name, bound) in tolerance {
        let w: &DataColumn = want
            .iter()
            .find(|c| &c.name == name)
            .ok_or_else(|| format!("tolerance column {name} is absent from expected.table"))?;
        let g = out
            .columns
            .iter_mut()
            .find(|c| &c.name == name)
            .ok_or_else(|| format!("tolerance column {name} is absent from this host's answer"))?;
        if g.column_type != w.column_type || g.cells.len() != w.cells.len() {
            return Err(format!("tolerance column {name} differs in type or length"));
        }
        let mut column_worst = 0;
        for (r, (wc, gc)) in w.cells.iter().zip(&g.cells).enumerate() {
            match (wc, gc) {
                (Cell::Float(a), Cell::Float(b))
                    if *a != 0.0
                        && *b != 0.0
                        && a.is_finite()
                        && b.is_finite()
                        && a.is_sign_negative() == b.is_sign_negative() =>
                {
                    let d = ulp_distance(*a, *b);
                    column_worst = column_worst.max(d);
                    if d > *bound {
                        return Err(format!(
                            "tolerance column {name} row {r}: {b} is {d} ulp from the recorded {a}; the bound is {bound}"
                        ));
                    }
                }
                (Cell::Float(a), Cell::Float(b))
                    if a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()) => {}
                (Cell::Float(_), Cell::Float(_)) => {
                    return Err(format!(
                        "tolerance column {name} row {r}: {gc:?} differs from the recorded {wc:?} (compared exactly)"
                    ));
                }
                (a, b) if a == b => {}
                (a, b) => {
                    return Err(format!(
                        "tolerance column {name} row {r}: {b:?} differs from the recorded {a:?}"
                    ));
                }
            }
        }
        g.cells.clone_from(&w.cells);
        worst.push((name.clone(), column_worst));
    }
    Ok(worst)
}

#[test]
fn the_evaluator_matches_the_reference_on_every_transform_law_vector() {
    let laws = common::corpus_root().join("laws");
    let manifest = parse(&read(&laws.join("manifest.json"))).expect("laws/manifest.json parses");
    let Some(JVal::Arr(families)) = manifest.field("families") else {
        panic!("laws/manifest.json declares no families");
    };
    let row = families
        .iter()
        .find(|f| str_field(f, "id").as_deref() == Some("transformLaws"))
        .expect("laws/manifest.json declares the transformLaws family");
    let file = str_field(row, "file").expect("the transformLaws row names its file");
    let doc = parse(&read(&laws.join(&file))).expect("the transform-law vectors parse");

    // The file agrees with its manifest row.
    assert_eq!(
        str_field(&doc, "family").as_deref(),
        Some("transformLaws"),
        "laws/{file} names another family"
    );
    assert_eq!(
        str_field(&doc, "kitVersion"),
        str_field(row, "kitVersion"),
        "laws/{file} and its manifest row disagree on kitVersion"
    );
    for key in ["seed", "iterations"] {
        assert_eq!(
            num_field(&doc, key),
            num_field(row, key),
            "laws/{file} and its manifest row disagree on {key}"
        );
    }
    let Some(JVal::Arr(vectors)) = doc.field("vectors") else {
        panic!("laws/{file} carries no vectors");
    };
    assert_eq!(
        Some(vectors.len() as f64),
        num_field(row, "vectors"),
        "laws/{file} carries a vector count its manifest row does not declare"
    );

    // The case kinds are exactly the three this leg knows.
    let kinds: BTreeSet<String> = vectors
        .iter()
        .map(|v| str_field(v, "case").expect("vector case"))
        .collect();
    let known: BTreeSet<String> = ["evalPipeline", "columnOp", "delta"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        kinds, known,
        "laws/{file} carries a case kind this leg does not know"
    );
    let eval_count = vectors
        .iter()
        .filter(|v| str_field(v, "case").as_deref() == Some("evalPipeline"))
        .count();
    assert_eq!(
        Some(eval_count as f64),
        num_field(&doc, "iterations"),
        "`iterations` counts the evalPipeline vectors"
    );

    let mut failures: Vec<String> = vec![];
    let (mut ran, mut column_ops, mut units) = (0usize, 0usize, 0usize);
    let mut not_run_kinds: Vec<(String, String)> = vec![];
    for v in vectors {
        let id = str_field(v, "id").expect("vector id");
        let case = str_field(v, "case").expect("vector case");
        let input = v.field("input").expect("vector input");
        let expected = v.field("expected").expect("vector expected");
        match case.as_str() {
            "columnOp" => {
                column_ops += 1;
                if let Err(why) = run_column_op_vector(input, expected) {
                    failures.push(format!("{id}: {why}"));
                }
                continue;
            }
            "delta" => {
                not_run_kinds.push((
                    id,
                    "this host derives a row delta from an op over the before table \
                     (grid_edit_delta); it has no surface that diffs two tables"
                        .to_string(),
                ));
                continue;
            }
            _ => {}
        }
        let source = str_field(input, "source").expect("input.source");
        let pipeline = str_field(input, "pipeline").expect("input.pipeline");
        if id.contains("-unit-") {
            units += 1;
        }
        ran += 1;
        let verdict = str_field(expected, "verdict").expect("expected.verdict");
        let node = match decode_node(&grid_json(&source, &pipeline)) {
            Ok(n) => n,
            Err(e) => {
                if verdict != "error" {
                    failures.push(format!("{id}: the carrier grid did not decode: {e:?}"));
                }
                continue;
            }
        };
        let encoded = encode_node(&node);
        for (member, want) in [("source", &source), ("pipeline", &pipeline)] {
            let got = grid_member(&encoded, member);
            if &got != want {
                failures.push(format!(
                    "{id}: {member} round-trip\n  got  {got}\n  want {want}"
                ));
            }
        }
        let NodeKind::DataGrid(spec) = &node.kind else {
            panic!("{id}: the carrier grid decoded to another kind");
        };
        let Binding::Transform {
            pipeline: steps,
            source: TransformSource::Data(DataSource::Embedded { schema, columns }),
            ..
        } = &spec.source
        else {
            panic!("{id}: the carrier grid's source is not an embedded Transform");
        };
        let table = Table {
            schema: schema.clone(),
            columns: columns.clone(),
        };
        match (eval_pipeline(steps, &table), verdict.as_str()) {
            (Err(e), "ok") => failures.push(format!(
                "{id}: the reference evaluated; this host refused: {e:?}"
            )),
            (Ok(_), "error") => {
                failures.push(format!("{id}: the reference refused; this host evaluated"))
            }
            (Err(_), _) => {}
            (Ok(mut out), _) => {
                let want = str_field(expected, "table").expect("expected.table");
                let tolerance: Vec<(String, u64)> = match expected.field("tolerance") {
                    Some(JVal::Obj(members)) => members
                        .iter()
                        .map(|(k, v)| match v {
                            JVal::Num(n) => (k.clone(), *n as u64),
                            other => panic!("{id}: tolerance {k} is not a number: {other:?}"),
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                if !tolerance.is_empty() {
                    match within_tolerance(&tolerance, &want, &mut out) {
                        Ok(worst) => {
                            for (name, d) in worst {
                                eprintln!(
                                    "transformLaws tolerance: {id} column {name} worst {d} ulp"
                                );
                            }
                        }
                        Err(why) => {
                            failures.push(format!("{id}: {why}"));
                            continue;
                        }
                    }
                }
                let got = encode_table(&node, out);
                if got != want {
                    failures.push(format!(
                        "{id}: evaluator ≠ reference\n  got  {got}\n  want {want}"
                    ));
                }
            }
        }
    }

    assert!(
        units > 0,
        "the family carries no `-unit-` vector; the unit leg is stale"
    );
    for (id, why) in &not_run_kinds {
        eprintln!("transformLaws: NOT RUN {id} — {why}");
    }
    assert!(
        failures.is_empty(),
        "{} of {} transformLaws vectors failed:\n{}",
        failures.len(),
        ran + column_ops,
        failures.join("\n")
    );
    assert!(column_ops > 0, "the family carries no columnOp vector");
    assert_eq!(
        ran + column_ops + not_run_kinds.len(),
        vectors.len(),
        "every vector is run or named as not run"
    );
    eprintln!(
        "transformLaws leg: {ran} evalPipeline vectors ({units} of them unit vectors) and \
         {column_ops} columnOp vectors agree with the reference; {} delta vectors not run",
        not_run_kinds.len()
    );
}

// ─── Phase 2208: the whole entry rides each verb class, and the refusals are
// the unit check ───────────────────────────────────────────────────────────────

mod units {
    use fuaran_rs::transform::{EvalError, Table, eval_pipeline};
    use fuaran_rs::wire::unit::{self, UnitOfMeasure};
    use fuaran_rs::wire::{
        Agg, AggFn, BinOp, Cell, ColExpr, ColPair, ColumnType, DataColumn, DataSource, JoinKind,
        SchemaEntry, SortDir, SortKey, TransformStep, WindowFn,
    };

    fn u(text: &str) -> UnitOfMeasure {
        unit::parse(text).expect("a unit")
    }

    fn col(name: &str) -> ColExpr {
        ColExpr::Col { name: name.into() }
    }

    fn entry(name: &str, ty: ColumnType, unit: Option<&str>, label: Option<&str>) -> SchemaEntry {
        let mut e = SchemaEntry::new(name, ty);
        e.unit = unit.map(self::u);
        e.label = label.map(String::from);
        e
    }

    /// d metres (labelled), t seconds, dk kilometres, n unitless and labelled,
    /// described, with an extension key.
    fn table(with_units: bool) -> Table {
        let unit_of = |text: &'static str| if with_units { Some(text) } else { None };
        let mut n = entry("n", ColumnType::Int, None, Some("Count"));
        n.description = Some("How many".into());
        n.ext.insert("acme.k".into(), "v".into());
        let column = |name: &str, ty: ColumnType, cells: Vec<Cell>| DataColumn {
            name: name.into(),
            column_type: ty,
            cells,
        };
        Table {
            schema: vec![
                entry("g", ColumnType::Str, None, None),
                entry("d", ColumnType::Int, unit_of("m"), Some("Distance")),
                entry("t", ColumnType::Float, unit_of("s"), None),
                entry("dk", ColumnType::Float, unit_of("km"), None),
                n,
            ],
            columns: vec![
                column(
                    "g",
                    ColumnType::Str,
                    vec![Cell::Str("a".into()), Cell::Str("b".into())],
                ),
                column("d", ColumnType::Int, vec![Cell::Int(100), Cell::Int(200)]),
                column(
                    "t",
                    ColumnType::Float,
                    vec![Cell::Float(4.0), Cell::Float(8.0)],
                ),
                column(
                    "dk",
                    ColumnType::Float,
                    vec![Cell::Float(0.5), Cell::Float(1.0)],
                ),
                column("n", ColumnType::Int, vec![Cell::Int(1), Cell::Int(2)]),
            ],
        }
    }

    fn out_entry(pipeline: Vec<TransformStep>, name: &str) -> SchemaEntry {
        let out = eval_pipeline(&pipeline, &table(true)).expect("evaluates");
        out.schema
            .into_iter()
            .find(|e| e.name == name)
            .expect("the output column")
    }

    #[test]
    fn the_conversion_factor_is_exact_and_in_lowest_terms() {
        let r = unit::conversion_factor(&u("km/h"), &u("m/s")).expect("compatible");
        assert_eq!(r.numerator().to_u128(), Some(5));
        assert_eq!(r.denominator().to_u128(), Some(18));
        let mi = unit::conversion_factor(&u("[mi_i]"), &u("km")).expect("compatible");
        assert_eq!(
            (mi.numerator().to_u128(), mi.denominator().to_u128()),
            (Some(25146), Some(15625))
        );
        assert_eq!(
            unit::conversion_factor(&u("km"), &u("m"))
                .expect("compatible")
                .to_f64(),
            1000.0
        );
        assert_eq!(
            unit::conversion_factor(&u("m"), &u("km"))
                .expect("compatible")
                .to_f64(),
            0.001
        );
        assert!(unit::conversion_factor(&u("m"), &u("s")).is_none());
    }

    #[test]
    fn a_big_side_rounds_to_the_nearest_double() {
        // Ym.m to um.m is 10^30: a side past 2^64, multiplied out and rounded once.
        let r = unit::conversion_factor(&u("Ym.m"), &u("um.m")).expect("compatible");
        assert_eq!(r.to_f64(), 1e30);
        // 201168^5 / 125^5: the numerator passes 2^64, so it is multiplied out and
        // rounded to its nearest double (3.294537771487337e26) before the divide,
        // as the reference's `double num / double den` reads it.
        let r = unit::conversion_factor(&u("[mi_i]5"), &u("m5")).expect("compatible");
        assert_eq!(r.numerator().to_f64(), 3.294537771487337e26);
        assert_eq!(r.to_f64(), 1.0795541369609706e16);
    }

    #[test]
    fn kept_a_projection_and_a_join_keep_the_whole_entry() {
        let p = out_entry(
            vec![TransformStep::Project {
                cols: vec![ColPair {
                    a: "n".into(),
                    b: "count".into(),
                }],
            }],
            "count",
        );
        assert_eq!(p.label.as_deref(), Some("Count"));
        assert_eq!(p.description.as_deref(), Some("How many"));
        assert_eq!(p.ext.get("acme.k").map(String::as_str), Some("v"));
        let t = table(true);
        let j = out_entry(
            vec![TransformStep::Join {
                source: DataSource::Embedded {
                    schema: t.schema,
                    columns: t.columns,
                },
                on: vec![ColPair {
                    a: "g".into(),
                    b: "g".into(),
                }],
                how: JoinKind::Inner,
            }],
            "d_right",
        );
        assert_eq!(
            (j.unit, j.label.as_deref()),
            (Some(u("m")), Some("Distance"))
        );
    }

    #[test]
    fn rebuilt_a_derive_an_aggregate_and_a_window_carry_the_derived_unit_and_no_label() {
        let d = out_entry(
            vec![TransformStep::Derive {
                name: "v".into(),
                expr: col("d"),
            }],
            "v",
        );
        assert_eq!((d.unit, d.label), (Some(u("m")), None));
        let n = out_entry(
            vec![TransformStep::Derive {
                name: "n2".into(),
                expr: col("n"),
            }],
            "n2",
        );
        assert!(!n.has_metadata());
        let agg = |func| {
            vec![TransformStep::GroupBy {
                keys: vec!["g".into()],
                aggs: vec![Agg {
                    name: "a".into(),
                    func,
                    of: "d".into(),
                }],
            }]
        };
        assert!(!out_entry(agg(AggFn::Count), "a").has_metadata());
        let s = out_entry(agg(AggFn::Sum), "a");
        assert_eq!((s.unit, s.label), (Some(u("m")), None));
        let w = out_entry(
            vec![TransformStep::Window {
                partition_by: vec![],
                order_by: vec![SortKey {
                    col: "n".into(),
                    dir: SortDir::Asc,
                }],
                func: WindowFn::Lag,
                of: "d".into(),
                alias: "p".into(),
            }],
            "p",
        );
        assert_eq!((w.unit, w.label), (Some(u("m")), None));
    }

    #[test]
    fn go_red_each_refusal_is_the_unit_check() {
        let add = |l: &str, r: &str| ColExpr::Binary {
            op: BinOp::Add,
            left: Box::new(col(l)),
            right: Box::new(col(r)),
        };
        let steps = [
            TransformStep::Derive {
                name: "x".into(),
                expr: add("d", "t"),
            },
            TransformStep::Derive {
                name: "x".into(),
                expr: add("dk", "d"),
            },
            TransformStep::Filter {
                pred: ColExpr::Binary {
                    op: BinOp::Gt,
                    left: Box::new(col("d")),
                    right: Box::new(col("t")),
                },
            },
            TransformStep::Derive {
                name: "x".into(),
                expr: ColExpr::Convert {
                    expr: Box::new(col("d")),
                    unit: u("s"),
                },
            },
        ];
        for step in steps {
            let refused = eval_pipeline(std::slice::from_ref(&step), &table(true));
            assert!(
                matches!(refused, Err(EvalError::UnitMismatch { .. })),
                "{step:?}: want a UnitMismatch, got {refused:?}"
            );
            assert!(eval_pipeline(&[step], &table(false)).is_ok());
        }
    }
}
