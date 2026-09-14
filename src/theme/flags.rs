//! The `StyleFlag` vocabulary + the pure flag-derivation core — "style is read,
//! not looked at". Given a node's SUPPLIED resolved-style facts ([`StyleInput`]:
//! foreground, the background layer stack, the computed font family, the emitted
//! tone), derive the typed [`StyleFlag`]s deterministically. Same input → same
//! flags on every host; no DOM, no pixels, no vision model in the conclusion —
//! the same pure-tier boundary [`crate::introspect::layout`] holds for layout.
//!
//! Two tiers. The three flags derived HERE are MANIFEST-FREE: they need only the
//! resolved colours and the WCAG chain [`crate::theme::contrast`] already carries
//! (compositing → effective background → relative luminance → contrast ratio).
//! The other four are MANIFEST-AWARE and are derived by
//! [`crate::theme::manifest_flags`] against a declared [`ThemeManifest`]; they
//! share this enum because the vocabulary a host reports is one closed set.
//!
//! [`encode_style_flag`] / [`encode_style_observation`] emit canonical JSON
//! byte-identical to the sibling hosts for the same facts, so a Rust or WASM
//! observation can be compared against a Go or Python one as bytes.
//!
//! [`ThemeManifest`]: crate::theme::manifest::ThemeManifest

use crate::theme::contrast::{Rgba, composite, contrast_ratio, effective_background};

// ─── FontRole ────────────────────────────────────────────────────────────────

/// A coarse classification of the computed `font-family` string. The variant's
/// [`FontRole::wire`] string is what the observation encode carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontRole {
    /// No font family was supplied, or it matched no known family token.
    #[default]
    Unknown,
    /// The family names a sans-serif.
    SansSerif,
    /// The family names a serif.
    Serif,
    /// The family names a monospace.
    Monospace,
}

impl FontRole {
    /// The stable wire string for this role.
    pub fn wire(self) -> &'static str {
        match self {
            FontRole::SansSerif => "SansSerif",
            FontRole::Serif => "Serif",
            FontRole::Monospace => "Monospace",
            FontRole::Unknown => "Unknown",
        }
    }
}

// ─── StyleFlag ───────────────────────────────────────────────────────────────

/// One AI-facing legibility interpretation of resolved style. Closed: a `match`
/// over it is exhaustive at compile time, which is the exhaustiveness the
/// sum-type-free sibling hosts recover with a corpus and a linter.
#[derive(Debug, Clone, PartialEq)]
pub enum StyleFlag {
    /// Composited foreground/background WCAG contrast below the AA floor but
    /// still faintly visible — the band
    /// `[invisible_text_threshold, contrast_aa_threshold)`.
    ContrastBelowAA {
        /// The measured contrast ratio.
        ratio: f64,
    },
    /// Contrast at/near 1.0 — text ≈ the surface behind it. The severe subset,
    /// carved out of `ContrastBelowAA` so the two never fire together.
    InvisibleText {
        /// The measured contrast ratio.
        ratio: f64,
    },
    /// A toned element's accent surface contrasts its container below the
    /// UI-component floor — the tint is there, but nobody can see it.
    AccentIndistinct {
        /// The measured accent-surface vs ancestor-surface contrast ratio.
        ratio: f64,
    },
    /// A tone/role the declared manifest has no token for (manifest-aware).
    TokenResolutionFailed {
        /// The unresolvable tone or named role.
        slot: String,
    },
    /// A toned element's resolved fill is not present in the manifest palette
    /// (manifest-aware).
    OffPaletteColour {
        /// The offending fill, as `rgb(r, g, b)`.
        value: String,
    },
    /// A token's surface-area share breached its declared usage budget
    /// (manifest-aware).
    UsageBudgetExceeded {
        /// The token whose budget was breached.
        token: String,
        /// The declared target share, as a percentage.
        declared_pct: f64,
        /// The observed share, as a percentage.
        observed_pct: f64,
    },
    /// A role's resolved contrast is below the manifest's declared per-role
    /// floor (manifest-aware).
    ContrastBelowDeclaredFloor {
        /// The role the floor was declared for.
        role: String,
        /// The measured contrast ratio.
        ratio: f64,
        /// The declared floor.
        floor: f64,
    },
}

impl StyleFlag {
    /// The stable PascalCase discriminator wire string.
    pub fn kind_name(&self) -> &'static str {
        match self {
            StyleFlag::ContrastBelowAA { .. } => "ContrastBelowAA",
            StyleFlag::InvisibleText { .. } => "InvisibleText",
            StyleFlag::AccentIndistinct { .. } => "AccentIndistinct",
            StyleFlag::TokenResolutionFailed { .. } => "TokenResolutionFailed",
            StyleFlag::OffPaletteColour { .. } => "OffPaletteColour",
            StyleFlag::UsageBudgetExceeded { .. } => "UsageBudgetExceeded",
            StyleFlag::ContrastBelowDeclaredFloor { .. } => "ContrastBelowDeclaredFloor",
        }
    }
}

