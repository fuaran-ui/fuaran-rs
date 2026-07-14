//! Chart → Drawing lowering (Phase 551, lower-in-host posture for fuaran-rs).
//!
//! `Chart` is a *semantic* wire kind; it must be *lowered* to a `Drawing`
//! subtree before it can paint. This module is the Rust port of the canonical
//! F# reference lowering (`Fuaran.UI.Charts.lower`, themed per the theme-aware
//! chart-lowering tier) — the bounded layout engine that turns a resolved
//! `ChartSpec` + data rows into a canonical [`DrawingSpec`] (scales, ticks,
//! axes, gridlines, legend, series geometry).
//!
//! **Why fuaran-rs lowers in-host (unlike the headless fuaran-go host).**
//! fuaran-rs ships a browser-native `wasm32` client that *renders* — through the
//! very same server renderer. Lowering a raw `Chart` to a `Drawing` here means a
//! chart reaches the browser as first-party inline SVG, so the WASM client hits
//! parity with the "Chart-as-data" demo without a bespoke per-host chart drawer.
//! A headless host that paints nothing has no such claim; this one does.
//!
//! **Determinism (the byte-parity contract).** A fixed pixel `viewBox`, a
//! `{1,2,5}·10ⁿ` nice-tick rule, and round-half-up-to-2dp coordinate rounding,
//! so the output depends only on the `ChartSpec` + data — never on enumeration
//! order or platform float printing. The F# output is the golden; the shared
//! `wire-format-fixtures/chart-lowering/*` corpus pins this port byte-identical
//! to it (see `tests/chart_lowering.rs`). Only `Bar` and `Line` lower; any other
//! `ChartKind` produces an empty drawing (its rule lands with its own tier).

use crate::canonical::format_number;
use crate::wire::{
    Binding, ChartKind, DrawPoint, DrawStyle, DrawingSpec, Emphasis, Shape, StaticValue,
    TextAnchor, TextSource, ViewBox,
};

// ─── Layout constants (the fixed canonical drawing space) ────────────────────

const W: f64 = 640.0;
const H: f64 = 400.0;
const MARGIN_TOP: f64 = 64.0; // title + legend band
const MARGIN_RIGHT: f64 = 28.0;
const MARGIN_BOTTOM: f64 = 56.0; // x-axis category labels + x-axis title
const MARGIN_LEFT: f64 = 64.0; // right-aligned y-axis tick labels

const PLOT_X0: f64 = MARGIN_LEFT;
const PLOT_X1: f64 = W - MARGIN_RIGHT;
const PLOT_Y0: f64 = MARGIN_TOP;
const PLOT_Y1: f64 = H - MARGIN_BOTTOM;
const PLOT_W: f64 = PLOT_X1 - PLOT_X0;
const PLOT_H: f64 = PLOT_Y1 - PLOT_Y0;

/// A fixed, deterministic categorical palette (series index → colour).
const PALETTE: [&str; 6] = [
    "#3366cc", "#dc3912", "#ff9900", "#109618", "#990099", "#0099c6",
];

fn colour_for(i: usize) -> &'static str {
    PALETTE[i % PALETTE.len()]
}

// ─── Surface-relative ink (theme-aware lowering) ─────────────────────────────
//
// Structural + text ink is `currentColor` at a per-role opacity, so a lowered
// chart inks from the surface's own text colour and is legible on a light OR a
// dark surface without a CSS override. Series (data) colours stay hex — they
// must stay distinct + read on both surfaces.
const INK: &str = "currentColor";
const AXIS_OPACITY: f64 = 0.8;
const GRID_OPACITY: f64 = 0.12;
const LABEL_OPACITY: f64 = 0.66;

/// The chart's own font stack — carried in the wire, so a lowered chart is
/// self-contained + legible on every host without host CSS.
const CHART_FONT: &str = "system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif";

// ─── Deterministic numeric helpers ───────────────────────────────────────────

/// Round-half-up to 2 dp — a single deterministic rule every host reproduces
/// (avoids banker's-rounding / platform float-print divergence).
fn r2(x: f64) -> f64 {
    (x * 100.0 + 0.5).floor() / 100.0
}

