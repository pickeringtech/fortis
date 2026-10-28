//! `Decimal` (Language Reference 6.4): a sign, a 96-bit coefficient and a scale 0..=28; value = ±coefficient /
//! 10^scale. Every operation is exact and bit-for-bit equal to the definitional interpreter (interpreter/numeric.py):
//! add, sub and mul keep the larger (or summed) scale, reducing it only when the coefficient would not fit; div
//! uses the largest scale up to 28 that fits; results round half to even.

use std::cmp::Ordering;
use std::fmt;

use crate::big::Big;

pub const MAX_COEFF: u128 = (1u128 << 96) - 1;
pub const MAX_SCALE: u32 = 28;

/// Equality and hashing are numeric (1.0 == 1.00): both use the value with trailing zeros removed.
#[derive(Clone, Copy, Debug)]
pub struct Decimal {
    pub neg: bool,
    pub coeff: u128,
    pub scale: u32,
}

/// A decimal operation that cannot produce a representable result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecError {
    Overflow,
    DivideByZero,
}

impl DecError {
    pub fn code(self) -> &'static str {
        match self {
            DecError::Overflow => "numeric.overflow",
            DecError::DivideByZero => "numeric.divide_by_zero",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    HalfEven,
    HalfAwayFromZero,
    TowardZero,
    AwayFromZero,
}

impl Rounding {
    pub fn from_name(n: &str) -> Option<Rounding> {
        Some(match n {
            "HalfEven" => Rounding::HalfEven,
            "HalfAwayFromZero" => Rounding::HalfAwayFromZero,
            "TowardZero" => Rounding::TowardZero,
            "AwayFromZero" => Rounding::AwayFromZero,
            _ => return None,
        })
    }
}

/// An exact rational ±num/den, den > 0.
struct Ratio {
    neg: bool,
    num: Big,
    den: Big,
}

/// Round num/den to an integer with the given mode.
fn round_div(num: &Big, den: &Big, neg: bool, mode: Rounding) -> Big {
    let (q, r) = num.divmod(den);
    if r.is_zero() {
        return q;
    }
    let up = match mode {
        Rounding::TowardZero => false,
        Rounding::AwayFromZero => true,
        Rounding::HalfEven | Rounding::HalfAwayFromZero => {
            let twice = r.mul_small(2);
            match twice.cmp(den) {
                Ordering::Greater => true,
                Ordering::Less => false,
                Ordering::Equal => mode == Rounding::HalfAwayFromZero || q.to_decimal_string().ends_with(['1', '3', '5', '7', '9']),
            }
        }
    };
    let _ = neg;
    if up {
        q.add(&Big::from_u128(1))
    } else {
        q
    }
}

impl Decimal {
    pub fn new(neg: bool, coeff: u128, scale: u32) -> Result<Decimal, DecError> {
        if coeff > MAX_COEFF || scale > MAX_SCALE {
            return Err(DecError::Overflow);
        }
        Ok(Decimal { neg: neg && coeff != 0, coeff, scale })
    }

    pub fn zero() -> Decimal {
        Decimal { neg: false, coeff: 0, scale: 0 }
    }

    pub fn from_int(v: i128) -> Result<Decimal, DecError> {
        Decimal::new(v < 0, v.unsigned_abs(), 0)
    }

    /// `12.50` (with an optional sign, `_` separators and a trailing `d`).
    pub fn parse(text: &str) -> Result<Decimal, DecError> {
        let t: String = text.trim_end_matches('d').chars().filter(|c| *c != '_').collect();
        let (neg, t) = match t.strip_prefix('-') {
            Some(r) => (true, r.to_string()),
            None => (false, t),
        };
        let (ip, fp) = match t.split_once('.') {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (t.clone(), String::new()),
        };
        let digits = format!("{ip}{fp}");
        let coeff: u128 = digits.parse().map_err(|_| DecError::Overflow)?;
        Decimal::new(neg, coeff, fp.len() as u32)
    }

    fn ratio(&self) -> Ratio {
        Ratio { neg: self.neg, num: Big::from_u128(self.coeff), den: Big::pow10(self.scale) }
    }

    /// The largest scale <= `scale` whose coefficient fits, rounding half to even (interpreter `Dec.fit`).
    fn fit(r: &Ratio, scale: u32) -> Result<Decimal, DecError> {
        let mut s = scale.min(MAX_SCALE) as i64;
        while s >= 0 {
            let c = round_div(&r.num.mul(&Big::pow10(s as u32)), &r.den, r.neg, Rounding::HalfEven);
            if let Some(c) = c.to_u128() {
                if c <= MAX_COEFF {
                    return Ok(Decimal { neg: r.neg && c != 0, coeff: c, scale: s as u32 });
                }
            }
            s -= 1;
        }
        Err(DecError::Overflow)
    }

