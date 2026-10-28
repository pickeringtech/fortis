//! Minimal native runtime for the synchronous core (DEL-25, experimental lowering).
//! The generated application crate carries `#![forbid(unsafe_code)]`; this runtime crate is audited
//! separately (Appendix C.1): its one `unsafe` is the counting allocator of the probe.
#![deny(unsafe_code)]
#![allow(dead_code, non_snake_case, unused_imports, unused_variables, unused_mut)]
use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

// ---- allocator probe (SP-04): counts every heap allocation of the process ---------------------
pub mod probe {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};
    pub struct Counting;
    pub static ALLOCS: AtomicUsize = AtomicUsize::new(0);
    #[allow(unsafe_code)]
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 { ALLOCS.fetch_add(1, Ordering::Relaxed); System.alloc(l) }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p, l) }
    }
    pub fn count() -> usize { ALLOCS.load(Ordering::Relaxed) }
    /// Runs a `noalloc` body under the probe: any allocation is reported and the process aborts (10.4 / C.3).
    pub fn noalloc<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let before = count();
        let out = f();
        let delta = count() - before;
        if delta > 0 {
            eprintln!("{{\"kind\":\"alloc-probe\",\"function\":\"{}\",\"allocations\":{}}}", name, delta);
            std::process::abort();
        }
        out
    }
}

