//! WCAG contrast derivation — the theme-observable accessibility mechanic
//! (Infinite Skins contrast auditor, Kintsugi's contrast sense). Pinned to the
//! WCAG reference constants so a restyle that drops a pair below AA is caught
//! structurally, with no pixels in the conclusion.
//!
//! **The style-observer encode literals in this file are the GO-RED PARTNER of
//! the shared `style-observer/` corpus family, not a duplicate of it** (Phase
//! 1752). Phase 1724 ported them here from the Python host by hand;
//! `tests/style_observer_corpus.rs` now certifies this host against the family
//! the reference host emits. Keeping both is the point: a regression that moved
//! the implementation AND the emitted family together would satisfy the corpus
//! checker and fail here, which is the one failure a family emitted from the
//! thing it certifies cannot see on its own. Do not delete them as redundant.

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

// ---- Phase 1724: style observer ----
//
// The pure style-observer tier: flag derivation, the manifest-aware per-node
// flags and usage budgets, the in-memory observer, and the byte-identical
// observation encode.
//
// **Where the expected bytes come from.** There is no shared vector file for
// this surface in the conformance corpus, so the ported cases below ARE the
// pin. They were not hand-computed: each expected string was emitted by the
// Python host (`fuaran_ui.style_observer`) and independently by the Go host
// (`styleobserver`) for the same facts, and the two agreed byte for byte on
// every case, budget arrangement and tree observation here. Lifting them into
// the shared corpus is a corpus act for a later phase; until then, a change to
// these strings is a change to a cross-host contract, not to a test fixture.

use fuaran_rs::theme::{
    InMemoryStyleObserver, NodeArea, StyleFlag, StyleInput, StyleObservation, StyleObserverOptions,
    ThemeManifest, derive_style_flags, encode_style_flag, encode_style_observation, font_role_of,
    per_node_flags, to_style_observation, try_parse_hex, verify_usage_budgets,
};

/// The declared manifest the ported cases run against — the same JSON the
/// Python and Go oracles decoded.
///
/// `color.veil` is deliberately an 8-digit hex: both sibling hosts' palette
/// parse accepts `#rrggbbaa` and then compares RGB with alpha ignored, so a node
/// filled `rgb(32, 48, 64)` is ON-palette. It is here because the manifest
/// tier's own `parse_hex` declines an alpha channel, and reusing that alone
/// would have made this host report `OffPaletteColour` where the siblings report
/// nothing. `color.spacing` is a non-colour token, present so the palette filter
/// is exercised rather than assumed.
const MANIFEST_JSON: &str = concat!(
    r##"{"meta":{"name":"loch","version":"1.0.0"},"##,
    r##""tokens":{"color":{"brand":{"$type":"color","$value":"#1f4f6f"},"##,
    r##""surface":{"$type":"color","$value":"#ffffff"},"##,
    r##""accent":{"$type":"color","$value":"#fafafa"},"##,
    r##""veil":{"$type":"color","$value":"#20304080"},"##,
    r##""spacing":{"$type":"dimension","$value":"8px"}}},"##,
    r##""roles":[{"role":{"tone":"Brand"},"token":"color.brand"},"##,
    r##"{"role":{"tone":"Default"},"token":"color.surface"}],"##,
    r##""invariants":[{"kind":"ContrastFloor","role":"Brand","minRatio":7.0},"##,
    r##"{"kind":"UsageBudget","token":"color.brand","targetPct":10.0,"tolerancePct":5.0},"##,
    r##"{"kind":"MotionVoice","maxDurationMs":200,"easing":"ease-out"}]}"##,
);

fn manifest() -> ThemeManifest {
    fuaran_rs::theme::decode(MANIFEST_JSON).expect("the manifest fixture decodes")
}

fn opts() -> StyleObserverOptions {
    StyleObserverOptions::default()
}

fn rgb(r: f64, g: f64, b: f64) -> Rgba {
    Rgba::rgb(r, g, b)
}

