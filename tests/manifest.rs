//! Theme-manifest decoder + projector coverage — the same inputs the `fuaran-go`
//! `thememanifest` unit tests use, asserting the Rust host decodes to the same
//! projection (Phase 560), plus the contrast-tier bridge that consumes projected
//! tokens.

use fuaran_rs::canonical::{parse, render_canonical};
use fuaran_rs::theme::manifest::{
    self, InvariantKind, ManifestRole, ThemeManifest, decode, encode, merge, of_json,
    project_from_css_custom_properties, project_from_fuaran_tone_vars, to_json,
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

// ─── Encode round trip (Phase 1725) ──────────────────────────────────────────
//
// `fuaran-rs` is the FIRST host to emit a theme manifest: the Go `thememanifest`
// package exposes Decode / OfJSON / the projectors / Merge and no encoder of any
// spelling, the TypeScript tier exports decodeManifest and no encode, and the F#
// tier has no theme-manifest module at all. So there are no sibling bytes to
// assert against, and the literals below are not a copy of another host's output
// — they are the portable oracle a later encode on another host is held to. Read
// a change to one of them as a wire-format change for every host, not as a
// fixture refresh.

/// The canonical bytes of [`sample_manifest`], written out rather than recorded
/// from a run: a byte pin whose recorder is the code under test pins nothing.
/// Note what is ABSENT — no `description` (None), no `weight` on the invariant
/// (it carries `DEFAULT_WEIGHT`), no `$description` or `$extensions` on any token.
const SAMPLE_BYTES: &str = concat!(
    r##"{"invariants":[{"kind":"ContrastFloor","minRatio":7,"role":"Brand"}],"##,
    r##""meta":{"name":"test","version":"1.0"},"##,
    r##""roles":[{"role":{"tone":"Brand"},"token":"color.brand.base"},"##,
    r##"{"role":{"named":"body-text"},"token":"color.surface"}],"##,
    r##""tokens":{"color":{"brand":{"base":{"$type":"color","$value":"#3b5bdb"}},"##,
    r##""surface":{"$type":"color","$value":"#ffffff"}},"##,
    r##""space":{"md":{"$type":"dimension","$value":"16px"}}}}"##,
);

#[test]
fn encode_pins_the_canonical_bytes_of_a_manifest() {
    assert_eq!(encode(&sample_manifest()), SAMPLE_BYTES);
    // encode∘decode is the identity ON CANONICAL BYTES — the half that says the
    // emitted shape is one the decoder reads back without normalising anything.
    let round = encode(&decode(SAMPLE_BYTES).expect("canonical bytes decode"));
    assert_eq!(round, SAMPLE_BYTES);
}

#[test]
fn encode_pins_the_canonical_bytes_of_a_projected_manifest() {
    // The same tone-vars source `projects_tone_vars_and_css_custom_properties`
    // projects — a manifest the crate could derive and never hand back.
    let m = project_from_fuaran_tone_vars(
        ":root { --fuaran-tone-brand-bg: #3b5bdb; --fuaran-tone-brand-fg: #fff; }",
    );
    assert_eq!(
        encode(&m),
        concat!(
            r##"{"roles":[{"role":{"tone":"Brand"},"token":"tone.brand.bg"}],"##,
            r##""tokens":{"tone":{"brand":{"bg":{"$type":"color","$value":"#3b5bdb"},"##,
            r##""fg":{"$type":"color","$value":"#fff"}}}}}"##,
        )
    );
}

/// Every manifest source the decoder tests above cover, plus one carrying each
/// invariant arm with its full payload.
fn decodable_sources() -> Vec<&'static str> {
    vec![
        r##"{"meta":{"name":"acme","version":"2.1","description":"x"},
             "tokens":{"color":{"brand":{"base":{"$type":"color","$value":"#3b5bdb","$description":"brand"}},
                                "surface":{"$type":"color","$value":"#ffffff"}}},
             "roles":[{"role":{"tone":"Brand"},"token":"color.brand.base"}],
             "invariants":[{"kind":"ContrastFloor","role":"Brand","minRatio":7,"weight":2}]}"##,
        r##"{"color":{"accent":{"$type":"color","$value":"#ff8800"}}}"##,
        r##"{"color":{"brand":{"$type":"color","$value":"#3b5bdb","$extensions":{"fuaran":{"role":"accent"}}}}}"##,
        r##"{"tokens":{"a":{"$value":"1"}},
             "invariants":[{"kind":"UsageBudget","token":"a","targetPct":12.5,"tolerancePct":2,"weight":0.25},
                           {"kind":"MotionVoice","maxDurationMs":240,"easing":"ease-out"},
                           {"kind":"MotionVoice"}]}"##,
    ]
}

