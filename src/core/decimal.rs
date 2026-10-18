//! The exact decimal (Phase 2001) — the carrier of a `Cell::Decimal`, and every
//! piece of arithmetic the evaluator computes one with. The Core twin of the
//! reference's `DecimalText` (the canonical form, the order, the sum and the
//! nearest double) and of the evaluator's `DecimalArith` (subtraction,
//! multiplication, the truncating remainder, and the ONE rounding kernel behind
//! `Quotient`, `Rounded`, `Round`, `Floor` and `Ceil` over a decimal).
//!
//! A decimal is TEXT, never a host number: the digits are strings, so nothing
//! here overflows or rounds except where a [`RoundingMode`] says it must, and no
//! host decimal type — whose range differs by host — is involved.
//!
//! **The grammar read** is `-?[0-9]+(\.[0-9]+)?` over ASCII digits: a leading
//! `+`, a bare point (`.5`, `5.`), an exponent, a separator and white space are
//! all refused. **The canonical form written** has no leading zero on the integer
//! part (a lone `0` stands for none), no trailing zero on the fraction, no point
//! where the fraction is zero, and no sign on zero — so two texts denote one
//! number exactly when their canonical forms are one string.

use std::cmp::Ordering;

use super::dataframe::RoundingMode;
use super::number::format_finite_double;

