//! Phase 2484: an `int` column value is a signed 32-bit integer, read by value
//! (WIRE_FORMAT §3.6.28). The corpus certifies the vectors; these pin the
//! reader around them — spellings and magnitudes the vectors do not carry.

use fuaran_rs::wire::{decode_node, encode_node};

fn grid(values: &str, schema: Option<&str>) -> String {
    let schema = schema
        .map(|ty| format!(r#","schema":[{{"name":"n","type":"{ty}"}}]"#))
        .unwrap_or_default();
    format!(
        r#"{{"id":"g","kind":{{"$type":"DataGrid","columns":[],"rowKey":"<closure>","source":{{"$type":"Transform","pipeline":[],"source":{{"columns":{{"n":{values}}}{schema}}}}}}}}}"#
    )
}

#[test]
fn an_int_column_accepts_a_whole_value_inside_int32() {
    for v in ["2147483647", "-2147483648", "2.0", "-3.0", "0", "2e0"] {
        let doc = grid(&format!("[{v}]"), Some("int"));
        assert!(decode_node(&doc).is_ok(), "{v} refused");
    }
}

#[test]
fn an_int_column_refuses_anything_else() {
    for v in ["2147483648", "-2147483649", "2.5", "1e10", "-0.5"] {
        let doc = grid(&format!("[{v}]"), Some("int"));
        let e = decode_node(&doc).expect_err(v);
        assert_eq!(e.code.to_string(), "WRONG_TYPE", "{v}");
        assert!(
            e.path.starts_with("$.kind.source.source"),
            "{v}: {}",
            e.path
        );
    }
}

#[test]
fn inference_reads_a_number_by_value() {
    for (values, ty) in [
        ("[1,2147483648]", "float"),
        ("[2.0,-2147483648]", "int"),
        ("[1,2.5]", "float"),
    ] {
        let out = encode_node(&decode_node(&grid(values, None)).expect(values));
        assert!(
            out.contains(&format!(r#""type":"{ty}""#)),
            "{values}: {out}"
        );
    }
}