#[test]
fn decode_of_encode_is_the_identity_on_every_decoded_manifest() {
    // A manifest `decode` produced already carries its tokens in the wire's own
    // order, so the round trip is an exact model identity — no normalisation.
    for src in decodable_sources() {
        let m = decode(src).expect("fixture decodes");
        assert_eq!(decode(&encode(&m)).expect("re-decodes"), m, "source: {src}");
        // The JVal-level seam is the same inverse one layer down.
        assert_eq!(
            of_json(&to_json(&m)),
            m,
            "of_json of to_json, source: {src}"
        );
    }
}

#[test]
fn encode_is_a_fixpoint_through_the_round_trip() {
    // A projector or `merge` result carries tokens in first-appearance order and
    // the wire's order is sorted, so the round trip there normalises rather than
    // preserving order. The total statement covering every manifest is that
    // encode is a fixpoint through it.
    let base = project_from_css_custom_properties(":root { --b: 2px; --a: 1px; --c: #fff; }");
    let over = project_from_css_custom_properties(":root { --b: 9px; }");
    let cases = vec![
        base.clone(),
        over.clone(),
        merge(&base, &over),
        project_from_fuaran_tone_vars(
            ":root { --fuaran-tone-critical-bg: #c92a2a; --fuaran-tone-brand-bg: #3b5bdb; }",
        ),
        sample_manifest(),
        ThemeManifest::default(),
    ];
    for m in &cases {
        let once = encode(m);
        let twice = encode(&decode(&once).expect("re-decodes"));
        assert_eq!(twice, once, "encode of decode of encode differs for {m:?}");

        // What the normalisation may NOT do is lose a token: the set of
        // (name, value) pairs survives even where the order does not.
        let mut before: Vec<(String, String)> = m
            .tokens
            .iter()
            .map(|t| (t.name.clone(), t.value.clone()))
            .collect();
        let mut after: Vec<(String, String)> = decode(&once)
            .expect("re-decodes")
            .tokens
            .iter()
            .map(|t| (t.name.clone(), t.value.clone()))
            .collect();
        before.sort();
        after.sort();
        assert_eq!(after, before, "token set lost through the round trip");
    }
}

#[test]
fn encode_emits_canonical_json() {
    // The host-neutral half of the claim, and the one a sibling host can check
    // without agreeing with this host about anything else: the output is a
    // fixpoint of the shared canonical renderer. Unsorted keys, a non-canonical
    // number or a stray escape all go red here.
    for src in decodable_sources() {
        let bytes = encode(&decode(src).expect("fixture decodes"));
        let reparsed = parse(&bytes).expect("encode emits parseable JSON");
        assert_eq!(render_canonical(&reparsed), bytes, "not canonical: {bytes}");
    }
}

