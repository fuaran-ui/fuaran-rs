//! An additive, encode-only example entry for the public Rosetta parity demo
//! (Phase 656). It receives the six scalar "holes" as a small JSON object —
//! exactly as the TypeScript and Python parity hosts do — then independently
//! builds the exemplar tree from them using **this crate's own typed model** and
//! runs the corpus-certified canonical encoder ([`encode_node`]) over it.
//!
//! **Why this shape keeps the "independent computation" claim honest.** Only the
//! six scalars cross the boundary; no page-built tree does. `fuaran-rs`
//! constructs its own `Node` tree and emits its own canonical bytes through the
//! same encoder every conformance fixture certifies — the exact twin of
//! `encodeWireTs(holes)` and the Python host's `rosetta_encode(holes_json)`. A
//! drift in this host's number form, key sort, or omit-when-default rule would
//! therefore surface as a divergent hash on the live parity strip, which is the
//! whole point of the demo.
//!
//! This is a demo-facing *example* over the public model, kept out of the general
//! codec; the session surface and the codec are unchanged by it.

use crate::canonical::{self, JVal};
use crate::wire::{
    Binding, BoxLayout, BoxRole, BoxSpec, CellFormat, Emphasis, MetricSpec, Node, NodeKind,
    Orientation, SemanticStyle, StateBehaviour, StaticValue, StyleWeight, TextSource, ToneVariant,
    encode_node,
};

/// The six typed holes that parameterise the exemplar — the only data that
/// crosses the boundary, mirroring the TS `Holes` interface and the Python host.
struct Holes {
    label_a: String,
    value_a: f64,
    label_b: String,
    value_b: f64,
    label_c: String,
    value_c: f64,
}

/// An empty (all-default) [`StateBehaviour`] — the common case the wire omits.
fn empty_state() -> StateBehaviour {
    StateBehaviour {
        on_loading: None,
        on_empty: None,
        on_error: None,
    }
}

/// One metric node, default-styled so it round-trips minimal: only `label` and
/// `value` are non-default, every stylistic field is the omitted-when-default
/// value (Phase 460), so the encoder emits the bare `{"$type":"Metric",…}` form.
fn metric(id: &str, label: &str, value: f64) -> Node {
    Node {
        id: id.to_string(),
        kind: NodeKind::Metric(MetricSpec {
            label: TextSource::Literal(label.to_string()),
            value: Binding::Static {
                value: StaticValue::Ast(JVal::Num(value)),
            },
            format: CellFormat::None,
            tone: ToneVariant::Default,
            weight: StyleWeight::Standard,
            emphasis: Emphasis::Normal,
            trend: None,
            trend_format: None,
            icon: None,
            subtext: None,
        }),
        state: empty_state(),
        style: SemanticStyle::default(),
        accessibility: None,
    }
}

/// A flex `Box` with the given axis / role / heading / children.
fn flex_box(
    id: &str,
    direction: Orientation,
    wrap: bool,
    role: BoxRole,
    heading: Option<&str>,
    children: Vec<Node>,
) -> Node {
    Node {
        id: id.to_string(),
        kind: NodeKind::Box(BoxSpec {
            children,
            heading: heading.map(|h| TextSource::Literal(h.to_string())),
            layout: BoxLayout::Flex {
                direction,
                gap: None,
                wrap,
            },
            role,
        }),
        state: empty_state(),
        style: SemanticStyle::default(),
        accessibility: None,
    }
}

/// The exemplar: a dashboard `Box` (heading + a horizontal three-metric strip),
/// the single signature-bearing tree every Rosetta host reproduces.
fn exemplar(h: &Holes) -> Node {
    let strip = flex_box(
        "rosetta-strip",
        Orientation::Horizontal,
        true,
        BoxRole::Group,
        None,
        vec![
            metric("rosetta-m-a", &h.label_a, h.value_a),
            metric("rosetta-m-b", &h.label_b, h.value_b),
            metric("rosetta-m-c", &h.label_c, h.value_c),
        ],
    );
    flex_box(
        "rosetta-root",
        Orientation::Vertical,
        false,
        BoxRole::Dashboard,
        Some("Revenue snapshot"),
        vec![strip],
    )
}

/// Read a required string hole from the parsed holes object.
fn str_hole(holes: &JVal, key: &str) -> Option<String> {
    match holes.field(key) {
        Some(JVal::Str(v)) => Some(v.clone()),
        _ => None,
    }
}

/// Read a required numeric hole from the parsed holes object.
fn num_hole(holes: &JVal, key: &str) -> Option<f64> {
    match holes.field(key) {
        Some(JVal::Num(v)) => Some(*v),
        _ => None,
    }
}

/// Build the exemplar tree from the six holes (a small JSON object
/// `{"labelA":…,"valueA":…,…}`) and return its canonical wire bytes, or `None`
/// when the holes JSON is malformed or missing a field.
pub fn encode_from_holes(holes_json: &str) -> Option<String> {
    let holes = canonical::parse(holes_json).ok()?;
    let h = Holes {
        label_a: str_hole(&holes, "labelA")?,
        value_a: num_hole(&holes, "valueA")?,
        label_b: str_hole(&holes, "labelB")?,
        value_b: num_hole(&holes, "valueB")?,
        label_c: str_hole(&holes, "labelC")?,
        value_c: num_hole(&holes, "valueC")?,
    };
    Some(encode_node(&exemplar(&h)))
}
