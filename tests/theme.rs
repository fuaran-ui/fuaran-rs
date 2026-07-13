//! WCAG contrast derivation — the theme-observable accessibility mechanic
//! (Infinite Skins contrast auditor, Kintsugi's contrast sense). Pinned to the
//! WCAG reference constants so a restyle that drops a pair below AA is caught
//! structurally, with no pixels in the conclusion.

use fuaran_rs::theme::{
    ContrastVerdict, Rgba, composite, contrast_ratio, effective_background, foreground_contrast,
    relative_luminance, verdict,
};

// Absolute float tolerance for the pinned WCAG reference values.
const EPS: f64 = 1e-6;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= EPS
}

#[test]
fn extremes_pin_to_the_wcag_bounds() {
    // Black on white is the maximum possible ratio, exactly 21.0.
    assert!(close(contrast_ratio(Rgba::BLACK, Rgba::WHITE), 21.0));
    // A colour against itself is the minimum, exactly 1.0.
    let brand = Rgba::rgb(0.0, 90.0, 200.0);
    assert!(close(contrast_ratio(brand, brand), 1.0));
    // The relation is symmetric.
    assert!(close(
        contrast_ratio(Rgba::BLACK, Rgba::WHITE),
        contrast_ratio(Rgba::WHITE, Rgba::BLACK)
    ));
}

#[test]
fn relative_luminance_pins_the_reference_endpoints() {
    assert!(close(relative_luminance(Rgba::WHITE), 1.0));
    assert!(close(relative_luminance(Rgba::BLACK), 0.0));
}

#[test]
fn the_wcag_grey_sits_on_the_aa_boundary() {
    // #767676 on white is the canonical "just passes AA normal text" grey —
    // ratio ≈ 4.54, the reference boundary case for the 4.5 threshold.
    let grey = Rgba::rgb(118.0, 118.0, 118.0);
    let ratio = contrast_ratio(grey, Rgba::WHITE);
    assert!((4.5..4.6).contains(&ratio), "expected ~4.54, got {ratio}");
    let v = ContrastVerdict::of(ratio);
    assert!(v.aa_normal, "grey-on-white passes AA normal");
    assert!(!v.aaa_normal, "but not AAA normal");
    assert!(!v.below_aa_large());
}

#[test]
fn a_low_contrast_pair_fails_aa() {
    // Light grey on white — the restyle a machine flags as illegible.
    let light = Rgba::rgb(200.0, 200.0, 200.0);
    let v = verdict(light, &[Rgba::WHITE]);
    assert!(!v.aa_normal);
    assert!(v.ratio < 4.5);
}

#[test]
fn alpha_is_composited_against_the_effective_background() {
    // A 50%-opaque black over white composites to mid-grey, not black — the
    // contrast is read against the *resolved* colour, not the nominal one.
    let translucent = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.5,
    };
    let ratio = foreground_contrast(translucent, &[Rgba::WHITE]);
    let opaque_ratio = contrast_ratio(Rgba::BLACK, Rgba::WHITE);
    assert!(
        ratio < opaque_ratio,
        "translucent text has less contrast ({ratio}) than opaque ({opaque_ratio})"
    );
    assert!(ratio > 1.0);
}

#[test]
fn effective_background_stops_at_the_first_opaque_layer() {
    // Element-first stack: translucent tint over an opaque panel over
    // (ignored) page — resolves to the tint composited onto the panel.
    let tint = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.25,
    };
    let panel = Rgba::rgb(240.0, 240.0, 240.0);
    let page = Rgba::rgb(10.0, 10.0, 10.0); // opaque but below the panel — ignored
    let bg = effective_background(&[tint, panel, page]);
    let expected = composite(tint, panel);
    assert!(close(bg.r, expected.r) && close(bg.g, expected.g) && close(bg.b, expected.b));
    assert!(close(bg.a, 1.0));
}

#[test]
fn an_all_translucent_stack_falls_back_to_white() {
    // No opaque layer anywhere → the browser's default white canvas backs it.
    let faint = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.1,
    };
    let bg = effective_background(&[faint]);
    assert!(close(bg.a, 1.0));
    // Composited over white, a 10% black is very light.
    assert!(bg.r > 220.0);
}
