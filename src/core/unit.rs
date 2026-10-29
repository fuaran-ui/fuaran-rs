//! The unit of measure (Phase 2208): the Core spine's unit algebra
//! (`fuaran_core::unit` — the parse, the canonical render, the product algebra
//! and `compatible`), re-exported whole, and beside it the one part the spine
//! does not port: the EXACT conversion factor between two compatible units, which
//! the dataframe evaluator's `convert` reads.
//!
//! The reference computes the factor over arbitrary-precision integers (each
//! atom's scale `num/den` relative to the coherent base, each prefix's power of
//! ten, each to its exponent, in lowest terms), then reads it into a float
//! literal as `double num / double den`. This host has no big-integer type in the
//! standard library, so the ratio is held by its PRIME FACTORISATION — every atom
//! scale and the ten factor into small primes, so a product is a sum of exponents
//! and lowest terms come for free — and each side is multiplied out into a
//! [`Natural`] only to be rounded to its nearest double, correctly (ties to even),
//! as the reference's conversion does.

use std::collections::BTreeMap;

pub use fuaran_core::unit::*;

/// Each admitted atom's scale relative to the coherent base (metre, GRAM,
/// second, ampere, kelvin, mole, candela) as `(num, den)` — the reference's
/// `AtomDef.Num` / `Den`, atom for atom. A currency has no entry: it is a
/// dimension of its own and scales by its prefix only.
const ATOM_SCALES: &[(&str, u64, u64)] = &[
    ("m", 1, 1),
    ("g", 1, 1),
    ("s", 1, 1),
    ("A", 1, 1),
    ("K", 1, 1),
    ("mol", 1, 1),
    ("cd", 1, 1),
    ("rad", 1, 1),
    ("sr", 1, 1),
    ("Hz", 1, 1),
    ("N", 1000, 1),
    ("Pa", 1000, 1),
    ("J", 1000, 1),
    ("W", 1000, 1),
    ("C", 1, 1),
    ("V", 1000, 1),
    ("F", 1, 1000),
    ("Ohm", 1000, 1),
    ("S", 1, 1000),
    ("Wb", 1000, 1),
    ("T", 1000, 1),
    ("H", 1000, 1),
    ("lm", 1, 1),
    ("lx", 1, 1),
    ("Bq", 1, 1),
    ("Gy", 1, 1),
    ("Sv", 1, 1),
    ("kat", 1, 1),
    ("L", 1, 1000),
    ("t", 1_000_000, 1),
    ("bar", 100_000_000, 1),
    ("min", 60, 1),
    ("h", 3600, 1),
    ("d", 86400, 1),
    ("wk", 604_800, 1),
    ("a", 31_557_600, 1),
    ("%", 1, 100),
    ("[ppm]", 1, 1_000_000),
    ("[ppb]", 1, 1_000_000_000),
    ("[in_i]", 127, 5000),
    ("[ft_i]", 381, 1250),
    ("[yd_i]", 1143, 1250),
    ("[mi_i]", 201_168, 125),
    ("[lb_av]", 45_359_237, 100_000),
    ("[oz_av]", 45_359_237, 1_600_000),
];

/// Each SI prefix's power of ten (the reference's `prefixPower`).
const PREFIX_POWERS: &[(&str, i64)] = &[
    ("da", 1),
    ("Y", 24),
    ("Z", 21),
    ("E", 18),
    ("P", 15),
    ("T", 12),
    ("G", 9),
    ("M", 6),
    ("k", 3),
    ("h", 2),
    ("d", -1),
    ("c", -2),
    ("m", -3),
    ("u", -6),
    ("n", -9),
    ("p", -12),
    ("f", -15),
    ("a", -18),
    ("z", -21),
    ("y", -24),
];

/// A positive rational held by its prime factorisation: prime → exponent, a
/// positive exponent in the numerator and a negative one in the denominator.
/// Two equal ratios have one map, so it is always in lowest terms.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ratio {
    primes: BTreeMap<u64, i64>,
}

/// A natural number of any size, little-endian base-2³² limbs, no trailing
/// zero limb. Enough arithmetic to multiply a ratio's side out and round it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Natural {
    limbs: Vec<u32>,
}

impl Natural {
    fn one() -> Natural {
        Natural { limbs: vec![1] }
    }

