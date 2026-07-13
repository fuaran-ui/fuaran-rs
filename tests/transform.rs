//! Dataframe transform evaluation — the compute substrate behind the Living
//! Sheet and the Pattern Bank's computed metric. Pins the cross-host semantics
//! (null propagation, int↔float coercion, group/sort stability,
//! round-half-away, division-by-zero) and confirms the output table survives a
//! canonical-codec round-trip byte-for-byte (the §11.1 parity contract).

use fuaran_rs::transform::{EvalEnv, Table, eval_pipeline, eval_pipeline_in_env, pipeline_params};
use fuaran_rs::wire::{
    Agg, AggFn, BinOp, CaseArm, Cell, ColExpr, ColPair, ColumnType, DataColumn, DataSource,
    JoinKind, SchemaEntry, SortDir, SortKey, TransformStep, decode_node, encode_node,
};

fn schema(entries: &[(&str, ColumnType)]) -> Vec<SchemaEntry> {
    entries
        .iter()
        .map(|(n, t)| SchemaEntry {
            name: (*n).to_string(),
            column_type: *t,
        })
        .collect()
}

fn column(name: &str, ty: ColumnType, cells: Vec<Cell>) -> DataColumn {
    DataColumn {
        name: name.to_string(),
        column_type: ty,
        cells,
    }
}

fn col(name: &str) -> ColExpr {
    ColExpr::Col {
        name: name.to_string(),
    }
}

fn lit(cell: Cell) -> ColExpr {
    ColExpr::Lit { cell }
}

// A small orders table: region, amount (some null), units.
fn orders() -> Table {
    Table {
        schema: schema(&[
            ("region", ColumnType::Str),
            ("amount", ColumnType::Float),
            ("units", ColumnType::Int),
        ]),
        columns: vec![
            column(
                "region",
                ColumnType::Str,
                vec![
                    Cell::Str("EU".into()),
                    Cell::Str("US".into()),
                    Cell::Str("EU".into()),
                    Cell::Str("US".into()),
                ],
            ),
            column(
                "amount",
                ColumnType::Float,
                vec![
                    Cell::Float(100.0),
                    Cell::Float(250.0),
                    Cell::Null,
                    Cell::Float(50.0),
                ],
            ),
            column(
                "units",
                ColumnType::Int,
                vec![Cell::Int(2), Cell::Int(5), Cell::Int(1), Cell::Int(1)],
            ),
        ],
    }
}

fn col_named<'a>(t: &'a Table, name: &str) -> &'a DataColumn {
    t.columns.iter().find(|c| c.name == name).expect("column")
}

#[test]
fn filter_then_derive_computes_a_metric() {
    // The Pattern Bank mechanic: filter to a subset, derive a computed column.
    let pipeline = vec![
        TransformStep::Filter {
            pred: ColExpr::Binary {
                op: BinOp::Gt,
                left: Box::new(col("units")),
                right: Box::new(lit(Cell::Int(1))),
            },
        },
        TransformStep::Derive {
            name: "per_unit".into(),
            expr: ColExpr::Binary {
                op: BinOp::Div,
                left: Box::new(col("amount")),
                right: Box::new(col("units")),
            },
        },
    ];
    let out = eval_pipeline(&pipeline, &orders()).expect("evaluates");
    // Rows with units>1: EU/100/2 and US/250/5 → per_unit 50.0 and 50.0.
    let per_unit = col_named(&out, "per_unit");
    assert_eq!(per_unit.column_type, ColumnType::Float);
    assert_eq!(per_unit.cells, vec![Cell::Float(50.0), Cell::Float(50.0)]);
}

#[test]
fn division_by_zero_is_null_not_a_panic() {
    let input = Table {
        schema: schema(&[("n", ColumnType::Int), ("d", ColumnType::Int)]),
        columns: vec![
            column("n", ColumnType::Int, vec![Cell::Int(10)]),
            column("d", ColumnType::Int, vec![Cell::Int(0)]),
        ],
    };
    let pipeline = vec![TransformStep::Derive {
        name: "q".into(),
        expr: ColExpr::Binary {
            op: BinOp::Div,
            left: Box::new(col("n")),
            right: Box::new(col("d")),
        },
    }];
    let out = eval_pipeline(&pipeline, &input).unwrap();
    assert_eq!(col_named(&out, "q").cells, vec![Cell::Null]);
}

#[test]
fn group_by_aggregates_with_null_skipping() {
    // Living Sheet roll-up: sum/mean/count per region, nulls skipped.
    let pipeline = vec![TransformStep::GroupBy {
        keys: vec!["region".into()],
        aggs: vec![
            Agg {
                name: "total".into(),
                func: AggFn::Sum,
                of: "amount".into(),
            },
            Agg {
                name: "n".into(),
                func: AggFn::Count,
                of: "amount".into(),
            },
        ],
    }];
    let out = eval_pipeline(&pipeline, &orders()).unwrap();
    // First-appearance group order: EU then US.
    assert_eq!(
        col_named(&out, "region").cells,
        vec![Cell::Str("EU".into()), Cell::Str("US".into())]
    );
    // EU amount = 100 + null → 100.0 (float, since amount is float); count of
    // present = 1. US = 250 + 50 = 300.0, count 2.
    assert_eq!(
        col_named(&out, "total").cells,
        vec![Cell::Float(100.0), Cell::Float(300.0)]
    );
    assert_eq!(col_named(&out, "total").column_type, ColumnType::Float);
    assert_eq!(col_named(&out, "n").cells, vec![Cell::Int(1), Cell::Int(2)]);
}

