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
//! **Legend placement (Phase 880).** ONE legend with four placements
//! (`legendPosition`: `Top | Right | Bottom | None`), the default `Right` — a
//! vertical column whose width is the MAX of its entries (bounded, truncated)
//! rather than a band whose width is their SUM (unbounded, and silently off the
//! canvas past enough entries). The cartesian arms legend their SERIES and only
//! when there is more than one; the PIE arm legends its CATEGORIES with
//! `name (NN%)` labels, which is why a single-series pie legends and a
//! single-series bar does not. Both arms now draw through the same emitter.
//!
//! **Keyed mark identity (Phase 642).** Every data-bearing shape's style is
//! stamped with a derivation-based `markId` — `series-field|category-key` on a
//! per-datum mark, the series field alone on a series-level mark (Line/Area) —
//! stable under row reorder and data refresh (object constancy). Chrome (axes,
//! gridlines, labels, legend) deliberately stays unstamped.

use crate::canonical::format_number;
use crate::wire::{
    Binding, ChartDataLabels, ChartKind, ChartLegendPosition, CurveCommand, DrawPoint, DrawStyle,
    DrawingSpec, Emphasis, Format, Shape, StaticValue, TextAnchor, TextSource, ViewBox,
};

// ─── Layout constants (the fixed canonical drawing space) ────────────────────

const W: f64 = 640.0;
const H: f64 = 400.0;
const MARGIN_TOP: f64 = 64.0; // title + legend band
const MARGIN_RIGHT: f64 = 28.0;
// Phase 879 — both of these are now the FLOOR of an autosized margin, not the
// margin itself: the left one is derived from the widest FORMATTED y tick, the
// bottom one from the drop a tilted (or vertical) category label needs.
const MARGIN_BOTTOM: f64 = 56.0; // x-axis category labels + x-axis title
const MARGIN_LEFT: f64 = 64.0; // right-aligned y-axis tick labels

// The plot rectangle is NOT a constant since Phase 879: it depends on the text
// the chart is going to print (the widest formatted y tick decides the left
// margin, the category labels' tilt decides the bottom one), so it is computed
// per lowering.

/// Ceiling on the autosized left margin, as a share of the canvas width.
const MARGIN_LEFT_MAX_SHARE: f64 = 0.3;
/// Ceiling on the autosized bottom margin, as a share of the canvas height.
const MARGIN_BOTTOM_MAX_SHARE: f64 = 0.35;
/// Breathing room between an autosized margin's content and the canvas edge —
/// also absorbs the few percent by which a real font differs from the table.
const AXIS_LABEL_PADDING: f64 = 6.0;
/// Font size of tick / category / axis-title / legend text.
const TICK_FONT_SIZE: f64 = 13.0;
/// A line's height as a multiple of its font size (Phase 879).
const TEXT_LINE_HEIGHT_FACTOR: f64 = 1.2;
/// Drop from the x-axis spine to the category / x-tick label baseline.
const CATEGORY_LABEL_OFFSET_Y: f64 = 20.0;
/// Distance from the canvas bottom to the x-axis title's BASELINE.
const AXIS_TITLE_BOTTOM_OFFSET: f64 = 12.0;
/// Font size of the subtitle (Phase 878). Deliberately BELOW the title's — the
/// subtitle is a qualifier on the title, and a qualifier set at the same size
/// competes with what it qualifies.
const SUBTITLE_FONT_SIZE: f64 = 13.0;
/// Baseline y of the subtitle — directly under the title, and sharing its
/// anchor, so the two read as one block.
const SUBTITLE_BASELINE_Y: f64 = 38.0;
/// x of the ROTATED y-axis title's baseline, measured from the canvas LEFT EDGE
/// (Phase 878) — not from the autosized margin, so the title does not slide
/// about as tick widths change. A rotated-by `-Y_AXIS_TITLE_DEGREES` label's
/// ascenders extend LEFT of its baseline, which is why this sits near the outer
/// edge of the reserved band rather than at it.
const Y_AXIS_TITLE_OFFSET_X: f64 = 18.0;
/// The MAGNITUDE of the y-axis title's rotation, in degrees (Phase 878).
/// Emitted as `rotation = -Y_AXIS_TITLE_DEGREES`: rotation is clockwise (SVG's
/// convention), so the negative angle reads BOTTOM-UP — the conventional
/// treatment, and the same sign convention `VERTICAL_TILT_DEGREES` already uses.
const Y_AXIS_TITLE_DEGREES: f64 = 90.0;
/// The MAGNITUDE of the MIDDLE RUNG of the category-label angle ladder, in
/// degrees. The ladder is fit-driven and UNIFORM per axis: flat while every label
/// fits its band, all at this angle when any does not, all vertical when this
/// angle no longer packs either. (Phase 879 read the tilt as the resting state;
/// Phase 903's correction makes it the middle rung.) `0` opts out of rotation
/// entirely — flat at every label length, never escalated instead.
const LABEL_TILT_DEGREES: f64 = 30.0;
/// The terminal rung of the ladder: one line height along the axis whatever the
/// label's length, so it packs at any category count.
const VERTICAL_TILT_DEGREES: f64 = 90.0;
// ── Legend geometry (Phase 880 — ONE legend, four placements) ───────────────
//
// Both shapes are here because both are reachable from any arm: a horizontal
// BAND (the `Top` / `Bottom` arms — Phase 879's per-entry pitch) and a vertical
// COLUMN (`Right`, the default — one row per entry, the plot shrinking by the
// column's width). The pie arm draws through exactly these constants too since
// Phase 880; its own inlined legend numbers are retired into them at their own
// values, so no pie geometry was restyled by the unification.
/// Gap from a legend swatch's left edge to its label's left edge.
const LEGEND_LABEL_OFFSET_X: f64 = 15.0;
/// BAND arms only. Horizontal padding after a legend entry's label, before the
/// next entry's swatch (Phase 879). The pitch itself is per-entry, not a fixed
/// stride.
const LEGEND_ENTRY_GAP: f64 = 24.0;
/// Side length of a (square) legend swatch.
const LEGEND_SWATCH_SIZE: f64 = 10.0;
/// Corner radius of a legend swatch.
const LEGEND_SWATCH_CORNER_RADIUS: f64 = 2.0;
/// BAND arms only. Top y of a legend swatch in the TOP band, measured from the
/// canvas top. The `Bottom` band mirrors from the canvas bottom via
/// `LEGEND_LABEL_BASELINE_DY`, so it needs no second constant.
const LEGEND_SWATCH_Y: f64 = 34.0;
/// BAND arms only. Baseline y of a legend label in the TOP band.
const LEGEND_LABEL_BASELINE_Y: f64 = 43.0;
/// COLUMN arms only. Vertical pitch between legend rows.
const LEGEND_ROW_PITCH_Y: f64 = 20.0;
/// COLUMN arms (and the `Bottom` band). Baseline nudge from a legend row's TOP
/// to its label's baseline — the relation that lets a row be placed by its top
/// edge and still read as one line.
const LEGEND_LABEL_BASELINE_DY: f64 = 9.0;
/// COLUMN arms only. Gap between the plot's edge and the legend column's
/// swatches. The column's own trailing clearance to the canvas edge is
/// `MARGIN_RIGHT`, which is what it always was.
const LEGEND_COLUMN_GAP: f64 = 16.0;
/// Ceiling on the legend column's width, as a share of `W`. Same posture as the
/// margin autosizes: a pathological series name is truncated with the
/// deterministic ellipsis rather than allowed to eat the plot. The column is
/// otherwise sized from the widest name.
const LEGEND_COLUMN_MAX_SHARE: f64 = 0.3;

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