    fn mul_small(&mut self, m: u32) {
        let mut carry = 0u64;
        for limb in &mut self.limbs {
            let v = u64::from(*limb) * u64::from(m) + carry;
            *limb = v as u32;
            carry = v >> 32;
        }
        if carry > 0 {
            self.limbs.push(carry as u32);
        }
    }

    fn bits(&self) -> u64 {
        let top = *self.limbs.last().unwrap_or(&0);
        (self.limbs.len() as u64 - 1) * 32 + u64::from(32 - top.leading_zeros())
    }

    fn bit(&self, i: u64) -> bool {
        let limb = (i / 32) as usize;
        limb < self.limbs.len() && (self.limbs[limb] >> (i % 32)) & 1 == 1
    }

    /// The value as `u128` where it fits.
    pub fn to_u128(&self) -> Option<u128> {
        if self.limbs.len() > 4 {
            return None;
        }
        Some(
            self.limbs
                .iter()
                .rev()
                .fold(0u128, |acc, l| (acc << 32) | u128::from(*l)),
        )
    }

    /// The nearest double, ties to even — the correctly rounded conversion the
    /// reference's `double` of a big integer is.
    pub fn to_f64(&self) -> f64 {
        let n = self.bits();
        if n <= 64 {
            // Exact in a u64; the `as` conversion rounds to nearest, ties to even.
            return self.to_u128().unwrap_or(0) as u64 as f64;
        }
        if n > 1024 {
            return f64::INFINITY;
        }
        // The top 53 bits, then the round bit and a sticky bit over the rest.
        let shift = n - 53;
        let mut mantissa = 0u64;
        for i in (shift..n).rev() {
            mantissa = (mantissa << 1) | u64::from(self.bit(i));
        }
        let round = self.bit(shift - 1);
        let sticky = (0..shift - 1).any(|i| self.bit(i));
        if round && (sticky || mantissa & 1 == 1) {
            mantissa += 1;
        }
        // `mantissa` may have carried to 2^53, which is still exact as a double.
        let mut v = mantissa as f64;
        for _ in 0..shift {
            v *= 2.0;
        }
        v
    }
}

impl Ratio {
    fn add_factor(&mut self, mut x: u64, e: i64) {
        let mut p = 2u64;
        while x > 1 {
            if p * p > x {
                p = x;
            }
            while x % p == 0 {
                *self.primes.entry(p).or_insert(0) += e;
                x /= p;
            }
            p += if p == 2 { 1 } else { 2 };
        }
        self.primes.retain(|_, v| *v != 0);
    }

    fn side(&self, positive: bool) -> Natural {
        let mut n = Natural::one();
        for (p, e) in &self.primes {
            if (*e > 0) == positive {
                for _ in 0..e.unsigned_abs() {
                    n.mul_small(*p as u32);
                }
            }
        }
        n
    }

    /// The numerator, in lowest terms.
    pub fn numerator(&self) -> Natural {
        self.side(true)
    }

    /// The denominator, in lowest terms.
    pub fn denominator(&self) -> Natural {
        self.side(false)
    }

    /// The ratio read into a float literal as the reference reads it: each side
    /// to its nearest double, then divided.
    pub fn to_f64(&self) -> f64 {
        self.numerator().to_f64() / self.denominator().to_f64()
    }
}

/// The unit's scale relative to the coherent base — the reference's `scale`.
fn scale(u: &UnitOfMeasure) -> Ratio {
    let mut r = Ratio::default();
    let mut ten = 0i64;
    for (atom, e) in u.factors() {
        let e = i64::from(*e);
        if let Some((_, power)) = PREFIX_POWERS.iter().find(|(p, _)| *p == atom.prefix) {
            ten += power * e;
        }
        if let Some((_, num, den)) = ATOM_SCALES.iter().find(|(s, _, _)| *s == atom.symbol) {
            r.add_factor(*num, e);
            r.add_factor(*den, -e);
        }
    }
    r.add_factor(10, ten);
    r
}

/// The exact factor that turns a value in `source` into the same quantity in
/// `target` (`km/h` to `m/s` is `5/18`) — the reference's `conversionFactor`;
/// `None` where the two units are not [`compatible`].
pub fn conversion_factor(source: &UnitOfMeasure, target: &UnitOfMeasure) -> Option<Ratio> {
    if !compatible(source, target) {
        return None;
    }
    div(source, target).map(|q| scale(&q))
}