/// A "nice" number for the magnitude of `x` — the classic `{1,2,5}·10ⁿ`
/// selection used for axis ticks.
fn nice_num(x: f64, round_it: bool) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let exp = x.log10().floor();
    let f = x / 10f64.powf(exp);
    let nf = if round_it {
        if f < 1.5 {
            1.0
        } else if f < 3.0 {
            2.0
        } else if f < 7.0 {
            5.0
        } else {
            10.0
        }
    } else if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    nf * 10f64.powf(exp)
}

/// A nice value domain + its tick values for `[lo, hi]`, targeting ~5 ticks.
fn nice_domain(lo: f64, hi: f64) -> (f64, f64, Vec<f64>) {
    let hi = if hi == lo { lo + 1.0 } else { hi };
    let target_ticks = 5.0;
    let range = nice_num(hi - lo, false);
    let step = nice_num(range / (target_ticks - 1.0), true);
    let nice_lo = (lo / step).floor() * step;
    let nice_hi = (hi / step).ceil() * step;
    // Enumerate ticks by integer count (float accumulation would drift).
    let count = ((nice_hi - nice_lo) / step).round() as i64;
    let ticks = (0..=count).map(|i| r2(nice_lo + i as f64 * step)).collect();
    (nice_lo, nice_hi, ticks)
}

/// Canonical SVG number form for a stored tick-label string — a whole value
/// drops the decimal (`10`), else the invariant shortest round-trip (`1.5`).
/// Matches the reference `DrawingSvg.formatNum` so ticks read consistently.
fn format_num(n: f64) -> String {
    if n.is_nan() || n.is_infinite() {
        "0".to_string()
    } else if n == n.floor() && n.abs() < 1e15 {
        (n as i64).to_string()
    } else {
        format_number(n)
    }
}

/// Format a tick value: whole → integer, else 2-dp trimmed.
fn tick_label(v: f64) -> String {
    format_num(r2(v))
}

// ─── DrawStyle builders ──────────────────────────────────────────────────────

fn static_str(v: &str) -> Binding {
    Binding::Static {
        value: StaticValue::Ast(crate::canonical::JVal::Str(v.to_string())),
    }
}

fn static_num(v: f64) -> Binding {
    Binding::Static {
        value: StaticValue::Ast(crate::canonical::JVal::Num(v)),
    }
}

fn style_fill(fill: &str) -> DrawStyle {
    DrawStyle {
        fill: Some(static_str(fill)),
        ..DrawStyle::default()
    }
}

fn style_stroke(stroke: &str, width: f64) -> DrawStyle {
    DrawStyle {
        stroke: Some(static_str(stroke)),
        stroke_width: Some(static_num(width)),
        ..DrawStyle::default()
    }
}

/// A surface-relative structural stroke: `currentColor` at a per-role opacity,
/// so axis + gridlines ink from the surface's own text colour.
fn style_stroke_ink(opacity: f64, width: f64) -> DrawStyle {
    DrawStyle {
        stroke: Some(static_str(INK)),
        stroke_width: Some(static_num(width)),
        opacity: Some(static_num(opacity)),
        ..DrawStyle::default()
    }
}

/// A text-label style: surface-relative ink (`currentColor`) + an optional
/// per-role opacity (`None` = full-strength, e.g. titles) + alignment + size +
/// weight + the chart font.
fn text_style(
    opacity: Option<f64>,
    anchor: TextAnchor,
    size: f64,
    emphasis: Emphasis,
) -> DrawStyle {
    DrawStyle {
        fill: Some(static_str(INK)),
        opacity: opacity.map(static_num),
        text_anchor: Some(anchor),
        font_size: Some(size),
        emphasis: Some(emphasis),
        font_family: Some(CHART_FONT.to_string()),
        stroke: None,
        stroke_width: None,
    }
}

// ─── The lowering ─────────────────────────────────────────────────────────────

