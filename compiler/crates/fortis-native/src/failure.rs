//! Failures (Language Reference 10): a code, a payload of named values, the origin function, a message, a cause
//! chain and annotations. The checker and the IR validator prove linearity statically (no live Failure is ever
//! dropped); the runtime carries no dynamic backstop that could abort a correct program.

use std::fmt;

use fortis_runtime::Decimal;

use crate::show::Show;

/// A payload value, kept with its type for pattern matching and with its display form for reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Dyn {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Dec(Decimal),
    Str(String),
    Other(String),
    /// A payload-free enum: `variant` is the discriminant and `shown` is its display text. The two are
    /// independent, so a `display` that does not name the variant still matches (D-55).
    Tag { variant: &'static str, shown: String },
}

impl Show for Dyn {
    fn show(&self) -> String {
        match self {
            Dyn::Null => "null".into(),
            Dyn::Bool(b) => b.show(),
            Dyn::Int(n) => n.show(),
            Dyn::Float(x) => x.show(),
            Dyn::Dec(d) => d.show(),
            Dyn::Str(s) | Dyn::Other(s) => s.clone(),
            Dyn::Tag { shown, .. } => shown.clone(),
        }
    }
}

/// Conversion of a payload field into a `Dyn` (and back, for a pattern binding its typed field).
pub trait IntoDyn {
    fn into_dyn(self) -> Dyn;
}
macro_rules! dyn_int {
    ($($t:ty),*) => {$(
        impl IntoDyn for $t { fn into_dyn(self) -> Dyn { Dyn::Int(self as i128) } }
    )*};
}
dyn_int!(i8, i16, i32, i64, u8, u16, u32, u64);
impl IntoDyn for bool {
    fn into_dyn(self) -> Dyn {
        Dyn::Bool(self)
    }
}
impl IntoDyn for f64 {
    fn into_dyn(self) -> Dyn {
        Dyn::Float(self)
    }
}
impl IntoDyn for Decimal {
    fn into_dyn(self) -> Dyn {
        Dyn::Dec(self)
    }
}
impl IntoDyn for String {
    fn into_dyn(self) -> Dyn {
        Dyn::Str(self)
    }
}
impl IntoDyn for &str {
    fn into_dyn(self) -> Dyn {
        Dyn::Str(self.to_string())
    }
}
impl<T: IntoDyn> IntoDyn for Option<T> {
    fn into_dyn(self) -> Dyn {
        match self {
            Some(v) => v.into_dyn(),
            None => Dyn::Null,
        }
    }
}

/// `Clone` serves the bounds of the generated generic library functions (`List` methods over any element,
/// D-55): a `Failure` itself is linear, and the lowering never clones one the checker did not copy.
#[derive(Clone)]
pub struct Failure {
    pub code: &'static str,
    pub payload: Vec<(&'static str, Dyn)>,
    pub origin: &'static str,
    pub message: String,
    pub cause: Option<Box<Failure>>,
    pub annotations: Vec<String>,
}

impl Failure {
    pub fn new(code: &'static str, origin: &'static str) -> Failure {
        Failure { code, payload: vec![], origin, message: String::new(), cause: None, annotations: vec![] }
    }
    pub fn with(mut self, field: &'static str, v: Dyn) -> Failure {
        self.payload.push((field, v));
        self
    }
    pub fn message(mut self, m: &str) -> Failure {
        self.message = m.to_string();
        self
    }
    pub fn code(&self) -> String {
        self.code.to_string()
    }
    pub fn field(&self, name: &str) -> Dyn {
        self.payload.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone()).unwrap_or(Dyn::Null)
    }
    pub fn int(&self, name: &str) -> i64 {
        match self.field(name) {
            Dyn::Int(n) => n as i64,
            _ => 0,
        }
    }
    pub fn text(&self, name: &str) -> String {
        match self.field(name) {
            Dyn::Str(s) => s,
            Dyn::Tag { shown, .. } => shown,
            other => other.show(),
        }
    }
    /// The variant name of a payload-free enum, or the stored text of any other payload (D-55).
    pub fn tag(&self, name: &str) -> String {
        match self.field(name) {
            Dyn::Tag { variant, .. } => variant.to_string(),
            Dyn::Str(s) | Dyn::Other(s) => s,
            other => other.show(),
        }
    }
    pub fn boolean(&self, name: &str) -> bool {
        matches!(self.field(name), Dyn::Bool(true))
    }
    pub fn decimal(&self, name: &str) -> Decimal {
        match self.field(name) {
            Dyn::Dec(d) => d,
            _ => Decimal::zero(),
        }
    }
    /// `annotate(f, note)` (10.6)
    pub fn annotate(mut self, note: String) -> Failure {
        self.annotations.push(note);
        self
    }
}

/// Two failures are equal when their codes and payloads are (a class holding an outcome derives equality).
impl PartialEq for Failure {
    fn eq(&self, o: &Failure) -> bool {
        self.code == o.code && self.payload == o.payload
    }
}

impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Failure({})", self.code)
    }
}

impl Show for Failure {
    fn show(&self) -> String {
        format!("Failure({})", self.code)
    }
}

/// `report f [as r]`: the failure's report (10.4).
pub struct Report {
    pub code: String,
}

impl Show for Report {
    fn show(&self) -> String {
        format!("Report(code='{}')", self.code)
    }
}

pub type Outcome<T> = Result<T, Failure>;

/// The error of an outcome. The result is destroyed here, before the caller runs block-exit drops. An inlined
/// match would keep the result's destructor in the caller, and a guard's borrow would then outlive an explicit
/// drop of the mutex it borrowed (E0505).
pub fn outcome_err<T, E>(r: Result<T, E>) -> E {
    match r {
        Err(e) => e,
        Ok(_) => unreachable!("outcome_err called on a success"),
    }
}

/// `Failures.aggregate(fs)`: one failure whose cause chain is the given failures, in order (10.6).
pub fn aggregate(fs: Vec<Failure>, origin: &'static str) -> Failure {
    let n = fs.len();
    let mut chain: Option<Box<Failure>> = None;
    for mut f in fs.into_iter().rev() {
        f.cause = chain;
        chain = Some(Box::new(f));
    }
    let mut agg = Failure::new("failure.aggregate", origin).with("count", Dyn::Int(n as i128));
    agg.cause = chain;
    agg
}