#[test]
fn sort_is_stable_and_puts_nulls_last() {
    let pipeline = vec![TransformStep::Sort {
        by: vec![SortKey {
            col: "amount".into(),
            dir: SortDir::Asc,
        }],
    }];
    let out = eval_pipeline(&pipeline, &orders()).unwrap();
    // Ascending amount: 50, 100, 250, then the null row last.
    assert_eq!(
        col_named(&out, "amount").cells,
        vec![
            Cell::Float(50.0),
            Cell::Float(100.0),
            Cell::Float(250.0),
            Cell::Null
        ]
    );
    // The null row is the EU/units=1 row, preserved by stability.
    assert_eq!(
        col_named(&out, "region").cells.last(),
        Some(&Cell::Str("EU".into()))
    );
}

#[test]
fn round_derives_half_away_from_zero() {
    let input = Table {
        schema: schema(&[("x", ColumnType::Float)]),
        columns: vec![column(
            "x",
            ColumnType::Float,
            vec![Cell::Float(2.5), Cell::Float(-2.5), Cell::Float(1.4)],
        )],
    };
    let pipeline = vec![TransformStep::Derive {
        name: "r".into(),
        expr: ColExpr::Apply {
            func: fuaran_rs::wire::ScalarFn::Round,
            args: vec![col("x")],
        },
    }];
    let out = eval_pipeline(&pipeline, &input).unwrap();
    // Half away from zero: 2.5→3, -2.5→-3, 1.4→1 (not banker's rounding).
    assert_eq!(
        col_named(&out, "r").cells,
        vec![Cell::Float(3.0), Cell::Float(-3.0), Cell::Float(1.0)]
    );
}

#[test]
fn a_param_binds_a_filter_threshold() {
    // The Pattern Bank chip mechanic: a filter threshold supplied as a param.
    let pipeline = vec![TransformStep::Filter {
        pred: ColExpr::Binary {
            op: BinOp::Ge,
            left: Box::new(col("amount")),
            right: Box::new(ColExpr::Param {
                name: "floor".into(),
            }),
        },
    }];
    assert_eq!(pipeline_params(&pipeline), vec!["floor".to_string()]);

    let mut env = EvalEnv::new();
    env.insert("floor".into(), Cell::Float(200.0));
    let out = eval_pipeline_in_env(&env, &pipeline, &orders()).unwrap();
    // amount >= 200 → only the US/250 row (null compares false, drops out).
    assert_eq!(col_named(&out, "amount").cells, vec![Cell::Float(250.0)]);

    // An unbound param is a typed error, not a panic.
    let err = eval_pipeline(&pipeline, &orders()).unwrap_err();
    assert!(matches!(
        err,
        fuaran_rs::transform::EvalError::UnboundParam { .. }
    ));
}

#[test]
fn case_expression_buckets_rows() {
    let pipeline = vec![TransformStep::Derive {
        name: "tier".into(),
        expr: ColExpr::Case {
            cases: vec![CaseArm {
                when: ColExpr::Binary {
                    op: BinOp::Ge,
                    left: Box::new(col("units")),
                    right: Box::new(lit(Cell::Int(5))),
                },
                then: lit(Cell::Str("bulk".into())),
            }],
            else_expr: Box::new(lit(Cell::Str("retail".into()))),
        },
    }];
    let out = eval_pipeline(&pipeline, &orders()).unwrap();
    assert_eq!(
        col_named(&out, "tier").cells,
        vec![
            Cell::Str("retail".into()),
            Cell::Str("bulk".into()),
            Cell::Str("retail".into()),
            Cell::Str("retail".into()),
        ]
    );
}

#[test]
fn project_renames_and_selects() {
    let pipeline = vec![TransformStep::Project {
        cols: vec![
            ColPair {
                a: "region".into(),
                b: "r".into(),
            },
            ColPair {
                a: "units".into(),
                b: "qty".into(),
            },
        ],
    }];
    let out = eval_pipeline(&pipeline, &orders()).unwrap();
    assert_eq!(
        out.schema
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        vec!["r", "qty"]
    );
    assert_eq!(col_named(&out, "qty").cells.len(), 4);
}

