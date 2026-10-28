//! Numeric intrinsics (6.4, 6.5, 15): checked integer operations with their failures, Duration and Timestamp.

use fortis_runtime::{DecError, Decimal, Rounding};

use crate::failure::{Dyn, Failure, Outcome};
use crate::show::Show;

fn overflow(op: &str) -> Failure {
    Failure::new("numeric.overflow", "").with("operation", Dyn::Str(op.to_string()))
}

#[inline]
pub fn int_add(a: i64, b: i64) -> Outcome<i64> {
    a.checked_add(b).ok_or_else(|| overflow("add"))
}
#[inline]
pub fn int_sub(a: i64, b: i64) -> Outcome<i64> {
    a.checked_sub(b).ok_or_else(|| overflow("sub"))
}
#[inline]
pub fn int_mul(a: i64, b: i64) -> Outcome<i64> {
    a.checked_mul(b).ok_or_else(|| overflow("mul"))
}
#[inline]
pub fn int_div(a: i64, b: i64) -> Outcome<i64> {
    if b == 0 {
        return Err(Failure::new("numeric.divide_by_zero", ""));
    }
    a.checked_div(b).ok_or_else(|| overflow("div"))
}
#[inline]
pub fn int_rem(a: i64, b: i64) -> Outcome<i64> {
    if b == 0 {
        return Err(Failure::new("numeric.divide_by_zero", ""));
    }
    Ok(a.wrapping_rem(b))
}
#[inline]
pub fn int_shift_left(a: i64, bits: i64) -> Outcome<i64> {
    if !(0..64).contains(&bits) {
        return Err(Failure::new("numeric.shift_range", "").with("bits", Dyn::Int(bits as i128)));
    }
    let r = (a as i128) << bits;
    if r < i64::MIN as i128 || r > i64::MAX as i128 {
        return Err(overflow("shiftLeft"));
    }
    Ok(r as i64)
}

#[inline]
pub fn dec(r: Result<Decimal, DecError>, op: &str) -> Outcome<Decimal> {
    r.map_err(|e| {
        let f = Failure::new(e.code(), "");
        if e == DecError::Overflow {
            f.with("operation", Dyn::Str(op.to_string()))
        } else {
            f
        }
    })
}

#[inline]
pub fn rounding(variant: &str) -> Rounding {
    Rounding::from_name(variant).unwrap_or(Rounding::HalfEven)
}

/// `Duration` (6.2): nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Duration(pub i64);

impl Show for Duration {
    fn show(&self) -> String {
        fortis_runtime::time::display_duration(self.0)
    }
}

fn dur(ns: i128) -> Outcome<Duration> {
    if ns < i64::MIN as i128 || ns > i64::MAX as i128 {
        Err(overflow("duration"))
    } else {
        Ok(Duration(ns as i64))
    }
}

impl Duration {
    #[inline]
    pub fn of(n: i64, unit_ns: i64) -> Outcome<Duration> {
        dur(n as i128 * unit_ns as i128)
    }
    #[inline]
    pub fn add(a: Duration, b: Duration) -> Outcome<Duration> {
        dur(a.0 as i128 + b.0 as i128)
    }
    #[inline]
    pub fn sub(a: Duration, b: Duration) -> Outcome<Duration> {
        dur(a.0 as i128 - b.0 as i128)
    }
    #[inline]
    pub fn multiply(a: Duration, k: i64) -> Outcome<Duration> {
        dur(a.0 as i128 * k as i128)
    }
    #[inline]
    pub fn nanoseconds(&self) -> i64 {
        self.0
    }
}

/// `Timestamp` (15): nanoseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(pub i64);

impl Show for Timestamp {
    fn show(&self) -> String {
        fortis_runtime::time::display_timestamp(self.0)
    }
}

impl Timestamp {
    #[inline]
    pub fn add(t: Timestamp, d: Duration) -> Outcome<Timestamp> {
        let r = t.0 as i128 + d.0 as i128;
        if r < i64::MIN as i128 || r > i64::MAX as i128 {
            Err(overflow("Timestamp.add"))
        } else {
            Ok(Timestamp(r as i64))
        }
    }
}

/// `T.validate(x)` against a refinement: the value, or `refinement.out_of_range`.
#[inline]
pub fn validate(n: i128, lo: Option<i128>, hi: Option<i128>, origin: &'static str) -> Outcome<()> {
    if lo.map_or(false, |l| n < l) || hi.map_or(false, |h| n > h) {
        let b = |x: Option<i128>| x.map_or(Dyn::Null, Dyn::Int);
        return Err(Failure::new("refinement.out_of_range", origin).with("value", Dyn::Int(n)).with("low", b(lo)).with("high", b(hi)));
    }
    Ok(())
}
