//! Phase 656 — the Rosetta demo's encode-only example entry
//! (`fuaran_rs::ffi::rosetta::encode_from_holes`) must build its own exemplar
//! tree from the six scalar holes and emit the pinned canonical reference bytes.
//!
//! The reference literal below IS the canonical wire the whole Rosetta parity
//! strip pins for the default holes (the same bytes `fuaran-live`'s
//! `test/rosettaParity.test.ts` and every other host reproduce). A failure here
//! means this host would show "diverges" on the live page — exactly the
//! regression the parity lock exists to catch. Keep it in lockstep with the
//! reference pin in the other hosts when a canonical-format rev lands.

use fuaran_rs::ffi::rosetta::encode_from_holes;

const DEFAULT_HOLES: &str = r#"{"labelA":"Signups","valueA":1280,"labelB":"Revenue","valueB":42.5,"labelC":"Churn %","valueC":12.4}"#;

const REFERENCE_WIRE: &str = concat!(
    r#"{"id":"rosetta-root","kind":{"$type":"Box","children":[{"id":"rosetta-strip","#,
    r#""kind":{"$type":"Box","children":[{"id":"rosetta-m-a","kind":{"$type":"Metric","#,
    r#""label":"Signups","value":{"$type":"Static","value":1280}}},{"id":"rosetta-m-b","#,
    r#""kind":{"$type":"Metric","label":"Revenue","value":{"$type":"Static","value":42.5}}},"#,
    r#"{"id":"rosetta-m-c","kind":{"$type":"Metric","label":"Churn %","value":"#,
    r#"{"$type":"Static","value":12.4}}}],"layout":{"$type":"Flex","direction":"Horizontal","#,
    r#""wrap":true},"role":"Group"}}],"heading":"Revenue snapshot","layout":"#,
    r#"{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Dashboard"}}"#
);

#[test]
fn encodes_the_reference_bytes_for_the_default_holes() {
    let wire = encode_from_holes(DEFAULT_HOLES).expect("default holes must encode");
    assert_eq!(wire, REFERENCE_WIRE);
}

#[test]
fn reflects_edited_holes() {
    // An edit-point change must re-derive: a new label and value flow through.
    let edited = r#"{"labelA":"Active users","valueA":9001,"labelB":"Revenue","valueB":42.5,"labelC":"Churn %","valueC":12.4}"#;
    let wire = encode_from_holes(edited).expect("edited holes must encode");
    assert!(wire.contains(r#""label":"Active users""#));
    assert!(wire.contains(r#""value":{"$type":"Static","value":9001}"#));
}

#[test]
fn malformed_holes_return_none() {
    assert!(encode_from_holes("not json").is_none());
    assert!(encode_from_holes(r#"{"labelA":"only one"}"#).is_none());
}