    /// Round to exactly `scale` digits with `mode`, failing when the coefficient does not fit (`Dec.from_value`).
    fn quantize(r: &Ratio, scale: u32, mode: Rounding) -> Result<Decimal, DecError> {
        let c = round_div(&r.num.mul(&Big::pow10(scale)), &r.den, r.neg, mode);
        match c.to_u128() {
            Some(c) if c <= MAX_COEFF && scale <= MAX_SCALE => Ok(Decimal { neg: r.neg && c != 0, coeff: c, scale }),
            _ => Err(DecError::Overflow),
        }
    }

    fn add_ratio(a: &Ratio, b: &Ratio, negate_b: bool) -> Ratio {
        // a.num/a.den ± b.num/b.den over the common denominator a.den*b.den
        let x = a.num.mul(&b.den);
        let y = b.num.mul(&a.den);
        let bneg = b.neg != negate_b;
        let den = a.den.mul(&b.den);
        if a.neg == bneg {
            Ratio { neg: a.neg, num: x.add(&y), den }
        } else {
            match x.cmp(&y) {
                Ordering::Less => Ratio { neg: bneg, num: y.sub(&x), den },
                _ => Ratio { neg: a.neg, num: x.sub(&y), den },
            }
        }
    }

    pub fn add(&self, o: &Decimal) -> Result<Decimal, DecError> {
        Decimal::fit(&Decimal::add_ratio(&self.ratio(), &o.ratio(), false), self.scale.max(o.scale))
    }
    pub fn sub(&self, o: &Decimal) -> Result<Decimal, DecError> {
        Decimal::fit(&Decimal::add_ratio(&self.ratio(), &o.ratio(), true), self.scale.max(o.scale))
    }
    pub fn mul(&self, o: &Decimal) -> Result<Decimal, DecError> {
        let r = Ratio { neg: self.neg != o.neg, num: Big::from_u128(self.coeff).mul(&Big::from_u128(o.coeff)), den: Big::pow10(self.scale + o.scale) };
        Decimal::fit(&r, self.scale + o.scale)
    }
    pub fn div(&self, o: &Decimal) -> Result<Decimal, DecError> {
        if o.coeff == 0 {
            return Err(DecError::DivideByZero);
        }
        let r = Ratio { neg: self.neg != o.neg, num: Big::from_u128(self.coeff).mul(&Big::pow10(o.scale)), den: Big::from_u128(o.coeff).mul(&Big::pow10(self.scale)) };
        Decimal::fit(&r, MAX_SCALE)
    }
    /// The remainder with the sign of the dividend: a - trunc(a/b) * b.
    pub fn rem(&self, o: &Decimal) -> Result<Decimal, DecError> {
        if o.coeff == 0 {
            return Err(DecError::DivideByZero);
        }
        let s = self.scale.max(o.scale);
        let a = Big::from_u128(self.coeff).mul(&Big::pow10(s - self.scale));
        let b = Big::from_u128(o.coeff).mul(&Big::pow10(s - o.scale));
        let (_, r) = a.divmod(&b);
        let r = Ratio { neg: self.neg, num: r, den: Big::pow10(s) };
        Decimal::fit(&r, s)
    }
    pub fn round(&self, scale: i128, mode: Rounding) -> Result<Decimal, DecError> {
        if !(0..=MAX_SCALE as i128).contains(&scale) {
            return Err(DecError::Overflow);
        }
        Decimal::quantize(&self.ratio(), scale as u32, mode)
    }
    /// The decimal nearest a float's shortest representation, rounded to `scale` (interpreter `dec_from_float`).
    pub fn from_float(x: f64, scale: i128, mode: Rounding) -> Result<Decimal, DecError> {
        if !x.is_finite() || !(0..=MAX_SCALE as i128).contains(&scale) {
            return Err(DecError::Overflow);
        }
        let text = crate::float::repr_digits(x);
        let r = parse_ratio(&text).ok_or(DecError::Overflow)?;
        Decimal::quantize(&r, scale as u32, mode)
    }
    pub fn negate(&self) -> Decimal {
        Decimal { neg: !self.neg && self.coeff != 0, ..*self }
    }
}

/// `-1.25e-7` style text as an exact ratio.
fn parse_ratio(text: &str) -> Option<Ratio> {
    let (neg, t) = match text.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, text),
    };
    let (mant, exp) = match t.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i64>().ok()?),
        None => (t, 0),
    };
    let (ip, fp) = mant.split_once('.').unwrap_or((mant, ""));
    let digits = format!("{ip}{fp}");
    let mut num = Big::zero();
    for c in digits.chars() {
        num = num.mul_small(10).add(&Big::from_u128(c.to_digit(10)? as u128));
    }
    let e = exp - fp.len() as i64;
    let (num, den) = if e >= 0 { (num.mul(&Big::pow10(e as u32)), Big::from_u128(1)) } else { (num, Big::pow10((-e) as u32)) };
    Some(Ratio { neg, num, den })
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, o: &Decimal) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Decimal {
    /// Numeric order (1.0 == 1.00).
    fn cmp(&self, o: &Decimal) -> Ordering {
        let sa = if self.coeff == 0 { 0 } else if self.neg { -1 } else { 1 };
        let sb = if o.coeff == 0 { 0 } else if o.neg { -1 } else { 1 };
        if sa != sb {
            return sa.cmp(&sb);
        }
        let x = Big::from_u128(self.coeff).mul(&Big::pow10(o.scale));
        let y = Big::from_u128(o.coeff).mul(&Big::pow10(self.scale));
        let m = x.cmp(&y);
        if sa < 0 {
            m.reverse()
        } else {
            m
        }
    }
}

