//! Type representations (Language Reference 6.1-6.3, 6.8, 6.9, 6.11): a port of checker/types.py. Integer bounds are
//! `i128`; interval arithmetic saturates far beyond every integer type, which keeps every containment decision.

use std::fmt;
use std::hash::{Hash, Hasher};

pub type Int = i128;

/// `origin` is the declaring module of a core type whose package identity is empty (`fortis.host`).
/// It is not part of equality: nominal identity stays `package` (LANG-04-04). Prelude types leave it empty.
#[derive(Clone, Debug)]
pub struct NamedT {
    pub name: String,
    pub args: Vec<Type>,
    /// nominal identity includes the declaring package@version (LANG-04-04)
    pub package: String,
    pub origin: String,
}

impl PartialEq for NamedT {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.args == other.args && self.package == other.package
    }
}
impl Eq for NamedT {}
impl Hash for NamedT {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.args.hash(state);
        self.package.hash(state);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Named(NamedT),
    Refined { base: NamedT, lo: Option<Int>, hi: Option<Int>, kind: String },
    Optional(Box<Type>),
    Tuple(Vec<Type>),
    Record(Vec<(String, Type)>),
    Fn { params: Vec<Type>, ret: Box<Type>, kind: String, modifiers: Vec<String>, origin: String, dynamic: bool, identity: String },
    Loan { inner: Box<Type>, mode: String, origin: String },
    Dynamic(NamedT),
    TypeParam(String),
    Unknown(String),
}

pub fn named(name: &str) -> Type {
    Type::Named(NamedT { name: name.to_string(), args: vec![], package: String::new(), origin: String::new() })
}
pub fn named_args(name: &str, args: Vec<Type>) -> Type {
    Type::Named(NamedT { name: name.to_string(), args, package: String::new(), origin: String::new() })
}
pub fn named_pkg(name: &str, args: Vec<Type>, package: &str) -> Type {
    Type::Named(NamedT { name: name.to_string(), args, package: package.to_string(), origin: String::new() })
}
pub fn nt(name: &str) -> NamedT {
    NamedT { name: name.to_string(), args: vec![], package: String::new(), origin: String::new() }
}
pub fn unknown(why: &str) -> Type {
    Type::Unknown(why.to_string())
}
pub fn optional(t: Type) -> Type {
    Type::Optional(Box::new(t))
}
pub fn refined(base: NamedT, lo: Option<Int>, hi: Option<Int>, kind: &str) -> Type {
    Type::Refined { base, lo, hi, kind: kind.to_string() }
}

pub fn void() -> Type { named("Void") }
pub fn never() -> Type { named("Never") }
pub fn boolean() -> Type { named("Bool") }
pub fn int() -> Type { named("Int") }
pub fn string() -> Type { named("String") }
pub fn string_view() -> Type { named("StringView") }
pub fn null_t() -> Type { optional(never()) }

impl NamedT {
    pub fn show(&self) -> String {
        if self.args.is_empty() {
            self.name.clone()
        } else {
            format!("{}[{}]", self.name, self.args.iter().map(|a| a.show()).collect::<Vec<_>>().join(", "))
        }
    }
    pub fn as_type(&self) -> Type {
        Type::Named(self.clone())
    }
}

impl Type {
    pub fn show(&self) -> String {
        match self {
            Type::Named(n) => n.show(),
            Type::Refined { base, lo, hi, kind } => {
                let lo = lo.map_or(String::new(), |v| v.to_string());
                let hi = hi.map_or(String::new(), |v| v.to_string());
                let prefix = if kind == "range" { String::new() } else { format!("{kind}: ") };
                format!("{}<{}{}..{}>", base.show(), prefix, lo, hi)
            }
            Type::Optional(i) => format!("{}?", i.show()),
            Type::Tuple(items) => format!("({})", items.iter().map(|t| t.show()).collect::<Vec<_>>().join(", ")),
            Type::Record(fields) => format!("{{ {} }}", fields.iter().map(|(k, t)| format!("{k}: {}", t.show())).collect::<Vec<_>>().join(", ")),
            Type::Fn { params, ret, kind, modifiers, origin, dynamic, .. } => {
                let m = if modifiers.is_empty() { String::new() } else { format!("{} ", modifiers.join(" ")) };
                let o = if origin.is_empty() { String::new() } else { format!(" from {origin}") };
                let d = if *dynamic { "dynamic " } else { "" };
                format!("{d}{m}{kind}({}) {}{o}", params.iter().map(|p| p.show()).collect::<Vec<_>>().join(", "), ret.show())
            }
            Type::Loan { inner, mode, origin } => format!("{mode} {} from {origin}", inner.show()),
            Type::Dynamic(i) => format!("dynamic {}", i.show()),
            Type::TypeParam(n) => n.clone(),
            Type::Unknown(_) => "<unknown>".into(),
        }
    }
    pub fn is_named(&self, name: &str) -> bool {
        matches!(self, Type::Named(n) if n.name == name)
    }
    pub fn named_ref(&self) -> Option<&NamedT> {
        match self {
            Type::Named(n) => Some(n),
            _ => None,
        }
    }
    pub fn is_optional(&self) -> bool {
        matches!(self, Type::Optional(_))
    }
    pub fn opt_inner(&self) -> Option<&Type> {
        match self {
            Type::Optional(i) => Some(i),
            _ => None,
        }
    }
    pub fn is_loan(&self) -> bool {
        matches!(self, Type::Loan { .. })
    }
    pub fn is_unknown(&self) -> bool {
        matches!(self, Type::Unknown(_))
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.show())
    }
}

