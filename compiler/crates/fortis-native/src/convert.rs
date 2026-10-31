//! The built-in conversions of the `as` operator (Language Reference 6.13, X-475), as native code runs them. The
//! lowering (fortis-lower `intrinsics`) writes a widening, a `String` and a `Bytes` as plain Rust; the rows that
//! can fail, and the parsers of the canonical text forms, are here. A fallible row gives an `Outcome`, as a `try`
//! intrinsic does, with the failures 6.13 names: `conversion.out_of_range`, `conversion.not_integral`,
//! `conversion.not_exact`, `conversion.parse` and `conversion.not_utf8`. The interpreter (interpreter/convert.py)
//! and the VM (fortis-vm `convert`) build the same failures with the same payloads.

use crate::failure::{Dyn, Failure, Outcome};
use fortis_runtime::Decimal;

pub const MAX_SCALAR: i128 = 0x10FFFF;

fn out_of_range<T>(n: i128, lo: i128, hi: i128, origin: &'static str) -> Outcome<T> {
    Err(Failure::new("conversion.out_of_range", origin).with("value", Dyn::Int(n)).with("low", Dyn::Int(lo)).with("high", Dyn::Int(hi)))
}

/// An integer into a narrower integer type: the value when it lies in `[lo, hi]`, else `conversion.out_of_range`.
#[inline]
pub fn narrow<T: TryFrom<i128>>(n: i128, lo: i128, hi: i128, origin: &'static str) -> Outcome<T> {
    if n < lo || n > hi {
        return out_of_range(n, lo, hi, origin);
    }
    match T::try_from(n) {
        Ok(v) => Ok(v),
        Err(_) => out_of_range(n, lo, hi, origin),
    }
}

/// An integer as a Decimal: total (every integer's coefficient fits 96 bits).
#[inline]
pub fn int_to_decimal(n: i128) -> Decimal {
    Decimal::from_int(n).unwrap_or_else(|_| Decimal::zero())
}

/// A Decimal into an integer type: `conversion.not_integral` for a fractional part, `conversion.out_of_range`
/// beyond `[lo, hi]`.
pub fn decimal_to_int<T: TryFrom<i128>>(d: &Decimal, lo: i128, hi: i128, origin: &'static str) -> Outcome<T> {
    let unit = 10u128.pow(d.scale);
    if d.coeff % unit != 0 {
        return Err(Failure::new("conversion.not_integral", origin).with("value", Dyn::Str(d.to_string())));
    }
    let q = (d.coeff / unit) as i128;
    narrow(if d.neg { -q } else { q }, lo, hi, origin)
}

/// An integer as a Float: the value when a binary64 holds it exactly, else `conversion.not_exact`.
#[inline]
pub fn int_to_float(n: i128, origin: &'static str) -> Outcome<f64> {
    let f = n as f64;
    if f as i128 != n {
        return Err(Failure::new("conversion.not_exact", origin).with("value", Dyn::Int(n)));
    }
    Ok(f)
}

pub use fortis_runtime::convert::{canonical_decimal, canonical_int};
use fortis_runtime::convert::{integer_text, IntegerText};

fn parse_failure<T>(text: &str, origin: &'static str) -> Outcome<T> {
    Err(Failure::new("conversion.parse", origin).with("text", Dyn::Str(text.to_string())))
}

/// The canonical text of an integer type: the value, `conversion.parse` for any other text, and
/// `conversion.out_of_range` for a canonical integer beyond `[lo, hi]`.
pub fn parse_int<T: TryFrom<i128>>(text: &str, lo: i128, hi: i128, origin: &'static str) -> Outcome<T> {
    match integer_text(text) {
        IntegerText::Value(n) => narrow(n, lo, hi, origin),
        IntegerText::OutOfRange => Err(Failure::new("conversion.out_of_range", origin)
            .with("value", Dyn::Str(text.to_string())).with("low", Dyn::Int(lo)).with("high", Dyn::Int(hi))),
        IntegerText::Invalid => parse_failure(text, origin),
    }
}

pub fn parse_decimal(text: &str, origin: &'static str) -> Outcome<Decimal> {
    canonical_decimal(text).ok_or_else(|| Failure::new("conversion.parse", origin).with("text", Dyn::Str(text.to_string())))
}

pub fn parse_bool(text: &str, origin: &'static str) -> Outcome<bool> {
    match text {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => parse_failure(text, origin),
    }
}

/// Bytes as text: the String when they are UTF-8, else `conversion.not_utf8` at the first byte that is not.
pub fn bytes_to_string(bytes: &[u8], origin: &'static str) -> Outcome<String> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_string()),
        Err(e) => Err(Failure::new("conversion.not_utf8", origin).with("position", Dyn::Int(e.valid_up_to() as i128))),
    }
}

/// An integer as a Scalar: `conversion.out_of_range` for a surrogate or a value beyond U+10FFFF.
pub fn int_to_scalar(n: i128, origin: &'static str) -> Outcome<char> {
    if (0..=MAX_SCALAR).contains(&n) {
        if let Some(c) = char::from_u32(n as u32) {
            return Ok(c);
        }
    }
    out_of_range(n, 0, MAX_SCALAR, origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrowing_and_parsing() {
        assert_eq!(narrow::<i8>(100, -128, 127, "t").unwrap(), 100i8);
        assert_eq!(narrow::<i8>(300, -128, 127, "t").err().map(|f| f.code), Some("conversion.out_of_range"));
        assert_eq!(parse_int::<i64>("42", i64::MIN as i128, i64::MAX as i128, "t").unwrap(), 42);
        assert_eq!(parse_int::<i64>("007", i64::MIN as i128, i64::MAX as i128, "t").err().map(|f| f.code), Some("conversion.parse"));
        assert_eq!(parse_decimal("1.50", "t").unwrap().to_string(), "1.50");
        assert!(parse_decimal("-0", "t").is_err());
        assert_eq!(decimal_to_int::<i64>(&Decimal::parse("3.00").unwrap(), i64::MIN as i128, i64::MAX as i128, "t").unwrap(), 3);
        assert_eq!(decimal_to_int::<i64>(&Decimal::parse("1.5").unwrap(), i64::MIN as i128, i64::MAX as i128, "t").err().map(|f| f.code), Some("conversion.not_integral"));
        assert_eq!(int_to_float(1 << 53, "t").unwrap(), 9007199254740992.0);
        assert_eq!(int_to_float((1 << 53) + 1, "t").err().map(|f| f.code), Some("conversion.not_exact"));
        assert_eq!(int_to_float(i64::MAX as i128, "t").err().map(|f| f.code), Some("conversion.not_exact"));
        assert_eq!(bytes_to_string(&[0x68, 0xff], "t").err().map(|f| f.code), Some("conversion.not_utf8"));
        assert_eq!(int_to_scalar(65, "t").unwrap(), 'A');
        assert!(int_to_scalar(0xD800, "t").is_err());
    }
}