fn clear() -> Rgba {
    Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    }
}

fn input(foreground: Rgba, layers: &[Rgba]) -> StyleInput {
    StyleInput {
        foreground,
        background_layers: layers.to_vec(),
        ..StyleInput::default()
    }
}

fn toned(foreground: Rgba, layers: &[Rgba], tone: &str) -> StyleInput {
    StyleInput {
        emitted_tone: Some(tone.to_string()),
        ..input(foreground, layers)
    }
}

fn observe(id: &str, inp: &StyleInput) -> StyleObservation {
    to_style_observation(&opts(), id, inp)
}

fn encoded_flags(flags: &[StyleFlag]) -> Vec<String> {
    flags.iter().map(encode_style_flag).collect()
}

/// Every ported case: `(id, input, expected observation bytes, expected
/// manifest-aware flag bytes)`.
fn ported_cases() -> Vec<(&'static str, StyleInput, &'static str, Vec<&'static str>)> {
    let white = Rgba::WHITE;
    vec![
        (
            "clean-root",
            StyleInput {
                font_family: Some("IBM Plex Sans".to_string()),
                ..input(Rgba::BLACK, &[white])
            },
            r#"{"nodeId":"clean-root","foreground":{"r":0.00,"g":0.00,"b":0.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"SansSerif","emittedTone":null,"contrastRatio":21.00,"flags":[]}"#,
            vec![],
        ),
        (
            "faint-label",
            input(rgb(153.0, 153.0, 153.0), &[white]),
            r#"{"nodeId":"faint-label","foreground":{"r":153.00,"g":153.00,"b":153.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"Unknown","emittedTone":null,"contrastRatio":2.85,"flags":[{"kind":"ContrastBelowAA","ratio":2.85}]}"#,
            vec![],
        ),
        (
            "ghost-text",
            StyleInput {
                font_family: Some("JetBrains Mono".to_string()),
                ..input(white, &[white])
            },
            r#"{"nodeId":"ghost-text","foreground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"Monospace","emittedTone":null,"contrastRatio":1.00,"flags":[{"kind":"InvisibleText","ratio":1.00}]}"#,
            vec![],
        ),
        (
            "accent-panel",
            StyleInput {
                font_family: Some("Georgia, serif".to_string()),
                ..toned(Rgba::BLACK, &[rgb(250.0, 250.0, 250.0), white], "Brand")
            },
            r#"{"nodeId":"accent-panel","foreground":{"r":0.00,"g":0.00,"b":0.00,"a":1.00},"effectiveBackground":{"r":250.00,"g":250.00,"b":250.00,"a":1.00},"fontRole":"Serif","emittedTone":"Brand","contrastRatio":20.12,"flags":[{"kind":"AccentIndistinct","ratio":1.04}]}"#,
            vec![],
        ),
        (
            "baseline",
            StyleInput::default(),
            r#"{"nodeId":"baseline","foreground":{"r":0.00,"g":0.00,"b":0.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"Unknown","emittedTone":null,"contrastRatio":21.00,"flags":[]}"#,
            vec![],
        ),
        (
            "toned-no-layers",
            toned(Rgba::BLACK, &[], "Brand"),
            r#"{"nodeId":"toned-no-layers","foreground":{"r":0.00,"g":0.00,"b":0.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"Unknown","emittedTone":"Brand","contrastRatio":21.00,"flags":[]}"#,
            vec![],
        ),
        (
            "translucent-own-layer",
            toned(Rgba::BLACK, &[clear(), white], "Brand"),
            r#"{"nodeId":"translucent-own-layer","foreground":{"r":0.00,"g":0.00,"b":0.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"Unknown","emittedTone":"Brand","contrastRatio":21.00,"flags":[]}"#,
            vec![],
        ),
        (
            "brand-surface",
            toned(rgb(90.0, 110.0, 125.0), &[rgb(31.0, 79.0, 111.0)], "Brand"),
            r#"{"nodeId":"brand-surface","foreground":{"r":90.00,"g":110.00,"b":125.00,"a":1.00},"effectiveBackground":{"r":31.00,"g":79.00,"b":111.00,"a":1.00},"fontRole":"Unknown","emittedTone":"Brand","contrastRatio":1.65,"flags":[{"kind":"ContrastBelowAA","ratio":1.65}]}"#,
            vec![
                r#"{"kind":"ContrastBelowDeclaredFloor","role":"Brand","ratio":1.65,"floor":7.00}"#,
            ],
        ),
        (
            "unknown-tone",
            toned(Rgba::BLACK, &[white], "Aubergine"),
            r#"{"nodeId":"unknown-tone","foreground":{"r":0.00,"g":0.00,"b":0.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"Unknown","emittedTone":"Aubergine","contrastRatio":21.00,"flags":[{"kind":"AccentIndistinct","ratio":1.00}]}"#,
            vec![r#"{"kind":"TokenResolutionFailed","slot":"Aubergine"}"#],
        ),
        (
            "off-palette",
            toned(Rgba::BLACK, &[rgb(200.0, 200.0, 200.0)], "Default"),
            r#"{"nodeId":"off-palette","foreground":{"r":0.00,"g":0.00,"b":0.00,"a":1.00},"effectiveBackground":{"r":200.00,"g":200.00,"b":200.00,"a":1.00},"fontRole":"Unknown","emittedTone":"Default","contrastRatio":12.55,"flags":[{"kind":"AccentIndistinct","ratio":1.67}]}"#,
            vec![r#"{"kind":"OffPaletteColour","value":"rgb(200, 200, 200)"}"#],
        ),
        (
            "veiled",
            toned(white, &[rgb(32.0, 48.0, 64.0)], "Default"),
            r#"{"nodeId":"veiled","foreground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"effectiveBackground":{"r":32.00,"g":48.00,"b":64.00,"a":1.00},"fontRole":"Unknown","emittedTone":"Default","contrastRatio":13.48,"flags":[]}"#,
            vec![],
        ),
    ]
}

#[test]
fn the_ported_cases_encode_to_the_sibling_hosts_bytes() {
    for (id, inp, expected, _) in ported_cases() {
        assert_eq!(
            encode_style_observation(&observe(id, &inp)),
            expected,
            "observation bytes diverged for case {id}"
        );
    }
}

#[test]
fn the_ported_cases_derive_the_sibling_hosts_manifest_flags() {
    let m = manifest();
    for (id, inp, _, expected) in ported_cases() {
        let obs = observe(id, &inp);
        assert_eq!(
            encoded_flags(&per_node_flags(&m, &obs)),
            expected,
            "manifest-aware flags diverged for case {id}"
        );
    }
}

#[test]
fn the_encode_is_stable_under_re_derivation() {
    // The derivation is a pure function of the facts: the same input encodes to
    // the same bytes every time, with no ordering or hash-iteration wobble.
    for (id, inp, expected, _) in ported_cases() {
        for _ in 0..3 {
            assert_eq!(encode_style_observation(&observe(id, &inp)), expected);
        }
    }
}

// ─── Per-flag behaviour: fires / does not fire / absent fact never fires ─────

#[test]
fn contrast_below_aa_fires_only_inside_its_band() {
    let white = Rgba::WHITE;
    // Fires: mid-grey on white is ~2.85 — legible, but below the AA floor.
    let faint = derive_style_flags(&opts(), &input(rgb(153.0, 153.0, 153.0), &[white]));
    assert!(matches!(
        faint.as_slice(),
        [StyleFlag::ContrastBelowAA { .. }]
    ));

    // Does not fire above the floor: black on white clears every bar.
    assert!(derive_style_flags(&opts(), &input(Rgba::BLACK, &[white])).is_empty());

    // Does not fire BELOW the invisible threshold either — the severe subset is
    // carved out, so the two flags are disjoint rather than nested.
    let ghost = derive_style_flags(&opts(), &input(white, &[white]));
    assert!(matches!(
        ghost.as_slice(),
        [StyleFlag::InvisibleText { .. }]
    ));
}

#[test]
fn invisible_text_fires_when_the_text_is_its_surface() {
    let white = Rgba::WHITE;
    let flags = derive_style_flags(&opts(), &input(white, &[white]));
    match flags.as_slice() {
        [StyleFlag::InvisibleText { ratio }] => assert!((ratio - 1.0).abs() < 1e-9),
        other => panic!("expected a single InvisibleText, got {other:?}"),
    }
    // Just clear of the threshold: nothing fires from this flag.
    let nearly = input(rgb(220.0, 220.0, 220.0), &[white]);
    assert!(
        !derive_style_flags(&opts(), &nearly)
            .iter()
            .any(|f| matches!(f, StyleFlag::InvisibleText { .. }))
    );
}

#[test]
fn accent_indistinct_needs_a_tone_a_layer_and_an_opaque_own_fill() {
    let white = Rgba::WHITE;
    let tint = rgb(250.0, 250.0, 250.0);

    // Fires: a toned element whose own fill barely differs from its container.
    let fires = derive_style_flags(&opts(), &toned(Rgba::BLACK, &[tint, white], "Brand"));
    assert!(
        fires
            .iter()
            .any(|f| matches!(f, StyleFlag::AccentIndistinct { .. }))
    );

    // ABSENT FACT 1 — no declared tone: the same colours, and silence. There is
    // no accent to judge.
    let untoned = derive_style_flags(&opts(), &input(Rgba::BLACK, &[tint, white]));
    assert!(
        !untoned
            .iter()
            .any(|f| matches!(f, StyleFlag::AccentIndistinct { .. }))
    );

    // ABSENT FACT 2 — a tone but no background layers at all.
    assert!(derive_style_flags(&opts(), &toned(Rgba::BLACK, &[], "Brand")).is_empty());

    // ABSENT FACT 3 — a fully transparent own layer: there is no accent surface.
    assert!(
        derive_style_flags(&opts(), &toned(Rgba::BLACK, &[clear(), white], "Brand")).is_empty()
    );

    // Does not fire when the accent is genuinely distinct from its container.
    let distinct = toned(Rgba::WHITE, &[rgb(31.0, 79.0, 111.0), white], "Brand");
    assert!(
        !derive_style_flags(&opts(), &distinct)
            .iter()
            .any(|f| matches!(f, StyleFlag::AccentIndistinct { .. }))
    );
}

#[test]
fn an_absent_font_family_classifies_as_unknown_rather_than_guessing() {
    assert_eq!(font_role_of(&StyleInput::default()).wire(), "Unknown");
    let named = |family: &str| {
        font_role_of(&StyleInput {
            font_family: Some(family.to_string()),
            ..StyleInput::default()
        })
        .wire()
    };
    assert_eq!(named("IBM Plex Sans"), "SansSerif");
    assert_eq!(named("Georgia, serif"), "Serif");
    assert_eq!(named("JetBrains Mono"), "Monospace");
    assert_eq!(named("Papyrus"), "Unknown");
    // The probe order is normative: a family naming both resolves as monospace.
    assert_eq!(named("Fira Mono Sans"), "Monospace");
}

#[test]
fn an_untoned_node_is_exempt_from_every_manifest_check() {
    // The Custom-subtree policy, stated as a test: the manifest-aware tier is
    // tone-gated, so untoned content cannot fire any of its four flags however
    // far off-palette it is painted.
    let m = manifest();
    let garish = observe("custom", &input(Rgba::BLACK, &[rgb(255.0, 0.0, 255.0)]));
    assert!(per_node_flags(&m, &garish).is_empty());
}

#[test]
fn an_unresolvable_slot_reports_only_the_resolution_failure() {
    // Asking whether an unresolvable slot's fill is on-palette is a question
    // about a token that does not exist, so the two fill checks are exclusive.
    let m = manifest();
    let obs = observe(
        "x",
        &toned(Rgba::BLACK, &[rgb(200.0, 200.0, 200.0)], "Aubergine"),
    );
    let flags = per_node_flags(&m, &obs);
    assert!(matches!(
        flags.as_slice(),
        [StyleFlag::TokenResolutionFailed { .. }]
    ));
}

#[test]
fn an_eight_digit_palette_token_is_parsed_and_compared_rgb_only() {
    // The divergence this case exists to pin: the manifest tier's own hex parse
    // declines an alpha channel, while the palette-membership parse accepts
    // `#rrggbbaa` and ignores alpha — as both sibling hosts do.
    let veil = try_parse_hex("#20304080").expect("an 8-digit hex parses");
    assert_eq!((veil.r, veil.g, veil.b), (32.0, 48.0, 64.0));
    assert!((veil.a - 128.0 / 255.0).abs() < 1e-9);
    assert!(try_parse_hex("#fff").is_some());
    assert!(try_parse_hex("#1f4f6f").is_some());
    assert!(try_parse_hex("oklch(0.5 0.1 200)").is_none());
}

// ─── Usage budgets ───────────────────────────────────────────────────────────

fn budget_nodes(brand_area: f64, plain_area: f64) -> Vec<NodeArea> {
    let brand = observe(
        "brand",
        &toned(Rgba::WHITE, &[rgb(31.0, 79.0, 111.0)], "Brand"),
    );
    let plain = observe("plain", &input(Rgba::BLACK, &[Rgba::WHITE]));
    vec![
        NodeArea {
            obs: brand,
            area: brand_area,
        },
        NodeArea {
            obs: plain,
            area: plain_area,
        },
    ]
}

#[test]
fn a_usage_budget_inside_its_tolerance_is_silent() {
    // 10 of 100 px², declared 10 ± 5.
    assert!(verify_usage_budgets(&manifest(), &budget_nodes(10.0, 90.0)).is_empty());
}

#[test]
fn a_breached_usage_budget_reports_the_declared_and_observed_share() {
    let flags = verify_usage_budgets(&manifest(), &budget_nodes(60.0, 40.0));
    assert_eq!(
        encoded_flags(&flags),
        vec![
            r#"{"kind":"UsageBudgetExceeded","token":"color.brand","declaredPct":10.00,"observedPct":60.00}"#
        ]
    );
}

#[test]
fn same_valued_tokens_attribute_to_the_path_first_token() {
    // Phase 1727 — the palette-attribution tie-break. Two colour tokens carry
    // the same value, declared secondary-before-brand; the rule (fuaran-dotnet's
    // docs/THEME-BRIDGE-GUIDE.md, "Palette attribution order") attributes the
    // fill to the FIRST token in canonical token-path order, so `color.brand`
    // takes the 60px² and `color.secondary` takes none. This host meets the rule
    // through its decoder's sorted walk, so the case goes through `decode`
    // rather than a token-list literal: a decoder that stopped sorting, or an
    // attribution that stopped trusting the list order, goes red here rather
    // than silently re-diverging. The corpus vector of the same name is the
    // cross-host law; this is its go-red partner.
    let manifest = fuaran_rs::theme::decode(concat!(
        r##"{"meta":{"name":"t","version":"1"},"tokens":{"color":{"##,
        r##""secondary":{"$type":"color","$value":"#010203"},"##,
        r##""brand":{"$type":"color","$value":"#010203"}}},"roles":[],"invariants":["##,
        r##"{"kind":"UsageBudget","token":"color.brand","targetPct":10,"tolerancePct":5},"##,
        r##"{"kind":"UsageBudget","token":"color.secondary","targetPct":0,"tolerancePct":5}]}"##,
    ))
    .expect("the tie manifest decodes");
    let fill = observe("a", &input(Rgba::BLACK, &[rgb(1.0, 2.0, 3.0)]));
    let other = observe("b", &input(Rgba::BLACK, &[rgb(9.0, 9.0, 9.0)]));
    let nodes = vec![
        NodeArea {
            obs: fill,
            area: 60.0,
        },
        NodeArea {
            obs: other,
            area: 40.0,
        },
    ];
    assert_eq!(
        encoded_flags(&verify_usage_budgets(&manifest, &nodes)),
        vec![
            r#"{"kind":"UsageBudgetExceeded","token":"color.brand","declaredPct":10.00,"observedPct":60.00}"#
        ],
        "a document-order attribution breaches both budgets"
    );
}

#[test]
fn token_path_order_is_segment_wise_not_a_string_sort() {
    // Phase 1727 — `color.brand.base` precedes `color.brand-alt` because the
    // key `brand` precedes `brand-alt`, although `-` sorts before `.` as a
    // character: a sort of the joined path would put `brand-alt` first and
    // diverge from every host that sorts per group.
    let manifest = fuaran_rs::theme::decode(concat!(
        r##"{"meta":{"name":"t","version":"1"},"tokens":{"color":{"##,
        r##""brand-alt":{"$type":"color","$value":"#010203"},"##,
        r##""brand":{"base":{"$type":"color","$value":"#010203"}}}},"roles":[],"invariants":["##,
        r##"{"kind":"UsageBudget","token":"color.brand.base","targetPct":10,"tolerancePct":5},"##,
        r##"{"kind":"UsageBudget","token":"color.brand-alt","targetPct":0,"tolerancePct":5}]}"##,
    ))
    .expect("the segment-order manifest decodes");
    let fill = observe("a", &input(Rgba::BLACK, &[rgb(1.0, 2.0, 3.0)]));
    let other = observe("b", &input(Rgba::BLACK, &[rgb(9.0, 9.0, 9.0)]));
    let nodes = vec![
        NodeArea {
            obs: fill,
            area: 60.0,
        },
        NodeArea {
            obs: other,
            area: 40.0,
        },
    ];
    assert_eq!(
        encoded_flags(&verify_usage_budgets(&manifest, &nodes)),
        vec![
            r#"{"kind":"UsageBudgetExceeded","token":"color.brand.base","declaredPct":10.00,"observedPct":60.00}"#
        ]
    );
}

#[test]
fn no_measured_area_verifies_nothing() {
    // A budget is a statement about SHARE, and a share of nothing is not a
    // breach — so a tree whose areas were never measured is silent rather than
    // reporting every declared budget as 0% and breached.
    assert!(verify_usage_budgets(&manifest(), &budget_nodes(0.0, 0.0)).is_empty());
    assert!(verify_usage_budgets(&manifest(), &[]).is_empty());
}

// ─── The in-memory observer ──────────────────────────────────────────────────

fn wired_observer() -> InMemoryStyleObserver {
    let white = Rgba::WHITE;
    let tint = rgb(250.0, 250.0, 250.0);
    let mut obv = InMemoryStyleObserver::new(opts(), Some(manifest()));
    obv.register_fixture("root", input(Rgba::BLACK, &[white]), None);
    obv.register_fixture(
        "panel",
        input(rgb(153.0, 153.0, 153.0), &[tint, white]),
        Some("root"),
    );
    obv.register_fixture(
        "label",
        toned(white, &[tint, white], "Brand"),
        Some("panel"),
    );
    obv
}

#[test]
fn observe_tree_walks_the_ancestry_breadth_first_in_registration_order() {
    let obv = wired_observer();
    let observations = obv.observe_tree("root");
    let ids: Vec<&str> = observations.iter().map(|o| o.node_id.as_str()).collect();
    assert_eq!(ids, vec!["root", "panel", "label"]);

    // The ported bytes for the whole walk — manifest-aware flags appended after
    // the manifest-free ones, in that order, on every node.
    let encoded: Vec<String> = observations.iter().map(encode_style_observation).collect();
    assert_eq!(
        encoded,
        vec![
            r#"{"nodeId":"root","foreground":{"r":0.00,"g":0.00,"b":0.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"Unknown","emittedTone":null,"contrastRatio":21.00,"flags":[]}"#,
            r#"{"nodeId":"panel","foreground":{"r":153.00,"g":153.00,"b":153.00,"a":1.00},"effectiveBackground":{"r":250.00,"g":250.00,"b":250.00,"a":1.00},"fontRole":"Unknown","emittedTone":null,"contrastRatio":2.73,"flags":[{"kind":"ContrastBelowAA","ratio":2.73}]}"#,
            r#"{"nodeId":"label","foreground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"effectiveBackground":{"r":250.00,"g":250.00,"b":250.00,"a":1.00},"fontRole":"Unknown","emittedTone":"Brand","contrastRatio":1.04,"flags":[{"kind":"InvisibleText","ratio":1.04},{"kind":"AccentIndistinct","ratio":1.04},{"kind":"ContrastBelowDeclaredFloor","role":"Brand","ratio":1.04,"floor":7.00}]}"#,
        ]
    );

    // The ancestry composes through the SUPPLIED layer stack: `panel` sits on
    // the tint, which is opaque, so the white below it is never reached.
    let panel = obv.observe("panel").expect("panel is registered");
    assert_eq!(panel.effective_background.r, 250.0);
    assert!(obv.observe("absent").is_none());
    assert!(obv.observe_tree("absent").is_empty());
}

#[test]
fn the_parent_is_a_tree_pointer_and_never_a_compositing_one() {
    // The premise worth pinning, because getting it wrong would be invisible in
    // this host and loud across the estate: registering a parent must not change
    // a node's DERIVED observation by one byte. The supplied layer stack is the
    // only thing compositing reads.
    let white = Rgba::WHITE;
    let facts = input(rgb(153.0, 153.0, 153.0), &[rgb(250.0, 250.0, 250.0), white]);

    let mut rooted = InMemoryStyleObserver::new(opts(), Some(manifest()));
    rooted.register_fixture("panel", facts.clone(), None);

    let mut parented = InMemoryStyleObserver::new(opts(), Some(manifest()));
    parented.register_fixture("root", toned(Rgba::BLACK, &[white], "Brand"), None);
    parented.register_fixture("panel", facts, Some("root"));

    assert_eq!(
        encode_style_observation(&rooted.observe("panel").unwrap()),
        encode_style_observation(&parented.observe("panel").unwrap())
    );
}

#[test]
fn update_re_emits_only_on_a_flag_change_when_that_is_the_policy() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let white = Rgba::WHITE;
    let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let mut obv = InMemoryStyleObserver::new(opts(), None);
    let sink = Rc::clone(&seen);
    let id = obv.subscribe(Box::new(move |node_id, _| {
        sink.borrow_mut().push(node_id.to_string());
    }));

    // Registration always emits — a node's first observation is not a "change".
    obv.register_fixture("t", input(Rgba::BLACK, &[white]), None);
    assert_eq!(seen.borrow().len(), 1);

    // A different colour with the same (empty) flag list: silent.
    obv.update("t", input(rgb(20.0, 20.0, 20.0), &[white]));
    assert_eq!(seen.borrow().len(), 1);

    // A colour that changes the flag list: emitted.
    obv.update("t", input(rgb(153.0, 153.0, 153.0), &[white]));
    assert_eq!(seen.borrow().len(), 2);

    // An unregistered node is a no-op, not a panic and not an emission.
    obv.update("absent", input(Rgba::BLACK, &[white]));
    assert_eq!(seen.borrow().len(), 2);

    // Unsubscribing stops the channel; unsubscribing twice reports the second.
    assert!(obv.unsubscribe(id));
    assert!(!obv.unsubscribe(id));
    obv.update("t", input(white, &[white]));
    assert_eq!(seen.borrow().len(), 2);
}

#[test]
fn always_emitting_is_reachable_by_policy() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let white = Rgba::WHITE;
    let count = Rc::new(RefCell::new(0usize));
    let options = StyleObserverOptions {
        emit_on_flag_change_only: false,
        ..StyleObserverOptions::default()
    };
    let mut obv = InMemoryStyleObserver::new(options, None);
    let sink = Rc::clone(&count);
    obv.subscribe(Box::new(move |_, _| *sink.borrow_mut() += 1));
    obv.register_fixture("t", input(Rgba::BLACK, &[white]), None);
    obv.update("t", input(rgb(20.0, 20.0, 20.0), &[white]));
    assert_eq!(*count.borrow(), 2);
}