/// One resolved data row: the x-axis category label + one numeric value per
/// `y_fields` series (in `y_fields` order). The caller projects the resolved
/// rows into this shape (a numeric slot missing / non-numeric reads `0.0`, the
/// reference `numericOf` behaviour).
pub struct LowerRow {
    pub category: String,
    pub values: Vec<f64>,
}

fn capitalise(sr: &str) -> String {
    let mut chars = sr.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Lower a resolved chart to a canonical [`DrawingSpec`]. Only `Bar` and `Line`
/// are lowered; any other `kind` produces an empty drawing.
pub fn lower_chart(
    kind: ChartKind,
    x_field: &str,
    y_fields: &[String],
    title: Option<&TextSource>,
    rows: &[LowerRow],
) -> DrawingSpec {
    let categories: Vec<&str> = rows.iter().map(|r| r.category.as_str()).collect();
    let n = rows.len();
    // series[j][i] — value of series j at row i (in y_fields order).
    let m = y_fields.len();
    let series: Vec<Vec<f64>> = (0..m)
        .map(|j| {
            rows.iter()
                .map(|r| r.values.get(j).copied().unwrap_or(0.0))
                .collect()
        })
        .collect();

    let all_values: Vec<f64> = series.iter().flatten().copied().collect();
    let all_values = if all_values.is_empty() {
        vec![0.0]
    } else {
        all_values
    };
    let data_min = all_values.iter().copied().fold(f64::INFINITY, f64::min);
    let data_max = all_values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    // Bars + lines share a zero-anchored domain — deterministic + honest for bars.
    let (nice_lo, nice_hi, ticks) = nice_domain(data_min.min(0.0), data_max.max(0.0));

    let y_scale = |v: f64| -> f64 { r2(PLOT_Y1 - (v - nice_lo) / (nice_hi - nice_lo) * PLOT_H) };

    let band_w = if n > 0 { PLOT_W / n as f64 } else { PLOT_W };
    let centre_x = |i: usize| -> f64 { r2(PLOT_X0 + band_w * (i as f64 + 0.5)) };

    let mut shapes: Vec<Shape> = Vec::new();

    // ── Gridlines (painter's order: first) ──
    for &t in &ticks {
        let y = y_scale(t);
        shapes.push(Shape::Line {
            x1: r2(PLOT_X0),
            y1: y,
            x2: r2(PLOT_X1),
            y2: y,
            style: style_stroke_ink(GRID_OPACITY, 1.0),
        });
    }

    // ── Axes ──
    shapes.push(Shape::Line {
        x1: r2(PLOT_X0),
        y1: r2(PLOT_Y0),
        x2: r2(PLOT_X0),
        y2: r2(PLOT_Y1),
        style: style_stroke_ink(AXIS_OPACITY, 1.0),
    });
    shapes.push(Shape::Line {
        x1: r2(PLOT_X0),
        y1: r2(PLOT_Y1),
        x2: r2(PLOT_X1),
        y2: r2(PLOT_Y1),
        style: style_stroke_ink(AXIS_OPACITY, 1.0),
    });

    let tick_size = 13.0;
    let title_size = 16.0;

    // ── y-axis tick labels — right-anchored (End) ──
    for &t in &ticks {
        shapes.push(Shape::Label {
            x: r2(PLOT_X0 - 8.0),
            y: r2(y_scale(t) + 4.0),
            text: TextSource::Literal(tick_label(t)),
            style: text_style(
                Some(LABEL_OPACITY),
                TextAnchor::End,
                tick_size,
                Emphasis::Normal,
            ),
        });
    }

    // ── x-axis category labels — centred (Middle) ──
    for (i, c) in categories.iter().enumerate() {
        shapes.push(Shape::Label {
            x: centre_x(i),
            y: r2(PLOT_Y1 + 20.0),
            text: TextSource::Literal((*c).to_string()),
            style: text_style(
                Some(LABEL_OPACITY),
                TextAnchor::Middle,
                tick_size,
                Emphasis::Normal,
            ),
        });
    }

    // ── Axis titles (a name on both axes) ──
    shapes.push(Shape::Label {
        x: r2((PLOT_X0 + PLOT_X1) / 2.0),
        y: r2(H - 12.0),
        text: TextSource::Literal(capitalise(x_field)),
        style: text_style(None, TextAnchor::Middle, tick_size, Emphasis::Normal),
    });
    shapes.push(Shape::Label {
        x: r2(8.0),
        y: r2(PLOT_Y0 - 12.0),
        text: TextSource::Literal("Value".to_string()),
        style: text_style(None, TextAnchor::Start, tick_size, Emphasis::Normal),
    });

    // ── Series geometry ──
    match kind {
        ChartKind::Bar => {
            let group_w = band_w * 0.7;
            let sub_w = if m > 0 { group_w / m as f64 } else { group_w };
            let base_y = y_scale(0.0);
            for (j, values) in series.iter().enumerate() {
                let colour = colour_for(j);
                for (i, &v) in values.iter().enumerate() {
                    let bx = r2(PLOT_X0
                        + band_w * i as f64
                        + (band_w - group_w) / 2.0
                        + j as f64 * sub_w);
                    let bw = r2(sub_w * 0.9);
                    let vy = y_scale(v);
                    let top = vy.min(base_y);
                    let hgt = r2((vy - base_y).abs());
                    shapes.push(Shape::Rectangle {
                        x: bx,
                        y: top,
                        width: bw,
                        height: hgt,
                        corner_radius: None,
                        style: style_fill(colour),
                    });
                }
            }
        }
        ChartKind::Line => {
            for (j, values) in series.iter().enumerate() {
                let colour = colour_for(j);
                let points = values
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| DrawPoint {
                        x: centre_x(i),
                        y: y_scale(v),
                    })
                    .collect();
                shapes.push(Shape::Polyline {
                    points,
                    style: style_stroke(colour, 2.0),
                });
            }
        }
        _ => {}
    }

    // ── Legend (only when >1 series) — a swatch + series name per series ──
    if m > 1 {
        for (j, yf) in y_fields.iter().enumerate() {
            let colour = colour_for(j);
            let lx = r2(PLOT_X0 + j as f64 * 100.0);
            shapes.push(Shape::Rectangle {
                x: lx,
                y: 34.0,
                width: 10.0,
                height: 10.0,
                corner_radius: Some(2.0),
                style: style_fill(colour),
            });
            shapes.push(Shape::Label {
                x: r2(lx + 15.0),
                y: 43.0,
                text: TextSource::Literal(yf.clone()),
                style: text_style(
                    Some(LABEL_OPACITY),
                    TextAnchor::Start,
                    tick_size,
                    Emphasis::Normal,
                ),
            });
        }
    }

    // ── Visible title (a Label — bigger + emphasised) + the a11y Title ──
    if let Some(t) = title {
        shapes.push(Shape::Label {
            x: r2(PLOT_X0),
            y: 22.0,
            text: t.clone(),
            style: text_style(None, TextAnchor::Start, title_size, Emphasis::Loud),
        });
    }

    DrawingSpec {
        view_box: ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: W,
            height: H,
        },
        shapes,
        style: DrawStyle::default(),
        title: title.cloned(),
        description: None,
    }
}

/// Project a resolved data row (`JVal::Obj`) into a [`LowerRow`]: the `x_field`
/// as a category string, each `y_fields` slot as a number (missing / non-numeric
/// reads `0.0`, the reference `numericOf`). The category mirrors the reference
/// `projectRowFieldString` (string as-is, number canonicalised, else empty).
pub fn project_row(row: &crate::canonical::JVal, x_field: &str, y_fields: &[String]) -> LowerRow {
    use crate::canonical::JVal;
    let category = match row.field(x_field) {
        Some(JVal::Str(v)) => v.clone(),
        Some(JVal::Num(v)) => format_num(*v),
        Some(JVal::Bool(v)) => v.to_string(),
        _ => String::new(),
    };
    let values = y_fields
        .iter()
        .map(|yf| match row.field(yf) {
            Some(JVal::Num(v)) => *v,
            Some(JVal::Bool(true)) => 1.0,
            _ => 0.0,
        })
        .collect();
    LowerRow { category, values }
}