// ── Data-label geometry (Phase 881 — the `Ends` placements) ─────────────────
//
// NONE of these feeds a margin: a data label never makes the plot smaller, it
// either fits the room the picture already has or it is suppressed. That is what
// keeps `Off` byte-identical to the pre-881 layout rather than merely visually
// similar.
//
// The font size is one point BELOW the tick size, and a constant of its own: a
// tick sits OUTSIDE the plot in a column, where a data label sits INSIDE it
// competing with the mark it describes.
const DATA_LABEL_FONT_SIZE: f64 = 12.0;
// Clearance between a bar's cap and the nearest ink of its label, in BOTH
// directions — one constant used twice, so the two placements are mirrors.
const DATA_LABEL_OFFSET_Y: f64 = 5.0;
// Clearance a label keeps from the plot edge, and half the clearance it keeps
// from its neighbour's. Feeds the fit gate only.
const DATA_LABEL_PADDING: f64 = 2.0;
// Gap from a line/area endpoint to the left edge of its label.
const DATA_LABEL_END_OFFSET_X: f64 = 6.0;
// Rise from a line/area endpoint to its label's baseline — the nudge that takes
// the text off the line it belongs to.
const DATA_LABEL_END_NUDGE_Y: f64 = 5.0;

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

// ─── Deterministic text metrics (Phase 879) ──────────────────────────────────
//
// A byte-for-byte MIRROR of the F# reference table
// (`Fuaran.UI.Charts.TextMetrics`) — mirrored, never re-derived, because the
// margins, the legend pitch and the label rotations it decides are all pinned
// by the shared `chart-lowering/*` corpus.
//
// THE APPROXIMATION IS THE SPEC. No host measures text: this one is headless
// (and, in the wasm32 arm, has no layout pass to consult either), and a
// browser's measurement depends on which member of the font stack actually
// resolved — either would make the lowering's output a function of the host,
// destroying the byte-identical cross-host property the corpus rests on. So the
// widths come from a FIXED table of per-character advance widths as a fraction
// of the font size (em), approximating a typical sans-serif. A real font
// differs by a few percent; `AXIS_LABEL_PADDING` absorbs it.
//
//   1. Five width classes; an unlisted character (including every non-ASCII
//      one) takes the DEFAULT, which is what makes the table total.
//   2. Width = font_size × Σ advance_em(ch), summed LEFT TO RIGHT (float
//      addition is not associative — the order is part of the spec), rounded
//      once at the end.
//   3. Line height = font_size × TEXT_LINE_HEIGHT_FACTOR.
//   4. Truncation keeps the longest prefix that still fits with the ellipsis;
//      when nothing fits the result is a bare `…`, never the empty string.

const THIN_EM: f64 = 0.28;
const NARROW_EM: f64 = 0.33;
const DEFAULT_EM: f64 = 0.55;
const WIDE_EM: f64 = 0.7;
const EXTRA_WIDE_EM: f64 = 0.9;
const ELLIPSIS: &str = "…";

/// One character's advance width as a fraction of the font size. Total: an
/// unlisted character takes `DEFAULT_EM`, so no host enumerates Unicode.
fn advance_em(ch: char) -> f64 {
    match ch {
        ' ' | '!' | '\'' | ',' | '.' | ':' | ';' | 'I' | 'i' | 'j' | 'l' | '|' => THIN_EM,
        '"' | '(' | ')' | '*' | '-' | '/' | '\\' | '[' | ']' | '{' | '}' | 'f' | 'r' | 't' => {
            NARROW_EM
        }
        '%' | '@' | 'M' | 'W' | 'm' => EXTRA_WIDE_EM,
        'J' | 'L' => DEFAULT_EM,
        'A'..='Z' | 'w' => WIDE_EM,
        _ => DEFAULT_EM,
    }
}

/// A string's advance width in em — summed LEFT TO RIGHT (rule 2).
fn advance_em_of(text: &str) -> f64 {
    let mut acc = 0.0;
    for ch in text.chars() {
        acc += advance_em(ch);
    }
    acc
}

/// The estimated rendered width of `text` at `font_size`, rounded once.
fn text_width(font_size: f64, text: &str) -> f64 {
    r2(font_size * advance_em_of(text))
}

/// The estimated line height at `font_size` (rule 3).
fn text_line_height(font_size: f64, line_height_factor: f64) -> f64 {
    r2(font_size * line_height_factor)
}

/// Does `text` fit a box `max_width` × `max_height` at `font_size`? The single
/// predicate a data-label gate answers inside/outside/suppress with, so a label
/// can never disagree with the margin that made room for it.
pub fn text_fits_box(
    font_size: f64,
    line_height_factor: f64,
    max_width: f64,
    max_height: f64,
    text: &str,
) -> bool {
    text_width(font_size, text) <= max_width
        && text_line_height(font_size, line_height_factor) <= max_height
}

/// Deterministic ellipsis truncation to `max_width` (rule 4). A string that
/// already fits comes back unchanged, so a host that never hits a bound never
/// sees a `…`.
fn truncate_to_width(font_size: f64, max_width: f64, text: &str) -> String {
    if text_width(font_size, text) <= max_width {
        return text.to_string();
    }
    let budget = max_width - text_width(font_size, ELLIPSIS);
    if budget < 0.0 {
        return ELLIPSIS.to_string();
    }
    let mut acc = 0.0;
    let mut take = 0usize;
    for (i, ch) in text.char_indices() {
        let next = acc + advance_em(ch);
        if r2(font_size * next) > budget {
            break;
        }
        acc = next;
        take = i + ch.len_utf8();
    }
    format!("{}{}", &text[..take], ELLIPSIS)
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
        rotation: None,
    }
}

