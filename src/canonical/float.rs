//! The canonical number formatter — the `.NET Double.ToString("R", InvariantCulture)`
//! layout the Fuaran wire form requires (`WIRE_FORMAT.md` §5).

/// Renders a finite `f64` in the canonical Fuaran wire form, matching
/// `.NET Double.ToString("R", InvariantCulture)`:
///
/// - shortest round-trip significant digits (Rust's shortest float formatting
///   shares the .NET / V8 / CPython / Go digit sequence — the shortest decimal that
///   round-trips to a given `f64` is unique, so every correct shortest algorithm
///   agrees);
/// - fixed-point layout iff the leading-digit exponent is in `[-4, 16]`;
/// - otherwise scientific with an uppercase `E`, an always-present exponent sign,
///   and a ≥2-digit zero-padded exponent;
/// - negative zero collapses to `"0"`.
///
/// The caller must ensure `f` is finite — `NaN` and `±Inf` are not representable on
/// the wire and are rejected upstream. This mirrors `fuaran-go`'s
/// `FormatFiniteDouble`, `fuaran-py`'s `format_finite_double`, and the F# host's
/// `formatFiniteDouble`; the corpus `metric-float-*` fixtures pin the
/// divergence-zone values.
pub fn format_finite_double(f: f64) -> String {
    if f == 0.0 {
        return "0".to_string(); // collapses -0.0 to 0 (−0.0 == 0.0 in IEEE 754)
    }

    // Shortest round-trip digits via the scientific form `d[.ddd]e±XX`.
    let mut s = format!("{f:e}");

    let neg = s.starts_with('-');
    if neg {
        s.remove(0);
    }

    let e_idx = s.find('e').expect("{:e} always emits an 'e'");
    let mantissa = &s[..e_idx];
    let exp: i32 = s[e_idx + 1..].parse().expect("{:e} exponent is an integer");

    let (int_part, frac_part) = match mantissa.find('.') {
        Some(dot) => (&mantissa[..dot], &mantissa[dot + 1..]),
        None => (mantissa, ""),
    };
    // Significant digits; the first digit has place value 10^exp.
    let digits = format!("{int_part}{frac_part}");

    let out = if (-4..=16).contains(&exp) {
        fixed_point(&digits, exp)
    } else {
        scientific(&digits, exp)
    };

    if neg { format!("-{out}") } else { out }
}

/// Lays significant digits out with a decimal point, where the first digit has
/// place value `10^exp` (`exp` in `[-4, 16]`).
fn fixed_point(digits: &str, exp: i32) -> String {
    if exp >= 0 {
        let int_len = (exp + 1) as usize;
        if digits.len() <= int_len {
            format!("{digits}{}", "0".repeat(int_len - digits.len()))
        } else {
            format!("{}.{}", &digits[..int_len], &digits[int_len..])
        }
    } else {
        // exp < 0: `0.<(-exp-1) zeros><digits>`
        format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
    }
}

/// Lays significant digits out as `d[.ddd]E±XX` with the .NET exponent conventions
/// (uppercase `E`, mandatory sign, ≥2 zero-padded exponent digits).
fn scientific(digits: &str, exp: i32) -> String {
    let mant = if digits.len() == 1 {
        digits.to_string()
    } else {
        format!("{}.{}", &digits[..1], &digits[1..])
    };

    let (sign, mag) = if exp < 0 { ('-', -exp) } else { ('+', exp) };
    let exp_str = format!("{mag:02}"); // ≥2 digits, zero-padded

    format!("{mant}E{sign}{exp_str}")
}

#[cfg(test)]
mod tests {
    use super::format_finite_double;

    /// Pins the canonical number form against the corpus divergence-zone values
    /// (`WIRE_FORMAT.md` §5; the `metric-float-*` fixtures) plus a spread of
    /// fixed-point / scientific boundary cases — the same vectors the `fuaran-go`
    /// and `fuaran-py` hosts verify.
    #[test]
    fn format_finite_double_matches_corpus() {
        let cases: &[(f64, &str)] = &[
            // Corpus divergence-zone (metric-float-*).
            (1e21, "1E+21"),
            (1e-7, "1E-07"),
            (0.30000000000000004, "0.30000000000000004"),
            (1.2345678901234568e17, "1.2345678901234568E+17"),
            (5e-324, "5E-324"), // smallest positive subnormal
            (-0.0, "0"),        // negative zero collapses
            // Fixed-point range [-4, 16].
            (0.0, "0"),
            (1.0, "1"),
            (100.0, "100"),
            (123.5, "123.5"),
            (0.5, "0.5"),
            (0.0001, "0.0001"), // exp = -4, last fixed-point value
            (-42.25, "-42.25"),
            // Scientific just outside the range.
            (0.00001, "1E-05"), // exp = -5, first scientific value
        ];
        for &(input, want) in cases {
            assert_eq!(
                format_finite_double(input),
                want,
                "format_finite_double({input})"
            );
        }
    }
}
