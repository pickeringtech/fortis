//! The canonical codec for natively lowered programs (Language Reference 6.10; DEL-39), with the formats of
//! conformance/codec/FORMAT.md and the behaviour of interpreter/codec.py.
//!
//! Every Encodable Rust type implements [`Codec`]: the runtime for primitives and collections, the lowering for
//! classes, enums and records. Refinements are erased from native types (a `type Percent = Int<0..100>` is an
//! `i64`), so each decode receives the [`Refine`] of its position: the bounds its declared type imposes, nested
//! for elements, keys and values.

use crate::failure::{Dyn, Failure, Outcome};
use crate::{Decimal, Duration, OrderedMap, OrderedSet, Timestamp};

pub const MAGIC: &[u8; 4] = b"FTB\x01";

pub const T_NULL: u8 = 0x00;
pub const T_FALSE: u8 = 0x01;
pub const T_TRUE: u8 = 0x02;
pub const T_INT: u8 = 0x03;
pub const T_FLOAT: u8 = 0x04;
pub const T_DECIMAL: u8 = 0x05;
pub const T_STRING: u8 = 0x06;
pub const T_BYTES: u8 = 0x07;
pub const T_LIST: u8 = 0x08;
pub const T_MAP: u8 = 0x09;
pub const T_RECORD: u8 = 0x0A;
pub const T_VARIANT: u8 = 0x0B;
pub const T_DURATION: u8 = 0x0C;
pub const T_TIMESTAMP: u8 = 0x0D;
pub const T_SCALAR: u8 = 0x0E;
pub const T_FLOAT32: u8 = 0x0F;
pub const T_SET: u8 = 0x10;
pub const T_TUPLE: u8 = 0x11;

/// A decoding failure: its code and detail become the failure value.
#[derive(Debug)]
pub struct Invalid {
    pub code: &'static str,
    pub detail: String,
}

pub type D<T> = Result<T, Invalid>;

pub fn bad<T>(detail: impl Into<String>) -> D<T> {
    Err(Invalid { code: "codec.invalid", detail: detail.into() })
}

fn out_of_range<T>(detail: impl Into<String>) -> D<T> {
    Err(Invalid { code: "refinement.out_of_range", detail: detail.into() })
}

/// The bounds a position's declared type imposes: an integer refinement, a scalar count, a collection length,
/// and the refinements of elements (and map values, tuple items) below it.
#[derive(Clone, Copy, Debug)]
pub struct Refine {
    pub lo: Option<i128>,
    pub hi: Option<i128>,
    pub scalars: Option<(Option<i128>, Option<i128>)>,
    pub length: Option<(Option<i128>, Option<i128>)>,
    pub elem: Option<&'static Refine>,
    pub value: Option<&'static Refine>,
    pub items: &'static [Refine],
}

impl Refine {
    pub const NONE: Refine = Refine { lo: None, hi: None, scalars: None, length: None, elem: None, value: None, items: &[] };
    fn elem(&self) -> &'static Refine {
        self.elem.unwrap_or(&Refine::NONE)
    }
    fn value(&self) -> &'static Refine {
        self.value.unwrap_or(&Refine::NONE)
    }
    fn item(&self, i: usize) -> &Refine {
        self.items.get(i).unwrap_or(&Refine::NONE)
    }
    fn check_int(&self, n: i128) -> D<i128> {
        if self.lo.map_or(false, |l| n < l) || self.hi.map_or(false, |h| n > h) {
            return out_of_range(format!("{n} outside its refinement"));
        }
        Ok(n)
    }
    fn check_scalars(&self, s: &str) -> D<()> {
        if let Some((lo, hi)) = self.scalars {
            let n = s.chars().count() as i128;
            if lo.map_or(false, |l| n < l) || hi.map_or(false, |h| n > h) {
                return out_of_range(format!("string of {n} scalars outside the refinement"));
            }
        }
        Ok(())
    }
    fn check_length(&self, n: usize) -> D<()> {
        if let Some((lo, hi)) = self.length {
            let n = n as i128;
            if lo.map_or(false, |l| n < l) || hi.map_or(false, |h| n > h) {
                return out_of_range(format!("{n} elements outside the refinement"));
            }
        }
        Ok(())
    }
}

// ---- the text tree ------------------------------------------------------------------------------------------

/// A parsed JSON value (RFC 8259); integers keep their text so one past every native type still decodes as
/// "does not fit".
#[derive(Debug)]
pub enum J {
    Obj(Vec<(String, J)>),
    Arr(Vec<J>),
    Str(String),
    Bool(bool),
    Null,
    Int(Option<i128>),
    Float(f64),
}

