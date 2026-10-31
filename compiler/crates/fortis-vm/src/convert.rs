//! The built-in conversions of the `as` operator (Language Reference 6.13, X-475): the rows of the table in
//! fortis-types `conversions`, run by name (`as:<kind>:<source>:<target>`) on the operand's value. A total row
//! gives the value; a fallible row gives the value or a failure value, as a `try` intrinsic does, with the
//! failures 6.13 names: `conversion.out_of_range`, `conversion.not_integral`, `conversion.not_exact`,
//! `conversion.parse` and `conversion.not_utf8`. The interpreter (interpreter/convert.py) and the native runtime
//! (fortis-native `convert`) build the same failures with the same payloads.

use crate::sync::Rc;

use fortis_runtime::Decimal;
use fortis_types::types::int_types;

use crate::intrinsics::failure;
use crate::machine::*;
use crate::value::*;

pub const MAX_SCALAR: i128 = 0x10FFFF;

fn dynamic<T>(what: &str) -> R<T> {
    Err(Exit::Halt(Halt::Dynamic(what.into())))
}

fn int(v: &Value) -> R<i128> {
    v.as_int().ok_or_else(|| Exit::Halt(Halt::Dynamic("conversion of a non-integer".into())))
}

fn text(v: &Value) -> R<Rc<str>> {
    match v.deref() {
        Value::Str(s) => Ok(s),
        _ => dynamic("conversion of a non-text"),
    }
}

/// An integer into the integer type `target`: the value when it lies in the target's range (a widening always
/// does), else `conversion.out_of_range`.
pub fn to_int(n: i128, target: &str) -> Value {
    let Some((lo, hi)) = int_types(target) else { return Value::Int(n) };
    if n < lo || n > hi {
        return failure("conversion.out_of_range", vec![("value", Value::Int(n)), ("low", Value::Int(lo)), ("high", Value::Int(hi))]);
    }
    Value::Int(n)
}

pub use fortis_runtime::convert::{canonical_decimal, canonical_int};
use fortis_runtime::convert::{integer_text, IntegerText};

/// A Unicode scalar value, or None for a surrogate or a value beyond U+10FFFF.
pub fn scalar_of(n: i128) -> Option<char> {
    if (0..=MAX_SCALAR).contains(&n) {
        char::from_u32(n as u32)
    } else {
        None
    }
}

/// `as:<kind>:<source>:<target>` on `v`.
pub fn convert(row: &str, v: Value) -> R<Value> {
    let mut parts = row.splitn(3, ':');
    let (kind, _source, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let v = v.deref();
    Ok(match kind {
        "newtype_unwrap" => v,
        "newtype_copy" => deep_copy(&v),
        "int_widen" | "int_narrow" => to_int(int(&v)?, target),
        "int_to_decimal" => match Decimal::from_int(int(&v)?) {
            Ok(d) => Value::Dec(d),
            Err(_) => return dynamic("an integer beyond a Decimal's coefficient"),
        },
        "decimal_to_int" => {
            let Value::Dec(d) = &v else { return dynamic("conversion of a non-decimal") };
            let unit = 10u128.pow(d.scale);
            if d.coeff % unit != 0 {
                return Ok(failure("conversion.not_integral", vec![("value", Value::Str(Rc::from(d.to_string())))]));
            }
            let q = (d.coeff / unit) as i128;
            to_int(if d.neg { -q } else { q }, target)
        }
        "int_to_float" => {
            let n = int(&v)?;
            let f = n as f64;
            if f as i128 != n {
                return Ok(failure("conversion.not_exact", vec![("value", Value::Int(n))]));
            }
            Value::Float(f)
        }
        "to_string" | "view_to_string" => Value::Str(Rc::from(display(&v))),
        "parse_int" => {
            let s = text(&v)?;
            match integer_text(&s) {
                IntegerText::Value(n) => to_int(n, target),
                IntegerText::OutOfRange => {
                    let (lo, hi) = int_types(target).expect("integer conversion target");
                    failure("conversion.out_of_range", vec![("value", Value::Str(s)), ("low", Value::Int(lo)), ("high", Value::Int(hi))])
                }
                IntegerText::Invalid => failure("conversion.parse", vec![("text", Value::Str(s))]),
            }
        }
        "parse_decimal" => {
            let s = text(&v)?;
            match canonical_decimal(&s) {
                Some(d) => Value::Dec(d),
                None => failure("conversion.parse", vec![("text", Value::Str(s))]),
            }
        }
        "parse_bool" => {
            let s = text(&v)?;
            match &*s {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => failure("conversion.parse", vec![("text", Value::Str(s))]),
            }
        }
        "bytes_to_string" => {
            let Value::Bytes(b) = &v else { return dynamic("conversion of a non-bytes") };
            match std::str::from_utf8(b) {
                Ok(s) => Value::Str(Rc::from(s)),
                Err(e) => failure("conversion.not_utf8", vec![("position", Value::Int(e.valid_up_to() as i128))]),
            }
        }
        "string_to_bytes" => Value::Bytes(Rc::new(text(&v)?.as_bytes().to_vec())),
        "scalar_to_int" => match &v {
            Value::Scalar(s) => Value::Int(s.chars().next().map_or(0, |c| c as i128)),
            _ => return dynamic("conversion of a non-scalar"),
        },
        "int_to_scalar" => {
            let n = int(&v)?;
            match scalar_of(n) {
                Some(c) => Value::Scalar(Rc::from(c.to_string())),
                None => failure("conversion.out_of_range", vec![("value", Value::Int(n)), ("low", Value::Int(0)), ("high", Value::Int(MAX_SCALAR))]),
            }
        }
        _ => return dynamic(&format!("unknown conversion `{row}`")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_integers() {
        assert_eq!(canonical_int("42"), Some(42));
        assert_eq!(canonical_int("-7"), Some(-7));
        assert_eq!(canonical_int("0"), Some(0));
        assert_eq!(canonical_int("007"), None);
        assert_eq!(canonical_int("+7"), None);
        assert_eq!(canonical_int("-0"), None);
        assert_eq!(canonical_int("4 2"), None);
        assert_eq!(canonical_int(""), None);
    }

    #[test]
    fn canonical_decimals() {
        assert_eq!(canonical_decimal("1.50").map(|d| d.to_string()), Some("1.50".into()));
        assert_eq!(canonical_decimal("-3").map(|d| d.to_string()), Some("-3".into()));
        assert!(canonical_decimal("1.5d").is_none());
        assert!(canonical_decimal("1.").is_none());
        assert!(canonical_decimal(".5").is_none());
        assert!(canonical_decimal("-0.00").is_none());
        assert!(canonical_decimal("00.5").is_none());
        assert!(canonical_decimal("1.00000000000000000000000000000").is_none());
    }

    #[test]
    fn scalars_and_bytes() {
        assert_eq!(scalar_of(65), Some('A'));
        assert_eq!(scalar_of(0xD800), None);
        assert_eq!(scalar_of(0x110000), None);
        let bad = convert("bytes_to_string:Bytes:String", Value::Bytes(Rc::new(vec![0x68, 0xff]))).unwrap();
        assert!(bad.is_failure());
        let ok = convert("bytes_to_string:Bytes:String", Value::Bytes(Rc::new(vec![0x68, 0x69]))).unwrap();
        assert_eq!(display(&ok), "hi");
    }
}
