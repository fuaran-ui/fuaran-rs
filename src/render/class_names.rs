//! The load-bearing class-name contract: the `fuaran-*` vocabulary this
//! renderer emits is **parity-locked** to the reference renderers — a
//! class-name change is a cross-host change, never a local edit.

use crate::wire::{
    BoxLayout, BoxRole, BoxSpec, Emphasis, FontVoice, IconSize, NodeKind, SemanticStyle, StyleRole,
    StyleWeight, ToneVariant, TrendPolarity,
};

/// Map a `ToneVariant` to its CSS-variable name root.
pub fn tone_var(tone: ToneVariant) -> &'static str {
    match tone {
        ToneVariant::Default => "default",
        ToneVariant::Subdued => "subdued",
        ToneVariant::Brand => "brand",
        ToneVariant::Success => "success",
        ToneVariant::Warning => "warning",
        ToneVariant::Critical => "critical",
        ToneVariant::Info => "info",
    }
}

/// Phase 821 — map an `IconSize` to the `fuaran-icon--<size>` modifier-class
/// fragment for the standalone `Icon` display kind. Parity-locked to the
/// reference renderers' shared size-class map.
pub fn icon_size_class(size: IconSize) -> &'static str {
    match size {
        IconSize::Small => "small",
        IconSize::Medium => "medium",
        IconSize::Large => "large",
    }
}

pub fn weight_var(weight: StyleWeight) -> &'static str {
    match weight {
        StyleWeight::Compact => "compact",
        StyleWeight::Standard => "standard",
        StyleWeight::Spacious => "spacious",
    }
}

pub fn emphasis_var(emphasis: Emphasis) -> &'static str {
    match emphasis {
        Emphasis::Quiet => "quiet",
        Emphasis::Normal => "normal",
        Emphasis::Loud => "loud",
    }
}

/// `fuaran-role-{suffix}` class suffix; the default (`None`) emits nothing.
pub fn style_role_var(role: StyleRole) -> Option<&'static str> {
    match role {
        StyleRole::None => None,
        StyleRole::Eyebrow => Some("eyebrow"),
        StyleRole::Data => Some("data"),
        StyleRole::Lede => Some("lede"),
        StyleRole::Caption => Some("caption"),
    }
}

/// `fuaran-voice-{suffix}` class suffix; the default emits nothing.
pub fn font_voice_var(voice: FontVoice) -> Option<&'static str> {
    match voice {
        FontVoice::Default => None,
        FontVoice::Display => Some("display"),
        FontVoice::Structural => Some("structural"),
    }
}

/// The semantic-style className fragment attached to every rendered element.
pub fn style_class_name(style: &SemanticStyle) -> String {
    let mut out = format!(
        "fuaran-node fuaran-tone-{} fuaran-weight-{} fuaran-emphasis-{}",
        tone_var(style.tone),
        weight_var(style.weight),
        emphasis_var(style.emphasis)
    );
    if let Some(role) = style_role_var(style.role) {
        out.push_str(" fuaran-role-");
        out.push_str(role);
    }
    if let Some(voice) = font_voice_var(style.voice) {
        out.push_str(" fuaran-voice-");
        out.push_str(voice);
    }
    out
}

/// Sanitise a moduleId / componentId fragment for use inside a class name.
fn sanitise_class_fragment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_run = false;
    for c in raw.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out
}

/// The per-kind class hook for the unified `Box` container, derived from
/// role + layout so each retired kind's `fuaran-kind-*` hook is preserved.
fn box_kind_class(spec: &BoxSpec) -> &'static str {
    if spec.role == BoxRole::Card {
        "fuaran-kind-card"
    } else if spec.role == BoxRole::Dashboard || matches!(spec.layout, BoxLayout::Auto) {
        "fuaran-kind-dashboard"
    } else if spec.role == BoxRole::Separator {
        "fuaran-kind-divider"
    } else if matches!(spec.layout, BoxLayout::Grid { .. }) {
        "fuaran-kind-grid-layout"
    } else {
        "fuaran-kind-stack"
    }
}