struct Reader<'s> {
    s: &'s [char],
    i: usize,
}

impl Reader<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], ' ' | '\t' | '\r' | '\n') {
            self.i += 1;
        }
    }
    fn at(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn starts(&self, w: &str) -> bool {
        w.chars().enumerate().all(|(k, c)| self.s.get(self.i + k) == Some(&c))
    }
    fn value(&mut self) -> D<J> {
        self.ws();
        match self.at() {
            None => bad("unexpected end of input"),
            Some('{') => self.obj(),
            Some('[') => self.arr(),
            Some('"') => Ok(J::Str(self.string()?)),
            _ if self.starts("true") => {
                self.i += 4;
                Ok(J::Bool(true))
            }
            _ if self.starts("false") => {
                self.i += 5;
                Ok(J::Bool(false))
            }
            _ if self.starts("null") => {
                self.i += 4;
                Ok(J::Null)
            }
            _ => self.number(),
        }
    }
    fn obj(&mut self) -> D<J> {
        self.i += 1;
        let mut out = vec![];
        self.ws();
        if self.at() == Some('}') {
            self.i += 1;
            return Ok(J::Obj(out));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            if self.at() != Some(':') {
                return bad(format!("expected ':' at {}", self.i));
            }
            self.i += 1;
            out.push((k, self.value()?));
            self.ws();
            let c = self.at();
            self.i += 1;
            match c {
                Some('}') => return Ok(J::Obj(out)),
                Some(',') => {}
                _ => return bad(format!("expected ',' or '}}' at {}", self.i - 1)),
            }
        }
    }
    fn arr(&mut self) -> D<J> {
        self.i += 1;
        let mut out = vec![];
        self.ws();
        if self.at() == Some(']') {
            self.i += 1;
            return Ok(J::Arr(out));
        }
        loop {
            out.push(self.value()?);
            self.ws();
            let c = self.at();
            self.i += 1;
            match c {
                Some(']') => return Ok(J::Arr(out)),
                Some(',') => {}
                _ => return bad(format!("expected ',' or ']' at {}", self.i - 1)),
            }
        }
    }
    fn hex4(&self, at: usize) -> Option<u32> {
        let h: String = self.s.get(at..at + 4)?.iter().collect();
        u32::from_str_radix(&h, 16).ok()
    }
    fn string(&mut self) -> D<String> {
        if self.at() != Some('"') {
            return bad(format!("expected a string at {}", self.i));
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = match self.at() {
                Some(c) => c,
                None => return bad("unterminated string"),
            };
            if c == '"' {
                self.i += 1;
                return Ok(out);
            }
            if c == '\\' {
                let e = self.s.get(self.i + 1).copied();
                let m = match e {
                    Some('"') => Some('"'),
                    Some('\\') => Some('\\'),
                    Some('/') => Some('/'),
                    Some('b') => Some('\u{8}'),
                    Some('f') => Some('\u{c}'),
                    Some('n') => Some('\n'),
                    Some('r') => Some('\r'),
                    Some('t') => Some('\t'),
                    _ => None,
                };
                if let Some(x) = m {
                    out.push(x);
                    self.i += 2;
                } else if e == Some('u') {
                    let mut cp = match self.hex4(self.i + 2) {
                        Some(cp) => cp,
                        None => return bad("bad \\u escape"),
                    };
                    self.i += 6;
                    if (0xD800..0xDC00).contains(&cp) && self.starts("\\u") {
                        if let Some(lo) = self.hex4(self.i + 2) {
                            cp = 0x10000 + ((cp - 0xD800) << 10) + lo.wrapping_sub(0xDC00);
                            self.i += 6;
                        }
                    }
                    match char::from_u32(cp) {
                        Some(ch) => out.push(ch),
                        None => return bad("bad \\u escape"),
                    }
                } else {
                    return bad("bad escape");
                }
            } else if (c as u32) < 0x20 {
                return bad("control character in string");
            } else {
                out.push(c);
                self.i += 1;
            }
        }
    }
    fn number(&mut self) -> D<J> {
        let mut j = self.i;
        while j < self.s.len() && "+-0123456789.eE".contains(self.s[j]) {
            j += 1;
        }
        let t: String = self.s[self.i..j].iter().collect();
        if t.is_empty() {
            return bad(format!("unexpected {:?} at {}", self.s[self.i], self.i));
        }
        self.i = j;
        if t.contains(['.', 'e', 'E']) {
            return t.parse::<f64>().map(J::Float).or_else(|_| bad(format!("bad number {t:?}")));
        }
        let digits = t.trim_start_matches(['+', '-']);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) || t.len() - digits.len() > 1 {
            return bad(format!("bad number {t:?}"));
        }
        Ok(J::Int(t.parse::<i128>().ok()))
    }
}

