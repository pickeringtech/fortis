//! The display form of every value (Language Reference 6.10), equal to the definitional interpreter's `display`,
//! and Python's `repr` for the values the interpreter prints inside tuples and records.

use fortis_runtime::Decimal;

pub trait Show {
    fn show(&self) -> String;
    /// Append the display form to `out` (interpolation writes every part into one buffer).
    #[inline]
    fn write_to(&self, out: &mut String) {
        out.push_str(&self.show());
    }
    /// the form a value takes inside a tuple or record display (the interpreter prints those with `repr`)
    fn repr(&self) -> String {
        self.show()
    }
}

macro_rules! show_int {
    ($($t:ty),*) => {$(
        impl Show for $t {
            #[inline]
            fn show(&self) -> String {
                self.to_string()
            }
            #[inline]
            fn write_to(&self, out: &mut String) {
                use std::fmt::Write;
                let _ = write!(out, "{}", self);
            }
        }
    )*};
}
show_int!(i8, i16, i32, i128, u8, u16, u32, usize);

const DIGIT_PAIRS: &[u8; 200] = b"0001020304050607080910111213141516171819\
2021222324252627282930313233343536373839\
4041424344454647484950515253545556575859\
6061626364656667686970717273747576777879\
8081828384858687888990919293949596979899";

/// Decimal digits of `n` appended to `out`, two at a time (no formatting machinery: interpolation of an
/// `Int` is the hottest display path).
#[inline]
pub fn write_u64(out: &mut String, mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while n >= 100 {
        let d = (n % 100) as usize * 2;
        n /= 100;
        i -= 2;
        buf[i] = DIGIT_PAIRS[d];
        buf[i + 1] = DIGIT_PAIRS[d + 1];
    }
    if n >= 10 {
        let d = n as usize * 2;
        i -= 2;
        buf[i] = DIGIT_PAIRS[d];
        buf[i + 1] = DIGIT_PAIRS[d + 1];
    } else {
        i -= 1;
        buf[i] = b'0' + n as u8;
    }
    for &b in &buf[i..] {
        out.push(b as char);
    }
}

#[inline]
pub fn write_i64(out: &mut String, n: i64) {
    if n < 0 {
        out.push('-');
    }
    write_u64(out, n.unsigned_abs());
}

impl Show for i64 {
    #[inline]
    fn show(&self) -> String {
        let mut s = String::with_capacity(20);
        write_i64(&mut s, *self);
        s
    }
    #[inline]
    fn write_to(&self, out: &mut String) {
        write_i64(out, *self);
    }
}

impl Show for u64 {
    #[inline]
    fn show(&self) -> String {
        let mut s = String::with_capacity(20);
        write_u64(&mut s, *self);
        s
    }
    #[inline]
    fn write_to(&self, out: &mut String) {
        write_u64(out, *self);
    }
}

#[cfg(test)]
mod int_tests {
    use super::Show;

    #[test]
    fn integers_display_as_rust_does() {
        for v in [0i64, 1, 9, 10, 99, 100, 101, 999, 1000, 12345, -1, -10, -99, -100, 1_000_000_007, i64::MAX, i64::MIN, i64::MIN + 1] {
            assert_eq!(v.show(), v.to_string());
        }
        for v in [0u64, 7, 42, 100, u64::MAX, u64::MAX / 3] {
            assert_eq!(v.show(), v.to_string());
        }
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..100_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            assert_eq!((x as i64).show(), (x as i64).to_string());
            assert_eq!((x >> (x % 64)).show(), (x >> (x % 64)).to_string());
        }
    }
}

#[cfg(test)]
mod interp_tests {
    use super::Show;
    use std::cell::RefCell;

    thread_local! {
        static ORDER: RefCell<Vec<String>> = RefCell::new(Vec::new());
    }