// ─── Encode primitives ───────────────────────────────────────────────────────

/// Two-decimal fixed-point, the number form every host's style encode uses.
/// Rust, Go's `strconv.FormatFloat(x, 'f', 2, 64)` and Python's `f"{x:.2f}"` all
/// round the exact binary double half-to-even, so the bytes agree; the parity
/// test pins that agreement rather than asserting it here.
fn f2(x: f64) -> String {
    format!("{x:.2}")
}

/// The minimal JSON string escape this encode needs (the flag payloads are
/// tone/role/token names and `rgb(…)` strings, never arbitrary text).
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Encode a colour as compact JSON — `{"r":R,"g":G,"b":B,"a":A}`, two decimals.
pub fn encode_rgba(c: Rgba) -> String {
    format!(
        "{{\"r\":{},\"g\":{},\"b\":{},\"a\":{}}}",
        f2(c.r),
        f2(c.g),
        f2(c.b),
        f2(c.a)
    )
}

/// Encode a flag as the AI-friendly tagged-object JSON.
pub fn encode_style_flag(flag: &StyleFlag) -> String {
    match flag {
        StyleFlag::ContrastBelowAA { ratio }
        | StyleFlag::InvisibleText { ratio }
        | StyleFlag::AccentIndistinct { ratio } => {
            format!(
                "{{\"kind\":\"{}\",\"ratio\":{}}}",
                flag.kind_name(),
                f2(*ratio)
            )
        }
        StyleFlag::TokenResolutionFailed { slot } => {
            format!(
                "{{\"kind\":\"TokenResolutionFailed\",\"slot\":\"{}\"}}",
                esc(slot)
            )
        }
        StyleFlag::OffPaletteColour { value } => {
            format!(
                "{{\"kind\":\"OffPaletteColour\",\"value\":\"{}\"}}",
                esc(value)
            )
        }
        StyleFlag::UsageBudgetExceeded {
            token,
            declared_pct,
            observed_pct,
        } => format!(
            "{{\"kind\":\"UsageBudgetExceeded\",\"token\":\"{}\",\"declaredPct\":{},\"observedPct\":{}}}",
            esc(token),
            f2(*declared_pct),
            f2(*observed_pct)
        ),
        StyleFlag::ContrastBelowDeclaredFloor { role, ratio, floor } => format!(
            "{{\"kind\":\"ContrastBelowDeclaredFloor\",\"role\":\"{}\",\"ratio\":{},\"floor\":{}}}",
            esc(role),
            f2(*ratio),
            f2(*floor)
        ),
    }
}

// ─── StyleObservation ────────────────────────────────────────────────────────

/// One resolved-style snapshot for a single addressable node.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleObservation {
    /// The addressable node this observation is about.
    pub node_id: String,
    /// The colour the text actually paints with (declared fg composited over
    /// the effective background).
    pub foreground: Rgba,
    /// The opaque surface the text sits on.
    pub effective_background: Rgba,
    /// The classified computed font family.
    pub font_role: FontRole,
    /// The tone the emitter declared for this node, when it declared one.
    pub emitted_tone: Option<String>,
    /// The WCAG contrast ratio between [`Self::foreground`] and
    /// [`Self::effective_background`].
    pub contrast_ratio: f64,
    /// The derived flags, in derivation order.
    pub flags: Vec<StyleFlag>,
}

/// Encode an observation as canonical JSON, byte-identical to the sibling hosts.
pub fn encode_style_observation(obs: &StyleObservation) -> String {
    let tone = match &obs.emitted_tone {
        None => "null".to_string(),
        Some(t) => format!("\"{}\"", esc(t)),
    };
    let flags = obs
        .flags
        .iter()
        .map(encode_style_flag)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"nodeId\":\"{}\",\"foreground\":{},\"effectiveBackground\":{},\"fontRole\":\"{}\",\"emittedTone\":{},\"contrastRatio\":{},\"flags\":[{}]}}",
        esc(&obs.node_id),
        encode_rgba(obs.foreground),
        encode_rgba(obs.effective_background),
        obs.font_role.wire(),
        tone,
        f2(obs.contrast_ratio),
        flags
    )
}

// ─── Options ─────────────────────────────────────────────────────────────────