/// A text-label style carrying a rotation (Phase 879): the clockwise rotation
/// in degrees about the label's own anchor point. Omitted from the wire when
/// `None`, so an unrotated drawing is byte-unchanged.
fn text_style_rotated(
    opacity: Option<f64>,
    anchor: TextAnchor,
    size: f64,
    emphasis: Emphasis,
    rotation: f64,
) -> DrawStyle {
    DrawStyle {
        rotation: Some(rotation),
        ..text_style(opacity, anchor, size, emphasis)
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
    /// Phase 880 — which edge the legend occupies when the `ChartSpec` does not
    /// say: the DEFAULT, not the answer. An explicit
    /// [`ChartTitles::legend_position`] beats it, because WHERE the legend goes
    /// is the author's meaning where the geometry realising it is the host's.
    pub legend_position: ChartLegendPosition,
}

impl Default for ChartLowerStyle {
    fn default() -> Self {
        Self {
            axis_unit_mode: ChartAxisUnitMode::default(),
            display_unit_min_exponent: DISPLAY_UNIT_MIN_EXPONENT,
            // The shipped default is the vertical RIGHT column (operator decision
            // 2026-08-16): a band's width is the SUM of its entries and runs off a
            // 640 px canvas once the names are long enough or numerous enough,
            // silently and with no ellipsis; a column's width is their MAX,
            // bounded by `LEGEND_COLUMN_MAX_SHARE` and truncated at it, and its
            // height is one pitch per entry into 400 px. Neither term grows
            // without limit, so the eight-slot palette legends itself by
            // construction rather than by luck of naming.
            legend_position: ChartLegendPosition::Right,
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
        &ChartTitles::default(),
        None,
        &ChartLowerStyle::default(),
        rows,
    )
}

/// The author's own optional `ChartSpec` declarations beyond the main title —
/// the Phase-878 axis names + subtitle, and the Phase-880 legend placement. All
/// optional, all WIRE declarations, grouped so the lowering's parameter list
/// stays readable; every resolution (the capitalised-field-name fallback, the
/// style default behind an absent placement) happens inside the lowering, never
/// here.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChartTitles<'a> {
    pub x_title: Option<&'a TextSource>,
    pub y_title: Option<&'a TextSource>,
    pub subtitle: Option<&'a TextSource>,
    /// Phase 880 — which edge the legend occupies. Absent means the host style's
    /// default ([`ChartLowerStyle::legend_position`], which ships as `Right`),
    /// NOT "no legend": suppression is the explicit `ChartLegendPosition::None`.
    pub legend_position: Option<ChartLegendPosition>,
    /// Phase 881 — whether the values are written onto the picture. Absent means
    /// `ChartDataLabels::Off`, which is ALSO the default, so an absent field lowers
    /// to the pre-881 picture byte-for-byte. `Ends` labels bar CAPS (a stacked
    /// bar's TOTAL only) and LINE/AREA ENDPOINTS, and there is no third value.
    pub data_labels: Option<ChartDataLabels>,
}

/// Lower under an explicit value-axis `Format` (a wire declaration) and an
/// explicit style (a host choice) — Phase 876; the Phase-878 axis names +
/// subtitle ride alongside.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub fn lower_chart_with(
    kind: ChartKind,
    stacked: bool,
    x_field: &str,
    y_fields: &[String],
    title: Option<&TextSource>,
    titles: &ChartTitles<'_>,
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

    let tick_size = TICK_FONT_SIZE;
    let title_size = 18.0;

    // ── Text-metric layout (Phase 879) ───────────────────────────────────────
    //
    // ORDER IS LOAD-BEARING. The plot rectangle used to be six consts; it is now
    // DERIVED from the text the chart prints — the widest formatted y tick
    // decides the left margin, and the category labels' tilt decides the bottom
    // one. So: the left margin, the band pitch that follows from it, the tilt,
    // and the bottom margin the tilt needs, in that order.

    let line_height = text_line_height(tick_size, TEXT_LINE_HEIGHT_FACTOR);
    let widest_of = |texts: &[String]| -> f64 {
        texts
            .iter()
            .fold(0.0f64, |acc, t| acc.max(text_width(tick_size, t)))
    };

    // ── Legend placement (Phase 880) ─────────────────────────────────────────
    //
    // ONE legend with four placements, resolved HERE — above the margins,
    // because a `Right` legend's column width is an INPUT to the plot rectangle
    // and a `Bottom` legend's band is an input to the bottom margin. The same
    // acyclicity discipline the text metrics established: everything the layout
    // reads is computed before the layout that reads it.
    //
    // The pie arm's guard + shares are resolved here for the same reason: its
    // legend labels carry them ("name (NN%)"), so they are layout input, not
    // output.
    let is_pie = matches!(kind, ChartKind::Pie);
    let pie_values: &[f64] = if is_pie && m == 1 { &series[0] } else { &[] };
    let pie_total: f64 = pie_values.iter().sum();
    // The Phase-638 bounded-v1 guard, unchanged and merely lifted: exactly one
    // series, no negative value, a positive total. A refused pie draws no
    // geometry AND no legend — a legend for a picture that was refused would be
    // a claim about data the drawing declined to show.
    let pie_refused = is_pie && (m != 1 || pie_values.iter().any(|&v| v < 0.0) || pie_total <= 0.0);
    let pie_fractions: Vec<f64> = if is_pie && !pie_refused {
        pie_values.iter().map(|v| v / pie_total).collect()
    } else {
        vec![]
    };

    // The legend's rows in draw order — `(colour, label)`. TWO sources, ONE
    // shape, which is what Phase 880 unified: the cartesian arms legend their
    // SERIES and only when there is more than one (with a single series the
    // title already names it — the pre-880 rule, preserved exactly), while the
    // pie arm legends its CATEGORIES, which is why a single-series pie legends
    // and a single-series bar does not. Before this phase these were two
    // separate emitters with two separate constant sets, and only one of them
    // could have honoured a position.
    let legend_entries: Vec<(&'static str, String)> = if is_pie {
        pie_fractions
            .iter()
            .enumerate()
            .map(|(i, f)| {
                // Routed through the canonical formatter (Phase 876) — one
                // rounding + rendering rule for every number this module prints.
                // A share is a whole percent, so the shipped `NN%` shape is
                // unchanged.
                let pct = format_value(None, 1.0, false, 1.0, f * 100.0);
                (colour_for(i), format!("{} ({pct}%)", categories[i]))
            })
            .collect()
    } else if m > 1 {
        (0..m)
            .map(|j| (colour_for(j), y_fields[j].clone()))
            .collect()
    } else {
        vec![]
    };

    // The placement actually used: the author's explicit `ChartSpec` value where
    // there is one, else the host style's default. With no entries at all the
    // answer is `None` whatever either of them said — so an explicit position on
    // a single-series chart still draws nothing and, more to the point, reserves
    // no space.
    let legend_pos = if legend_entries.is_empty() {
        ChartLegendPosition::None
    } else {
        titles.legend_position.unwrap_or(style.legend_position)
    };

    // COLUMN arms: the widest label decides the column, bounded by
    // `LEGEND_COLUMN_MAX_SHARE` of the canvas and truncated beyond it — the
    // margin autosizes' posture, adopted for the same reason. A name with no
    // bound is a data problem the layout should report by truncating, not absorb
    // by shrinking the picture.
    let legend_name_budget =
        (LEGEND_COLUMN_MAX_SHARE * W - LEGEND_LABEL_OFFSET_X - LEGEND_COLUMN_GAP).max(0.0);
    let legend_texts: Vec<String> = legend_entries
        .iter()
        .map(|(_, t)| match legend_pos {
            ChartLegendPosition::Right => truncate_to_width(tick_size, legend_name_budget, t),
            // The band arms pack at each entry's natural width and still run off
            // the right edge past enough entries. Truncating there would not fix
            // it (the overflow is in the SUM, not in one name), so the band is
            // left as Phase 879 shipped it and the default moved.
            _ => t.clone(),
        })
        .collect();

    let legend_column_w = match legend_pos {
        ChartLegendPosition::Right => {
            r2(LEGEND_COLUMN_GAP + LEGEND_LABEL_OFFSET_X + widest_of(&legend_texts))
        }
        _ => 0.0,
    };

    // The `Bottom` band's height — one line plus its padding, reserved BELOW
    // everything the bottom margin's autosize already accounts for (the x-axis
    // title's line included), so the two computations never contend for the same
    // pixels. The exact mirror of `subtitle_band` at the top: one term that
    // shifts the whole band, present only when the arm is.
    let legend_band_h = match legend_pos {
        ChartLegendPosition::Bottom => r2(line_height + AXIS_LABEL_PADDING),
        _ => 0.0,
    };

    // ── Axis names + subtitle (Phase 878) ────────────────────────────────────
    //
    // Resolved HERE, before any margin, because both margins have to reserve a
    // line for text whose presence is decided by these three fields — the left
    // margin for the rotated y-axis title, the top margin for the subtitle. The
    // same dependency Phase 879 established when the bottom margin started
    // reserving the x-axis title's line.
    //
    // An axis title is the author's own `TextSource` when declared, else the
    // capitalised field name — which is exactly what the x axis has always
    // drawn, now stated once and applied to both axes. `None` only where there
    // is no honest fallback: an empty field name, or a y axis carrying no
    // series at all.
    let axis_title_of =
        |declared: Option<&TextSource>, fallback_field: &str| -> Option<TextSource> {
            match declared {
                Some(t) => Some(t.clone()),
                None if fallback_field.is_empty() => None,
                None => Some(TextSource::Literal(capitalise(fallback_field))),
            }
        };

    let x_title = axis_title_of(titles.x_title, x_field);

    // The y fallback is the capitalised FIRST y-field. It is the honest answer
    // to "what is on this axis", where the retired `"Value"` literal named
    // neither the measure nor its unit — and it makes ONE rule cover both axes
    // rather than a rule for x and a constant for y. The multi-series chart is
    // the case it serves least well; there the legend already names every
    // series, and an author plotting genuinely different measures should
    // declare `yTitle`, which is precisely why the field exists.
    let y_title = axis_title_of(titles.y_title, y_fields.first().map_or("", String::as_str));

    // ── Top margin ──
    // A subtitle takes one line under the visible title, and EVERYTHING below
    // it in the top band moves down by exactly that line: the legend row, the
    // display-unit slot, and the plot itself (so on the Pie arm the wedge
    // centre moves too). Reserved only when a subtitle is present, so a chart
    // without one keeps the pre-878 layout byte-for-byte.
    let subtitle_band = if titles.subtitle.is_some() {
        text_line_height(SUBTITLE_FONT_SIZE, TEXT_LINE_HEIGHT_FACTOR)
    } else {
        0.0
    };
    let margin_top = r2(MARGIN_TOP + subtitle_band);

    // ── Left margin ──
    // The truncation budget is derived from the CEILING — a constant — so the
    // truncation that feeds the margin never depends on the margin it decides.
    let left_ceiling = MARGIN_LEFT_MAX_SHARE * W;
    // Phase 878 — the rotated y-axis title occupies one LINE of the left margin,
    // outboard of the tick column. Only its line height (plus the padding beside
    // it) is reserved here: the title is rotated, so its LENGTH runs vertically
    // and is bounded against the plot height further down. That is what keeps
    // this acyclic — exactly the shape Phase 879 gave the x-axis title's line in
    // the bottom margin.
    let y_title_band = if y_title.is_some() {
        line_height + AXIS_LABEL_PADDING
    } else {
        0.0
    };
    let tick_text_budget =
        (left_ceiling - TICK_LABEL_GAP - AXIS_LABEL_PADDING - y_title_band).max(0.0);
    let y_tick_label_text =
        |v: f64| -> String { truncate_to_width(tick_size, tick_text_budget, &y_tick_text(v)) };
    let tick_label_texts: Vec<String> = ticks.iter().map(|t| y_tick_label_text(*t)).collect();
    let required_left =
        TICK_LABEL_GAP + widest_of(&tick_label_texts) + AXIS_LABEL_PADDING + y_title_band;
    let margin_left = r2(MARGIN_LEFT.max(left_ceiling.min(required_left)));

    let plot_x0 = margin_left;
    // Phase 880 — a `Right` legend takes its column off the PLOT, not off the
    // right margin: the margin stays the clearance between the legend's widest
    // label and the canvas edge, exactly as it was the clearance to the plot
    // before. Every other placement leaves `legend_column_w = 0`, so the pre-880
    // rectangle is recovered term-for-term.
    let plot_x1 = W - MARGIN_RIGHT - legend_column_w;
    let plot_w = plot_x1 - plot_x0;

    let band_w = if n > 0 { plot_w / n as f64 } else { plot_w };
    let centre_x = |i: usize| -> f64 { r2(plot_x0 + band_w * (i as f64 + 0.5)) };
    // The `i`th BAND BOUNDARY — `n` bands have `n+1` of them, boundary `0` on the
    // y-axis spine and boundary `n` on the plot's right edge. Phase 903's category
    // tick marks land here, where a label lands at `centre_x`.
    let boundary_x = |i: usize| -> f64 { r2(plot_x0 + band_w * i as f64) };

    // ── The category-label ANGLE LADDER (Phase 903, correcting Phase 879) ──
    // Only the BAND arms label categories: Scatter labels numeric x ticks (short
    // by construction, left horizontal) and Pie has no x axis. Both must
    // therefore contribute NO drop, or their bottom margin — and with it the
    // pie's centre — would move for a decision they never take.
    let draws_category_labels = !is_scatter && !matches!(kind, ChartKind::Pie);

    // A rotated label's footprint ALONG the axis is `w·cos θ + h·sin θ`. At 0°
    // that is the bare width (`cos 0 = 1`, `sin 0 = 0`, both exact on every
    // IEEE-754 host, so the flat rung needs no special case); at 90° the width
    // term vanishes, so the vertical rung takes one line height per label at any
    // count — which is why it is terminal.
    let along_axis_footprint = |deg: f64, w: f64| -> f64 {
        w * deg.to_radians().cos() + line_height * deg.to_radians().sin()
    };

    let category_strings: Vec<String> = categories.iter().map(|c| (*c).to_string()).collect();

    // THREE RUNGS, ONE PREDICATE, applied to the WIDEST label and therefore
    // UNIFORMLY to the axis: flat while every label fits its band, 30° when it
    // does not, vertical when 30° no longer packs either. Deciding on the widest
    // label rather than per-label is what keeps an axis from mixing angles.
    //
    // Decided on the labels AS AUTHORED (`category_strings`, not
    // `category_texts`): the truncation budget below is a function of the angle,
    // so reading truncated text here would be circular as well as wrong.
    let widest_category = widest_of(&category_strings);
    let packs_at = |deg: f64| -> bool { along_axis_footprint(deg, widest_category) <= band_w };

    let tilt_degrees = if !draws_category_labels || n == 0 || LABEL_TILT_DEGREES <= 0.0 {
        // A zero angle is FLAT-ALWAYS, not "the ladder with a flat rung": a host
        // that zeroed it named the one rotation the ladder may use, so escalating
        // past it to vertical would override an explicit choice with a computed
        // one.
        0.0
    } else if packs_at(0.0) {
        0.0
    } else if packs_at(LABEL_TILT_DEGREES) {
        LABEL_TILT_DEGREES
    } else {
        VERTICAL_TILT_DEGREES
    };

    // ── Bottom margin ──
    // Below the plot, top to bottom: the label offset, the tilted label's drop
    // (`w·sin θ`), the padding, the x-axis title's own LINE (its offset measures
    // to its BASELINE, so the glyphs above it need reserving separately), and
    // that offset. Same ceiling-then-truncate posture as the left margin.
    let sin_tilt = tilt_degrees.to_radians().sin();
    let bottom_ceiling = MARGIN_BOTTOM_MAX_SHARE * H;
    let drop_ceiling = (bottom_ceiling
        - CATEGORY_LABEL_OFFSET_Y
        - AXIS_LABEL_PADDING
        - line_height
        - AXIS_TITLE_BOTTOM_OFFSET)
        .max(0.0);
    let category_text_budget = if sin_tilt > 0.0 {
        drop_ceiling / sin_tilt
    } else {
        f64::INFINITY
    };
    let category_texts: Vec<String> = if draws_category_labels {
        category_strings
            .iter()
            .map(|c| truncate_to_width(tick_size, category_text_budget, c))
            .collect()
    } else {
        vec![]
    };
    let required_bottom = CATEGORY_LABEL_OFFSET_Y
        + sin_tilt * widest_of(&category_texts)
        + AXIS_LABEL_PADDING
        + line_height
        + AXIS_TITLE_BOTTOM_OFFSET;
    // Phase 880 — the `Bottom` legend's band is ADDED to the autosized margin
    // rather than competing inside its ceiling: the ceiling exists to stop LABELS
    // eating the plot, and the legend is not a label. So the picture shrinks by
    // the band, and the tilt escalation still sees the budget it had.
    let margin_bottom = r2(legend_band_h + MARGIN_BOTTOM.max(bottom_ceiling.min(required_bottom)));

    let plot_y0 = margin_top;
    let plot_y1 = H - margin_bottom;
    let plot_h = plot_y1 - plot_y0;

    let y_scale = |v: f64| -> f64 { r2(plot_y1 - (v - nice_lo) / (nice_hi - nice_lo) * plot_h) };

    let x_scale =
        |v: f64| -> f64 { r2(plot_x0 + (v - x_nice_lo) / (x_nice_hi - x_nice_lo) * plot_w) };

    let mut shapes: Vec<Shape> = Vec::new();

    /// Bound a title to the extent it runs along. Only a `Literal` can be
    /// truncated — the text behind a `Bound` or `I18n` arm is not known here —
    /// and that is the honest boundary: those pass through and may overrun,
    /// which is a visible fact rather than a silently wrong measurement.
    fn bound_text(font_size: f64, extent: f64, t: &TextSource) -> TextSource {
        match t {
            TextSource::Literal(s) => TextSource::Literal(truncate_to_width(font_size, extent, s)),
            other => other.clone(),
        }
    }

    // ── Visible title (a Label — bigger + emphasised) ──
    let push_title = |shapes: &mut Vec<Shape>| {
        if let Some(t) = title {
            shapes.push(Shape::Label {
                x: r2(plot_x0),
                y: 22.0,
                text: t.clone(),
                style: text_style(None, TextAnchor::Start, title_size, Emphasis::Loud),
            });
        }
    };

    // ── Subtitle (Phase 878) — the muted line under the title ──
    //
    // MUTED (label-role opacity, not full-strength ink) and SMALLER than the
    // title, sharing its x and its anchor, so the pair reads as one block and
    // the subtitle is unmistakably subordinate. It draws independently of the
    // title: an author who sets one and not the other gets what they asked for,
    // and the top margin has already reserved the line either way.
    let push_subtitle = |shapes: &mut Vec<Shape>| {
        if let Some(s) = titles.subtitle {
            shapes.push(Shape::Label {
                x: r2(plot_x0),
                y: SUBTITLE_BASELINE_Y,
                text: bound_text(SUBTITLE_FONT_SIZE, plot_w, s),
                style: text_style(
                    Some(LABEL_OPACITY),
                    TextAnchor::Start,
                    SUBTITLE_FONT_SIZE,
                    Emphasis::Normal,
                ),
            });
        }
    };

    // ── Legend (Phase 880) — one entry list, four placements ──
    //
    // COLUMN (`Right`, the shipped default): one row per entry, each a swatch and
    // its label, the plot already shrunk by the column above. Rows are
    // TOP-ALIGNED with the plot rather than vertically centred, deliberately:
    // centring makes row j's y a function of the entry COUNT, so adding a series
    // moves every row that was already there — chrome sliding under a data
    // refresh is precisely what this module's mark-identity rule exists to avoid,
    // and there is no reason to reintroduce it for the legend. Reading order is
    // also series order, which is the order the rows are in.
    //
    // This is what structurally retires the overflow. A BAND's width is the SUM of
    // its entries, so it runs off the canvas once the names are long enough or
    // numerous enough, silently and with no ellipsis. A COLUMN's width is the MAX
    // of its entries — bounded by `LEGEND_COLUMN_MAX_SHARE` and truncated at it —
    // and its height is one pitch per entry into 400 px of canvas. Neither term
    // grows without limit, so the eight-slot palette's eight-series chart legends
    // itself by construction rather than by luck of naming.
    //
    // BAND (`Top` / `Bottom`): Phase 879's horizontal row, entries laid out
    // cumulatively from the plot's left edge at each entry's own natural width —
    // unchanged for `Top`, which is the pre-880 shape every pre-880 golden pins.
    // It still runs off the right edge past enough entries; that survives only on
    // the arms an author asks for explicitly.
    let push_legend = |shapes: &mut Vec<Shape>| {
        // ONE row emitter for all four placements: a swatch at `x`/`swatch_y` and
        // its label one `LEGEND_LABEL_OFFSET_X` to the right on `baseline_y`.
        let legend_row =
            |shapes: &mut Vec<Shape>, x: f64, swatch_y: f64, baseline_y: f64, j: usize| {
                shapes.push(Shape::Rectangle {
                    x: r2(x),
                    y: r2(swatch_y),
                    width: LEGEND_SWATCH_SIZE,
                    height: LEGEND_SWATCH_SIZE,
                    corner_radius: Some(LEGEND_SWATCH_CORNER_RADIUS),
                    style: style_fill(legend_entries[j].0),
                });
                shapes.push(Shape::Label {
                    x: r2(x + LEGEND_LABEL_OFFSET_X),
                    y: r2(baseline_y),
                    text: TextSource::Literal(legend_texts[j].clone()),
                    style: text_style(
                        Some(LABEL_OPACITY),
                        TextAnchor::Start,
                        tick_size,
                        Emphasis::Normal,
                    ),
                });
            };
        match legend_pos {
            ChartLegendPosition::None => {}
            ChartLegendPosition::Right => {
                let swatch_x = plot_x1 + LEGEND_COLUMN_GAP;
                for j in 0..legend_entries.len() {
                    let row_top = plot_y0 + LEGEND_ROW_PITCH_Y * j as f64;
                    legend_row(
                        shapes,
                        swatch_x,
                        row_top,
                        row_top + LEGEND_LABEL_BASELINE_DY,
                        j,
                    );
                }
            }
            ChartLegendPosition::Top | ChartLegendPosition::Bottom => {
                // Phase 878 — the TOP band sits BELOW the subtitle, so it moves
                // down by the line the subtitle took; `subtitle_band` is 0 without
                // one, leaving the pre-878 constants exactly where they were. The
                // BOTTOM band mirrors from the canvas bottom off the band the
                // margin already reserved, so it needs no constants of its own.
                let (swatch_y, baseline_y) = match legend_pos {
                    ChartLegendPosition::Bottom => {
                        let row_top = H - legend_band_h;
                        (row_top, row_top + LEGEND_LABEL_BASELINE_DY)
                    }
                    _ => (
                        LEGEND_SWATCH_Y + subtitle_band,
                        LEGEND_LABEL_BASELINE_Y + subtitle_band,
                    ),
                };
                // Prefix sums — entry j starts where every earlier entry ended.
                let mut lx_acc = plot_x0;
                for (j, text) in legend_texts.iter().enumerate() {
                    let lx = r2(lx_acc);
                    lx_acc +=
                        LEGEND_LABEL_OFFSET_X + text_width(tick_size, text) + LEGEND_ENTRY_GAP;
                    legend_row(shapes, lx, swatch_y, baseline_y, j);
                }
            }
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
    //
    // Phase 880 — this emits WEDGES ONLY. The pie's legend was the vertical
    // right-hand column the cartesian arms have now converged on, so it is
    // emitted by the shared `push_legend` (from the shared `legend_entries`,
    // which carry the shares) and honours the placement like any other arm. The
    // guard + the shares themselves were lifted above the margins, because the
    // legend's width is layout input.
    if is_pie {
        if !pie_refused {
            let cx = r2((plot_x0 + plot_x1) / 2.0);
            let cy = r2((plot_y0 + plot_y1) / 2.0);
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

            let starts: Vec<f64> = {
                let mut acc = 0.0;
                let mut out = Vec::with_capacity(pie_fractions.len() + 1);
                out.push(0.0);
                for f in &pie_fractions {
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
            for (i, &f) in pie_fractions.iter().enumerate() {
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
        }
        // Painter's order on the polar arm since Phase 880: wedges, then the
        // shared legend, then the titles — the same slot the legend takes on
        // every cartesian arm.
        push_legend(&mut shapes);
        push_title(&mut shapes);
        push_subtitle(&mut shapes);
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
            x1: r2(plot_x0),
            y1: y,
            x2: r2(plot_x1),
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
                y1: r2(plot_y0),
                x2: x,
                y2: r2(plot_y1),
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
            x1: r2(plot_x0),
            y1: y,
            x2: r2(plot_x1),
            y2: y,
            style: style_stroke_ink(AXIS_OPACITY, 1.0),
        });
    }

    // ── Axes ──
    shapes.push(Shape::Line {
        x1: r2(plot_x0),
        y1: r2(plot_y0),
        x2: r2(plot_x0),
        y2: r2(plot_y1),
        style: style_stroke_ink(AXIS_OPACITY, 1.0),
    });
    shapes.push(Shape::Line {
        x1: r2(plot_x0),
        y1: r2(plot_y1),
        x2: r2(plot_x1),
        y2: r2(plot_y1),
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
                x1: r2(plot_x0 - TICK_MARK_LENGTH),
                y1: y,
                x2: r2(plot_x0),
                y2: y,
                style: style_stroke_ink(AXIS_OPACITY, 1.0),
            });
        }
        let x_mark = |x: f64| -> Shape {
            Shape::Line {
                x1: x,
                y1: r2(plot_y1),
                x2: x,
                y2: r2(plot_y1 + TICK_MARK_LENGTH),
                style: style_stroke_ink(AXIS_OPACITY, 1.0),
            }
        };
        // BAND vs CONTINUOUS (Phase 903). Where the axis is CONTINUOUS a tick
        // marks a VALUE and sits at it: the y axis above, and Scatter's numeric
        // x. Where it is a BAND axis a tick DELIMITS a group, so the `n+1` marks
        // land on the band BOUNDARIES and the label stays centred between two of
        // them — the category-axis convention, and the honest one: a category has
        // an extent, not a position, so a mark under its centre claims a
        // coordinate the axis does not have.
        if is_scatter {
            for &t in &x_ticks {
                shapes.push(x_mark(x_scale(t)));
            }
        } else if n > 0 {
            for i in 0..=n {
                shapes.push(x_mark(boundary_x(i)));
            }
        }
    }

    // ── y-axis tick labels — right-anchored (End) ──
    for &t in &ticks {
        shapes.push(Shape::Label {
            x: r2(plot_x0 - TICK_LABEL_GAP),
            y: r2(y_scale(t) + 4.0),
            // The margin-bounded text (Phase 879): whatever the margin was
            // sized for is exactly what gets drawn.
            text: TextSource::Literal(y_tick_label_text(t)),
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
                y: r2(plot_y1 + CATEGORY_LABEL_OFFSET_Y),
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
        // Every category label sits at its band CENTRE — including since Phase
        // 903, when the tick marks moved to the boundaries: the label names the
        // band, the marks delimit it.
        //
        // The ANCHOR follows the ladder's rung. At the FLAT rung a label is
        // `Middle`-anchored on the band centre (the pre-879 convention,
        // restored). At either ROTATED rung it is `End`-anchored at the same
        // point and rotated NEGATIVELY (counter-clockwise, against `rotation`'s
        // clockwise convention): the anchor is the pivot, so the text ENDS under
        // the band centre and runs back down-and-left, reading up-to-the-right
        // into it. The opposite sign would swing the same text up into the plot
        // area. At 90° this degenerates to reading bottom-up.
        for (i, c) in category_texts.iter().enumerate() {
            shapes.push(Shape::Label {
                x: centre_x(i),
                y: r2(plot_y1 + CATEGORY_LABEL_OFFSET_Y),
                text: TextSource::Literal(c.clone()),
                style: if tilt_degrees > 0.0 {
                    text_style_rotated(
                        Some(LABEL_OPACITY),
                        TextAnchor::End,
                        tick_size,
                        Emphasis::Normal,
                        r2(-tilt_degrees),
                    )
                } else {
                    text_style(
                        Some(LABEL_OPACITY),
                        TextAnchor::Middle,
                        tick_size,
                        Emphasis::Normal,
                    )
                },
            });
        }
    }

    // ── Axis titles + the display-unit slot (Phase 878) ──
    //
    // Three rules, and together they retire the hardcoded `"Value"`:
    //
    //   1. NAMES. The x title stays centred under the tick band (where it has
    //      always been); the y title is ROTATED by `-Y_AXIS_TITLE_DEGREES` in
    //      the left margin, centred on the plot, reading BOTTOM-UP — the
    //      conventional treatment, and the same sign convention Phase 879's
    //      vertical category labels already use. Each falls back to its
    //      capitalised field name, so an axis is never nameless.
    //
    //   2. UNITS KEEP THEIR OWN SLOT. The top-left label states the Phase-876
    //      display unit and NOTHING else: with no scaling in play it is not
    //      drawn at all, where it previously fell back to the literal `"Value"`
    //      — a word naming neither the measure nor its unit, printed on every
    //      chart in the corpus. Composing the unit INTO the rotated title
    //      ("Revenue (Millions of £)") was the alternative and was rejected:
    //      that concatenation is only expressible when the title is a
    //      `Literal`, so a bound or i18n title would silently fall back to a
    //      different layout — and a layout rule with a shape that depends on
    //      which `TextSource` arm an author reached for is not a rule. Two
    //      slots, always the same two, is what stays total.
    //
    //   3. DEDUPE. An explicit `subtitle` SUPPRESSES the unit slot. The
    //      subtitle is the author's own place to say "£m", and the machine
    //      restating it two lines away is exactly the clutter this rule exists
    //      to prevent — so the author's sentence wins. PRESENCE is the whole
    //      test: no string comparison, which is what keeps the rule total over
    //      every `TextSource` arm and identical on every host.
    //
    // A SELF-EVIDENT DATE AXIS SUPPRESSES ITS DEFAULT TITLE — an axis reading
    // "Jan Feb Mar" does not need the word "Month" beneath it. The rule is
    // recorded here and is WIRED when the temporal axis lands: nothing in the
    // lowering can currently tell a date column from a string one, and
    // inferring it from the label text would be a guess dressed as a rule. It
    // will apply to the FALLBACK only — an explicit `xTitle` is the author
    // overriding the default, and always draws.
    if let Some(t) = &x_title {
        shapes.push(Shape::Label {
            x: r2((plot_x0 + plot_x1) / 2.0),
            // Phase 880 — the x title rides ABOVE a `Bottom` legend band, keeping
            // its own inset from whatever is beneath it. `legend_band_h` is 0 on
            // every other arm, so the pre-880 baseline is unchanged.
            y: r2(H - legend_band_h - AXIS_TITLE_BOTTOM_OFFSET),
            text: bound_text(tick_size, plot_w, t),
            style: text_style(None, TextAnchor::Middle, tick_size, Emphasis::Normal),
        });
    }
    if let Some(t) = &y_title {
        // `Middle`-anchored at the plot's vertical centre: the anchor is the
        // pivot, so the rotated text stays centred on the axis it names,
        // whatever its length. The x is measured from the CANVAS edge, not the
        // autosized margin, so the title does not slide as tick widths change.
        shapes.push(Shape::Label {
            x: r2(Y_AXIS_TITLE_OFFSET_X),
            y: r2((plot_y0 + plot_y1) / 2.0),
            text: bound_text(tick_size, plot_h, t),
            style: text_style_rotated(
                None,
                TextAnchor::Middle,
                tick_size,
                Emphasis::Normal,
                r2(-Y_AXIS_TITLE_DEGREES),
            ),
        });
    }
    if !y_display_unit.label.is_empty() && titles.subtitle.is_none() {
        shapes.push(Shape::Label {
            x: r2(8.0),
            y: r2(plot_y0 - 12.0),
            text: TextSource::Literal(y_display_unit.label.clone()),
            style: text_style(None, TextAnchor::Start, tick_size, Emphasis::Normal),
        });
    }

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
                let bx = r2(plot_x0 + band_w * i as f64 + (band_w - bw) / 2.0);
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
                        plot_x0 + band_w * i as f64 + (band_w - group_w) / 2.0 + j as f64 * sub_w;
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

    // ── Data labels (Phase 881) — the values, written selectively ────────────
    //
    // Two states and no third: `Off` (the default, and what an absent field means)
    // and `Ends`. There is deliberately NO all-points mode — a number on every
    // interior point is the clutter this vocabulary exists to prevent, so the API
    // cannot express it. `Ends` names the placements that read on their own:
    //
    //   * BARS label the CAP — above a positive cap, below a negative one, the two
    //     exact mirrors about the cap.
    //   * A GROUPED bar labels every bar. A STACKED bar labels the TOTAL at the
    //     stack cap and nothing else: an interior segment's value is unreadable
    //     against the segment above it, and the legend plus the hover readout
    //     already serve it.
    //   * LINES and AREA EDGES label the LAST point of each series, right of the
    //     endpoint and nudged up off the line.
    //   * SCATTER gets nothing in v1 (recorded decision): a scatter's x IS a value
    //     axis, so its last ROW carries no meaning its first does not, and
    //     labelling by row order would present an accident of the feed as a
    //     reading of the chart.
    //   * PIE is unchanged — its legend already carries `name (NN%)`.
    //
    // Every value goes through `y_tick_text`, so a label and a tick agree by
    // construction. NO LABEL EVER MOVES A MARGIN: the plot rectangle is decided
    // long before this point, so a label either fits the room the picture already
    // has or it is SUPPRESSED — never clipped, never overlapped, never relocated
    // inside the bar.
    if titles.data_labels.unwrap_or(ChartDataLabels::Off) == ChartDataLabels::Ends {
        let data_label_line = text_line_height(DATA_LABEL_FONT_SIZE, TEXT_LINE_HEIGHT_FACTOR);

        // The single fit gate: `text_fits_box` against the room the placement
        // actually has. Returns whether the label was admitted, which the endpoint
        // arm needs so a suppressed label does not claim a separation slot.
        let push_data_label = |shapes: &mut Vec<Shape>,
                               anchor: TextAnchor,
                               x: f64,
                               baseline: f64,
                               max_width: f64,
                               max_height: f64,
                               text: String|
         -> bool {
            if !text_fits_box(
                DATA_LABEL_FONT_SIZE,
                TEXT_LINE_HEIGHT_FACTOR,
                max_width,
                max_height,
                &text,
            ) {
                return false;
            }
            // Label-role ink at the chrome opacity — NEVER the series colour: a
            // value is a reading of the mark, not a second copy of its identity.
            shapes.push(Shape::Label {
                x: r2(x),
                y: r2(baseline),
                text: TextSource::Literal(text),
                style: text_style(
                    Some(LABEL_OPACITY),
                    anchor,
                    DATA_LABEL_FONT_SIZE,
                    Emphasis::Normal,
                ),
            });
            true
        };

        // A value at a bar's cap, centred on `cx`. `pitch` is the distance to the
        // NEXT label's centre — the neighbouring bar's slot — so the budget is what
        // separates two labels rather than what fits one bar: a label may
        // legitimately be wider than the bar it caps, and may not be wider than the
        // room beside it.
        let push_cap_label = |shapes: &mut Vec<Shape>, cx: f64, pitch: f64, v: f64| {
            let cap_y = y_scale(v);
            let max_width = (pitch - 2.0 * DATA_LABEL_PADDING).max(0.0);
            if v < 0.0 {
                push_data_label(
                    shapes,
                    TextAnchor::Middle,
                    cx,
                    cap_y + DATA_LABEL_OFFSET_Y + DATA_LABEL_FONT_SIZE,
                    max_width,
                    plot_y1 - cap_y - DATA_LABEL_OFFSET_Y - DATA_LABEL_PADDING,
                    y_tick_text(v),
                );
            } else {
                push_data_label(
                    shapes,
                    TextAnchor::Middle,
                    cx,
                    cap_y - DATA_LABEL_OFFSET_Y,
                    max_width,
                    cap_y - plot_y0 - DATA_LABEL_OFFSET_Y - DATA_LABEL_PADDING,
                    y_tick_text(v),
                );
            }
        };

        match kind {
            ChartKind::Bar if stacked => {
                // The TOTAL at the stack cap, once per category.
                let bar_group_w = band_w * 0.7;
                let bw = r2((bar_group_w * 0.9).min(BAR_MAX_THICKNESS));
                for i in 0..n {
                    let bx = r2(plot_x0 + band_w * i as f64 + (band_w - bw) / 2.0);
                    push_cap_label(&mut shapes, bx + bw / 2.0, band_w, cums_for(i)[m]);
                }
            }
            ChartKind::Bar => {
                let bar_group_w = band_w * 0.7;
                let sub_w = if m > 0 {
                    bar_group_w / m as f64
                } else {
                    bar_group_w
                };
                let bw = r2((sub_w * 0.9).min(BAR_MAX_THICKNESS));
                for (j, values) in series.iter().enumerate() {
                    for (i, &v) in values.iter().enumerate() {
                        let slot_x = plot_x0
                            + band_w * i as f64
                            + (band_w - bar_group_w) / 2.0
                            + j as f64 * sub_w;
                        let bx = r2(slot_x + (sub_w - bw) / 2.0);
                        push_cap_label(&mut shapes, bx + bw / 2.0, sub_w, v);
                    }
                }
            }
            ChartKind::Line | ChartKind::Area if n > 0 => {
                // The series-endpoint labels, in series order. Two gates, the second
                // the vertical analogue of the cap labels' pitch: every endpoint
                // label shares one x, so the thing they collide with is each other.
                // A label is admitted only when its line clears every
                // ALREADY-ADMITTED one — series order decides who yields, which
                // makes the outcome deterministic and identical on every host.
                //
                // A stacked area's labelled value is the CUMULATIVE boundary,
                // because that is the edge that was drawn; the series' own datum is
                // nowhere on the picture.
                let last_cums = cums_for(n - 1);
                let label_x = centre_x(n - 1) + DATA_LABEL_END_OFFSET_X;
                // The budget runs to the PLOT's right edge, not the canvas's: beyond
                // it lies the legend column, and running into it is the collision
                // the gate refuses.
                let max_width = (plot_x1 - label_x - DATA_LABEL_PADDING).max(0.0);
                let stacked_area = stacked && kind == ChartKind::Area;
                let mut admitted: Vec<f64> = Vec::new();
                for j in 0..m {
                    let v = if stacked_area {
                        last_cums[j + 1]
                    } else {
                        series[j][n - 1]
                    };
                    let baseline = y_scale(v) - DATA_LABEL_END_NUDGE_Y;
                    if !admitted
                        .iter()
                        .all(|b| (b - baseline).abs() >= data_label_line + DATA_LABEL_PADDING)
                    {
                        continue;
                    }
                    if push_data_label(
                        &mut shapes,
                        TextAnchor::Start,
                        label_x,
                        baseline,
                        max_width,
                        baseline - plot_y0 - DATA_LABEL_PADDING,
                        y_tick_text(v),
                    ) {
                        admitted.push(baseline);
                    }
                }
            }
            _ => {}
        }
    }

    // ── Legend (Phase 880) — the shared emitter, in the slot it always had ──
    push_legend(&mut shapes);

    push_title(&mut shapes);
    push_subtitle(&mut shapes);

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