/// The largest scale a rounding may name. A quotient is computed to the scale
/// it names one digit at a time, so the bound is a resource limit rather than a
/// statement about decimals.
pub const MAX_SCALE: i64 = 1000;

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `(negative, integer digits, fraction digits)` with the zeros stripped (an
/// empty digit string is zero), or `None` for text outside the grammar.
fn text_parts(s: &str) -> Option<(bool, &str, &str)> {
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (ip, fp) = match body.find('.') {
        None => (body, ""),
        Some(dot) => {
            let (ip, fp) = (&body[..dot], &body[dot + 1..]);
            if !is_digits(fp) {
                return None;
            }
            (ip, fp)
        }
    };
    if !is_digits(ip) {
        return None;
    }
    let ip = ip.trim_start_matches('0');
    let fp = fp.trim_end_matches('0');
    let zero = ip.is_empty() && fp.is_empty();
    Some((negative && !zero, ip, fp))
}

fn render_parts(negative: bool, ip: &str, fp: &str) -> String {
    let mut out = String::with_capacity(ip.len() + fp.len() + 2);
    if negative {
        out.push('-');
    }
    out.push_str(if ip.is_empty() { "0" } else { ip });
    if !fp.is_empty() {
        out.push('.');
        out.push_str(fp);
    }
    out
}

/// The canonical form of `s`, or `None` where `s` is not decimal text.
pub fn try_canonical(s: &str) -> Option<String> {
    text_parts(s).map(|(n, ip, fp)| render_parts(n, ip, fp))
}

/// True where `s` is decimal text already in canonical form.
pub fn is_canonical(s: &str) -> bool {
    try_canonical(s).as_deref() == Some(s)
}

/// Two magnitudes aligned at the point: fractions padded right to one scale,
/// the whole padded left to one width.
fn aligned_text(ia: &str, fa: &str, ib: &str, fb: &str) -> (String, String, usize) {
    let scale = fa.len().max(fb.len());
    let a = format!("{ia}{fa:0<scale$}");
    let b = format!("{ib}{fb:0<scale$}");
    let width = a.len().max(b.len());
    (format!("{a:0>width$}"), format!("{b:0>width$}"), scale)
}

/// The numeric order of two decimal texts, or `None` where either is not
/// decimal text. Canonical or not: `1.50` and `1.5` compare equal.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    let (na, ia, fa) = text_parts(a)?;
    let (nb, ib, fb) = text_parts(b)?;
    if na != nb {
        return Some(if na {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let (ma, mb, _) = aligned_text(ia, fa, ib, fb);
    let magnitude = ma.cmp(&mb);
    Some(if na { magnitude.reverse() } else { magnitude })
}

/// The exact sum of two decimal texts, in canonical form, or `None` where
/// either is not decimal text.
pub fn add(a: &str, b: &str) -> Option<String> {
    let (na, da, sa) = parts(a)?;
    let (nb, db, sb) = parts(b)?;
    let (scale, ma, mb) = align(sa, &da, sb, &db);
    let (n, m) = add_signed(na, &ma, nb, &mb);
    Some(render(n, &m, scale))
}

/// The nearest `f64` to a decimal text, or `None` where it is not decimal text
/// or its magnitude is past the float range — an infinity is not the nearest
/// float to any decimal. The one place the type rounds to a float.
pub fn try_to_float(s: &str) -> Option<f64> {
    try_canonical(s)
        .and_then(|c| c.parse::<f64>().ok())
        .filter(|f| f.is_finite())
}

// ─── the evaluator's arithmetic (the reference `DecimalArith`) ──────────────
//
// Every value is held as `(negative, magnitude digits, scale)` — the number
// `±digits × 10^-scale`, the digits with no leading zero (a lone `0` for zero)
// — and every answer is rendered back through the canonical form.

fn trim_leading(d: &str) -> String {
    let t = d.trim_start_matches('0');
    if t.is_empty() {
        "0".to_string()
    } else {
        t.to_string()
    }
}

fn is_zero_digits(d: &str) -> bool {
    d.bytes().all(|b| b == b'0')
}

/// `(negative, digits, scale)` of a decimal text, or `None` where it is not one.
fn parts(s: &str) -> Option<(bool, String, usize)> {
    let c = try_canonical(s)?;
    let (negative, body) = match c.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, c.as_str()),
    };
    Some(match body.find('.') {
        None => (negative, body.to_string(), 0),
        Some(dot) => (
            negative,
            trim_leading(&format!("{}{}", &body[..dot], &body[dot + 1..])),
            body.len() - dot - 1,
        ),
    })
}

/// The canonical text of `±digits × 10^-scale`.
fn render(negative: bool, digits: &str, scale: usize) -> String {
    let digits = trim_leading(digits);
    let text = if scale == 0 {
        digits.clone()
    } else {
        let padded = format!("{digits:0>width$}", width = scale + 1);
        let cut = padded.len() - scale;
        format!("{}.{}", &padded[..cut], &padded[cut..])
    };
    let signed = if negative && !is_zero_digits(&digits) {
        format!("-{text}")
    } else {
        text
    };
    try_canonical(&signed).unwrap_or(signed)
}

fn digit(b: u8) -> u32 {
    u32::from(b - b'0')
}

/// Compare two magnitudes (no leading zeros).
fn compare_mag(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn add_mag(a: &str, b: &str) -> String {
    let n = a.len().max(b.len());
    let (a, b) = (format!("{a:0>n$}"), format!("{b:0>n$}"));
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut out = vec![b'0'; n + 1];
    let mut carry = 0;
    for i in (0..n).rev() {
        let d = digit(a[i]) + digit(b[i]) + carry;
        out[i + 1] = b'0' + (d % 10) as u8;
        carry = d / 10;
    }
    out[0] = b'0' + carry as u8;
    trim_leading(std::str::from_utf8(&out).expect("ASCII digits"))
}

/// `a - b` for magnitudes with `a >= b`.
fn sub_mag(a: &str, b: &str) -> String {
    let b = format!("{b:0>width$}", width = a.len());
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut out = vec![b'0'; a.len()];
    let mut borrow = 0i32;
    for i in (0..a.len()).rev() {
        let mut d = digit(a[i]) as i32 - digit(b[i]) as i32 - borrow;
        borrow = 0;
        if d < 0 {
            d += 10;
            borrow = 1;
        }
        out[i] = b'0' + d as u8;
    }
    trim_leading(std::str::from_utf8(&out).expect("ASCII digits"))
}

fn mul_mag(a: &str, b: &str) -> String {
    if a == "0" || b == "0" {
        return "0".to_string();
    }
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let mut acc = vec![0u64; ab.len() + bb.len()];
    for i in (0..ab.len()).rev() {
        for j in (0..bb.len()).rev() {
            acc[i + j + 1] += u64::from(digit(ab[i]) * digit(bb[j]));
        }
    }
    for k in (1..acc.len()).rev() {
        acc[k - 1] += acc[k] / 10;
        acc[k] %= 10;
    }
    let text: String = acc.iter().map(|d| char::from(b'0' + *d as u8)).collect();
    trim_leading(&text)
}

/// Quotient and remainder of two magnitudes, `b` non-zero — long division, one
/// digit at a time.
fn div_mod_mag(a: &str, b: &str) -> (String, String) {
    let mut q = String::with_capacity(a.len());
    let mut r = "0".to_string();
    for c in a.chars() {
        r.push(c);
        r = trim_leading(&r);
        let mut d = 0u8;
        while compare_mag(&r, b) != Ordering::Less {
            r = sub_mag(&r, b);
            d += 1;
        }
        q.push(char::from(b'0' + d));
    }
    (trim_leading(&q), r)
}

fn shift(digits: &str, places: usize) -> String {
    if digits == "0" {
        digits.to_string()
    } else {
        format!("{digits}{}", "0".repeat(places))
    }
}

fn add_signed(na: bool, ma: &str, nb: bool, mb: &str) -> (bool, String) {
    if na == nb {
        return (na, add_mag(ma, mb));
    }
    match compare_mag(ma, mb) {
        Ordering::Equal => (false, "0".to_string()),
        Ordering::Greater => (na, sub_mag(ma, mb)),
        Ordering::Less => (nb, sub_mag(mb, ma)),
    }
}

/// Both operands at their common scale.
fn align(sa: usize, da: &str, sb: usize, db: &str) -> (usize, String, String) {
    let scale = sa.max(sb);
    (scale, shift(da, scale - sa), shift(db, scale - sb))
}

/// The negation of a decimal text.
pub fn negate(a: &str) -> Option<String> {
    parts(a).map(|(n, d, s)| render(!n, &d, s))
}

/// The magnitude of a decimal text.
pub fn abs(a: &str) -> Option<String> {
    parts(a).map(|(_, d, s)| render(false, &d, s))
}

/// The exact difference `a - b`.
pub fn sub(a: &str, b: &str) -> Option<String> {
    negate(b).and_then(|nb| add(a, &nb))
}

/// The exact product `a × b`.
pub fn mul(a: &str, b: &str) -> Option<String> {
    let (na, da, sa) = parts(a)?;
    let (nb, db, sb) = parts(b)?;
    Some(render(na != nb, &mul_mag(&da, &db), sa + sb))
}

/// The remainder of a truncating division, with the dividend's sign — the
/// integer `Mod`'s convention, and exact. `None` for a zero divisor (or text
/// that is not decimal).
pub fn rem(a: &str, b: &str) -> Option<String> {
    let (na, da, sa) = parts(a)?;
    let (_, db, sb) = parts(b)?;
    if is_zero_digits(&db) {
        return None;
    }
    let (scale, ma, mb) = align(sa, &da, sb, &db);
    let (_, r) = div_mod_mag(&ma, &mb);
    Some(render(na, &r, scale))
}

/// True where `b`'s value is zero (and it is decimal text).
pub fn is_zero(b: &str) -> bool {
    parts(b).is_some_and(|(_, d, _)| is_zero_digits(&d))
}

/// `numerator / denominator` (magnitudes, the denominator non-zero) rounded to
/// an integer under `mode`, where `negative` is the sign of the exact quotient:
/// the quotient's digits by long division, then the remainder decides the last
/// one — twice the remainder against the divisor for the three half-way modes,
/// its being non-zero for the four directed ones. ONE rounding, of the exact
/// value: never a double rounding.
fn round_quotient(
    mode: RoundingMode,
    negative: bool,
    numerator: &str,
    denominator: &str,
) -> String {
    let (q, r) = div_mod_mag(numerator, denominator);
    if r == "0" {
        return q;
    }
    let half = compare_mag(&add_mag(&r, &r), denominator);
    let bump = || add_mag(&q, "1");
    match mode {
        RoundingMode::Down => q,
        RoundingMode::Up => bump(),
        RoundingMode::Ceiling => {
            if negative {
                q
            } else {
                bump()
            }
        }
        RoundingMode::Floor => {
            if negative {
                bump()
            } else {
                q
            }
        }
        RoundingMode::HalfUp => {
            if half != Ordering::Less {
                bump()
            } else {
                q
            }
        }
        RoundingMode::HalfDown => {
            if half == Ordering::Greater {
                bump()
            } else {
                q
            }
        }
        RoundingMode::HalfEven => match half {
            Ordering::Greater => bump(),
            Ordering::Less => q,
            Ordering::Equal => {
                if digit(*q.as_bytes().last().expect("non-empty")) % 2 == 0 {
                    q
                } else {
                    bump()
                }
            }
        },
    }
}

fn in_scale(scale: i64) -> Option<usize> {
    (0..=MAX_SCALE).contains(&scale).then_some(scale as usize)
}

/// `decimal` brought to `scale` places under `mode` — the value itself where it
/// already has no more places than that. `None` where the text is not decimal
/// text, or the scale is outside `0 ..= MAX_SCALE`.
pub fn quantize(mode: RoundingMode, scale: i64, decimal: &str) -> Option<String> {
    let scale = in_scale(scale)?;
    let (n, d, s) = parts(decimal)?;
    if s <= scale {
        Some(render(n, &d, s))
    } else {
        Some(render(
            n,
            &round_quotient(mode, n, &d, &shift("1", s - scale)),
            scale,
        ))
    }
}

/// `dividend / divisor` correctly rounded to `scale` places under `mode`.
/// `Some(None)` for a zero divisor; `None` where an operand is not decimal text
/// or the scale is outside `0 ..= MAX_SCALE`.
pub fn divide(
    mode: RoundingMode,
    scale: i64,
    dividend: &str,
    divisor: &str,
) -> Option<Option<String>> {
    let scale = in_scale(scale)?;
    let (na, da, sa) = parts(dividend)?;
    let (nb, db, sb) = parts(divisor)?;
    if is_zero_digits(&db) {
        return Some(None);
    }
    // dividend / divisor = (da / db) × 10^(sb - sa); scaled by 10^scale, the
    // integer to round is da × 10^e / db with e = sb - sa + scale.
    let e = sb as i64 - sa as i64 + scale as i64;
    let (numerator, denominator) = if e >= 0 {
        (shift(&da, e as usize), db)
    } else {
        (da.clone(), shift(&db, (-e) as usize))
    };
    let negative = na != nb && !is_zero_digits(&da);
    Some(Some(render(
        negative,
        &round_quotient(mode, negative, &numerator, &denominator),
        scale,
    )))
}

/// The decimal text of a FINITE float — its shortest round-trip digits (the
/// canonical float layout) laid out without an exponent. `None` for NaN and the
/// infinities. The one place an approximation enters a decimal.
pub fn of_float(f: f64) -> Option<String> {
    if !f.is_finite() {
        return None;
    }
    let text = format_finite_double(f);
    match text.find(['E', 'e']) {
        None => try_canonical(&text),
        Some(e) => {
            let exponent: i64 = text[e + 1..].parse().ok()?;
            let (n, d, s) = parts(&text[..e])?;
            let scale = s as i64 - exponent;
            Some(if scale >= 0 {
                render(n, &d, scale as usize)
            } else {
                render(n, &shift(&d, (-scale) as usize), 0)
            })
        }
    }
}

/// The integer part of a decimal (truncated toward zero) as an `i64`, or `None`
/// past `i64`.
pub fn truncate_to_i64(a: &str) -> Option<i64> {
    let (n, d, s) = parts(a)?;
    let ip = if s >= d.len() { "0" } else { &d[..d.len() - s] };
    let v: i64 = ip.parse().ok()?;
    Some(if n { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rounding_kernel_rounds_once() {
        let q = |m, s, a, b| divide(m, s, a, b).unwrap().unwrap();
        assert_eq!(q(RoundingMode::HalfEven, 4, "1", "3"), "0.3333");
        assert_eq!(q(RoundingMode::HalfEven, 0, "5", "2"), "2");
        assert_eq!(q(RoundingMode::HalfEven, 0, "7", "2"), "4");
        assert_eq!(q(RoundingMode::HalfUp, 0, "-5", "2"), "-3");
        assert_eq!(q(RoundingMode::HalfDown, 0, "-5", "2"), "-2");
        assert_eq!(q(RoundingMode::Floor, 0, "-1", "3"), "-1");
        assert_eq!(q(RoundingMode::Ceiling, 0, "-1", "3"), "0");
        assert_eq!(q(RoundingMode::Up, 1, "0.01", "1"), "0.1");
        assert_eq!(divide(RoundingMode::Down, 2, "1", "0"), Some(None));
        assert_eq!(divide(RoundingMode::Down, 1001, "1", "3"), None);
        assert_eq!(
            quantize(RoundingMode::HalfEven, 1, "0.25").as_deref(),
            Some("0.2")
        );
        assert_eq!(rem("-7.5", "2").as_deref(), Some("-1.5"));
        assert_eq!(mul("-0.5", "0.2").as_deref(), Some("-0.1"));
        assert_eq!(of_float(1e21).as_deref(), Some("1000000000000000000000"));
        assert_eq!(of_float(1e-7).as_deref(), Some("0.0000001"));
        assert_eq!(truncate_to_i64("-12.9"), Some(-12));
    }
}