impl Decimal {
    /// Numeric equality (1.0 == 1.00).
    pub fn num_eq(&self, o: &Decimal) -> bool {
        self.cmp(o) == Ordering::Equal
    }
    /// The same value with no trailing fractional zeros.
    pub fn normalized(&self) -> Decimal {
        let (mut c, mut s) = (self.coeff, self.scale);
        while s > 0 && c % 10 == 0 {
            c /= 10;
            s -= 1;
        }
        Decimal { neg: self.neg && c != 0, coeff: c, scale: s }
    }
}

impl PartialEq for Decimal {
    fn eq(&self, o: &Decimal) -> bool {
        self.num_eq(o)
    }
}
impl Eq for Decimal {}
impl std::hash::Hash for Decimal {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        let n = self.normalized();
        (n.neg, n.coeff, n.scale).hash(h);
    }
}

impl fmt::Display for Decimal {
    /// The coefficient with exactly `scale` fractional digits (interpreter `Dec.display`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.scale as usize;
        let mut digits = self.coeff.to_string();
        while digits.len() < s + 1 {
            digits.insert(0, '0');
        }
        if s > 0 {
            digits.insert(digits.len() - s, '.');
        }
        write!(f, "{}{}", if self.neg { "-" } else { "" }, digits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::parse(s).unwrap()
    }

    #[test]
    fn scale_rules_match_the_interpreter() {
        assert_eq!(d("1.10").add(&d("2.205")).unwrap().to_string(), "3.305");
        assert_eq!(d("1.5").mul(&d("2.25")).unwrap().to_string(), "3.375");
        assert_eq!(d("1").div(&d("3")).unwrap().to_string(), "0.3333333333333333333333333333");
        assert_eq!(d("2").div(&d("3")).unwrap().to_string(), "0.6666666666666666666666666667");
        assert_eq!(d("10").div(&d("4")).unwrap().to_string(), "2.5000000000000000000000000000");
        assert_eq!(d("-7.5").rem(&d("2")).unwrap().to_string(), "-1.5");
        assert_eq!(d("0.1").sub(&d("0.3")).unwrap().to_string(), "-0.2");
        assert_eq!(d("1").div(&d("0")), Err(DecError::DivideByZero));
    }

    #[test]
    fn rounding_modes() {
        assert_eq!(d("2.345").round(2, Rounding::HalfEven).unwrap().to_string(), "2.34");
        assert_eq!(d("2.355").round(2, Rounding::HalfEven).unwrap().to_string(), "2.36");
        assert_eq!(d("2.345").round(2, Rounding::HalfAwayFromZero).unwrap().to_string(), "2.35");
        assert_eq!(d("-2.349").round(2, Rounding::TowardZero).unwrap().to_string(), "-2.34");
        assert_eq!(d("-2.341").round(2, Rounding::AwayFromZero).unwrap().to_string(), "-2.35");
        assert_eq!(d("5").round(29, Rounding::HalfEven), Err(DecError::Overflow));
        assert_eq!(Decimal::from_float(0.1, 3, Rounding::HalfEven).unwrap().to_string(), "0.100");
    }

    #[test]
    fn overflow_and_order() {
        let big = Decimal::new(false, MAX_COEFF, 0).unwrap();
        assert_eq!(big.add(&d("1")), Err(DecError::Overflow));
        assert!(d("1.0").num_eq(&d("1.00")));
        assert_eq!(d("1.0"), d("1.00"));
        let h = |x: Decimal| {
            use std::hash::{Hash, Hasher};
            let mut s = std::collections::hash_map::DefaultHasher::new();
            x.hash(&mut s);
            s.finish()
        };
        assert_eq!(h(d("2.50")), h(d("2.5")));
        assert!(d("-2") < d("1.5"));
        assert_eq!(d("-0.0").to_string(), "0.0");
    }
}