/// Parse a whole canonical (or RFC 8259) text.
pub fn parse_text(text: &str) -> D<J> {
    let chars: Vec<char> = text.chars().collect();
    let mut r = Reader { s: &chars, i: 0 };
    let v = r.value()?;
    r.ws();
    if r.i != chars.len() {
        return bad("trailing characters after the value");
    }
    Ok(v)
}

// ---- binary reading and writing ----------------------------------------------------------------------------------

pub struct Bin<'b> {
    d: &'b [u8],
    i: usize,
}

impl<'b> Bin<'b> {
    pub fn peek(&self) -> Option<u8> {
        self.d.get(self.i).copied()
    }
    pub fn skip(&mut self) {
        self.i += 1;
    }
    pub fn byte(&mut self) -> D<u8> {
        match self.d.get(self.i) {
            Some(b) => {
                self.i += 1;
                Ok(*b)
            }
            None => bad("truncated input"),
        }
    }
    pub fn take(&mut self, n: usize) -> D<&'b [u8]> {
        if self.i + n > self.d.len() {
            return bad("truncated input");
        }
        let out = &self.d[self.i..self.i + n];
        self.i += n;
        Ok(out)
    }
    pub fn varint(&mut self) -> D<Option<u128>> {
        let (mut n, mut shift, mut over) = (0u128, 0u32, false);
        loop {
            let b = self.byte()?;
            let part = (b & 0x7F) as u128;
            if shift < 128 {
                if shift > 121 && part >> (128 - shift) != 0 {
                    over = true;
                }
                n |= part.checked_shl(shift).unwrap_or(0);
            } else if part != 0 {
                over = true;
            }
            if b & 0x80 == 0 {
                return Ok(if over { None } else { Some(n) });
            }
            shift += 7;
            if shift > 70 {
                return bad("varint too long");
            }
        }
    }
    pub fn len(&mut self) -> D<usize> {
        match self.varint()?.and_then(|n| usize::try_from(n).ok()) {
            Some(n) => Ok(n),
            None => bad("length out of range"),
        }
    }
    pub fn zigzag(&mut self) -> D<Option<i128>> {
        Ok(self.varint()?.map(|n| if n & 1 == 0 { (n >> 1) as i128 } else { -(((n + 1) >> 1) as i128) }))
    }
    pub fn expect(&mut self, tag: u8, what: &str) -> D<()> {
        let b = self.byte()?;
        if b != tag {
            return bad(format!("expected {what} (tag {tag:#04x}), found tag {b:#04x}"));
        }
        Ok(())
    }
}

pub fn varint(out: &mut Vec<u8>, mut n: u128) {
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n != 0 {
            out.push(b | 0x80);
        } else {
            out.push(b);
            return;
        }
    }
}

pub fn zigzag(n: i128) -> u128 {
    if n >= 0 {
        (n as u128) << 1
    } else {
        (((-(n + 1)) as u128) << 1) + 1
    }
}

