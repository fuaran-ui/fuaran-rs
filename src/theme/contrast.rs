//! WCAG contrast derivation — the "observable accessibility" mechanic behind
//! the Infinite Skins live contrast auditor and Kintsugi's contrast sense. A
//! host reads whether text is legible on its background from the resolved
//! colours (which the demo samples back from computed styles), deriving the
//! WCAG contrast ratio + AA/AAA verdict deterministically — a faithful port of
//! the cross-host `StyleObserver` contrast derivation (alpha compositing →
//! effective background → relative luminance → contrast ratio).

/// An sRGB colour: 0–255 channels + a 0.0–1.0 alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub const fn rgb(r: f64, g: f64, b: f64) -> Self {
        Rgba { r, g, b, a: 1.0 }
    }

    pub const WHITE: Rgba = Rgba::rgb(255.0, 255.0, 255.0);
    pub const BLACK: Rgba = Rgba::rgb(0.0, 0.0, 0.0);
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    fn is_opaque(self) -> bool {
        self.a >= 1.0
    }
}

/// Source-over composite of `top` (with its alpha) over `bottom` — the standard
/// premultiplied-then-normalised alpha blend.
pub fn composite(top: Rgba, bottom: Rgba) -> Rgba {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 0.0 {
        return Rgba::TRANSPARENT;
    }
    let blend = |tc: f64, bc: f64| (tc * top.a + bc * bottom.a * (1.0 - top.a)) / a;
    Rgba {
        r: blend(top.r, bottom.r),
        g: blend(top.g, bottom.g),
        b: blend(top.b, bottom.b),
        a,
    }
}

/// Composite a background layer stack (element-first) down to the first opaque
/// layer, returning the opaque colour the text sits on. When no layer is
/// opaque, an opaque-white base (the browser's default canvas) is appended.
pub fn effective_background(layers: &[Rgba]) -> Rgba {
    // Truncate at (and including) the first opaque layer.
    let mut truncated: Vec<Rgba> = Vec::new();
    let mut found_opaque = false;
    for &layer in layers {
        truncated.push(layer);
        if layer.is_opaque() {
            found_opaque = true;
            break;
        }
    }
    if !found_opaque {
        truncated.push(Rgba::WHITE);
    }
    // Composite from the opaque base upward to the element layer.
    let Some((base, above_bottom_up)) = truncated.split_last() else {
        return Rgba::WHITE;
    };
    above_bottom_up
        .iter()
        .rev()
        .fold(*base, |acc, &top| composite(top, acc))
}

/// WCAG relative luminance of an (assumed opaque) colour — sRGB channels
/// linearised then weighted.
pub fn relative_luminance(c: Rgba) -> f64 {
    let channel = |v: f64| {
        let s = v / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// WCAG contrast ratio between two opaque colours — `(L_lighter + 0.05) /
/// (L_darker + 0.05)`, in `1.0` (identical) … `21.0` (black-on-white).
pub fn contrast_ratio(a: Rgba, b: Rgba) -> f64 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The WCAG contrast ratio of a foreground over a background layer stack (the
/// foreground is composited over the effective opaque background first).
pub fn foreground_contrast(foreground: Rgba, background_layers: &[Rgba]) -> f64 {
    let bg = effective_background(background_layers);
    let fg = composite(foreground, bg);
    contrast_ratio(fg, bg)
}

// ─── WCAG verdict ────────────────────────────────────────────────────────────

/// WCAG AA/AAA thresholds. Normal text: AA 4.5, AAA 7.0. Large text
/// (≥ 18pt, or ≥ 14pt bold): AA 3.0, AAA 4.5.
pub const AA_NORMAL: f64 = 4.5;
pub const AA_LARGE: f64 = 3.0;
pub const AAA_NORMAL: f64 = 7.0;
pub const AAA_LARGE: f64 = 4.5;

/// A contrast verdict for a resolved foreground/background pair — the value the
/// Infinite Skins auditor renders per theme, and the `ContrastBelowAA` sense
/// Kintsugi reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContrastVerdict {
    pub ratio: f64,
    pub aa_normal: bool,
    pub aa_large: bool,
    pub aaa_normal: bool,
    pub aaa_large: bool,
}

impl ContrastVerdict {
    /// The verdict for a ratio.
    pub fn of(ratio: f64) -> Self {
        ContrastVerdict {
            ratio,
            aa_normal: ratio >= AA_NORMAL,
            aa_large: ratio >= AA_LARGE,
            aaa_normal: ratio >= AAA_NORMAL,
            aaa_large: ratio >= AAA_LARGE,
        }
    }

    /// `true` when the pair fails even the most lenient WCAG bar (AA large) —
    /// the `ContrastBelowAA` structural defect a machine flags without pixels.
    pub fn below_aa_large(self) -> bool {
        !self.aa_large
    }
}

/// The contrast verdict for a foreground over a background layer stack.
pub fn verdict(foreground: Rgba, background_layers: &[Rgba]) -> ContrastVerdict {
    ContrastVerdict::of(foreground_contrast(foreground, background_layers))
}