/// Host-tunable derivation policy. The defaults pin the standard WCAG floors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleObserverOptions {
    /// The re-derivation debounce a live host applies (carried for parity with
    /// the browser tiers; the pure tier never waits).
    pub debounce_ms: i64,
    /// The AA contrast floor. A ratio below this (and at/above
    /// [`Self::invisible_text_threshold`]) fires
    /// [`StyleFlag::ContrastBelowAA`].
    pub contrast_aa_threshold: f64,
    /// The "text ≈ its surface" floor. A ratio below this fires
    /// [`StyleFlag::InvisibleText`] INSTEAD of `ContrastBelowAA`.
    pub invisible_text_threshold: f64,
    /// The UI-component floor an accent surface must clear against the surface
    /// behind it.
    pub accent_indistinct_threshold: f64,
    /// When set, an observer re-emits only when a node's flag list CHANGED.
    pub emit_on_flag_change_only: bool,
}

impl Default for StyleObserverOptions {
    fn default() -> Self {
        StyleObserverOptions {
            debounce_ms: 100,
            contrast_aa_threshold: 4.5,
            invisible_text_threshold: 1.1,
            accent_indistinct_threshold: 3.0,
            emit_on_flag_change_only: true,
        }
    }
}

// ─── StyleInput ──────────────────────────────────────────────────────────────

/// The abstract evidence envelope the derivation operates on — a node's
/// SUPPLIED resolved style. A browser host fills it from `getComputedStyle`
/// (walking `parentElement` to build [`Self::background_layers`]); a headless
/// host fills it from a fixture. Nothing here reads a DOM.
///
/// An ABSENT fact never fires a flag: no `font_family` classifies as
/// [`FontRole::Unknown`] rather than guessing, and no `emitted_tone` makes every
/// tone-gated check (accent, and the whole manifest-aware tier) silent by
/// construction.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleInput {
    /// The declared foreground colour, before compositing.
    pub foreground: Rgba,
    /// The background layer stack, ELEMENT-FIRST: this node's own background,
    /// then each ancestor's, outward. Empty means "nothing declared", which
    /// composites to the implicit white canvas.
    pub background_layers: Vec<Rgba>,
    /// The computed `font-family` string, when the host could read one.
    pub font_family: Option<String>,
    /// The tone the emitter declared on this node, when it declared one.
    pub emitted_tone: Option<String>,
}

impl Default for StyleInput {
    /// Opaque-black text on the implicit white canvas. Written out rather than
    /// derived because the foreground default is `Rgba::BLACK`, not the
    /// all-zero (fully transparent) `Rgba` a derive would produce.
    fn default() -> Self {
        StyleInput {
            foreground: Rgba::BLACK,
            background_layers: Vec::new(),
            font_family: None,
            emitted_tone: None,
        }
    }
}

/// Opaque-black text on the implicit white canvas, no font, no tone — the
/// baseline a bare `register` creates so a mount hook cannot fail.
pub fn baseline_style_input() -> StyleInput {
    StyleInput::default()
}

// ─── Derived evidence ────────────────────────────────────────────────────────

/// The opaque background the text sits on, after the composite walk.
pub fn resolved_background(input: &StyleInput) -> Rgba {
    effective_background(&input.background_layers)
}

/// The colour the text actually paints with — the declared foreground
/// composited over the effective background.
pub fn resolved_foreground(input: &StyleInput) -> Rgba {
    composite(input.foreground, resolved_background(input))
}

/// The WCAG contrast ratio between the resolved foreground and the effective
/// background.
pub fn contrast(input: &StyleInput) -> f64 {
    contrast_ratio(resolved_foreground(input), resolved_background(input))
}

/// Classify the computed font-family string. The order is normative — a family
/// naming both (`"Fira Mono Sans"`) resolves as monospace on every host.
pub fn font_role_of(input: &StyleInput) -> FontRole {
    let Some(family) = &input.font_family else {
        return FontRole::Unknown;
    };
    let f = family.to_lowercase();
    if f.contains("mono") {
        FontRole::Monospace
    } else if f.contains("sans") {
        FontRole::SansSerif
    } else if f.contains("serif") {
        FontRole::Serif
    } else {
        FontRole::Unknown
    }
}

// ─── Per-flag predicates ─────────────────────────────────────────────────────

/// [`StyleFlag::InvisibleText`] — contrast strictly below the invisible
/// threshold (default 1.1: the text is the surface).
pub fn invisible_text_flag(invisible_threshold: f64, input: &StyleInput) -> Option<StyleFlag> {
    let c = contrast(input);
    (c < invisible_threshold).then_some(StyleFlag::InvisibleText { ratio: c })
}