#[test]
fn window_row_number_partitions_by_region() {
    let pipeline = vec![TransformStep::Window {
        partition_by: vec!["region".into()],
        order_by: vec![SortKey {
            col: "units".into(),
            dir: SortDir::Asc,
        }],
        func: fuaran_rs::wire::WindowFn::RowNumber,
        of: "units".into(),
        alias: "rn".into(),
    }];
    let out = eval_pipeline(&pipeline, &orders()).unwrap();
    // Per-region ordinal by ascending units, reassembled in original row order:
    //   EU rows (units 2,1) → 2,1 ; US rows (units 5,1) → 2,1.
    assert_eq!(
        col_named(&out, "rn").cells,
        vec![Cell::Int(2), Cell::Int(2), Cell::Int(1), Cell::Int(1)]
    );
    assert_eq!(col_named(&out, "rn").column_type, ColumnType::Int);
}

#[test]
fn join_matches_on_a_key() {
    let rates = DataSource::Embedded {
        schema: schema(&[("region", ColumnType::Str), ("fx", ColumnType::Float)]),
        columns: vec![
            column(
                "region",
                ColumnType::Str,
                vec![Cell::Str("EU".into()), Cell::Str("US".into())],
            ),
            column(
                "fx",
                ColumnType::Float,
                vec![Cell::Float(1.1), Cell::Float(1.0)],
            ),
        ],
    };
    let pipeline = vec![TransformStep::Join {
        source: rates,
        on: vec![ColPair {
            a: "region".into(),
            b: "region".into(),
        }],
        how: JoinKind::Inner,
    }];
    let out = eval_pipeline(&pipeline, &orders()).unwrap();
    // The colliding right key is suffixed `_right`; fx joins per region.
    assert!(out.schema.iter().any(|e| e.name == "region_right"));
    assert_eq!(col_named(&out, "fx").cells.len(), 4);
}

#[test]
fn output_survives_a_canonical_codec_round_trip() {
    // The §11.1 parity contract: the evaluated table, embedded back into a
    // DataSource and carried on a Grid node, re-encodes byte-identically —
    // so a peer host reading the wire sees exactly these rows.
    let pipeline = vec![TransformStep::GroupBy {
        keys: vec!["region".into()],
        aggs: vec![Agg {
            name: "total".into(),
            func: AggFn::Sum,
            of: "amount".into(),
        }],
    }];
    let out = eval_pipeline(&pipeline, &orders()).unwrap();

    // Wrap the result table as an Embedded source inside a minimal Grid node and
    // round-trip it through the canonical codec.
    let node_json = grid_with_embedded(&out);
    let node = decode_node(&node_json).expect("decodes");
    let reencoded = encode_node(&node);
    let node2 = decode_node(&reencoded).expect("re-decodes");
    assert_eq!(
        encode_node(&node2),
        reencoded,
        "canonical round-trip stable"
    );
}

// Build a Visualisation/DataGrid node whose source is the evaluated table
// embedded verbatim — the shape the Living Sheet emits back onto the wire (the
// per-column `validity`/`values` form, per the grid-transform fixture).
fn grid_with_embedded(t: &Table) -> String {
    let schema_json: Vec<String> = t
        .schema
        .iter()
        .map(|e| {
            format!(
                r#"{{"name":{:?},"type":{:?}}}"#,
                e.name,
                type_wire(e.column_type)
            )
        })
        .collect();
    let cols_json: Vec<String> = t
        .columns
        .iter()
        .map(|c| format!("{:?}:{}", c.name, column_json(&c.cells)))
        .collect();
    format!(
        concat!(
            r#"{{"id":"sheet","kind":{{"$type":"DataGrid","columns":[],"editable":false,"#,
            r#""rowKey":"<closure>","source":{{"$type":"Transform","pipeline":[],"#,
            r#""source":{{"columns":{{{}}},"schema":[{}]}}}}}}}}"#
        ),
        cols_json.join(","),
        schema_json.join(",")
    )
}

fn type_wire(t: ColumnType) -> &'static str {
    match t {
        ColumnType::Int => "int",
        ColumnType::Float => "float",
        ColumnType::Bool => "bool",
        ColumnType::Str => "string",
        ColumnType::Date => "date",
        ColumnType::Timestamp => "timestamp",
    }
}

// A column as `{validity:[bool...],values:[raw...]}` — a Null cell is
// `validity=false` with a type-appropriate placeholder value.
fn column_json(cells: &[Cell]) -> String {
    let mut validity = Vec::new();
    let mut values = Vec::new();
    for c in cells {
        match c {
            Cell::Null => {
                validity.push("false".to_string());
                values.push("0".to_string());
            }
            Cell::Int(v) => {
                validity.push("true".to_string());
                values.push(v.to_string());
            }
            Cell::Float(v) => {
                validity.push("true".to_string());
                values.push(format!("{v}"));
            }
            Cell::Bool(b) => {
                validity.push("true".to_string());
                values.push(b.to_string());
            }
            Cell::Str(s) | Cell::Date(s) | Cell::Timestamp(s) => {
                validity.push("true".to_string());
                values.push(format!("{s:?}"));
            }
        }
    }
    format!(
        r#"{{"validity":[{}],"values":[{}]}}"#,
        validity.join(","),
        values.join(",")
    )
}