    struct Temp(&'static str);
    impl Show for Temp {
        fn show(&self) -> String {
            ORDER.with(|o| o.borrow_mut().push(format!("show {}", self.0)));
            self.0.to_string()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            ORDER.with(|o| o.borrow_mut().push(format!("drop {}", self.0)));
        }
    }

    /// An interpolation is part of its statement's expression (7.9; D-150): a temporary its parts create
    /// lives to the end of the whole statement — the statement's call runs first — and the temporaries are
    /// destroyed there in reverse creation order.
    #[test]
    fn temporaries_live_to_the_end_of_the_enclosing_statement() {
        fn consume(s: String) {
            ORDER.with(|o| o.borrow_mut().push(format!("use {s}")));
        }
        ORDER.with(|o| o.borrow_mut().clear());
        consume(crate::interp!(Temp("a"), Temp("b"), " tail"));
        ORDER.with(|o| o.borrow_mut().push("after".into()));
        let log = ORDER.with(|o| (*o.borrow()).clone());
        assert_eq!(log, ["show a", "show b", "use ab tail", "drop b", "drop a", "after"]);
    }

    /// A later part may edit a place an earlier part already showed (3.6). The first part's borrow ends when
    /// its text is owned, before the later part runs.
    #[test]
    fn a_later_part_may_edit_a_place_an_earlier_part_showed() {
        struct Cell {
            n: i64,
        }
        impl Cell {
            fn bump(&mut self) -> i64 {
                self.n += 1;
                self.n
            }
        }
        let mut c = Cell { n: 1 };
        assert_eq!(crate::interp!(c.n, c.bump()), "12");
    }
}

impl Show for bool {
    #[inline]
    fn show(&self) -> String {
        if *self { "true".into() } else { "false".into() }
    }
    fn repr(&self) -> String {
        if *self { "True".into() } else { "False".into() }
    }
}
impl Show for f64 {
    #[inline]
    fn show(&self) -> String {
        fortis_runtime::float::display(*self)
    }
}
impl Show for f32 {
    #[inline]
    fn show(&self) -> String {
        fortis_runtime::float::display32(*self)
    }
}
impl Show for Decimal {
    #[inline]
    fn show(&self) -> String {
        self.to_string()
    }
}
impl Show for char {
    #[inline]
    fn show(&self) -> String {
        self.to_string()
    }
}
fn py_str_repr(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') {
        format!("\"{s}\"")
    } else {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}
impl Show for String {
    #[inline]
    fn show(&self) -> String {
        self.clone()
    }
    #[inline]
    fn write_to(&self, out: &mut String) {
        out.push_str(self);
    }
    fn repr(&self) -> String {
        py_str_repr(self)
    }
}
impl Show for str {
    #[inline]
    fn show(&self) -> String {
        self.to_string()
    }
    #[inline]
    fn write_to(&self, out: &mut String) {
        out.push_str(self);
    }
    fn repr(&self) -> String {
        py_str_repr(self)
    }
}
impl Show for () {
    #[inline]
    fn show(&self) -> String {
        "Void".into()
    }
}

fn show_fn_ptr() -> String {
    "<fn>".into()
}
impl<R: Show> Show for fn() -> R {
    fn show(&self) -> String { show_fn_ptr() }
}
impl<A: Show, R: Show> Show for fn(A) -> R {
    fn show(&self) -> String { show_fn_ptr() }
}
impl<A: Show, B: Show, R: Show> Show for fn(A, B) -> R {
    fn show(&self) -> String { show_fn_ptr() }
}
impl<A: Show, B: Show, C: Show, R: Show> Show for fn(A, B, C) -> R {
    fn show(&self) -> String { show_fn_ptr() }
}
impl<A: Show, B: Show, C: Show, D: Show, R: Show> Show for fn(A, B, C, D) -> R {
    fn show(&self) -> String { show_fn_ptr() }
}
impl<A: Show, B: Show, C: Show, D: Show, E: Show, R: Show> Show for fn(A, B, C, D, E) -> R {
    fn show(&self) -> String { show_fn_ptr() }
}
impl<A: Show, B: Show, C: Show, D: Show, E: Show, F: Show, R: Show> Show for fn(A, B, C, D, E, F) -> R {
    fn show(&self) -> String { show_fn_ptr() }
}
impl<T: Show + ?Sized> Show for &T {
    #[inline]
    fn show(&self) -> String {
        (**self).show()
    }
    #[inline]
    fn write_to(&self, out: &mut String) {
        (**self).write_to(out)
    }
    fn repr(&self) -> String {
        (**self).repr()
    }
}
impl<T: Show + ?Sized> Show for &mut T {
    #[inline]
    fn show(&self) -> String {
        (**self).show()
    }
    fn repr(&self) -> String {
        (**self).repr()
    }
}
impl<T: Show + ?Sized> Show for Box<T> {
    #[inline]
    fn show(&self) -> String {
        (**self).show()
    }
}
impl<T: Show> Show for Vec<T> {
    #[inline]
    fn show(&self) -> String {
        format!("[{}]", self.iter().map(|x| x.show()).collect::<Vec<_>>().join(", "))
    }
}
impl<T: Show> Show for [T] {
    #[inline]
    fn show(&self) -> String {
        format!("[{}]", self.iter().map(|x| x.show()).collect::<Vec<_>>().join(", "))
    }
}
impl<T: Show> Show for Option<T> {
    #[inline]
    fn show(&self) -> String {
        match self {
            Some(v) => v.show(),
            None => "null".into(),
        }
    }
    fn repr(&self) -> String {
        match self {
            Some(v) => v.repr(),
            None => "None".into(),
        }
    }
}
impl<A: Show, B: Show> Show for (A, B) {
    #[inline]
    fn show(&self) -> String {
        format!("({}, {})", self.0.repr(), self.1.repr())
    }
}
impl<A: Show, B: Show, C: Show> Show for (A, B, C) {
    #[inline]
    fn show(&self) -> String {
        format!("({}, {}, {})", self.0.repr(), self.1.repr(), self.2.repr())
    }
}
impl<T: Show, E: Show> Show for Result<T, E> {
    #[inline]
    fn show(&self) -> String {
        match self {
            Ok(v) => v.show(),
            Err(e) => e.show(),
        }
    }
}

/// The display forms of `parts` concatenated (the expansion of `interp!`).
#[inline]
pub fn joined(parts: &[&dyn Show]) -> String {
    let mut s = String::with_capacity(32);
    for p in parts {
        p.write_to(&mut s);
    }
    s
}

/// One interpolation part as owned text. The borrow of `p` ends when this returns, so the next part may
/// edit a place this part displayed (3.6; D-150).
#[inline]
pub fn part_text(p: &dyn Show) -> String {
    let mut s = String::new();
    p.write_to(&mut s);
    s
}

/// Owned part texts concatenated. A call, so each part's temporary still lives to the end of the enclosing
/// statement (7.9; D-150): a block would drop it before the statement's call runs.
#[inline]
pub fn joined_owned(parts: &[String]) -> String {
    let mut s = String::with_capacity(32);
    for p in parts {
        s.push_str(p);
    }
    s
}

/// `"${a}${b}"`: the display forms concatenated. The expansion is one call, so a temporary any part creates
/// lives to the end of the whole statement the interpolation appears in and is destroyed there, in reverse
/// creation order (7.9; D-150). Each part is shown to an owned `String` before the next part is evaluated, so
/// a later part may edit a place an earlier part displayed (3.6).
#[macro_export]
macro_rules! interp {
    ($($e:expr),* $(,)?) => {
        $crate::show::joined_owned(&[$($crate::show::part_text(&$e)),*])
    };
}

/// A string-like operand as `&str` (comparisons of String, &String and &str).
#[inline]
pub fn s<S: AsRef<str> + ?Sized>(x: &S) -> &str {
    x.as_ref()
}