pub fn json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| B64.iter().position(|&x| x == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.len() / 4;
    for (ci, chunk) in bytes.chunks(4).enumerate() {
        let pads = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pads > 2 || (pads > 0 && ci != chunks - 1) {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n = (n << 6) | if i >= 4 - pads { 0 } else { val(c)? };
        }
        out.push((n >> 16) as u8);
        if pads < 2 {
            out.push((n >> 8) as u8);
        }
        if pads < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

// ---- the trait -------------------------------------------------------------------------------------------------

/// A canonically encodable and decodable native type.
pub trait Codec: Sized {
    fn enc_text(&self, out: &mut String);
    fn enc_bin(&self, out: &mut Vec<u8>);
    fn dec_text(j: &J, r: &Refine) -> D<Self>;
    fn dec_bin(b: &mut Bin, r: &Refine) -> D<Self>;
    /// whether `null` decodes to this type (an optional field may be omitted)
    fn optional() -> bool {
        false
    }
}

fn float_text(x: f64, out: &mut String) {
    if x.is_nan() {
        out.push_str("\"NaN\"");
    } else if x.is_infinite() {
        out.push_str(if x > 0.0 { "\"Infinity\"" } else { "\"-Infinity\"" });
    } else {
        out.push_str(&crate::float::display(x));
    }
}

macro_rules! int_codec {
    ($($t:ty),*) => {$(
        impl Codec for $t {
            fn enc_text(&self, out: &mut String) {
                out.push_str(&self.to_string());
            }
            fn enc_bin(&self, out: &mut Vec<u8>) {
                out.push(T_INT);
                varint(out, zigzag(*self as i128));
            }
            fn dec_text(j: &J, r: &Refine) -> D<Self> {
                match j {
                    J::Int(Some(n)) => match <$t>::try_from(*n) {
                        Ok(v) => Ok(r.check_int(*n).map(|_| v)?),
                        Err(_) => bad(format!("{n} does not fit {}", stringify!($t))),
                    },
                    J::Int(None) => bad(format!("an integer that does not fit {}", stringify!($t))),
                    J::Null => bad("null where an integer is expected"),
                    _ => bad("expected an integer"),
                }
            }
            fn dec_bin(b: &mut Bin, r: &Refine) -> D<Self> {
                b.expect(T_INT, "an integer")?;
                match b.zigzag()? {
                    Some(n) => match <$t>::try_from(n) {
                        Ok(v) => Ok(r.check_int(n).map(|_| v)?),
                        Err(_) => bad(format!("{n} does not fit {}", stringify!($t))),
                    },
                    None => bad("an integer that does not fit"),
                }
            }
        }
    )*};
}
int_codec!(i8, i16, i32, i64, u8, u16, u32, u64);

impl Codec for bool {
    fn enc_text(&self, out: &mut String) {
        out.push_str(if *self { "true" } else { "false" });
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(if *self { T_TRUE } else { T_FALSE });
    }
    fn dec_text(j: &J, _r: &Refine) -> D<Self> {
        match j {
            J::Bool(b) => Ok(*b),
            J::Null => bad("null where a boolean is expected"),
            _ => bad("expected true or false"),
        }
    }
    fn dec_bin(b: &mut Bin, _r: &Refine) -> D<Self> {
        match b.byte()? {
            T_FALSE => Ok(false),
            T_TRUE => Ok(true),
            _ => bad("expected a boolean"),
        }
    }
}

fn float_of(j: &J) -> D<f64> {
    Ok(match j {
        J::Float(x) => *x,
        J::Int(Some(n)) => *n as f64,
        J::Int(None) => f64::INFINITY,
        J::Str(s) if s == "NaN" => f64::NAN,
        J::Str(s) if s == "Infinity" => f64::INFINITY,
        J::Str(s) if s == "-Infinity" => f64::NEG_INFINITY,
        J::Null => return bad("null where a number is expected"),
        _ => return bad("expected a number"),
    })
}

impl Codec for f64 {
    fn enc_text(&self, out: &mut String) {
        float_text(*self, out);
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_FLOAT);
        out.extend_from_slice(&self.to_le_bytes());
    }
    fn dec_text(j: &J, _r: &Refine) -> D<Self> {
        Ok(crate::float::canon(float_of(j)?))
    }
    fn dec_bin(b: &mut Bin, _r: &Refine) -> D<Self> {
        b.expect(T_FLOAT, "a float")?;
        let x = b.take(8)?;
        Ok(crate::float::canon(f64::from_le_bytes([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]])))
    }
}

impl Codec for f32 {
    fn enc_text(&self, out: &mut String) {
        float_text(*self as f64, out);
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_FLOAT32);
        out.extend_from_slice(&self.to_le_bytes());
    }
    fn dec_text(j: &J, _r: &Refine) -> D<Self> {
        Ok(crate::float::canon32(float_of(j)? as f32))
    }
    fn dec_bin(b: &mut Bin, _r: &Refine) -> D<Self> {
        b.expect(T_FLOAT32, "a float32")?;
        let x = b.take(4)?;
        Ok(crate::float::canon32(f32::from_le_bytes([x[0], x[1], x[2], x[3]])))
    }
}

fn valid_decimal_text(s: &str) -> bool {
    let t = s.strip_prefix('-').unwrap_or(s);
    let (a, b) = t.split_once('.').unwrap_or((t, ""));
    !a.is_empty() && a.chars().all(|c| c.is_ascii_digit() || c == '_') && b.chars().all(|c| c.is_ascii_digit() || c == '_') && !(t.contains('.') && b.is_empty())
}

impl Codec for Decimal {
    fn enc_text(&self, out: &mut String) {
        json_string(&self.to_string(), out);
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_DECIMAL);
        out.push(if self.neg { 1 } else { 0 });
        out.push(self.scale as u8);
        varint(out, self.coeff);
    }
    fn dec_text(j: &J, _r: &Refine) -> D<Self> {
        match j {
            J::Str(s) if valid_decimal_text(s) => Decimal::parse(&format!("{s}d")).or_else(|_| bad(format!("bad decimal {s:?}"))),
            J::Str(s) => bad(format!("bad decimal {s:?}")),
            J::Null => bad("null where a decimal is expected"),
            _ => bad("expected a decimal string"),
        }
    }
    fn dec_bin(b: &mut Bin, _r: &Refine) -> D<Self> {
        b.expect(T_DECIMAL, "a decimal")?;
        let neg = b.byte()?;
        let scale = b.byte()?;
        match (neg <= 1, b.varint()?.and_then(|c| Decimal::new(neg == 1, c, scale as u32).ok())) {
            (true, Some(d)) => Ok(d),
            _ => bad("bad decimal"),
        }
    }
}