/// [`StyleFlag::ContrastBelowAA`] — contrast in
/// `[invisible_threshold, aa_threshold)` (defaults `[1.1, 4.5)`). The lower
/// bound is what keeps this disjoint from [`invisible_text_flag`]: a node is
/// reported as invisible or as below-AA, never as both.
pub fn contrast_below_aa_flag(
    invisible_threshold: f64,
    aa_threshold: f64,
    input: &StyleInput,
) -> Option<StyleFlag> {
    let c = contrast(input);
    (invisible_threshold <= c && c < aa_threshold)
        .then_some(StyleFlag::ContrastBelowAA { ratio: c })
}

/// [`StyleFlag::AccentIndistinct`] — a toned element's own fill barely contrasts
/// the surface behind it (default floor 3.0, the WCAG UI-component floor).
///
/// Three absent facts each keep it silent, by construction rather than by a
/// threshold: no declared tone (an untoned node has no accent to judge), no
/// background layers at all, and a fully transparent own layer (there is no
/// accent surface to contrast).
pub fn accent_indistinct_flag(accent_threshold: f64, input: &StyleInput) -> Option<StyleFlag> {
    input.emitted_tone.as_ref()?;
    let own = *input.background_layers.first()?;
    if own.a <= 0.0 {
        return None;
    }
    let accent_surface = resolved_background(input);
    let ancestor_surface = effective_background(&input.background_layers[1..]);
    let c = contrast_ratio(accent_surface, ancestor_surface);
    (c < accent_threshold).then_some(StyleFlag::AccentIndistinct { ratio: c })
}

/// Derive the manifest-free flag list for one input. The order is normative —
/// invisible, then below-AA, then accent — because a flag list is compared as a
/// sequence (see [`flags_equal`]) and encoded as a JSON array.
pub fn derive_style_flags(options: &StyleObserverOptions, input: &StyleInput) -> Vec<StyleFlag> {
    let mut out = Vec::new();
    if let Some(f) = invisible_text_flag(options.invisible_text_threshold, input) {
        out.push(f);
    }
    if let Some(f) = contrast_below_aa_flag(
        options.invisible_text_threshold,
        options.contrast_aa_threshold,
        input,
    ) {
        out.push(f);
    }
    if let Some(f) = accent_indistinct_flag(options.accent_indistinct_threshold, input) {
        out.push(f);
    }
    out
}

/// Build a fully-populated manifest-free observation — the shared shape every
/// observer emits.
pub fn to_style_observation(
    options: &StyleObserverOptions,
    node_id: &str,
    input: &StyleInput,
) -> StyleObservation {
    StyleObservation {
        node_id: node_id.to_string(),
        foreground: resolved_foreground(input),
        effective_background: resolved_background(input),
        font_role: font_role_of(input),
        emitted_tone: input.emitted_tone.clone(),
        contrast_ratio: contrast(input),
        flags: derive_style_flags(options, input),
    }
}

/// Order-sensitive flag-list equality (the derivation order above).
pub fn flags_equal(a: &[StyleFlag], b: &[StyleFlag]) -> bool {
    a == b
}

// ─── Palette colour helpers ──────────────────────────────────────────────────

/// RGB equality after rounding each channel half-to-even, alpha IGNORED — the
/// palette-membership test the manifest-aware tier uses. Half-to-even matches
/// Python's `round()` and Go's `math.RoundToEven`, so the three hosts agree on
/// a `.5` channel.
pub fn same_rgb(a: Rgba, b: Rgba) -> bool {
    a.r.round_ties_even() == b.r.round_ties_even()
        && a.g.round_ties_even() == b.g.round_ties_even()
        && a.b.round_ties_even() == b.b.round_ties_even()
}

/// Parse a CSS hex colour (`#rgb` / `#rrggbb` / `#rrggbbaa`), or `None`.
///
/// The 3- and 6-digit arms delegate to [`crate::theme::manifest::parse_hex`] —
/// there is one hex parser in this crate and this is not a second one. The
/// 8-digit arm is the difference, and it is load-bearing rather than
/// decorative: `parse_hex` is the manifest tier's *opaque-colour* projection and
/// declines an alpha channel, while both sibling hosts' palette parse accepts
/// `#rrggbbaa` and then compares RGB with alpha ignored ([`same_rgb`]). A
/// manifest declaring `#20304080` is therefore ON-palette on every host, and
/// reusing `parse_hex` alone here would have made this host disagree.
pub fn try_parse_hex(raw: &str) -> Option<Rgba> {
    let s = raw.trim().trim_start_matches('#');
    if s.len() == 8 {
        let byte = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok().map(f64::from);
        return Some(Rgba {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: byte(6)? / 255.0,
        });
    }
    crate::theme::manifest::parse_hex(&format!("#{s}"))
}
