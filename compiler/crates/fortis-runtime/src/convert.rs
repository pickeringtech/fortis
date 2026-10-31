//! Canonical numeric text (LR 6.13), shared by the VM and native conversions.
use crate::Decimal;

/// A canonical integer beyond i128 is still a range failure, never malformed text.
#[derive(Debug, PartialEq, Eq)]
pub enum IntegerText {
    Value(i128),
    OutOfRange,
    Invalid,
}

pub fn integer_text(text: &str) -> IntegerText {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0')) || text == "-0"
    {
        return IntegerText::Invalid;
    }
    match text.parse() {
        Ok(n) => IntegerText::Value(n),
        Err(_) => IntegerText::OutOfRange,
    }
}

pub fn canonical_int(text: &str) -> Option<i128> {
    match integer_text(text) {
        IntegerText::Value(n) => Some(n),
        _ => None,
    }
}

pub fn canonical_decimal(text: &str) -> Option<Decimal> {
    let (neg, body) = match text.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, text),
    };
    let (ip, fp) = body.split_once('.').unwrap_or((body, ""));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(ip) || (ip.len() > 1 && ip.starts_with('0'))
        || (body.contains('.') && !digits(fp)) || ip.len() + fp.len() > 29 || fp.len() > 28
    {
        return None;
    }
    let coeff: u128 = format!("{ip}{fp}").parse().ok()?;
    if neg && coeff == 0 {
        return None;
    }
    Decimal::new(neg, coeff, fp.len() as u32).ok()
}
