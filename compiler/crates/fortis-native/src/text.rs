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
/// An optional loan of text as a result: the signature is `Option<&str>`, while an intrinsic (`xs.get(0)`,
/// `xs.first()`, `m.get(k)`), a binding or an `if` or `match` of them gives `Option<&String>` (D-353). The
/// referent is anything read as a `str`, so an already viewed option passes through as it is (D-390).
#[inline]
pub fn opt_view<'a, T: AsRef<str> + ?Sized>(o: Option<&'a T>) -> Option<&'a str> {
    o.map(|s| s.as_ref())
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

// The everyday text operations (6.6, X-479) are the runtime's own: the VM calls the same functions.
pub use fortis_runtime::text::{ascii_lower, ascii_upper, find, lines, replace, split, trim, trim_end, trim_start};

/// `s.startsWith(prefix)`.
#[inline]
pub fn starts_with(s: &str, prefix: &str) -> bool {
    s.starts_with(prefix)
}
/// `s.endsWith(suffix)`.
#[inline]
pub fn ends_with(s: &str, suffix: &str) -> bool {
    s.ends_with(suffix)
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

/// The shared Unicode grapheme implementation (D-713).
pub fn graphemes(s: &str) -> i64 {
    fortis_runtime::graphemes::count(s) as i64
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