#[test]
fn members_the_decoder_tolerates_the_absence_of_are_omitted_at_their_default() {
    // The empty manifest is `tokens` and nothing else — and `tokens` is never
    // omitted even when empty, because a top-level `tokens` key is what selects
    // the wrapper shape in `of_json`. Dropping it would decode as vanilla DTCG
    // and silently discard meta, roles and invariants.
    assert_eq!(encode(&ThemeManifest::default()), r#"{"tokens":{}}"#);

    // A default-weight invariant carries no `weight`; a doubled one does.
    let with = |weight: f64| ThemeManifest {
        invariants: vec![manifest::Invariant {
            kind: InvariantKind::MotionVoice {
                budget: manifest::MotionBudget {
                    max_duration_ms: 0,
                    easing: None,
                },
            },
            weight,
        }],
        ..ThemeManifest::default()
    };
    assert_eq!(
        encode(&with(manifest::DEFAULT_WEIGHT)),
        r#"{"invariants":[{"kind":"MotionVoice"}],"tokens":{}}"#
    );
    assert_eq!(
        encode(&with(2.0)),
        r#"{"invariants":[{"kind":"MotionVoice","weight":2}],"tokens":{}}"#
    );

    // An absent `role` decodes to `Named("")`, so that one binding omits it.
    let anonymous = ThemeManifest {
        roles: vec![RoleBinding {
            role: ManifestRole::Named(String::new()),
            token_name: "t".into(),
        }],
        ..ThemeManifest::default()
    };
    assert_eq!(
        encode(&anonymous),
        r#"{"roles":[{"token":"t"}],"tokens":{}}"#
    );
    assert_eq!(decode(&encode(&anonymous)).unwrap(), anonymous);
}

// The three model states the wire cannot carry. Each is reachable only by
// hand-building a `ThemeManifest` — no decoder or projector in the module
// produces one — so these are recorded negative results rather than defects:
// widening any of them is a wire-format question for every host at once.

fn bare(name: &str, value: &str) -> ManifestToken {
    ManifestToken {
        name: name.into(),
        token_type: String::new(),
        value: value.into(),
        description: None,
        role: None,
    }
}

#[test]
fn colliding_token_paths_resolve_last_write_wins() {
    // A DTCG path addresses a group or a token, never both. The later write wins
    // — the precedence `dedupe_tokens` and `merge` already apply — in BOTH
    // directions, which is the half an implementation gets wrong: descending past
    // a leaf must clear it, or the EARLIER token would win instead.
    let deeper_last = ThemeManifest {
        tokens: vec![bare("a", "1"), bare("a.b", "2")],
        ..ThemeManifest::default()
    };
    assert_eq!(
        encode(&deeper_last),
        r#"{"tokens":{"a":{"b":{"$value":"2"}}}}"#
    );
    assert_eq!(
        decode(&encode(&deeper_last)).unwrap().tokens,
        vec![bare("a.b", "2")]
    );

    let shallower_last = ThemeManifest {
        tokens: vec![bare("a.b", "2"), bare("a", "1")],
        ..ThemeManifest::default()
    };
    assert_eq!(
        encode(&shallower_last),
        r#"{"tokens":{"a":{"$value":"1"}}}"#
    );
    assert_eq!(
        decode(&encode(&shallower_last)).unwrap().tokens,
        vec![bare("a", "1")]
    );
}

#[test]
fn a_dollar_prefixed_first_segment_is_unreachable_to_the_decoder() {
    // `walk_tokens` skips `$`-prefixed keys as DTCG metadata, so such a token is
    // emitted and then not read back. Escaping it would mint wire vocabulary this
    // host may not mint alone.
    let m = ThemeManifest {
        tokens: vec![bare("$meta", "x")],
        ..ThemeManifest::default()
    };
    assert_eq!(encode(&m), r#"{"tokens":{"$meta":{"$value":"x"}}}"#);
    assert!(decode(&encode(&m)).unwrap().tokens.is_empty());
}

#[test]
fn a_tone_outside_the_canonical_palette_decodes_back_as_a_named_role() {
    // `parse_role` validates the tone, so an unrecognised one is a named role on
    // the way back in. A `Named` holding a VALID tone string is unaffected — it
    // travels on the `named` member and returns as itself.
    let bogus = ThemeManifest {
        roles: vec![RoleBinding {
            role: ManifestRole::Tone("Bogus".into()),
            token_name: "t".into(),
        }],
        ..ThemeManifest::default()
    };
    assert_eq!(
        decode(&encode(&bogus)).unwrap().roles[0].role,
        ManifestRole::Named("Bogus".into())
    );

    let named_brand = ThemeManifest {
        roles: vec![RoleBinding {
            role: ManifestRole::Named("Brand".into()),
            token_name: "t".into(),
        }],
        ..ThemeManifest::default()
    };
    assert_eq!(decode(&encode(&named_brand)).unwrap(), named_brand);
}