impl Codec for String {
    fn enc_text(&self, out: &mut String) {
        json_string(self, out);
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_STRING);
        varint(out, self.len() as u128);
        out.extend_from_slice(self.as_bytes());
    }
    fn dec_text(j: &J, r: &Refine) -> D<Self> {
        match j {
            J::Str(s) => {
                r.check_scalars(s)?;
                Ok(s.clone())
            }
            J::Null => bad("null where a string is expected"),
            _ => bad("expected a string"),
        }
    }
    fn dec_bin(b: &mut Bin, r: &Refine) -> D<Self> {
        b.expect(T_STRING, "a string")?;
        let n = b.len()?;
        let s = std::str::from_utf8(b.take(n)?).map_err(|_| Invalid { code: "codec.invalid", detail: "invalid UTF-8".into() })?.to_string();
        r.check_scalars(&s)?;
        Ok(s)
    }
}

impl Codec for char {
    fn enc_text(&self, out: &mut String) {
        json_string(&self.to_string(), out);
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_SCALAR);
        varint(out, *self as u128);
    }
    fn dec_text(j: &J, _r: &Refine) -> D<Self> {
        match j {
            J::Str(s) if s.chars().count() == 1 => Ok(s.chars().next().unwrap()),
            _ => bad("expected a one-scalar string"),
        }
    }
    fn dec_bin(b: &mut Bin, _r: &Refine) -> D<Self> {
        b.expect(T_SCALAR, "a scalar")?;
        match b.varint()?.and_then(|n| u32::try_from(n).ok()).and_then(char::from_u32) {
            Some(c) => Ok(c),
            None => bad("not a Unicode scalar"),
        }
    }
}

impl Codec for Duration {
    fn enc_text(&self, out: &mut String) {
        out.push_str(&self.0.to_string());
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_DURATION);
        varint(out, zigzag(self.0 as i128));
    }
    fn dec_text(j: &J, _r: &Refine) -> D<Self> {
        match j {
            J::Int(Some(n)) => i64::try_from(*n).map(Duration).map_err(|_| Invalid { code: "numeric.overflow", detail: "duration out of range".into() }),
            J::Int(None) => Err(Invalid { code: "numeric.overflow", detail: "duration out of range".into() }),
            _ => bad("expected nanoseconds"),
        }
    }
    fn dec_bin(b: &mut Bin, _r: &Refine) -> D<Self> {
        b.expect(T_DURATION, "a duration")?;
        match b.zigzag()?.and_then(|n| i64::try_from(n).ok()) {
            Some(ns) => Ok(Duration(ns)),
            None => Err(Invalid { code: "numeric.overflow", detail: "duration out of range".into() }),
        }
    }
}

fn parse_rfc3339(t: &str) -> D<i64> {
    let b = t.as_bytes();
    let num = |a: usize, n: usize| -> Option<i64> { t.get(a..a + n).filter(|s| s.bytes().all(|c| c.is_ascii_digit())).and_then(|s| s.parse().ok()) };
    let shape = b.len() >= 20 && b[4] == b'-' && b[7] == b'-' && b[10] == b'T' && b[13] == b':' && b[16] == b':' && b[b.len() - 1] == b'Z';
    let (y, mo, da, h, mi, s) = match (shape, num(0, 4), num(5, 2), num(8, 2), num(11, 2), num(14, 2), num(17, 2)) {
        (true, Some(y), Some(mo), Some(da), Some(h), Some(mi), Some(s)) => (y, mo, da, h, mi, s),
        _ => return bad(format!("not an RFC 3339 UTC timestamp: {t:?}")),
    };
    let frac = &t[19..t.len() - 1];
    let frac_ns = if frac.is_empty() {
        0
    } else if (2..=10).contains(&frac.len()) && frac.starts_with('.') && frac[1..].bytes().all(|c| c.is_ascii_digit()) {
        format!("{:0<9}", &frac[1..]).parse::<i64>().unwrap_or(0)
    } else {
        return bad(format!("not an RFC 3339 UTC timestamp: {t:?}"));
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=9999).contains(&y) || !(1..=12).contains(&mo) || da < 1 || da > dim[(mo - 1) as usize] || h > 23 || mi > 59 || s > 59 {
        return bad(format!("bad date {t:?}"));
    }
    let (yy, m) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + (153 * m + 2) / 5 + da - 1;
    let days = era * 146097 + doe - 719468;
    Ok((days * 86400 + h * 3600 + mi * 60 + s) * 1_000_000_000 + frac_ns)
}

