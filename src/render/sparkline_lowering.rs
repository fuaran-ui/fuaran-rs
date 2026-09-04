//! `Sparkline → Drawing` lowering (Phase 1099, the cross-host parity leg of the
//! reference lowering Phase 1098 landed).
//!
//! A `Sparkline` carries a bare bound series and nothing else, so every host
//! that draws one has to turn that series into geometry. Before this module
//! that arithmetic was written out a second time inside the server renderer's
//! own `<polyline>` builder — one of the three hand-written copies the Phase 644
//! §4k inventory found across the render surfaces, sharing no code while
//! claiming byte-identical output. This module is the single Rust placement of
//! that geometry, emitted as a canonical [`DrawingSpec`] and painted through the
//! SAME `Drawing` builder the `Drawing` and lowered-`Chart` arms already use, so
//! this host's picture agrees with every other host's **by construction** rather
//! than by two copies kept in step.
//!
//! **The contract is the corpus, not this file.** The language-neutral
//! `wire-format-fixtures/sparkline-lowering/*` family (authored by the reference
//! host) pins the canvas, the flat-series guard, the centring rule for a lone
//! point, the rounding and the chrome; `tests/sparkline_lowering.rs` certifies
//! every committed vector byte-for-byte. A change to the geometry here that the
//! corpus does not carry is a divergence, not an improvement.
//!
//! **Non-finite values are deliberately NOT special-cased.** A `"NaN"` /
//! `"Infinity"` / `"-Infinity"` sentinel propagates through the arithmetic and
//! reaches the wire as the same sentinel, exactly as the reference emits it —
//! this input class is where a hand-copied path drifts first, and the
//! `nonfinite-sentinel` vector is what turns such a drift into a failing test.

use crate::wire::{DrawPoint, DrawingSpec, Shape, ViewBox};

use super::chart_lowering::{r2, style_stroke};

/// The sparkline canvas — 100 × 30 user units, the viewBox every host has
/// emitted since the kind shipped.
fn sparkline_view_box() -> ViewBox {
    ViewBox {
        min_x: 0.0,
        min_y: 0.0,
        width: 100.0,
        height: 30.0,
    }
}

/// The shipped sparkline stroke width. Corpus-pinned by `sparkline-lowering/*`.
const STROKE_WIDTH: f64 = 1.5;

/// The vertical inset the shipped geometry keeps at each edge, so a peak or a
/// trough is not clipped by the stroke's own width.
const INSET: f64 = 1.0;

/// The plotted height — the canvas less one inset at each edge.
const PLOT_HEIGHT: f64 = 28.0;

/// The shipped flat-series guard: a range below this is treated as `1.0`, which
/// places a constant series on its own line rather than dividing by zero.
const FLAT_EPSILON: f64 = 1e-9;

/// The series minimum, reproducing the reference host's `Array.min` exactly: a
/// left fold seeded on the FIRST element, keeping a candidate only when it
/// compares strictly less.
///
/// Written out rather than reached for through `f64::min` because the two differ
/// on `NaN` — `f64::min` returns the OTHER operand, where a `<` comparison
/// against `NaN` is always false and so keeps the accumulator. The corpus's one
/// non-finite vector happens to agree either way; a series LEADING with a
/// sentinel does not, and the reference's answer is the one every host owes.
fn series_min(values: &[f64]) -> f64 {
    let mut acc = values[0];
    for &v in &values[1..] {
        if v < acc {
            acc = v;
        }
    }
    acc
}

/// The series maximum — `series_min`'s mirror, and `Array.max`'s.
fn series_max(values: &[f64]) -> f64 {
    let mut acc = values[0];
    for &v in &values[1..] {
        if v > acc {
            acc = v;
        }
    }
    acc
}

/// Lower a resolved `Sparkline` series to the canonical `DrawingSpec` every
/// conformant host reproduces byte-for-byte — or `None` when there is nothing to
/// draw, which is the caller's cue to render its own declared fallback element.
///
/// **`None` is a claim, not an absence.** The empty-series fallback — the
/// `fuaran-sparkline-empty` hook carrying an em-dash — is a HOST element rather
/// than a `Shape`, so a lowering structurally cannot express it and must not
/// pretend to by returning an empty canvas nobody can read. The corpus says the
/// same thing in its own vocabulary: `empty.expected.json` is the JSON literal
/// `null`.
///
/// The reference pairs this with a total `lowerSparkline` yielding that empty
/// canvas, for callers other than a renderer. This host has no such caller, so
/// the total form is deliberately not ported — an unreachable second entry point
/// is a surface to keep in step for nothing.
///
/// The reference also takes the `SparklineSpec` alongside the series, and there
/// the parameter carries a null guard: F#'s `Binding.Static None` resolves a list
/// slot to `null` rather than to the empty list. Rust's `&[f64]` cannot be null,
/// so the guard has no shape to take here and the parameter would be unused.
pub fn try_lower_sparkline(series: &[f64]) -> Option<DrawingSpec> {
    if series.is_empty() {
        return None;
    }
    let n = series.len();
    let min_v = series_min(series);
    let max_v = series_max(series);
    let range = if max_v - min_v < FLAT_EPSILON {
        1.0
    } else {
        max_v - min_v
    };
    let points: Vec<DrawPoint> = series
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = if n <= 1 {
                50.0
            } else {
                i as f64 / (n - 1) as f64 * 100.0
            };
            let y = sparkline_view_box().height - (v - min_v) / range * PLOT_HEIGHT - INSET;
            DrawPoint { x: r2(x), y: r2(y) }
        })
        .collect();
    Some(DrawingSpec {
        view_box: sparkline_view_box(),
        // No generated title or description: a sparkline has no spec to derive a
        // summary from, so it carries no accessible name of its own. The corpus
        // pins the absence, so adding one later is a deliberate cross-host act.
        title: None,
        description: None,
        style: crate::wire::DrawStyle::default(),
        shapes: vec![Shape::Polyline {
            points,
            style: style_stroke("currentColor", STROKE_WIDTH),
        }],
    })
}
