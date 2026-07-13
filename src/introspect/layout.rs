//! Layout-flag derivation — "layout is read, not looked at". Given a node's
//! measured geometry (`LayoutInput`, sampled from a laid-out DOM by the host),
//! derive the typed structural `LayoutFlag`s deterministically. Same input →
//! same output on every host; the derivation carries no pixels, only the
//! structural conclusion ("this overflows on a phone"). This is the machine-
//! readable layout signal behind the Blind Surveyor, the geometric Unit-Test
//! assertion, and Kintsugi's overflow sense — a faithful port of the cross-host
//! `LayoutObserver.Flags.derive`.

/// A structural layout finding derived from measured geometry.
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutFlag {
    /// Content extends beyond the paint region horizontally AND the element
    /// clips (computed `overflow-x` is not `visible`).
    OverflowHorizontal,
    /// The vertical counterpart.
    OverflowVertical,
    /// A rendered dimension collapsed to ≤ 0.5px (the named axis).
    ZeroDimension(&'static str),
    /// A rendered dimension is within 0.5px of its computed minimum.
    SqueezedToMin(&'static str),
    /// The element's rect extends beyond a clipping ancestor's rect.
    ChildClippedByAncestor,
    /// The observed width/height ratio diverges from the expected by the
    /// carried magnitude factor (always ≥ 1.0, direction-agnostic).
    AspectRatioWildlyOff(f64),
}

/// The measured geometry of one element (sampled from a laid-out DOM). Optional
/// fields are absent when the host could not measure them — a missing measure
/// never fires a flag.
#[derive(Debug, Clone, Default)]
pub struct LayoutInput {
    pub width: f64,
    pub height: f64,
    pub scroll_width: Option<f64>,
    pub client_width: Option<f64>,
    pub scroll_height: Option<f64>,
    pub client_height: Option<f64>,
    /// The computed `overflow-x` keyword (`"visible"`, `"hidden"`, `"auto"`, …).
    pub overflow_x: Option<String>,
    pub overflow_y: Option<String>,
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
    /// `(left, top, right, bottom)` of this element's bounding rect.
    pub element_rect: (f64, f64, f64, f64),
    /// The nearest clipping ancestor's rect, when one was found.
    pub clipping_ancestor_rect: Option<(f64, f64, f64, f64)>,
    pub expected_aspect_ratio: Option<f64>,
}

/// Derivation options (the aspect-ratio threshold factor).
#[derive(Debug, Clone, Copy)]
pub struct LayoutOptions {
    pub aspect_ratio_wildly_off_factor: f64,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        // The reference default factor.
        LayoutOptions {
            aspect_ratio_wildly_off_factor: 3.0,
        }
    }
}

fn overflow_horizontal(input: &LayoutInput) -> Option<LayoutFlag> {
    match (input.scroll_width, input.client_width, &input.overflow_x) {
        (Some(sw), Some(cw), Some(ox)) if sw > cw && ox != "visible" => {
            Some(LayoutFlag::OverflowHorizontal)
        }
        _ => None,
    }
}

fn overflow_vertical(input: &LayoutInput) -> Option<LayoutFlag> {
    match (input.scroll_height, input.client_height, &input.overflow_y) {
        (Some(sh), Some(ch), Some(oy)) if sh > ch && oy != "visible" => {
            Some(LayoutFlag::OverflowVertical)
        }
        _ => None,
    }
}

fn zero_dimension(input: &LayoutInput) -> Vec<LayoutFlag> {
    let mut out = Vec::new();
    if input.width <= 0.5 {
        out.push(LayoutFlag::ZeroDimension("width"));
    }
    if input.height <= 0.5 {
        out.push(LayoutFlag::ZeroDimension("height"));
    }
    out
}

fn squeezed_to_min(input: &LayoutInput) -> Vec<LayoutFlag> {
    let mut out = Vec::new();
    if let Some(mw) = input.min_width
        && mw > 0.0
        && (input.width - mw).abs() <= 0.5
    {
        out.push(LayoutFlag::SqueezedToMin("width"));
    }
    if let Some(mh) = input.min_height
        && mh > 0.0
        && (input.height - mh).abs() <= 0.5
    {
        out.push(LayoutFlag::SqueezedToMin("height"));
    }
    out
}

fn child_clipped(input: &LayoutInput) -> Option<LayoutFlag> {
    let (anc_l, anc_t, anc_r, anc_b) = input.clipping_ancestor_rect?;
    let (el_l, el_t, el_r, el_b) = input.element_rect;
    if el_l < anc_l - 0.5 || el_t < anc_t - 0.5 || el_r > anc_r + 0.5 || el_b > anc_b + 0.5 {
        Some(LayoutFlag::ChildClippedByAncestor)
    } else {
        None
    }
}

fn aspect_ratio_off(factor_threshold: f64, input: &LayoutInput) -> Option<LayoutFlag> {
    let expected = input.expected_aspect_ratio?;
    if expected <= 0.0 || input.height <= 0.0 || input.width <= 0.0 {
        return None;
    }
    let observed = input.width / input.height;
    let ratio = observed / expected;
    let magnitude = if ratio >= 1.0 { ratio } else { 1.0 / ratio };
    (magnitude >= factor_threshold).then_some(LayoutFlag::AspectRatioWildlyOff(magnitude))
}

/// Derive the full flag list for one measured element. Deterministic order
/// (overflow-h, overflow-v, zero-dim, squeezed, clipped, aspect) so cross-host
/// comparisons stay stable.
pub fn derive(options: LayoutOptions, input: &LayoutInput) -> Vec<LayoutFlag> {
    let mut flags = Vec::new();
    flags.extend(overflow_horizontal(input));
    flags.extend(overflow_vertical(input));
    flags.extend(zero_dimension(input));
    flags.extend(squeezed_to_min(input));
    flags.extend(child_clipped(input));
    flags.extend(aspect_ratio_off(
        options.aspect_ratio_wildly_off_factor,
        input,
    ));
    flags
}
