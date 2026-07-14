//! Theme-manifest decoder + projector coverage — the same inputs the `fuaran-go`
//! `thememanifest` unit tests use, asserting the Rust host decodes to the same
//! projection (Phase 560), plus the contrast-tier bridge that consumes projected
//! tokens.

use fuaran_rs::theme::manifest::{
    self, InvariantKind, ManifestRole, ThemeManifest, decode, merge,
    project_from_css_custom_properties, project_from_fuaran_tone_vars,
};
use fuaran_rs::theme::{ManifestMeta, ManifestToken, RoleBinding};

fn sample_manifest() -> ThemeManifest {
    ThemeManifest {
        meta: ManifestMeta {
            name: "test".into(),
            version: "1.0".into(),
            description: None,
        },
        tokens: vec![
            ManifestToken {
                name: "color.brand.base".into(),
                token_type: "color".into(),
                value: "#3b5bdb".into(),
                description: None,
                role: None,
            },
            ManifestToken {
                name: "color.surface".into(),
                token_type: "color".into(),
                value: "#ffffff".into(),
                description: None,
                role: None,
            },
            ManifestToken {
                name: "space.md".into(),
                token_type: "dimension".into(),
                value: "16px".into(),
                description: None,
                role: None,
            },
        ],
        roles: vec![
            RoleBinding {
                role: ManifestRole::Tone("Brand".into()),
                token_name: "color.brand.base".into(),
            },
            RoleBinding {
                role: ManifestRole::Named("body-text".into()),
                token_name: "color.surface".into(),
            },
        ],
        invariants: vec![manifest::Invariant::new(InvariantKind::ContrastFloor {
            role: "Brand".into(),
            min_ratio: 7.0,
        })],
    }
}

#[test]
fn helpers_resolve_tokens_roles_and_palette() {
    let m = sample_manifest();
    assert_eq!(m.try_get_token("color.surface").unwrap().value, "#ffffff");
    assert!(m.try_get_token("missing").is_none());
    assert_eq!(m.resolve_role("Brand").unwrap().name, "color.brand.base");
    assert!(m.resolve_role("Critical").is_none());
    assert_eq!(m.resolve_named_role("body-text").unwrap().value, "#ffffff");
    let pal = m.palette_colours();
    assert_eq!(pal.len(), 2);
    assert!(pal.contains(&"#3b5bdb".to_string()));
    assert!(pal.contains(&"#ffffff".to_string()));
}

#[test]
fn decodes_the_fuaran_wrapper() {
    let payload = r##"{
        "meta": {"name": "acme", "version": "2.1", "description": "x"},
        "tokens": {"color": {"brand": {"base": {"$type":"color","$value":"#3b5bdb","$description":"brand"}},
                             "surface": {"$type":"color","$value":"#ffffff"}}},
        "roles": [{"role": {"tone": "Brand"}, "token": "color.brand.base"}],
        "invariants": [{"kind":"ContrastFloor","role":"Brand","minRatio":7,"weight":2}]
    }"##;
    let m = decode(payload).expect("decode");
    assert_eq!(m.meta.name, "acme");
    assert_eq!(m.meta.version, "2.1");
    assert_eq!(m.meta.description.as_deref(), Some("x"));
    let tok = m.try_get_token("color.brand.base").expect("brand token");
    assert_eq!(tok.token_type, "color");
    assert_eq!(tok.value, "#3b5bdb");
    assert_eq!(tok.description.as_deref(), Some("brand"));
    assert_eq!(m.resolve_role("Brand").unwrap().name, "color.brand.base");
    assert_eq!(m.invariants.len(), 1);
    match &m.invariants[0].kind {
        InvariantKind::ContrastFloor { role, min_ratio } => {
            assert_eq!(role, "Brand");
            assert_eq!(*min_ratio, 7.0);
        }
        other => panic!("expected ContrastFloor, got {other:?}"),
    }
    assert_eq!(m.invariants[0].weight, 2.0);
}

#[test]
fn decodes_vanilla_dtcg() {
    let m = decode(r##"{"color": {"accent": {"$type":"color","$value":"#ff8800"}}}"##).unwrap();
    assert_eq!(m.tokens.len(), 1);
    assert_eq!(m.tokens[0].name, "color.accent");
    assert_eq!(m.tokens[0].value, "#ff8800");
    assert!(m.roles.is_empty());
}

#[test]
fn decodes_the_role_extension() {
    let m = decode(
        r##"{"color": {"brand": {"$type":"color","$value":"#3b5bdb","$extensions":{"fuaran":{"role":"accent"}}}}}"##,
    )
    .unwrap();
    assert_eq!(m.tokens.len(), 1);
    assert_eq!(m.tokens[0].role.as_deref(), Some("accent"));
}

#[test]
fn projects_tone_vars_and_css_custom_properties() {
    let m = project_from_fuaran_tone_vars(
        ":root { --fuaran-tone-brand-bg: #3b5bdb; --fuaran-tone-brand-fg: #fff; }",
    );
    assert_eq!(m.try_get_token("tone.brand.bg").unwrap().value, "#3b5bdb");
    assert_eq!(m.resolve_role("Brand").unwrap().name, "tone.brand.bg");

    let css = r#":root { --color-x: #111; } [data-theme="dark"] { --color-x: #eee; }"#;
    let g = project_from_css_custom_properties(css);
    assert_eq!(g.try_get_token("color-x").unwrap().value, "#111");
    assert_eq!(g.try_get_token("color-x@dark").unwrap().value, "#eee");
    assert!(g.roles.is_empty());
}

#[test]
fn merge_is_last_write_wins() {
    let base = project_from_css_custom_properties(":root { --a: 1px; --b: 2px; }");
    let over = project_from_css_custom_properties(":root { --b: 9px; }");
    let m = merge(&base, &over);
    assert_eq!(m.try_get_token("b").unwrap().value, "9px");
    assert_eq!(m.try_get_token("a").unwrap().value, "1px");
}

#[test]
fn contrast_tier_consumes_projected_tokens() {
    // A manifest binding Brand → white and Default → black: the contrast tier
    // resolves both tones through the role bindings and derives a WCAG verdict.
    let m = decode(
        r##"{
            "tokens": {"fg": {"$type":"color","$value":"#ffffff"},
                       "bg": {"$type":"color","$value":"#000000"}},
            "roles": [{"role": {"tone": "Brand"}, "token": "fg"},
                      {"role": {"tone": "Default"}, "token": "bg"}]
        }"##,
    )
    .unwrap();
    let v = manifest::tone_contrast(&m, "Brand", "Default").expect("both tones resolve");
    // White on black is the maximal WCAG ratio (21:1) — passes every bar.
    assert!((v.ratio - 21.0).abs() < 1e-9);
    assert!(v.aaa_normal);
    assert!(manifest::tone_contrast(&m, "Brand", "Critical").is_none());
}
