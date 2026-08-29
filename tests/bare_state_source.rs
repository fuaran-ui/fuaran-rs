//! `WIRE_FORMAT.md` §16 — a bare `{"$type":"State","key":k}` in a `Transform`'s
//! `source` slot is a LIVE source over the EMPTY initial snapshot.
//!
//! This host refused it until now, surfacing the columnar codec's missing-field
//! didactic. That was correct while nothing else could fill the slot; under
//! §24.4 a SIBLING reader's declaration fills it, so the refusal was rejecting
//! the most direct spelling of "I read this key and carry no data of my own" —
//! the one `FUARAN106`'s remedy text tells an author to write.
//!
//! No corpus fixture spells the bare form yet: the corpus is a shared gate and
//! keeps the `"defaultValue":[]` spelling deliberately, so respelling it there
//! would redden a host that has not adopted this. The pin therefore lives here.

use fuaran_rs::wire::{decode_node, encode_node};

const BARE: &str = r#"{"$type":"State","key":"members"}"#;
const EMPTY_ARRAY: &str = r#"{"$type":"State","defaultValue":[],"key":"members"}"#;

/// A `Badge` whose label is a `Transform` over the given canonical source, in
/// the member order the encoder emits.
fn badge_with_source(source: &str) -> String {
    format!(
        concat!(
            r#"{{"id":"member-count","kind":{{"$type":"Badge","label":{{"$type":"Bound","binding":"#,
            r#"{{"$type":"Transform","pipeline":[{{"$type":"groupBy","aggs":[{{"fn":"count","name":"n","of":"team"}}],"keys":[]}}],"#,
            r#""source":{source}}}}},"variant":"Info"}}}}"#
        ),
        source = source
    )
}

/// The acceptance pin. Both spellings decode, and — because the binding is
/// PRESERVED rather than normalised — each re-encodes to the bytes it arrived
/// as. The round-trip is what proves the bare form's decoded binding kept its
/// `defaultValue` ABSENT rather than gaining the Rows slot's empty-list
/// placeholder, which would silently respell a source that declares NOTHING as
/// a declaration of the empty table.
#[test]
fn a_data_less_state_source_decodes_and_re_encodes_verbatim() {
    for source in [BARE, EMPTY_ARRAY] {
        let doc = badge_with_source(source);
        let node = decode_node(&doc)
            .unwrap_or_else(|e| panic!("decode refused a §16 live source ({source}): {e:?}"));
        assert_eq!(
            encode_node(&node),
            doc,
            "re-encode is not byte-identical for {source}"
        );
    }
}

/// The go-red half. An assertion that only ever passes cannot tell a decoder
/// that ACCEPTS the bare wrapper from one that stopped reading the slot at all,
/// so the same slot is handed shapes §16 does not sanction. Each must still
/// refuse, at a `$`-rooted path.
#[test]
fn the_widening_is_scoped_to_the_bare_state_wrapper() {
    for source in [
        // A non-binding object with neither `columns` nor `ref`: still the
        // missing-field didactic.
        r#"{"schema":[]}"#,
        // A `Static` envelope carrying no payload is NOT what §16 widened — the
        // widening names the State wrapper, whose `key` IS the live slot. A
        // `Static` with nothing in it names nothing at all.
        r#"{"$type":"Static"}"#,
        // Carried data that is not row objects still fails its snapshot decode.
        r#"{"$type":"State","defaultValue":[1,2],"key":"members"}"#,
    ] {
        let err = decode_node(&badge_with_source(source))
            .expect_err("the source was accepted — the widening is not scoped to the bare wrapper");
        assert!(
            err.path.starts_with('$'),
            "refusal path {:?} is not $-rooted",
            err.path
        );
    }
}
