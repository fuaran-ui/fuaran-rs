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
//! to it (see `tests/chart_lowering.rs`).
//!
//! **Lowered arms:** `Bar` (grouped + stacked), `Line`, `Area` (overlaid +
//! stacked), `Scatter` (linear numeric x-scale, point marks), `Pie` (polar,
//! cubic-approximated wedges — the donut variant is deferred with the
//! reference). `Heatmap` produces an empty drawing (its rule lands with its
//! own tier). `stacked` on a kind where stacking is meaningless (`Line` /
//! `Scatter` / `Pie`) is ignored — the flag only changes `Bar` / `Area`
//! geometry.
//!
//! **Keyed mark identity (Phase 642).** Every data-bearing shape's style is
//! stamped with a derivation-based `markId` — `series-field|category-key` on a
//! per-datum mark, the series field alone on a series-level mark (Line/Area) —
//! stable under row reorder and data refresh (object constancy). Chrome (axes,
//! gridlines, labels, legend) deliberately stays unstamped.

use crate::canonical::format_number;
use crate::wire::{
    Binding, ChartKind, CurveCommand, DrawPoint, DrawStyle, DrawingSpec, Emphasis, Format, Shape,
    StaticValue, TextAnchor, TextSource, ViewBox,
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
///
/// Phase 875 — palette v2, 8 slots, fixed assignment order. Validated on both
/// surfaces (light + dark) against the OKLab gate set (lightness band, chroma
/// floor, adjacent-pair CVD ΔE, adjacent-pair normal-vision ΔE). The
/// ASSIGNMENT ORDER is load-bearing — the gates are measured over ADJACENT
/// pairs, so re-ordering the array can drop a passing set below the floor.
/// Do not cycle or sort it.
const PALETTE: [&str; 8] = [
    "#1a86ac", // loch blue
    "#bf831c", // ochre
    "#a51574", // magenta
    "#21a766", // green
    "#6454e5", // violet
    "#af153d", // crimson
    "#21a2b2", // teal
    "#d3241b", // vermilion
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

/// Gap between the y-axis spine and the right edge of a tick label (Phase 875:
/// widened alongside `TICK_MARK_LENGTH`, which occupies the first stretch of
/// this gap).
const TICK_LABEL_GAP: f64 = 12.0;

/// Length of the small OUTSIDE tick marks on both axes (Phase 875): y-axis
/// marks run left from the spine, x-axis marks run down from it, so neither
/// eats plot area. Inked at axis strength, one per y tick and one per
/// category band centre (or per x tick on the Scatter arm).
const TICK_MARK_LENGTH: f64 = 5.0;

/// Hard pixel ceiling on a single bar's thickness (Phase 875). The bar takes
/// the MIN of its band share and this cap, then is centred in its slot.
const BAR_MAX_THICKNESS: f64 = 28.0;

/// GEOMETRIC gap between consecutive segments of a stacked bar (Phase 875) —
/// the segment is shortened on the side facing the next segment, so the
/// separation is absence of ink, not a surface-coloured stroke.
const STACK_SEGMENT_GAP: f64 = 2.0;

/// GEOMETRIC angular padding between pie wedges, in DEGREES (Phase 875) — half
/// is taken from each end of every wedge's sweep.
const WEDGE_GAP_DEGREES: f64 = 0.75;

/// A translucent categorical fill (Phase 637 — area bands; opacity dropped to
/// a wash by Phase 875). The gridlines stay legible through the band; the
/// series' full-strength Polyline edge on top carries the categorical colour
/// at full contrast.
const AREA_FILL_OPACITY: f64 = 0.12;

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
fn nice_domain(lo: f64, hi: f64) -> (f64, f64, f64, Vec<f64>) {
    let hi = if hi == lo { lo + 1.0 } else { hi };
    let target_ticks = 5.0;
    let range = nice_num(hi - lo, false);
    let step = nice_num(range / (target_ticks - 1.0), true);
    let nice_lo = (lo / step).floor() * step;
    let nice_hi = (hi / step).ceil() * step;
    // Enumerate ticks by integer count (float accumulation would drift).
    let count = ((nice_hi - nice_lo) / step).round() as i64;
    let ticks = (0..=count).map(|i| r2(nice_lo + i as f64 * step)).collect();
    (nice_lo, nice_hi, step, ticks)
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

// ─── The canonical invariant number formatter (Phase 876) ────────────────────
//
// A byte-for-byte port of the reference spec. The chart lowering does NOT
// inherit the locale-aware rendering other surfaces give `Format`: a chart's
// ticks are part of a drawing whose bytes must be identical on every host, so
// the rendering here is locale-INVARIANT by definition — period decimal
// separator, comma thousands separator, no locale data anywhere.
//
//   1. Decimals come from the TICK STEP, never the data (`dps_of_step`).
//   2. The base render is round-half-up on the magnitude at that precision,
//      grouped in threes, zero-padded to exactly d places, a leading `-` only
//      when the rounded magnitude is non-zero.
//   3. The `Format` arms layer meaning over that base; `Date` / `RelativeTime`
//      / `Duration` are not value-axis formats and fall through to the base.
//   4. Display-unit scaling divides BOTH the value and the step by 10^n.

/// Decimal places implied by a tick step: the smallest `d <= 6` for which
/// `step * 10^d` is (within relative float tolerance) an integer.
fn dps_of_step(step: f64) -> i32 {
    let s = step.abs();
    if s.is_nan() || s.is_infinite() || s <= 0.0 {
        return 0;
    }
    let mut scaled = s;
    for d in 0..6 {
        if (scaled - (scaled + 0.5).floor()).abs() <= 1e-9 * 1.0_f64.max(scaled) {
            return d;
        }
        scaled *= 10.0;
    }
    6
}

/// Group an integral digit string in threes from the right with `,`.
fn group_thousands(digits: &str) -> String {
    let n = digits.len();
    if n <= 3 {
        return digits.to_string();
    }
    let head = n % 3;
    let mut parts: Vec<&str> = Vec::new();
    if head > 0 {
        parts.push(&digits[..head]);
    }
    let mut i = head;
    while i + 3 <= n {
        parts.push(&digits[i..i + 3]);
        i += 3;
    }
    parts.join(",")
}

/// Render `v` with EXACTLY `dps` decimals — round-half-up on the magnitude,
/// comma thousands separators, period decimal point, locale-invariant.
fn render_fixed(dps: i32, v: f64) -> String {
    if v.is_nan() || v.is_infinite() {
        return "0".to_string();
    }
    let d = dps.clamp(0, 6);
    let scale = 10.0_f64.powi(d);
    let units = (v.abs() * scale + 0.5).floor();
    let int_part = (units / scale).floor();
    let frac_part = units - int_part * scale;
    let int_str = group_thousands(&format_num(int_part));
    let body = if d == 0 {
        int_str
    } else {
        let raw = format_num(frac_part);
        let pad = "0".repeat((d as usize).saturating_sub(raw.len()));
        format!("{int_str}.{pad}{raw}")
    };
    if v < 0.0 && units > 0.0 {
        format!("-{body}")
    } else {
        body
    }
}

/// ISO-4217 code -> symbol, the invariant table. An unlisted code renders as
/// the code itself — deterministic, and never a wrong symbol.
fn currency_symbol(iso: &str) -> String {
    match iso {
        "EUR" => "\u{20ac}",
        "USD" => "$",
        "GBP" => "\u{a3}",
        "JPY" | "CNY" => "\u{a5}",
        "CHF" => "CHF",
        "AUD" | "CAD" | "NZD" | "HKD" | "SGD" | "MXN" => "$",
        "INR" => "\u{20b9}",
        "KRW" => "\u{20a9}",
        "BRL" => "R$",
        "RUB" => "\u{20bd}",
        "ZAR" => "R",
        "SEK" | "NOK" | "DKK" => "kr",
        "PLN" => "z\u{142}",
        "CZK" => "K\u{10d}",
        "HUF" => "Ft",
        "TRY" => "\u{20ba}",
        "THB" => "\u{e3f}",
        "ILS" => "\u{20aa}",
        other => return other.to_string(),
    }
    .to_string()
}

/// The unit symbol a `Format` contributes to an axis-unit label.
fn format_unit_symbol(fmt: Option<&Format>) -> String {
    match fmt {
        Some(Format::Currency { iso_code }) => currency_symbol(iso_code),
        _ => String::new(),
    }
}

/// The x100 a `Format::Percent` applies to BOTH the value and the step.
fn format_value_scale(fmt: Option<&Format>) -> f64 {
    match fmt {
        Some(Format::Percent { .. }) => 100.0,
        _ => 1.0,
    }
}

/// Render one value-axis number. `divisor` is the display unit (`1.0` when no
/// scaling applies); `drop_symbol` suppresses a currency symbol on the ticks
/// because the axis-unit label already states it once.
fn format_value(
    fmt: Option<&Format>,
    divisor: f64,
    drop_symbol: bool,
    step: f64,
    v: f64,
) -> String {
    let pct = format_value_scale(fmt);
    let dv = v * pct / divisor;
    let ds = step * pct / divisor;
    let pinned = match fmt {
        Some(Format::Number { decimals }) | Some(Format::Percent { decimals }) => *decimals,
        _ => None,
    };
    let dps = pinned.map_or_else(|| dps_of_step(ds), |d| d as i32);
    let body = render_fixed(dps, dv);
    match fmt {
        Some(Format::Percent { .. }) => format!("{body}%"),
        Some(Format::Currency { iso_code }) if !drop_symbol => {
            let sym = currency_symbol(iso_code);
            if let Some(rest) = body.strip_prefix('-') {
                format!("-{sym}{rest}")
            } else {
                format!("{sym}{body}")
            }
        }
        _ => body,
    }
}

// ─── Display units (Phase 876) ───────────────────────────────────────────────
//
// The operator's prefix table: thresholds sit at 1 + 3k and the selected
// threshold `t` for a magnitude of exponent `e` satisfies `e - 1 <= t < e + 2`,
// giving the unit exponent `n = t - 1`. Each unit covers three exponents —
// Thousands for e in {3,4,5}, Millions for {6,7,8} — which is why a 12-million
// axis and a 900-million axis both read in millions.

/// How a value axis states its display unit once scaling applies. NOT a wire
/// value: the chart style is a lowering parameter, so a display-unit convention
/// is the host's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartAxisUnitMode {
    /// One word in the axis-unit slot — "Millions" (the shipped default).
    #[default]
    Words,
    /// The word plus the value format's unit symbol — "Millions of £".
    WordsWithSymbol,
    /// The SI prefix plus the unit symbol — "M£" (or bare "M").
    SIAbbreviation,
    /// No axis-unit label: every tick carries its own compact suffix (`12K`).
    CompactPerTick,
    /// Never scale; every tick prints its full magnitude.
    Off,
}

/// The smallest unit exponent that triggers scaling at the shipped default —
/// the operator's `unit > 3` gate, so scaling begins at MILLIONS and a
/// thousands-range axis still reads `12,500` in full.
pub const DISPLAY_UNIT_MIN_EXPONENT: i32 = 6;

fn unit_exponent_of(max_abs: f64) -> i32 {
    if max_abs.is_nan() || max_abs.is_infinite() || max_abs <= 0.0 {
        return 0;
    }
    let e = (max_abs.log10() + 0.5).floor() as i32;
    let n = 3 * (f64::from(e - 2) / 3.0).ceil() as i32;
    n.clamp(-15, 15)
}

fn unit_words(n: i32) -> &'static str {
    match n {
        3 => "Thousands",
        6 => "Millions",
        9 => "Billions",
        12 => "Trillions",
        15 => "Quadrillions",
        _ => "",
    }
}

fn unit_si(n: i32) -> &'static str {
    match n {
        3 => "k",
        6 => "M",
        9 => "G",
        12 => "T",
        15 => "P",
        _ => "",
    }
}