// ---- failures (10): linear; a live Failure is never dropped (the aborting Drop is the backstop) --------
#[must_use]
pub struct Failure {
    pub code: &'static str,
    pub payload: Payload,
    pub origin: &'static str,
    pub cause: Option<Box<Failure>>,
    consumed: bool,
}
impl Failure {
    pub fn new(code: &'static str, payload: Payload, origin: &'static str) -> Failure {
        Failure { code, payload, origin, cause: None, consumed: false }
    }
    /// `report f`: one runtime-failure record, then the failure is consumed (10.6).
    pub fn report(mut self, by: &str) -> Report {
        let code = self.code;
        eprintln!("{{\"cause\":[],\"code\":\"{}\",\"kind\":\"runtime-failure\",\"message\":\"\",\"origin\":{{\"function\":\"{}\"}},\"payload\":{},\"reported_by\":\"{}\",\"rule\":\"LANG-10-06\"}}",
                  code, self.origin, self.payload.json(), by);
        self.consumed = true;
        Report { code }
    }
    /// `or recover` / `catch … recover` without a report: the failure is discharged (10.4).
    pub fn recover(mut self) { self.consumed = true; }
    /// consumed by a move into trusted code or a container that will consume it
    pub fn consume(mut self) { self.consumed = true; }
    pub fn code(&self) -> &'static str { self.code }
}
impl Drop for Failure {
    fn drop(&mut self) {
        if !self.consumed && !std::thread::panicking() {
            eprintln!("{{\"kind\":\"defect\",\"class\":\"runtime_invariant\",\"detail\":\"a live Failure was dropped: {}\"}}", self.code);
            std::process::abort();
        }
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "Failure({})", self.code) }
}
pub struct Report { pub code: &'static str }
/// The bounded failure payload (10.4): up to two named integers inline, text or more by allocation.
pub enum Payload {
    None,
    Int(&'static str, i64),
    Ints((&'static str, i64), (&'static str, i64)),
    Text(&'static str, String),
    Many(Vec<(&'static str, String)>),
}
impl Payload {
    pub fn json(&self) -> String {
        match self {
            Payload::None => "{}".into(),
            Payload::Int(k, v) => format!("{{\"{}\":\"{}\"}}", k, v),
            Payload::Ints((k1, v1), (k2, v2)) => format!("{{\"{}\":\"{}\",\"{}\":\"{}\"}}", k1, v1, k2, v2),
            Payload::Text(k, v) => format!("{{\"{}\":\"{}\"}}", k, v),
            Payload::Many(items) => format!("{{{}}}", items.iter().map(|(k, v)| format!("\"{}\":\"{}\"", k, v)).collect::<Vec<_>>().join(",")),
        }
    }
    pub fn int(&self, key: &str) -> i64 {
        match self {
            Payload::Int(k, v) if *k == key => *v,
            Payload::Ints((k1, v1), (k2, v2)) => if *k1 == key { *v1 } else if *k2 == key { *v2 } else { 0 },
            Payload::Many(items) => items.iter().find(|(k, _)| *k == key).and_then(|(_, v)| v.parse().ok()).unwrap_or(0),
            _ => 0,
        }
    }
    pub fn text(&self, key: &str) -> String {
        match self {
            Payload::Text(k, v) if *k == key => v.clone(),   /* copy: payload field read out of a matched failure (10.5) */
            Payload::Many(items) => items.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap_or_default(),
            _ => String::new(),
        }
    }
}
pub fn annotate(mut f: Failure, _note: &str) -> Failure { f }
pub type Outcome<T> = Result<T, Failure>;
pub fn collect<T>(items: Vec<Outcome<T>>) -> Outcome<Vec<T>> {
    let mut out = Vec::with_capacity(items.len());
    let mut failed: Option<Failure> = None;
    for it in items {
        match it {
            Ok(v) => out.push(v),
            Err(f) => { if failed.is_none() { failed = Some(f) } else { f.consume() } }
        }
    }
    match failed { Some(f) => { for _ in out.drain(..) {} Err(f) }, None => Ok(out) }
}

// ---- display (interpolation parity with the interpreter) ---------------------------------------
pub trait Show { fn show(&self) -> String; }
impl Show for i64 { fn show(&self) -> String { self.to_string() } }
impl Show for bool { fn show(&self) -> String { if *self { "true".into() } else { "false".into() } } }
impl Show for String { fn show(&self) -> String { self.clone() } }
impl Show for &str { fn show(&self) -> String { self.to_string() } }
impl Show for str { fn show(&self) -> String { self.to_string() } }
impl<T: Show> Show for Vec<T> { fn show(&self) -> String { format!("[{}]", self.iter().map(|x| x.show()).collect::<Vec<_>>().join(", ")) } }
impl<T: Show> Show for Option<T> { fn show(&self) -> String { match self { Some(v) => v.show(), None => "null".into() } } }
impl<T: Show> Show for &T { fn show(&self) -> String { (**self).show() } }
impl<T: Show> Show for Rc<T> { fn show(&self) -> String { (**self).show() } }
impl Show for Failure { fn show(&self) -> String { format!("Failure({})", self.code) } }
impl<T: Show> Show for Result<T, Failure> { fn show(&self) -> String { match self { Ok(v) => v.show(), Err(f) => f.show() } } }
impl Show for Report { fn show(&self) -> String { self.code.to_string() } }
impl Show for () { fn show(&self) -> String { "Void".into() } }
impl Show for f64 {
    fn show(&self) -> String {
        if self.is_nan() { return "NaN".into() }
        if self.is_infinite() { return if *self > 0.0 { "Infinity".into() } else { "-Infinity".into() } }
        let s = format!("{:?}", self);
        if s.contains('e') { s } else if s.contains('.') { s } else { format!("{}.0", s) }
    }
}
impl Show for f32 { fn show(&self) -> String { (*self as f64).show() } }
impl Show for Console { fn show(&self) -> String { "Console".into() } }
impl Show for Clock { fn show(&self) -> String { "Clock".into() } }
impl Show for FileSystem { fn show(&self) -> String { "FileSystem".into() } }
impl Show for Must { fn show(&self) -> String { "Must".into() } }
impl Show for Path { fn show(&self) -> String { self.0.clone() } }

// ---- integers (6.4): checked operations are outcomes; wrapping and saturating are total -----------
pub mod int {
    use super::{Failure, Outcome, Payload};
    pub fn add(a: i64, b: i64) -> Outcome<i64> { a.checked_add(b).ok_or_else(|| Failure::new("numeric.overflow", Payload::None, "Int.add")) }
    pub fn sub(a: i64, b: i64) -> Outcome<i64> { a.checked_sub(b).ok_or_else(|| Failure::new("numeric.overflow", Payload::None, "Int.sub")) }
    pub fn mul(a: i64, b: i64) -> Outcome<i64> { a.checked_mul(b).ok_or_else(|| Failure::new("numeric.overflow", Payload::None, "Int.mul")) }
    pub fn div(a: i64, b: i64) -> Outcome<i64> { if b == 0 { Err(Failure::new("numeric.divide_by_zero", Payload::None, "Int.div")) } else { a.checked_div(b).ok_or_else(|| Failure::new("numeric.overflow", Payload::None, "Int.div")) } }
    pub fn rem(a: i64, b: i64) -> Outcome<i64> { if b == 0 { Err(Failure::new("numeric.divide_by_zero", Payload::None, "Int.rem")) } else { Ok(a.wrapping_rem(b)) } }
    pub fn shiftLeft(a: i64, b: i64) -> Outcome<i64> { if !(0..64).contains(&b) { return Err(Failure::new("numeric.shift_range", Payload::None, "Int.shiftLeft")) } a.checked_shl(b as u32).ok_or_else(|| Failure::new("numeric.overflow", Payload::None, "Int.shiftLeft")) }
    pub fn wrappingAdd(a: i64, b: i64) -> i64 { a.wrapping_add(b) }
    pub fn wrappingSub(a: i64, b: i64) -> i64 { a.wrapping_sub(b) }
    pub fn wrappingMul(a: i64, b: i64) -> i64 { a.wrapping_mul(b) }
    pub fn saturatingAdd(a: i64, b: i64) -> i64 { a.saturating_add(b) }
    pub fn saturatingSub(a: i64, b: i64) -> i64 { a.saturating_sub(b) }
    pub fn saturatingMul(a: i64, b: i64) -> i64 { a.saturating_mul(b) }
}
/// proof-required operators (6.4): the checker proved these in range, so the plain operation is total here
pub fn pdiv(a: i64, b: i64) -> i64 { a / b }
pub fn prem(a: i64, b: i64) -> i64 { a % b }
pub fn validate_int(x: i64, lo: Option<i64>, hi: Option<i64>, what: &'static str) -> Outcome<i64> {
    if lo.map_or(false, |l| x < l) || hi.map_or(false, |h| x > h) { Err(Failure::new("refinement.out_of_range", Payload::None, what)) } else { Ok(x) }
}

// ---- lists (6.7) -------------------------------------------------------------------------------------
pub trait ListExt<T> {
    fn count(&self) -> i64;
    fn isEmpty(&self) -> bool;
    fn get(&self, i: i64) -> Option<&T>;
    fn at(&self, i: i64) -> Outcome<&T>;
    fn first(&self) -> Option<&T>;
    fn reversed(&self) -> Vec<T> where T: Clone;
    fn concat(&self, other: &Vec<T>) -> Vec<T> where T: Clone;
    fn countWhere(&self, f: impl Fn(&T) -> bool) -> i64;
    fn map<U>(&self, f: impl Fn(&T) -> U) -> Vec<U>;
    fn filter(&self, f: impl Fn(&T) -> bool) -> Vec<T> where T: Clone;
    fn set(&mut self, i: i64, v: T) -> Outcome<()>;
}
impl<T> ListExt<T> for Vec<T> {
    fn count(&self) -> i64 { self.len() as i64 }
    fn isEmpty(&self) -> bool { self.is_empty() }
    fn get(&self, i: i64) -> Option<&T> { if i < 0 { None } else { <[T]>::get(self, i as usize) } }
    fn at(&self, i: i64) -> Outcome<&T> { self.get(i).ok_or_else(|| Failure::new("collection.index", Payload::None, "List.at")) }
    fn first(&self) -> Option<&T> { <[T]>::first(self) }
    fn reversed(&self) -> Vec<T> where T: Clone { let mut v = self.clone(); v.reverse(); v }   /* copy: List.reversed allocates a new list (6.7) */
    fn concat(&self, other: &Vec<T>) -> Vec<T> where T: Clone { let mut v = self.clone(); v.extend(other.iter().cloned()); v }   /* copy: List.concat (6.7) */
    fn countWhere(&self, f: impl Fn(&T) -> bool) -> i64 { self.iter().filter(|x| f(x)).count() as i64 }
    fn map<U>(&self, f: impl Fn(&T) -> U) -> Vec<U> { self.iter().map(f).collect() }
    fn filter(&self, f: impl Fn(&T) -> bool) -> Vec<T> where T: Clone { self.iter().filter(|x| f(x)).cloned().collect() }   /* copy: List.filter copies the kept elements (6.7) */
    fn set(&mut self, i: i64, v: T) -> Outcome<()> { if i < 0 || i as usize >= self.len() { return Err(Failure::new("collection.index", Payload::None, "List.set")) } self[i as usize] = v; Ok(()) }
}
pub fn list_pop<T>(v: &mut Vec<T>) -> Option<T> { v.pop() }

// ---- text (6.6) --------------------------------------------------------------------------------------
pub trait TextExt { fn scalarCount(&self) -> i64; fn byteCount(&self) -> i64; fn isEmpty(&self) -> bool; }
impl TextExt for str { fn scalarCount(&self) -> i64 { self.chars().count() as i64 } fn byteCount(&self) -> i64 { self.len() as i64 } fn isEmpty(&self) -> bool { self.is_empty() } }
impl TextExt for String { fn scalarCount(&self) -> i64 { self.chars().count() as i64 } fn byteCount(&self) -> i64 { self.len() as i64 } fn isEmpty(&self) -> bool { self.is_empty() } }
pub struct Builder { parts: String }
impl Builder {
    pub fn create() -> Builder { Builder { parts: String::new() } }
    pub fn append(&mut self, s: &str) { self.parts.push_str(s) }
    pub fn appendLine(&mut self, s: &str) { self.parts.push_str(s); self.parts.push('\n') }
    pub fn finish(self) -> String { self.parts }
}

// ---- shared ownership (7.8): Rc in the synchronous core --------------------------------------------
pub type Shared<T> = Rc<T>;
pub type Weak<T> = std::rc::Weak<T>;
pub fn shared_create<T>(v: T) -> Rc<T> { Rc::new(v) }   /* reference count: Shared creation (7.8) */

// ---- linear witness (6.12) ---------------------------------------------------------------------------
#[must_use]
pub struct Must(());
impl Must { pub fn create() -> Must { Must(()) } pub fn discharge(_m: Must) {} }

// ---- capabilities (11.6): zero-sized handles with private constructors ------------------------------
pub struct Console(());
impl Console { pub fn print(&self, text: &str) { println!("{}", text) } }
pub struct FileSystem(());
pub struct Path(pub String);
impl FileSystem {
    pub fn readText(&self, p: &Path) -> Outcome<String> {
        std::fs::read_to_string(&p.0).map_err(|_| Failure::new("fs.not_found", Payload::None, "FileSystem.readText"))
    }
}
pub struct Clock(());
pub struct Network(());
pub struct Random(());
pub struct Environment(());
pub struct Process(());
pub struct Supervisor(());
pub struct SecretAccess(());
pub struct RawSql(());
pub struct Capabilities {
    pub fileSystem: FileSystem, pub network: Network, pub clock: Clock, pub random: Random, pub environment: Environment,
    pub process: Process, pub supervisor: Supervisor, pub console: Console, pub secretAccess: SecretAccess, pub rawSql: RawSql,
}
pub struct Host;
impl Host {
    pub fn capabilities() -> Capabilities {
        Capabilities { fileSystem: FileSystem(()), network: Network(()), clock: Clock(()), random: Random(()), environment: Environment(()),
                       process: Process(()), supervisor: Supervisor(()), console: Console(()), secretAccess: SecretAccess(()), rawSql: RawSql(()) }
    }
}

/// `main` driver: a failing `try main` writes one record and exits 1 (4.6).
pub fn run_main(result: Outcome<()>) -> i32 {
    match result { Ok(()) => 0, Err(f) => { f.report("main"); 1 } }
}