#[test]
fn a_bare_register_creates_a_baseline_and_unregister_removes_it() {
    let mut obv = InMemoryStyleObserver::new(opts(), None);
    obv.register("mounted");
    let obs = obv.observe("mounted").expect("a baseline entry exists");
    // Opaque-black text on the implicit white canvas.
    assert_eq!(
        encode_style_observation(&obs),
        r#"{"nodeId":"mounted","foreground":{"r":0.00,"g":0.00,"b":0.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"Unknown","emittedTone":null,"contrastRatio":21.00,"flags":[]}"#
    );
    obv.unregister("mounted");
    assert!(obv.observe("mounted").is_none());
}

// ─── Go-red: flip one threshold and the ported case fails ────────────────────

#[test]
fn flipping_one_threshold_breaks_the_ported_case() {
    // The falsifier for every pinned string above. If the AA floor moves, the
    // `faint-label` case stops encoding to the sibling hosts' bytes — so these
    // assertions are load-bearing rather than tautological, and a future change
    // to the derivation cannot pass this file unnoticed.
    let faint = input(rgb(153.0, 153.0, 153.0), &[Rgba::WHITE]);
    let pinned = r#"{"nodeId":"faint-label","foreground":{"r":153.00,"g":153.00,"b":153.00,"a":1.00},"effectiveBackground":{"r":255.00,"g":255.00,"b":255.00,"a":1.00},"fontRole":"Unknown","emittedTone":null,"contrastRatio":2.85,"flags":[{"kind":"ContrastBelowAA","ratio":2.85}]}"#;
    assert_eq!(
        encode_style_observation(&to_style_observation(&opts(), "faint-label", &faint)),
        pinned
    );

    let lowered = StyleObserverOptions {
        contrast_aa_threshold: 2.0,
        ..StyleObserverOptions::default()
    };
    let under_the_mutant = to_style_observation(&lowered, "faint-label", &faint);
    assert_ne!(encode_style_observation(&under_the_mutant), pinned);
    assert!(under_the_mutant.flags.is_empty());

    // The same, one level down: raising the invisible threshold reclassifies the
    // very same facts from below-AA to invisible.
    let raised = StyleObserverOptions {
        invisible_text_threshold: 3.0,
        ..StyleObserverOptions::default()
    };
    assert!(matches!(
        derive_style_flags(&raised, &faint).as_slice(),
        [StyleFlag::InvisibleText { .. }]
    ));

    // And on the manifest side: loosen the declared floor and the finding goes.
    let m = manifest();
    let obs = observe(
        "brand-surface",
        &toned(rgb(90.0, 110.0, 125.0), &[rgb(31.0, 79.0, 111.0)], "Brand"),
    );
    assert_eq!(per_node_flags(&m, &obs).len(), 1);
    let loosened =
        fuaran_rs::theme::decode(&MANIFEST_JSON.replace(r#""minRatio":7.0"#, r#""minRatio":1.0"#))
            .expect("the loosened manifest decodes");
    assert!(per_node_flags(&loosened, &obs).is_empty());
}
