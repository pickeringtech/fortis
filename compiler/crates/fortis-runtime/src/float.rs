//! Float semantics (Language Reference 6.5): strict IEEE 754 with one canonical NaN, and the display form —
//! shortest round-trip digits in plain notation with a fractional part (`NaN`, `Infinity`, `-Infinity`).

/// The canonical NaN: positive, quiet, zero payload (Rust's `f64::NAN` promises no bit pattern).
pub const NAN_BITS: u64 = 0x7FF8_0000_0000_0000;
pub const NAN32_BITS: u32 = 0x7FC0_0000;
pub const NAN: f64 = f64::from_bits(NAN_BITS);
pub const NAN32: f32 = f32::from_bits(NAN32_BITS);

/// Every NaN result is the canonical NaN. The test runs on the bits as integers: LLVM treats NaN payloads and
/// signs as interchangeable, so a floating-point `is_nan` branch returning a NaN constant may be folded away.
#[inline(never)]
pub fn canon(x: f64) -> f64 {
    let b = x.to_bits();
    let nan = (b & 0x7FF0_0000_0000_0000) == 0x7FF0_0000_0000_0000 && (b & 0x000F_FFFF_FFFF_FFFF) != 0;
    f64::from_bits(if nan { NAN_BITS } else { b })
}

#[inline(never)]
pub fn canon32(x: f32) -> f32 {
    let b = x.to_bits();
    let nan = (b & 0x7F80_0000) == 0x7F80_0000 && (b & 0x007F_FFFF) != 0;
    f32::from_bits(if nan { NAN32_BITS } else { b })
}

/// `a op b` for `+ - * /`, with IEEE division by zero.
pub fn f64_op(op: char, a: f64, b: f64) -> f64 {
    match op {
        '+' => canon(a + b),
        '-' => canon(a - b),
        '*' => canon(a * b),
        _ => {
            if b == 0.0 {
                if a == 0.0 || a.is_nan() {
                    return canon(f64::NAN);
                }
                return f64::INFINITY.copysign(a) * 1.0f64.copysign(b);
            }
            canon(a / b)
        }
    }
}

pub fn f32_op(op: char, a: f32, b: f32) -> f32 {
    canon32(f64_op(op, a as f64, b as f64) as f32)
}

pub fn sqrt(x: f64) -> f64 {
    canon(x.sqrt())
}

/// IEEE totalOrder as a signed key.
pub fn total_order(x: f64) -> i64 {
    let mut b = x.to_bits();
    if b >> 63 == 1 {
        b ^= 0x7FFF_FFFF_FFFF_FFFF;
    }
    b as i64
}

/// The display form of a binary64.
pub fn display(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let mut s = format!("{x}");
    if !s.contains('.') {
        s.push_str(".0");
    }
    s
}

/// The display form of a binary32.
pub fn display32(x: f32) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let mut s = format!("{x}");
    if !s.contains('.') {
        s.push_str(".0");
    }
    s
}

/// The shortest digits of `x` as Python's `repr` writes them (used where the interpreter parses `repr(x)`).
pub fn repr_digits(x: f64) -> String {
    format!("{x:e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_the_interpreter() {
        assert_eq!(display(1.0), "1.0");
        assert_eq!(display(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(display(1e300), format!("1{}.0", "0".repeat(300)));
        assert_eq!(display(1e-7), "0.0000001");
        assert_eq!(display(-0.0), "-0.0");
        assert_eq!(display(f64::NAN), "NaN");
        assert_eq!(display(-f64::INFINITY), "-Infinity");
        assert_eq!(display32(0.1f32), "0.1");
        assert_eq!(display32(16777216.0f32), "16777216.0");
    }

    #[test]
    fn ieee_division_and_order() {
        assert!(f64_op('/', 0.0, 0.0).is_nan());
        assert_eq!(f64_op('/', 0.0, 0.0).to_bits(), 0x7FF8_0000_0000_0000);
        assert_eq!(sqrt(-1.0).to_bits(), 0x7FF8_0000_0000_0000);
        assert!(total_order(sqrt(-1.0)) > total_order(f64::INFINITY));
        assert_eq!(f64_op('/', 1.0, -0.0), f64::NEG_INFINITY);
        assert!(total_order(-1.0) < total_order(-0.0));
        assert!(total_order(-0.0) < total_order(0.0));
        assert!(total_order(1.0) < total_order(f64::INFINITY));
    }
}