impl Codec for Timestamp {
    fn enc_text(&self, out: &mut String) {
        json_string(&fortis_runtime::time::display_timestamp(self.0), out);
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_TIMESTAMP);
        varint(out, zigzag(self.0 as i128));
    }
    fn dec_text(j: &J, _r: &Refine) -> D<Self> {
        match j {
            J::Str(s) => parse_rfc3339(s).map(Timestamp),
            _ => bad("expected an RFC 3339 string"),
        }
    }
    fn dec_bin(b: &mut Bin, _r: &Refine) -> D<Self> {
        b.expect(T_TIMESTAMP, "a timestamp")?;
        match b.zigzag()?.and_then(|n| i64::try_from(n).ok()) {
            Some(ns) => Ok(Timestamp(ns)),
            None => bad("timestamp out of range"),
        }
    }
}

/// `Bytes` is a byte vector natively; the codec writes it as base64 text or a byte string, never as a list.
pub struct BytesCodec;

impl BytesCodec {
    pub fn enc_text(v: &[u8], out: &mut String) {
        json_string(&base64_encode(v), out);
    }
    pub fn enc_bin(v: &[u8], out: &mut Vec<u8>) {
        out.push(T_BYTES);
        varint(out, v.len() as u128);
        out.extend_from_slice(v);
    }
}

impl<T: Codec> Codec for Vec<T> {
    fn enc_text(&self, out: &mut String) {
        out.push('[');
        for (i, x) in self.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            x.enc_text(out);
        }
        out.push(']');
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_LIST);
        varint(out, self.len() as u128);
        for x in self {
            x.enc_bin(out);
        }
    }
    fn dec_text(j: &J, r: &Refine) -> D<Self> {
        match j {
            J::Arr(xs) => {
                let v: Vec<T> = xs.iter().map(|x| T::dec_text(x, r.elem())).collect::<D<_>>()?;
                r.check_length(v.len())?;
                Ok(v)
            }
            J::Null => bad("null where a list is expected"),
            _ => bad("expected an array"),
        }
    }
    fn dec_bin(b: &mut Bin, r: &Refine) -> D<Self> {
        b.expect(T_LIST, "a list")?;
        let n = b.len()?;
        let mut v = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            v.push(T::dec_bin(b, r.elem())?);
        }
        r.check_length(v.len())?;
        Ok(v)
    }
}

impl<T: Codec> Codec for Option<T> {
    fn enc_text(&self, out: &mut String) {
        match self {
            Some(v) => v.enc_text(out),
            None => out.push_str("null"),
        }
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        match self {
            Some(v) => v.enc_bin(out),
            None => out.push(T_NULL),
        }
    }
    fn dec_text(j: &J, r: &Refine) -> D<Self> {
        match j {
            J::Null => Ok(None),
            _ => T::dec_text(j, r).map(Some),
        }
    }
    fn dec_bin(b: &mut Bin, r: &Refine) -> D<Self> {
        if b.peek() == Some(T_NULL) {
            b.skip();
            return Ok(None);
        }
        T::dec_bin(b, r).map(Some)
    }
    fn optional() -> bool {
        true
    }
}

impl<K: Codec + std::hash::Hash + Eq + Clone, V: Codec> Codec for OrderedMap<K, V> {
    fn enc_text(&self, out: &mut String) {
        out.push('[');
        for (i, (k, v)) in self.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('[');
            k.enc_text(out);
            out.push(',');
            v.enc_text(out);
            out.push(']');
        }
        out.push(']');
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_MAP);
        varint(out, self.count() as u128);
        for (k, v) in self.iter() {
            k.enc_bin(out);
            v.enc_bin(out);
        }
    }
    fn dec_text(j: &J, r: &Refine) -> D<Self> {
        let pairs = match j {
            J::Arr(xs) => xs,
            _ => return bad("expected an array of pairs"),
        };
        let mut m = OrderedMap::new();
        for p in pairs {
            let (k, v) = match p {
                J::Arr(kv) if kv.len() == 2 => (&kv[0], &kv[1]),
                _ => return bad("expected a [key, value] pair"),
            };
            let key = K::dec_text(k, r.elem())?;
            if m.get(&key).is_some() {
                return bad("duplicate map key");
            }
            let val = V::dec_text(v, r.value())?;
            m.set(key, val);
        }
        r.check_length(m.count() as usize)?;
        Ok(m)
    }
    fn dec_bin(b: &mut Bin, r: &Refine) -> D<Self> {
        b.expect(T_MAP, "a map")?;
        let n = b.len()?;
        let mut m = OrderedMap::new();
        for _ in 0..n {
            let key = K::dec_bin(b, r.elem())?;
            let val = V::dec_bin(b, r.value())?;
            if m.get(&key).is_some() {
                return bad("duplicate map key");
            }
            m.set(key, val);
        }
        r.check_length(m.count() as usize)?;
        Ok(m)
    }
}