/// Per-`NodeKind` class fragment (e.g. `fuaran-kind-metric`).
pub fn kind_class(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Box(spec) => box_kind_class(spec).to_string(),
        NodeKind::SplitPanel(_) => "fuaran-kind-split-panel".to_string(),
        NodeKind::Tabs(_) => "fuaran-kind-tabs".to_string(),
        NodeKind::Stepper(_) => "fuaran-kind-stepper".to_string(),
        NodeKind::SummaryList(_) => "fuaran-kind-summary-list".to_string(),
        NodeKind::Disclosure(_) => "fuaran-kind-disclosure".to_string(),
        NodeKind::Modal(_) => "fuaran-kind-modal".to_string(),
        NodeKind::ScrollArea(_) => "fuaran-kind-scroll-area".to_string(),
        NodeKind::Heading(_) => "fuaran-kind-heading".to_string(),
        NodeKind::Markdown(_) => "fuaran-kind-markdown".to_string(),
        NodeKind::Metric(_) => "fuaran-kind-metric".to_string(),
        NodeKind::Badge(_) => "fuaran-kind-badge".to_string(),
        NodeKind::Sparkline(_) => "fuaran-kind-sparkline".to_string(),
        NodeKind::Callout(_) => "fuaran-kind-callout".to_string(),
        NodeKind::Progress(_) => "fuaran-kind-progress".to_string(),
        NodeKind::Skeleton(_) => "fuaran-kind-skeleton".to_string(),
        NodeKind::Icon(_) => "fuaran-kind-icon".to_string(),
        NodeKind::LabelValueRow(_) => "fuaran-kind-label-value-row".to_string(),
        NodeKind::Fact(_) => "fuaran-kind-fact".to_string(),
        NodeKind::Link(_) => "fuaran-kind-link".to_string(),
        NodeKind::Image(_) => "fuaran-kind-image".to_string(),
        NodeKind::List(_) => "fuaran-kind-list".to_string(),
        NodeKind::Toast(_) => "fuaran-kind-toast".to_string(),
        NodeKind::CodeBlock(_) => "fuaran-kind-code-block".to_string(),
        NodeKind::Math(_) => "fuaran-kind-math".to_string(),
        NodeKind::Drawing(_) => "fuaran-kind-drawing".to_string(),
        NodeKind::Form(_) => "fuaran-kind-form".to_string(),
        NodeKind::Filters(_) => "fuaran-kind-filters".to_string(),
        NodeKind::Button(_) => "fuaran-kind-button".to_string(),
        NodeKind::FileUpload(_) => "fuaran-kind-file-upload".to_string(),
        NodeKind::Select(_) => "fuaran-kind-select".to_string(),
        NodeKind::DataGrid(_) => "fuaran-kind-grid".to_string(),
        NodeKind::Chart(_) => "fuaran-kind-chart".to_string(),
        NodeKind::Map(_) => "fuaran-kind-map".to_string(),
        NodeKind::Custom(spec) => format!(
            "fuaran-kind-custom fuaran-custom-{}-{}",
            sanitise_class_fragment(&spec.module_id),
            sanitise_class_fragment(&spec.component_id)
        ),
        NodeKind::ErrorBoundary(_) => "fuaran-kind-error-boundary".to_string(),
        NodeKind::Switch(_) => "fuaran-kind-switch".to_string(),
        NodeKind::FragmentDecl(_) => "fuaran-kind-fragment-decl".to_string(),
        NodeKind::FragmentRef(_) => "fuaran-kind-fragment-ref".to_string(),
        NodeKind::Mount(_) => "fuaran-kind-mount".to_string(),
    }
}

/// Compose the full className for a node: kind class + semantic style.
pub fn node_class_name(kind: &NodeKind, style: &SemanticStyle) -> String {
    format!("{} {}", kind_class(kind), style_class_name(style))
}

/// Phase 867 — the `Metric` trend's SENTIMENT and its glyph, composed from the
/// resolved trend and the declared polarity per `WIRE_FORMAT.md` §3.6.1:
/// `sentiment = sign(trend) × polarity`.
///
/// Returns `(sentiment, glyph)` — the sentiment names the
/// `fuaran-metric-trend-<sentiment>` modifier class AND the glyph's
/// `aria-label`, so a renderer cannot emit one without the other.
///
/// `tone` IS NOT AN INPUT AND IS NEVER AN OUTPUT. It colours the tile and says
/// how the reading STANDS; this says which way the quantity MOVED, and a host
/// derives neither from the other. A renderer that inferred "improving ⇒ the
/// tile is `Success`" would re-create in the render the exact conflation the
/// wire slot was added to remove, and would override an emitter's deliberate
/// `Critical` on a metric improving from a bad place.
///
/// The glyph tracks the SENTIMENT, not the number's direction: under
/// `LowerIsBetter` the triangle deliberately disagrees with the sign, and that
/// disagreement is the visible evidence the declaration was honoured. The
/// numeric text — sign included — is never touched by this function.
///
/// Glyphs are U+25B2 BLACK UP-POINTING TRIANGLE, U+25BC BLACK DOWN-POINTING
/// TRIANGLE and U+2192 RIGHTWARDS ARROW, named here so a mojibake in this file
/// is a diff a reviewer can catch rather than a rendered byte nobody pinned.
/// They carry the sentiment on a NON-COLOUR channel (WCAG 1.4.1 — colour alone
/// fails), which is why this returns the label as well as the class fragment.
///
/// Parity-locked to the reference renderers' shared helper: both emitted
/// strings are cross-host contract, never a local naming choice.
pub fn trend_sentiment(polarity: TrendPolarity, trend: f64) -> (&'static str, &'static str) {
    let direction = match polarity {
        TrendPolarity::HigherIsBetter => 1.0,
        TrendPolarity::LowerIsBetter => -1.0,
    };
    let sentiment = trend * direction;
    // Ordered `> 0` / `< 0` / else, so a NaN trend (which compares false against
    // both) lands on `unchanged` rather than on whichever branch a total
    // ordering would have picked — matching the reference's `if/elif/else`.
    if sentiment > 0.0 {
        ("improving", "\u{25B2}")
    } else if sentiment < 0.0 {
        ("regressing", "\u{25BC}")
    } else {
        ("unchanged", "\u{2192}")
    }
}
