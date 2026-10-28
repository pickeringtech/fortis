//! Text intrinsics (6.6): scalar counts and slices with their failures, scalars, the string builder.

use crate::failure::{Dyn, Failure, Outcome};
use crate::show::Show;

#[inline]
pub fn scalar_count(s: &str) -> i64 {
    s.chars().count() as i64
}
#[inline]
pub fn byte_count(s: &str) -> i64 {
    s.len() as i64
}
/// `s.slice(start, end)` in scalars, or `text.slice_bounds`.
#[inline]
pub fn slice(s: &str, a: i64, b: i64) -> Outcome<String> {
    let n = scalar_count(s);
    if 0 <= a && a <= b && b <= n {
        Ok(s.chars().skip(a as usize).take((b - a) as usize).collect())
    } else {
        Err(Failure::new("text.slice_bounds", "").with("start", Dyn::Int(a as i128)).with("end", Dyn::Int(b as i128)))
    }
}
/// `s.slice(a, b)` as a view of `s` (no copy): scalar indices mapped to byte offsets.
#[inline]
pub fn slice_view(s: &str, a: i64, b: i64) -> Outcome<&str> {
    let byte_at = |k: i64| -> Option<usize> {
        if k < 0 {
            return None;
        }
        let k = k as usize;
        let mut it = s.char_indices();
        match it.nth(k) {
            Some((i, _)) => Some(i),
            None if s.chars().count() == k => Some(s.len()),
            None => None,
        }
    };
    match (a <= b).then(|| (byte_at(a), byte_at(b))) {
        Some((Some(x), Some(y))) => Ok(&s[x..y]),
        _ => Err(Failure::new("text.slice_bounds", "").with("start", Dyn::Int(a as i128)).with("end", Dyn::Int(b as i128))),
    }
}

/// A text view of an owned or borrowed string (for `Option<&String>` and `Option<&str>` alike).
#[inline]
pub fn as_str<S: AsRef<str> + ?Sized>(s: &S) -> &str {
    s.as_ref()
}

/// `s.scalarAt(i)`, or `text.index`.
#[inline]
pub fn scalar_at(s: &str, i: i64) -> Outcome<char> {
    match (i >= 0).then(|| s.chars().nth(i as usize)).flatten() {
        Some(c) => Ok(c),
        None => Err(Failure::new("text.index", "").with("index", Dyn::Int(i as i128))),
    }
}
#[inline]
pub fn digit_value(c: char) -> Option<i64> {
    c.to_digit(10).map(|d| d as i64)
}
#[inline]
pub fn hex_value(c: char) -> Option<i64> {
    c.to_digit(16).map(|d| d as i64)
}
#[inline]
pub fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(|w| w.to_string()).collect()
}

/// `text.Builder`
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Builder(pub String);

impl Builder {
    #[inline]
    pub fn create() -> Builder {
        Builder(String::new())
    }
    #[inline]
    pub fn append<T: Show + ?Sized>(&mut self, v: &T) {
        self.0.push_str(&v.show());
    }
    #[inline]
    pub fn append_line<T: Show + ?Sized>(&mut self, v: &T) {
        self.0.push_str(&v.show());
        self.0.push('\n');
    }
    #[inline]
    pub fn finish(&self) -> String {
        self.0.clone()
    }
}

impl Show for Builder {
    fn show(&self) -> String {
        "<Builder>".into()
    }
}

/// Extended grapheme clusters for the rules the contract names (GB3, GB9, GB11, GB12/13), as the interpreter
/// approximates them.
pub fn graphemes(s: &str) -> i64 {
    fn is_extend(c: char) -> bool {
        let o = c as u32;
        (0x0300..=0x036F).contains(&o) || (0x1AB0..=0x1AFF).contains(&o) || (0x1DC0..=0x1DFF).contains(&o) || (0x20D0..=0x20FF).contains(&o) || (0xFE20..=0xFE2F).contains(&o) || (0xFE00..=0xFE0F).contains(&o) || (0x1F3FB..=0x1F3FF).contains(&o)
    }
    fn is_pict(c: char) -> bool {
        let o = c as u32;
        (0x1F000..=0x1FAFF).contains(&o) || (0x2600..=0x27BF).contains(&o)
    }
    fn is_ri(c: char) -> bool {
        (0x1F1E6..=0x1F1FF).contains(&(c as u32))
    }
    let (mut count, mut prev, mut ri_run) = (0i64, None::<char>, 0u32);
    for c in s.chars() {
        match prev {
            None => count += 1,
            Some(p) => {
                let joined = (p == '\r' && c == '\n') || is_extend(c) || c == '\u{200D}' || (p == '\u{200D}' && is_pict(c)) || (is_ri(c) && is_ri(p) && ri_run % 2 == 1);
                if !joined {
                    count += 1;
                }
            }
        }
        ri_run = if is_ri(c) { ri_run + 1 } else { 0 };
        prev = Some(c);
    }
    count
}

/// `Scalar.of(s)`: the one scalar of a one-scalar text.
#[inline]
pub fn scalar_of(s: &str) -> char {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => c,
        _ => crate::dynamic("Scalar.of of a view that is not one scalar"),
    }
}