impl<T: Codec + std::hash::Hash + Eq + Clone> Codec for OrderedSet<T> {
    fn enc_text(&self, out: &mut String) {
        out.push('[');
        for (i, x) in self.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            x.enc_text(out);
        }
        out.push(']');
    }
    fn enc_bin(&self, out: &mut Vec<u8>) {
        out.push(T_SET);
        varint(out, self.count() as u128);
        for x in self.iter() {
            x.enc_bin(out);
        }
    }
    fn dec_text(j: &J, r: &Refine) -> D<Self> {
        let xs = match j {
            J::Arr(xs) => xs,
            _ => return bad("expected an array"),
        };
        let mut s = OrderedSet::new();
        for x in xs {
            if !s.add(T::dec_text(x, r.elem())?) {
                return bad("duplicate set element");
            }
        }
        r.check_length(s.count() as usize)?;
        Ok(s)
    }
    fn dec_bin(b: &mut Bin, r: &Refine) -> D<Self> {
        b.expect(T_SET, "a set")?;
        let n = b.len()?;
        let mut s = OrderedSet::new();
        for _ in 0..n {
            if !s.add(T::dec_bin(b, r.elem())?) {
                return bad("duplicate set element");
            }
        }
        r.check_length(s.count() as usize)?;
        Ok(s)
    }
}

macro_rules! tuple_codec {
    ($n:expr; $($t:ident $i:tt),+) => {
        impl<$($t: Codec),+> Codec for ($($t,)+) {
            fn enc_text(&self, out: &mut String) {
                out.push('[');
                let mut first = true;
                $(
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    self.$i.enc_text(out);
                )+
                let _ = first;
                out.push(']');
            }
            fn enc_bin(&self, out: &mut Vec<u8>) {
                out.push(T_TUPLE);
                varint(out, $n);
                $( self.$i.enc_bin(out); )+
            }
            fn dec_text(j: &J, r: &Refine) -> D<Self> {
                match j {
                    J::Arr(xs) if xs.len() == $n => Ok(($($t::dec_text(&xs[$i], r.item($i))?,)+)),
                    _ => bad(format!("expected {} elements", $n)),
                }
            }
            fn dec_bin(b: &mut Bin, r: &Refine) -> D<Self> {
                b.expect(T_TUPLE, "a tuple")?;
                if b.len()? != $n {
                    return bad("wrong tuple arity");
                }
                Ok(($($t::dec_bin(b, r.item($i))?,)+))
            }
        }
    };
}
tuple_codec!(2; A 0, B 1);
tuple_codec!(3; A 0, B 1, C 2);
tuple_codec!(4; A 0, B 1, C 2, E 3);

// ---- helpers for the generated impls (classes, records, enums) ----------------------------------------------------

/// The fields of an object, by name, with unknown and duplicate keys refused (6.10).
pub fn fields<'j>(j: &'j J, names: &[&str]) -> D<Vec<Option<&'j J>>> {
    let items = match j {
        J::Obj(items) => items,
        J::Null => return bad("null where an object is expected"),
        _ => return bad("expected an object"),
    };
    let mut out: Vec<Option<&J>> = vec![None; names.len()];
    for (k, v) in items {
        let i = match names.iter().position(|n| n == k) {
            Some(i) => i,
            None => return Err(Invalid { code: "codec.unknown_field", detail: k.clone() }),
        };
        if out[i].is_some() {
            return bad(format!("duplicate field {k:?}"));
        }
        out[i] = Some(v);
    }
    Ok(out)
}

/// One field of an object: decoded when present, else its default, else `null` for an optional, else missing.
pub fn field<T: Codec>(v: Option<&J>, name: &str, r: &Refine, default: Option<fn() -> T>) -> D<T> {
    match v {
        Some(j) => T::dec_text(j, r),
        None => match default {
            Some(d) => Ok(d()),
            None if T::optional() => T::dec_text(&J::Null, r),
            None => bad(format!("missing field {name:?}")),
        },
    }
}

