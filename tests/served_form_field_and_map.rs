//! The served Map and the ruled control classes, in the reference host's shape
//! (Phase 2391).
//!
//! The reference settled both under its 0.98.0 rulings. A Map is ONE plan (R12):
//! `div.fuaran-map.fuaran-map-ssr-placeholder` carrying the marker count and the
//! hydration caption, then one `li` per resolved marker; `fuaran-map-placeholder`
//! is retired, so a host still emitting it serves a class the reference
//! vocabulary no longer spells. Per-kind control attributes follow the client's
//! (R5): a Color input is `fuaran-form-input fuaran-color-input`, and a
//! DateTimeRange's ends are `fuaran-form-input fuaran-form-date
//! fuaran-field-range-min/max` and carry `id` and `required`.

use fuaran_rs::render::{BindingSources, render_to_html};
use fuaran_rs::wire::decode_node;

fn render(json: &str) -> String {
    render_to_html(
        &decode_node(json).expect("test tree decodes"),
        &BindingSources::default(),
    )
}

#[test]
fn a_map_whose_markers_resolve_serves_the_ruled_placeholder_and_marker_list() {
    let html = render(
        r#"{"id":"map-1","kind":{"$type":"Map","centreLatitude":51.5,"centreLongitude":-0.12,"source":{"$type":"Static","value":[{"label":"London","latitude":51.5,"longitude":-0.12}]},"zoom":6}}"#,
    );
    assert!(
        html.contains(concat!(
            r#"<div class="fuaran-map fuaran-map-ssr-placeholder" data-fuaran-ssr-placeholder="Map" data-fuaran-marker-count="1">"#,
            "[Map: 1 markers \u{2014} hydrates client-side]",
            r#"<ul class="fuaran-map-marker-list"><li class="fuaran-map-marker">London @ (51.5000, -0.1200)</li></ul></div>"#
        )),
        "{html}"
    );
    assert!(!html.contains("fuaran-map-placeholder"), "{html}");
}

#[test]
fn a_map_with_no_markers_is_the_caption_alone() {
    let html = render(
        r#"{"id":"m","kind":{"$type":"Map","centreLatitude":0,"centreLongitude":0,"source":{"$type":"Static","value":[]},"zoom":3}}"#,
    );
    assert!(
        html.contains(
            "data-fuaran-marker-count=\"0\">[Map: 0 markers \u{2014} hydrates client-side]</div>"
        ),
        "{html}"
    );
    assert!(!html.contains("fuaran-map-marker-list"), "{html}");
}

#[test]
fn a_color_input_is_classed_as_the_reference_classes_it() {
    let html = render(
        r##"{"id":"f1","kind":{"$type":"Form","fields":[{"id":"brand","kind":{"$type":"Color","value":"#3366FF"},"label":"Brand"}],"onSubmit":{"$type":"Dispatch"},"submitLabel":"Save"}}"##,
    );
    assert!(
        html.contains(r#"class="fuaran-form-input fuaran-color-input""#),
        "{html}"
    );
    assert!(
        !html.contains("fuaran-form-field-control fuaran-color-input"),
        "{html}"
    );
}

#[test]
fn a_date_range_s_ends_are_date_inputs_carrying_id_and_required() {
    let html = render(
        r#"{"id":"f1","kind":{"$type":"Form","fields":[{"id":"stay","kind":{"$type":"DateTimeRange","value":{"from":"2026-03-01","to":"2026-03-08"},"variant":"Date"},"label":"Stay","required":true}],"onSubmit":{"$type":"Dispatch"},"submitLabel":"Book"}}"#,
    );
    assert!(
        html.contains(r#"class="fuaran-form-input fuaran-form-date fuaran-field-range-min" id="stay" data-fuaran-field="stay""#),
        "{html}"
    );
    assert!(
        html.contains(r#"class="fuaran-form-input fuaran-form-date fuaran-field-range-max""#),
        "{html}"
    );
    assert_eq!(html.matches("required").count(), 2, "{html}");
    assert!(
        !html.contains("fuaran-form-field-control fuaran-field-range"),
        "{html}"
    );
}