pub fn int_types(name: &str) -> Option<(Int, Int)> {
    Some(match name {
        "Int" | "Int64" => (-(1i128 << 63), (1i128 << 63) - 1),
        "Int32" => (-(1i128 << 31), (1i128 << 31) - 1),
        "Int16" => (-(1i128 << 15), (1i128 << 15) - 1),
        "Int8" => (-128, 127),
        "UInt" | "UInt64" => (0, (1i128 << 64) - 1),
        "UInt32" => (0, (1i128 << 32) - 1),
        "UInt16" => (0, 65535),
        "UInt8" | "Byte" => (0, 255),
        _ => return None,
    })
}
pub const INT_TYPE_NAMES: &[&str] = &["Int", "Int64", "Int32", "Int16", "Int8", "UInt", "UInt64", "UInt32", "UInt16", "UInt8", "Byte"];
pub fn is_int_type(name: &str) -> bool {
    int_types(name).is_some()
}
pub const FLOAT_TYPES: &[&str] = &["Float", "Float32"];
pub fn is_trivial_builtin(name: &str) -> bool {
    matches!(name, "Void" | "Never" | "Bool" | "Float32" | "Float" | "Decimal" | "Byte" | "Duration" | "Instant" | "Timestamp" | "Scalar" | "Ordering") || is_int_type(name)
}
pub const COPYABLE_HEAP: &[&str] = &["String", "Bytes", "BigInt"];
pub const COPYABLE_CONTAINERS: &[&str] = &["List", "Array", "Map", "OrderedMap", "Set", "FixedList", "FixedMap"];
pub const NOT_COPYABLE: &[&str] = &["Shared", "Weak", "Mutex", "MutexGuard", "AtomicInt", "AtomicBool", "Future", "Task", "Outcome", "Failure", "StringView", "BytesView", "ListView"];
pub const LINEAR_BUILTINS: &[&str] = &["Outcome", "Failure", "Future", "Task", "Must"];

pub fn base_name(t: &Type) -> Option<&str> {
    match t {
        Type::Named(n) => Some(&n.name),
        Type::Refined { base, .. } => Some(&base.name),
        _ => None,
    }
}

/// The declared interval of an integer type (None for non-integers).
pub fn int_bounds(t: &Type) -> Option<(Int, Int)> {
    match t {
        Type::Refined { base, lo, hi, kind } if (kind == "range" || kind == "exact") && is_int_type(&base.name) => {
            let (l, h) = int_types(&base.name).unwrap();
            Some((lo.unwrap_or(l), hi.unwrap_or(h)))
        }
        Type::Named(n) => int_types(&n.name),
        _ => None,
    }
}

pub fn strip(t: &Type) -> &Type {
    match t {
        Type::Loan { inner, .. } => inner,
        _ => t,
    }
}
pub fn strip_owned(t: &Type) -> Type {
    strip(t).clone()
}

pub fn known(t: &Type) -> bool {
    !matches!(t, Type::Unknown(_) | Type::TypeParam(_))
}

pub fn base_type(t: &Type) -> Type {
    let mut t = strip(t).clone();
    while let Type::Refined { base, .. } = t {
        t = Type::Named(base);
    }
    t
}

/// An insertion-ordered map with Python dict semantics: assigning to an existing key keeps its position.
#[derive(Clone, Debug)]
pub struct OMap<V> {
    keys: Vec<String>,
    vals: Vec<V>,
    index: std::collections::HashMap<String, usize>,
}

impl<V> Default for OMap<V> {
    fn default() -> Self {
        OMap { keys: Vec::new(), vals: Vec::new(), index: std::collections::HashMap::new() }
    }
}

impl<V> OMap<V> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, k: &str) -> Option<&V> {
        self.index.get(k).map(|&i| &self.vals[i])
    }
    pub fn get_mut(&mut self, k: &str) -> Option<&mut V> {
        match self.index.get(k) {
            Some(&i) => Some(&mut self.vals[i]),
            None => None,
        }
    }
    pub fn contains(&self, k: &str) -> bool {
        self.index.contains_key(k)
    }
    pub fn insert(&mut self, k: &str, v: V) {
        if let Some(&i) = self.index.get(k) {
            self.vals[i] = v;
        } else {
            self.index.insert(k.to_string(), self.keys.len());
            self.keys.push(k.to_string());
            self.vals.push(v);
        }
    }
    pub fn set_default(&mut self, k: &str, v: V) {
        if !self.contains(k) {
            self.insert(k, v);
        }
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.keys.iter()
    }
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.vals.iter()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.keys.iter().zip(self.vals.iter())
    }
    pub fn sorted_keys(&self) -> Vec<String> {
        let mut k = self.keys.clone();
        k.sort();
        k
    }
}