pub fn record_header(b: &mut Bin, n: usize) -> D<()> {
    b.expect(T_RECORD, "a record")?;
    if b.len()? != n {
        return bad("wrong field count");
    }
    Ok(())
}

pub fn text_field_name(name: &str, out: &mut String) {
    json_string(name, out);
    out.push(':');
}

// ---- entry points ------------------------------------------------------------------------------------------------

pub fn to_text<T: Codec>(v: &T) -> String {
    let mut out = String::new();
    v.enc_text(&mut out);
    out
}

pub fn to_bytes<T: Codec>(v: &T) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    v.enc_bin(&mut out);
    out
}

fn failure(e: Invalid, origin: &'static str) -> Failure {
    Failure::new(e.code, origin).with("detail", Dyn::Str(e.detail))
}

/// `T.fromText(s)`: the value, or `codec.invalid`, `codec.unknown_field` or a refinement failure.
pub fn from_text<T: Codec>(s: &str, r: &Refine, origin: &'static str) -> Outcome<T> {
    parse_text(s).and_then(|j| T::dec_text(&j, r)).map_err(|e| failure(e, origin))
}

/// `T.fromBytes(b)`.
pub fn from_bytes<T: Codec>(data: &[u8], r: &Refine, origin: &'static str) -> Outcome<T> {
    if data.len() < 4 || &data[..4] != MAGIC {
        return Err(failure(Invalid { code: "codec.invalid", detail: "missing canonical binary header FTB\\x01".into() }, origin));
    }
    let mut b = Bin { d: data, i: 4 };
    let v = T::dec_bin(&mut b, r).map_err(|e| failure(e, origin))?;
    if b.i != data.len() {
        return Err(failure(Invalid { code: "codec.invalid", detail: "trailing bytes after the value".into() }, origin));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_round_trip_in_both_formats() {
        let v: Vec<(i64, String)> = vec![(1, "a".into()), (-300, "é\n".into())];
        let t = to_text(&v);
        assert_eq!(t, "[[1,\"a\"],[-300,\"é\\n\"]]");
        assert_eq!(from_text::<Vec<(i64, String)>>(&t, &Refine::NONE, "t").ok(), Some(v.clone()));
        let b = to_bytes(&v);
        assert_eq!(from_bytes::<Vec<(i64, String)>>(&b, &Refine::NONE, "t").ok(), Some(v));
    }

    #[test]
    fn refinements_and_ranges_fail_with_their_codes() {
        const PERCENT: Refine = Refine { lo: Some(0), hi: Some(100), ..Refine::NONE };
        assert_eq!(from_text::<i64>("101", &PERCENT, "t").err().map(|f| f.code), Some("refinement.out_of_range"));
        assert_eq!(from_text::<i64>("99999999999999999999", &Refine::NONE, "t").err().map(|f| f.code), Some("codec.invalid"));
        assert_eq!(from_text::<i64>("\"1\"", &Refine::NONE, "t").err().map(|f| f.code), Some("codec.invalid"));
        assert_eq!(from_text::<i64>("1 x", &Refine::NONE, "t").err().map(|f| f.code), Some("codec.invalid"));
    }

    #[test]
    fn base64_is_strict() {
        assert_eq!(base64_decode(&base64_encode(b"foobar")).as_deref(), Some(&b"foobar"[..]));
        assert!(base64_decode("Zm9").is_none());
    }
}

/// A `Bytes` field or value: base64 text, or a byte string.
pub fn dec_bytes_text(j: &J) -> D<Vec<u8>> {
    match j {
        J::Str(s) => base64_decode(s).ok_or(Invalid { code: "codec.invalid", detail: "bad base64".into() }),
        _ => bad("expected base64 text"),
    }
}

pub fn dec_bytes_bin(b: &mut Bin) -> D<Vec<u8>> {
    b.expect(T_BYTES, "bytes")?;
    let n = b.len()?;
    Ok(b.take(n)?.to_vec())
}

/// `Bytes.toText()` / `toBytes()` (a byte vector is not a list in the canonical formats).
pub fn bytes_to_text(v: &[u8]) -> String {
    let mut out = String::new();
    BytesCodec::enc_text(v, &mut out);
    out
}

pub fn bytes_to_bytes(v: &[u8]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    BytesCodec::enc_bin(v, &mut out);
    out
}

/// A variant's tag in an object (`{"Variant":{...}}`): the only key, and its body.
pub fn variant(j: &J) -> D<(&str, Option<&J>)> {
    match j {
        J::Str(s) => Ok((s.as_str(), None)),
        J::Obj(items) if items.len() == 1 => Ok((items[0].0.as_str(), Some(&items[0].1))),
        _ => bad("expected a variant"),
    }
}