fn unit_compact(n: i32) -> &'static str {
    match n {
        3 => "K",
        6 => "M",
        9 => "B",
        12 => "T",
        15 => "Q",
        _ => "",
    }
}

/// A resolved display unit for one value axis.
struct DisplayUnit {
    divisor: f64,
    tick_suffix: String,
    drop_symbol: bool,
    label: String,
}

impl Default for DisplayUnit {
    fn default() -> Self {
        Self {
            divisor: 1.0,
            tick_suffix: String::new(),
            drop_symbol: false,
            label: String::new(),
        }
    }
}

/// Resolve the display unit for a value axis whose PRINTED magnitudes peak at
/// `max_abs` (already through any `Format::Percent` x100).
fn resolve_display_unit(
    mode: ChartAxisUnitMode,
    min_exponent: i32,
    fmt: Option<&Format>,
    max_abs: f64,
) -> DisplayUnit {
    let n = unit_exponent_of(max_abs);
    let threshold = if mode == ChartAxisUnitMode::CompactPerTick {
        3
    } else {
        min_exponent
    };
    let words = unit_words(n);
    if mode == ChartAxisUnitMode::Off || n < 3 || n < threshold || words.is_empty() {
        return DisplayUnit::default();
    }
    let symbol = format_unit_symbol(fmt);
    let divisor = 10.0_f64.powi(n);
    match mode {
        ChartAxisUnitMode::Words => DisplayUnit {
            divisor,
            label: words.to_string(),
            ..DisplayUnit::default()
        },
        ChartAxisUnitMode::WordsWithSymbol => DisplayUnit {
            divisor,
            drop_symbol: !symbol.is_empty(),
            label: if symbol.is_empty() {
                words.to_string()
            } else {
                format!("{words} of {symbol}")
            },
            ..DisplayUnit::default()
        },
        ChartAxisUnitMode::SIAbbreviation => DisplayUnit {
            divisor,
            drop_symbol: !symbol.is_empty(),
            label: format!("{}{symbol}", unit_si(n)),
            ..DisplayUnit::default()
        },
        ChartAxisUnitMode::CompactPerTick => DisplayUnit {
            divisor,
            tick_suffix: unit_compact(n).to_string(),
            ..DisplayUnit::default()
        },
        ChartAxisUnitMode::Off => DisplayUnit::default(),
    }
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

fn style_fill_opacity(fill: &str, opacity: f64) -> DrawStyle {
    DrawStyle {
        fill: Some(static_str(fill)),
        opacity: Some(static_num(opacity)),
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

/// Phase 642 — stamp a derivation-based mark identity onto a data-bearing
/// shape's style: `series-field|category-key`, stable under row reorder and
/// data refresh (object constancy).
fn with_mark(series_field: &str, category_key: &str, style: DrawStyle) -> DrawStyle {
    DrawStyle {
        mark_id: Some(format!("{series_field}|{category_key}")),
        ..style
    }
}

/// A series-level mark (one shape carries the whole series — Line/Area): the
/// identity is the series field alone.
fn with_series_mark(series_field: &str, style: DrawStyle) -> DrawStyle {
    DrawStyle {
        mark_id: Some(series_field.to_string()),
        ..style
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
        mark_id: None,
        // Phase 877 — the lowering stays upright here; the tilt / vertical
        // escalation / rotated y-title APPLICATIONS land with the chart-style
        // phases, so this host's goldens are deliberately unchanged.
        rotation: None,
    }
}

// ─── The lowering ─────────────────────────────────────────────────────────────

/// One resolved data row: the x-axis category label, the x value read
/// NUMERICALLY (the Scatter arm's linear x-scale), and one numeric value per
/// `y_fields` series (in `y_fields` order). The caller projects the resolved
/// rows into this shape (a numeric slot missing / non-numeric / non-finite
/// reads `0.0`, the reference `numericOf` behaviour with the Phase 640
/// non-finite guard).
pub struct LowerRow {
    pub category: String,
    pub x_value: f64,
    pub values: Vec<f64>,
}

fn capitalise(sr: &str) -> String {
    let mut chars = sr.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Lower a resolved chart to a canonical [`DrawingSpec`]. Lowered arms: `Bar`
/// (grouped + stacked), `Line`, `Area` (overlaid + stacked), `Scatter`, `Pie`;
/// `Heatmap` produces an empty drawing. `stacked` is honoured on `Bar` /
/// `Area` only.
/// The styling knobs this lowering exposes (Phase 876). NOT wire values — a
/// display-unit convention is the host's, made at render time, and must never
/// rewrite a semantic node. `Default` is what the `chart-lowering/*` goldens
/// pin.
#[derive(Debug, Clone, Copy)]
pub struct ChartLowerStyle {
    pub axis_unit_mode: ChartAxisUnitMode,
    pub display_unit_min_exponent: i32,
}

impl Default for ChartLowerStyle {
    fn default() -> Self {
        Self {
            axis_unit_mode: ChartAxisUnitMode::default(),
            display_unit_min_exponent: DISPLAY_UNIT_MIN_EXPONENT,
        }
    }
}

/// Lower under the shipped default style — the corpus-pinned form.
pub fn lower_chart(
    kind: ChartKind,
    stacked: bool,
    x_field: &str,
    y_fields: &[String],
    title: Option<&TextSource>,
    rows: &[LowerRow],
) -> DrawingSpec {
    lower_chart_with(
        kind,
        stacked,
        x_field,
        y_fields,
        title,
        None,
        &ChartLowerStyle::default(),
        rows,
    )
}

/// Lower under an explicit value-axis `Format` (a wire declaration) and an
/// explicit style (a host choice) — Phase 876.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub fn lower_chart_with(
    kind: ChartKind,
    stacked: bool,
    x_field: &str,
    y_fields: &[String],
    title: Option<&TextSource>,
    value_format: Option<&Format>,
    style: &ChartLowerStyle,
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

    // Stacking applies to Bar + Area only (Phase 637). Values stack as-is by
    // plain cumulative sum per category — deterministic and total; a negative
    // value simply lowers the running sum.
    let stacked = stacked && matches!(kind, ChartKind::Bar | ChartKind::Area);

    // Per-category running sums across the series, INCLUDING the leading 0
    // baseline: `cums_for(i)` has length m+1.
    let cums_for = |i: usize| -> Vec<f64> {
        let mut acc = 0.0;
        let mut out = Vec::with_capacity(m + 1);
        out.push(0.0);
        for s in &series {
            acc += s[i];
            out.push(acc);
        }
        out
    };

    let all_values: Vec<f64> = if stacked {
        (0..n).flat_map(&cums_for).collect()
    } else {
        series.iter().flatten().copied().collect()
    };
    let all_values = if all_values.is_empty() {
        vec![0.0]
    } else {
        all_values
    };
    let data_min = all_values.iter().copied().fold(f64::INFINITY, f64::min);
    let data_max = all_values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    // Bars + lines share a zero-anchored domain — deterministic + honest for
    // bars. Stacked domains come from the cumulative partial sums, so the axis
    // covers the stack totals, never a single series' range.
    let (nice_lo, nice_hi, y_step, ticks) = nice_domain(data_min.min(0.0), data_max.max(0.0));

    // ── Value-axis number formatting (Phase 876) ──
    // The declared meaning (`value_format`) chooses the arms; the style chooses
    // whether a large magnitude is stated once as a display unit; the tick STEP
    // chooses the precision. The unit is resolved from the PRINTED magnitude,
    // so a `Percent` axis is measured after its x100.
    let y_display_unit = resolve_display_unit(
        style.axis_unit_mode,
        style.display_unit_min_exponent,
        value_format,
        nice_lo.abs().max(nice_hi.abs()) * format_value_scale(value_format),
    );
    let y_tick_text = |v: f64| -> String {
        format!(
            "{}{}",
            format_value(
                value_format,
                y_display_unit.divisor,
                y_display_unit.drop_symbol,
                y_step,
                v
            ),
            y_display_unit.tick_suffix
        )
    };

    let y_scale = |v: f64| -> f64 { r2(PLOT_Y1 - (v - nice_lo) / (nice_hi - nice_lo) * PLOT_H) };

    let band_w = if n > 0 { PLOT_W / n as f64 } else { PLOT_W };
    let centre_x = |i: usize| -> f64 { r2(PLOT_X0 + band_w * (i as f64 + 0.5)) };

    // ── Linear x-scale (Phase 636 — the Scatter arm's numeric x axis) ──
    // Scatter reads the x-field NUMERICALLY and plots on a linear x-domain (the
    // first non-band x-scale arm). The domain is NOT zero-anchored — a
    // scatter's x range carries no baseline semantics (the y domain stays
    // zero-anchored with the other arms, deliberately: one shared y-domain
    // rule).
    let is_scatter = matches!(kind, ChartKind::Scatter);
    let x_values: Vec<f64> = if is_scatter {
        rows.iter().map(|r| r.x_value).collect()
    } else {
        vec![]
    };
    let (x_nice_lo, x_nice_hi, x_step, x_ticks) = if is_scatter {
        if x_values.is_empty() {
            nice_domain(0.0, 1.0)
        } else {
            let lo = x_values.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = x_values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            nice_domain(lo, hi)
        }
    } else {
        (0.0, 1.0, 1.0, vec![])
    };
    // The Scatter arm's x IS a value axis, so its ticks take the same canonical
    // formatter (Phase 876). `value_format` is deliberately NOT applied to it:
    // one declared meaning cannot be true of two different measures, and there
    // is no second axis-unit slot to state an x display unit in.
    let x_tick_text = |v: f64| -> String { format_value(None, 1.0, false, x_step, v) };
    let x_scale =
        |v: f64| -> f64 { r2(PLOT_X0 + (v - x_nice_lo) / (x_nice_hi - x_nice_lo) * PLOT_W) };

    let tick_size = 13.0;
    let title_size = 18.0;

    let mut shapes: Vec<Shape> = Vec::new();

    // ── Visible title (a Label — bigger + emphasised) ──
    let push_title = |shapes: &mut Vec<Shape>| {
        if let Some(t) = title {
            shapes.push(Shape::Label {
                x: r2(PLOT_X0),
                y: 22.0,
                text: t.clone(),
                style: text_style(None, TextAnchor::Start, title_size, Emphasis::Loud),
            });
        }
    };

    // ── Pie (Phase 638) — the polar arm: no cartesian chrome ──
    //
    // Bounded v1: exactly ONE series (multi-series pie is a grounded-validation
    // refusal upstream, never a silent first-series truncation) and
    // non-negative values (any negative refuses the geometry — a mixed-sign
    // pie has no honest reading). Zero-value categories draw no wedge but keep
    // their legend row. Wedges start at 12 o'clock and sweep clockwise; arcs
    // are the standard <=90-degree-segment cubic-Bezier approximation (the
    // closed `CurveCommand` vocabulary has no arc case, deliberately). A lone
    // 100% category degenerates to a `Circle`. Category share reads in the
    // legend ("name (NN%)") — outside labels with leader lines are a later
    // variant.
    if matches!(kind, ChartKind::Pie) {
        let values: &[f64] = if m == 1 { &series[0] } else { &[] };
        let refused = m != 1 || values.iter().any(|&v| v < 0.0);
        let total: f64 = values.iter().sum();
        if !refused && total > 0.0 {
            let cx = r2((PLOT_X0 + PLOT_X1) / 2.0);
            let cy = r2((PLOT_Y0 + PLOT_Y1) / 2.0);
            let radius = 130.0;

            let pt = |a: f64| -> DrawPoint {
                DrawPoint {
                    x: r2(cx + radius * a.cos()),
                    y: r2(cy + radius * a.sin()),
                }
            };

            let arc_cubics = |a0: f64, a1: f64| -> Vec<CurveCommand> {
                let segments =
                    (((a1 - a0) / (std::f64::consts::PI / 2.0) - 1e-9).ceil() as i64).max(1);
                (0..segments)
                    .map(|s| {
                        let t0 = a0 + (a1 - a0) * s as f64 / segments as f64;
                        let t1 = a0 + (a1 - a0) * (s + 1) as f64 / segments as f64;
                        let k = 4.0 / 3.0 * ((t1 - t0) / 4.0).tan();
                        let c1 = DrawPoint {
                            x: r2(cx + radius * (t0.cos() - k * t0.sin())),
                            y: r2(cy + radius * (t0.sin() + k * t0.cos())),
                        };
                        let c2 = DrawPoint {
                            x: r2(cx + radius * (t1.cos() + k * t1.sin())),
                            y: r2(cy + radius * (t1.sin() - k * t1.cos())),
                        };
                        CurveCommand::CubicTo {
                            control1: c1,
                            control2: c2,
                            to: pt(t1),
                        }
                    })
                    .collect()
            };

            let fractions: Vec<f64> = values.iter().map(|v| v / total).collect();
            let starts: Vec<f64> = {
                let mut acc = 0.0;
                let mut out = Vec::with_capacity(fractions.len() + 1);
                out.push(0.0);
                for f in &fractions {
                    acc += f;
                    out.push(acc);
                }
                out
            };
            let top = -std::f64::consts::PI / 2.0;

            // Half the angular padding comes off each end of every wedge
            // (Phase 875), so the separation is a sliver of absent ink — no
            // surface colour is needed and the result is theme-invariant,
            // which a stroked wedge border could not be.
            let half_gap = WEDGE_GAP_DEGREES * std::f64::consts::PI / 360.0;

            let yf = &y_fields[0];
            for (i, &f) in fractions.iter().enumerate() {
                if f > 0.0 {
                    let mark_style = with_mark(yf, categories[i], style_fill(colour_for(i)));
                    if f >= 1.0 - 1e-9 {
                        // A lone 100% category is a circle — there is no
                        // neighbour to separate from, so no padding.
                        shapes.push(Shape::Circle {
                            cx,
                            cy,
                            r: radius,
                            style: mark_style,
                        });
                    } else {
                        let a0 = top + 2.0 * std::f64::consts::PI * starts[i] + half_gap;
                        let a1 = top + 2.0 * std::f64::consts::PI * starts[i + 1] - half_gap;
                        // A wedge narrower than the padding is DROPPED rather
                        // than drawn inverted — the alternative is a sliver
                        // sweeping the wrong way round the circle, which is a
                        // wrong picture, not a small one.
                        if a1 > a0 {
                            let mut cmds = vec![
                                CurveCommand::MoveTo(DrawPoint { x: cx, y: cy }),
                                CurveCommand::LineTo(pt(a0)),
                            ];
                            cmds.extend(arc_cubics(a0, a1));
                            cmds.push(CurveCommand::Close);
                            shapes.push(Shape::Curve {
                                commands: cmds,
                                style: mark_style,
                            });
                        }
                    }
                }
            }

            // Vertical category legend on the right — categories take the
            // palette roles a cartesian chart gives its series.
            for i in 0..n {
                let ly = 70.0 + 20.0 * i as f64;
                shapes.push(Shape::Rectangle {
                    x: r2(W - 168.0),
                    y: r2(ly),
                    width: 10.0,
                    height: 10.0,
                    corner_radius: Some(2.0),
                    style: style_fill(colour_for(i)),
                });
                // Routed through the canonical formatter (Phase 876) — one
                // rounding + rendering rule for every number this module
                // prints. A share is a whole percent here, so the shipped
                // `NN%` shape is unchanged.
                let pct = format_value(None, 1.0, false, 1.0, fractions[i] * 100.0);
                shapes.push(Shape::Label {
                    x: r2(W - 153.0),
                    y: r2(ly + 9.0),
                    text: TextSource::Literal(format!("{} ({pct}%)", categories[i])),
                    style: text_style(
                        Some(LABEL_OPACITY),
                        TextAnchor::Start,
                        tick_size,
                        Emphasis::Normal,
                    ),
                });
            }
        }
        push_title(&mut shapes);
        return DrawingSpec {
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
        };
    }

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

    // Vertical gridlines — the Scatter arm only (Phase 875). A linear x-scale
    // has readable x positions to trace back to; a BAND x-axis has none (a
    // category is a label, not a magnitude), so a vertical rule there would
    // be decoration.
    if is_scatter {
        for &t in &x_ticks {
            let x = x_scale(t);
            shapes.push(Shape::Line {
                x1: x,
                y1: r2(PLOT_Y0),
                x2: x,
                y2: r2(PLOT_Y1),
                style: style_stroke_ink(GRID_OPACITY, 1.0),
            });
        }
    }

    // Zero baseline (Phase 875) — only when the domain CROSSES zero, drawn at
    // axis strength over the ordinary gridline it shares a y with. When the
    // domain does not cross zero the axis spine already IS the baseline.
    if nice_lo < 0.0 && nice_hi > 0.0 {
        let y = y_scale(0.0);
        shapes.push(Shape::Line {
            x1: r2(PLOT_X0),
            y1: y,
            x2: r2(PLOT_X1),
            y2: y,
            style: style_stroke_ink(AXIS_OPACITY, 1.0),
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

    // Outside tick marks (Phase 875) — outside the plot on both axes, so the
    // plot area stays ink-free and the marks tie each label to its position.
    // y marks first, then x marks; suppressed entirely when the length is
    // non-positive (it never is for the shipped default, but the port keeps
    // the guard for parity with the reference's style-driven suppression).
    if TICK_MARK_LENGTH > 0.0 {
        for &t in &ticks {
            let y = y_scale(t);
            shapes.push(Shape::Line {
                x1: r2(PLOT_X0 - TICK_MARK_LENGTH),
                y1: y,
                x2: r2(PLOT_X0),
                y2: y,
                style: style_stroke_ink(AXIS_OPACITY, 1.0),
            });
        }
        let x_mark = |x: f64| -> Shape {
            Shape::Line {
                x1: x,
                y1: r2(PLOT_Y1),
                x2: x,
                y2: r2(PLOT_Y1 + TICK_MARK_LENGTH),
                style: style_stroke_ink(AXIS_OPACITY, 1.0),
            }
        };
        if is_scatter {
            for &t in &x_ticks {
                shapes.push(x_mark(x_scale(t)));
            }
        } else {
            for i in 0..n {
                shapes.push(x_mark(centre_x(i)));
            }
        }
    }

    // ── y-axis tick labels — right-anchored (End) ──
    for &t in &ticks {
        shapes.push(Shape::Label {
            x: r2(PLOT_X0 - TICK_LABEL_GAP),
            y: r2(y_scale(t) + 4.0),
            text: TextSource::Literal(y_tick_text(t)),
            style: text_style(
                Some(LABEL_OPACITY),
                TextAnchor::End,
                tick_size,
                Emphasis::Normal,
            ),
        });
    }

    // ── x-axis labels — band arms label each category under its band centre;
    // Scatter labels its numeric x-ticks along the linear axis (Phase 636) ──
    if is_scatter {
        for &t in &x_ticks {
            shapes.push(Shape::Label {
                x: x_scale(t),
                y: r2(PLOT_Y1 + 20.0),
                text: TextSource::Literal(x_tick_text(t)),
                style: text_style(
                    Some(LABEL_OPACITY),
                    TextAnchor::Middle,
                    tick_size,
                    Emphasis::Normal,
                ),
            });
        }
    } else {
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
        // The top-left slot states the value axis's DISPLAY UNIT once when
        // scaling applies, and otherwise keeps the horizontal "Value" hint.
        text: TextSource::Literal(if y_display_unit.label.is_empty() {
            "Value".to_string()
        } else {
            y_display_unit.label.clone()
        }),
        style: text_style(None, TextAnchor::Start, tick_size, Emphasis::Normal),
    });

    // ── Series geometry ──
    match kind {
        ChartKind::Bar if stacked => {
            // One capped bar per category, centred in its band; series stack
            // as segments between consecutive cumulative sums (Phase 637),
            // each shortened by `STACK_SEGMENT_GAP` on the side facing the
            // next segment (Phase 875). Category-major emit order (i outer),
            // matching the reference.
            let group_w = band_w * 0.7;
            let bw = r2((group_w * 0.9).min(BAR_MAX_THICKNESS));
            for (i, category) in categories.iter().enumerate() {
                let bx = r2(PLOT_X0 + band_w * i as f64 + (band_w - bw) / 2.0);
                let cums = cums_for(i);
                for j in 0..m {
                    let y0 = y_scale(cums[j]);
                    let y1 = y_scale(cums[j + 1]);
                    // The gap comes off the far side from the baseline, and
                    // only where another segment follows — so the stack's
                    // outer tip keeps its full height and the total stays
                    // honest.
                    let gap = if j < m - 1 { STACK_SEGMENT_GAP } else { 0.0 };
                    let top = r2(y0.min(y1) + if y1 < y0 { gap } else { 0.0 });
                    let hgt = r2(((y1 - y0).abs() - gap).max(0.0));
                    shapes.push(Shape::Rectangle {
                        x: bx,
                        y: top,
                        width: bw,
                        height: hgt,
                        corner_radius: None,
                        style: with_mark(&y_fields[j], category, style_fill(colour_for(j))),
                    });
                }
            }
        }
        ChartKind::Bar => {
            let group_w = band_w * 0.7;
            let sub_w = if m > 0 { group_w / m as f64 } else { group_w };
            let bw = r2((sub_w * 0.9).min(BAR_MAX_THICKNESS));
            let base_y = y_scale(0.0);
            for (j, values) in series.iter().enumerate() {
                let colour = colour_for(j);
                for (i, &v) in values.iter().enumerate() {
                    // Centre the (possibly capped) bar in its own sub-slot, so
                    // a cap takes air off BOTH sides and the group stays
                    // symmetric about the band centre (Phase 875).
                    let slot_x =
                        PLOT_X0 + band_w * i as f64 + (band_w - group_w) / 2.0 + j as f64 * sub_w;
                    let bx = r2(slot_x + (sub_w - bw) / 2.0);
                    let vy = y_scale(v);
                    let top = vy.min(base_y);
                    let hgt = r2((vy - base_y).abs());
                    shapes.push(Shape::Rectangle {
                        x: bx,
                        y: top,
                        width: bw,
                        height: hgt,
                        corner_radius: None,
                        style: with_mark(&y_fields[j], categories[i], style_fill(colour)),
                    });
                }
            }
        }
        ChartKind::Area if stacked => {
            // Cumulative bands, bottom band first (painter's order): band j
            // fills between boundary j (below) and boundary j+1 (above); its
            // upper boundary carries the full-strength series edge (Phase 637).
            if n > 0 {
                let cums: Vec<Vec<f64>> = (0..n).map(&cums_for).collect();
                for j in 0..m {
                    let colour = colour_for(j);
                    let yf = &y_fields[j];
                    let upper: Vec<DrawPoint> = (0..n)
                        .map(|i| DrawPoint {
                            x: centre_x(i),
                            y: y_scale(cums[i][j + 1]),
                        })
                        .collect();
                    let lower = (0..n).rev().map(|i| DrawPoint {
                        x: centre_x(i),
                        y: y_scale(cums[i][j]),
                    });
                    let mut band = upper.clone();
                    band.extend(lower);
                    shapes.push(Shape::Polygon {
                        points: band,
                        style: with_series_mark(yf, style_fill_opacity(colour, AREA_FILL_OPACITY)),
                    });
                    shapes.push(Shape::Polyline {
                        points: upper,
                        style: with_series_mark(yf, style_stroke(colour, 2.0)),
                    });
                }
            }
        }
        ChartKind::Area => {
            // Overlaid baseline-closed bands in palette order (painter's
            // order: later series draw over earlier); the translucent fill
            // keeps the overlap legible, the Polyline edge keeps each series
            // distinct.
            if n > 0 {
                let base_y = y_scale(0.0);
                for (j, values) in series.iter().enumerate() {
                    let colour = colour_for(j);
                    let yf = &y_fields[j];
                    let points: Vec<DrawPoint> = values
                        .iter()
                        .enumerate()
                        .map(|(i, &v)| DrawPoint {
                            x: centre_x(i),
                            y: y_scale(v),
                        })
                        .collect();
                    let mut band = vec![DrawPoint {
                        x: centre_x(0),
                        y: base_y,
                    }];
                    band.extend(points.iter().cloned());
                    band.push(DrawPoint {
                        x: centre_x(n - 1),
                        y: base_y,
                    });
                    shapes.push(Shape::Polygon {
                        points: band,
                        style: with_series_mark(yf, style_fill_opacity(colour, AREA_FILL_OPACITY)),
                    });
                    shapes.push(Shape::Polyline {
                        points,
                        style: with_series_mark(yf, style_stroke(colour, 2.0)),
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
                    style: with_series_mark(&y_fields[j], style_stroke(colour, 2.0)),
                });
            }
        }
        ChartKind::Scatter => {
            // Fixed-radius point marks per datum (Phase 636). A non-numeric
            // x/y cell reads 0.0 (`numericOf`'s posture, shared with the
            // other arms) — grounded validation makes that loud upstream,
            // not here.
            for (j, values) in series.iter().enumerate() {
                let colour = colour_for(j);
                let yf = &y_fields[j];
                for (i, &v) in values.iter().enumerate() {
                    shapes.push(Shape::Circle {
                        cx: x_scale(x_values[i]),
                        cy: y_scale(v),
                        r: 4.0,
                        style: with_mark(yf, &format_num(x_values[i]), style_fill(colour)),
                    });
                }
            }
        }
        ChartKind::Pie | ChartKind::Heatmap => {}
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

    push_title(&mut shapes);

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
/// as a category string, the x value numerically (the Scatter arm), each
/// `y_fields` slot as a number (missing / non-numeric / non-finite reads
/// `0.0`, the reference `numericOf` with the Phase 640 non-finite guard). The
/// category mirrors the reference `projectRowFieldString` (string as-is,
/// number canonicalised, else empty).
pub fn project_row(row: &crate::canonical::JVal, x_field: &str, y_fields: &[String]) -> LowerRow {
    use crate::canonical::JVal;
    let numeric_of = |field: &str| -> f64 {
        match row.field(field) {
            Some(JVal::Num(v)) if v.is_finite() => *v,
            Some(JVal::Bool(true)) => 1.0,
            _ => 0.0,
        }
    };
    let category = match row.field(x_field) {
        Some(JVal::Str(v)) => v.clone(),
        Some(JVal::Num(v)) => format_num(*v),
        Some(JVal::Bool(v)) => v.to_string(),
        _ => String::new(),
    };
    let x_value = numeric_of(x_field);
    let values = y_fields.iter().map(|yf| numeric_of(yf)).collect();
    LowerRow {
        category,
        x_value,
        values,
    }
}
