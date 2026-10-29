//! The corpus's `grid-edit/` family (WIRE_FORMAT §3.2, "Edits as column ops —
//! GridEdit"; enumerated by Phase 2500). This host decodes every vector's
//! `GridEdit` with its own codec, applies the op, derives the row delta, inverts
//! it and restores the table, and runs the three scripted histories through its
//! own edit history. Every answer is compared after decoding, through this
//! host's canonical encoders — the family pins behaviour, not file framing.

mod common;

use std::collections::BTreeMap;

use fuaran_rs::canonical::{JVal, parse, render_canonical};
use fuaran_rs::wire::grid_edit::{
    GridEdit, GridEditHistory, TableDelta, apply_column_op, decode_column_op_json,
    decode_grid_edit, decode_grid_edit_json, decode_table_json, encode_column_op, encode_grid_edit,
    encode_table, encode_table_delta, grid_edit_delta, invert_column_op,
};

fn family() -> JVal {
    let dir = common::corpus_root().join("grid-edit");
    let read = |name: &str| {
        let path = dir.join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        parse(&text).unwrap_or_else(|e| panic!("parsing {}: {}", path.display(), e.message))
    };
    let manifest = read("manifest.json");
    assert_eq!(
        render_canonical(
            manifest
                .field("files")
                .expect("the manifest names its files")
        ),
        r#"["grid-edit-vectors.json"]"#
    );
    read("grid-edit-vectors.json")
}

fn items<'a>(j: &'a JVal, key: &str) -> &'a [JVal] {
    match j.field(key) {
        Some(JVal::Arr(xs)) => xs,
        _ => panic!("{key} is not an array"),
    }
}

fn id(j: &JVal) -> String {
    match j.field("id") {
        Some(JVal::Str(s)) => s.clone(),
        _ => panic!("an entry carries no id"),
    }
}

fn table_text(j: &JVal) -> String {
    encode_table(&decode_table_json(j).expect("an embedded table decodes"))
}

fn edit_of(j: &JVal) -> GridEdit {
    decode_grid_edit_json(j).expect("the edit decodes")
}

/// What this host answers for one vector, keyed as the vector's `expected` is.
fn answer(table: &fuaran_rs::transform::Table, edit: &GridEdit) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let after = match apply_column_op(&edit.op, table) {
        Ok(t) => t,
        Err(r) => {
            out.insert("refused".to_string(), r.token.to_string());
            return out;
        }
    };
    out.insert("after".to_string(), encode_table(&after));
    match &edit.row_key_field {
        Some(field) => {
            out.insert(
                "delta".to_string(),
                encode_table_delta(&grid_edit_delta(field, table, &edit.op)),
            );
        }
        None => {
            out.insert(
                "delta".to_string(),
                encode_table_delta(&TableDelta::FullRefresh),
            );
            out.insert(
                "referencePath".to_string(),
                "noRowIdentityDeclared".to_string(),
            );
        }
    }
    match invert_column_op(&edit.op, table) {
        Ok(inverse) => {
            let undone = apply_column_op(&inverse, &after).expect("the inverse applies");
            out.insert("inverse".to_string(), encode_column_op(&inverse));
            out.insert("undone".to_string(), encode_table(&undone));
        }
        Err(_) => {
            out.insert("notInvertible".to_string(), "true".to_string());
        }
    }
    out
}

fn expected(j: &JVal) -> BTreeMap<String, String> {
    let JVal::Obj(fields) = j else {
        panic!("expected is not an object")
    };
    fields
        .iter()
        .map(|(k, v)| {
            let text = match (k.as_str(), v) {
                ("after" | "undone", _) => table_text(v),
                ("inverse", _) => {
                    encode_column_op(&decode_column_op_json(v).expect("the inverse decodes"))
                }
                ("delta", _) => render_canonical(v),
                (_, JVal::Str(s)) => s.clone(),
                (_, other) => render_canonical(other),
            };
            (k.clone(), text)
        })
        .collect()
}

#[test]
fn every_grid_edit_vector() {
    let family = family();
    let vectors = items(&family, "vectors");
    assert!(
        vectors.len() >= 15,
        "a family certified by reading nothing is no family"
    );
    for v in vectors {
        let input = v.field("input").expect("input");
        let edit_json = input.field("edit").expect("input.edit");
        let edit = edit_of(edit_json);
        assert_eq!(
            encode_grid_edit(&edit),
            render_canonical(edit_json),
            "{}: the edit's bytes",
            id(v)
        );
        let table = decode_table_json(input.field("table").expect("input.table"))
            .expect("the table decodes");
        assert_eq!(
            answer(&table, &edit),
            expected(v.field("expected").expect("expected")),
            "{}",
            id(v)
        );
    }
}

#[test]
fn every_grid_edit_history() {
    let family = family();
    let histories = items(&family, "histories");
    assert!(histories.len() >= 3, "the undo vectors are read");
    for h in histories {
        let input = h.field("input").expect("input");
        let mut history = GridEditHistory::new(
            decode_table_json(input.field("table").expect("table")).unwrap(),
            100,
        );
        let mut steps = Vec::new();
        for g in items(input, "gestures") {
            let next = if let Some(edit) = g.field("edit") {
                history.apply(&edit_of(edit).op)
            } else if g.field("undo").is_some() {
                history.undo()
            } else {
                history.redo()
            };
            match next {
                Ok(n) => {
                    history = n;
                    steps.push(format!(
                        "canRedo={} canUndo={} table={}",
                        history.can_redo(),
                        history.can_undo(),
                        encode_table(history.table())
                    ));
                }
                Err(r) => steps.push(format!("refused={}", r.token)),
            }
        }
        let want: Vec<String> = items(h.field("expected").expect("expected"), "steps")
            .iter()
            .map(|s| match s.field("refused") {
                Some(JVal::Str(token)) => format!("refused={token}"),
                _ => format!(
                    "canRedo={} canUndo={} table={}",
                    render_canonical(s.field("canRedo").unwrap()),
                    render_canonical(s.field("canUndo").unwrap()),
                    table_text(s.field("table").unwrap())
                ),
            })
            .collect();
        assert_eq!(steps, want, "history {}", id(h));
    }
}

#[test]
fn the_codec_refuses_what_it_could_not_apply_faithfully() {
    for text in [
        r#"{"$type":"GridEdit","key":"k","op":{"$type":"removeColumn","name":"x"},"extra":1}"#,
        r#"{"$type":"GridEdit","key":"","op":{"$type":"removeColumn","name":"x"}}"#,
        r#"{"$type":"GridEdit","key":"k","op":{"$type":"removeColumn","name":"x"},"rowKeyField":""}"#,
        r#"{"$type":"GridEdit","key":"k","op":{"$type":"removeColumn","name":"x"},"rowKeyField":3}"#,
        r#"{"$type":"GridEdit","key":"k"}"#,
        r#"{"$type":"GridEdit","key":"k","op":{"$type":"dropTable"}}"#,
        r#"{"$type":"Edit","key":"k","op":{"$type":"removeColumn","name":"x"}}"#,
        "[1]",
        "{",
    ] {
        assert!(decode_grid_edit(text).is_err(), "decoded {text}");
    }
    let text = r#"{"$type":"GridEdit","key":"orders","op":{"$type":"removeColumn","name":"note"}}"#;
    assert_eq!(encode_grid_edit(&decode_grid_edit(text).unwrap()), text);
}
