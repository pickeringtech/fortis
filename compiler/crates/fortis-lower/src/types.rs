//! Fortis types as Rust types (Appendix C.1): primitives as Rust primitives, classes as structs, enums as enums,
//! `T?` as `Option<T>`, an outcome as `Result<T, Failure>`, a read loan as `&T`, an exclusive loan as `&mut T`,
//! `dynamic I` as `Box<dyn I>`, a function type in parameter position as `impl Fn…` (never boxed unless `dynamic fn`).

use fortis_mir::model::*;
use fortis_types::types::{strip, Type};

pub const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro", "override", "priv", "typeof",
    "unsized", "virtual", "yield", "try", "gen",
];

/// The trait a `dynamic I` names, with the interface's type arguments (`dyn Store<i64>`).
pub fn dyn_name(n: &fortis_types::types::NamedT, prog: &Program) -> String {
    if n.args.is_empty() {
        ident(&n.name)
    } else {
        format!("{}<{}>", ident(&n.name), trait_args(prog, &n.name, &n.args).join(", "))
    }
}

fn mentions_param(t: &Type, p: &str) -> bool {
    match t {
        Type::TypeParam(x) => x == p,
        Type::Named(n) => n.name == p || n.args.iter().any(|a| mentions_param(a, p)),
        Type::Optional(i) | Type::Loan { inner: i, .. } => mentions_param(i, p),
        Type::Tuple(ts) => ts.iter().any(|x| mentions_param(x, p)),
        Type::Record(fs) => fs.iter().any(|(_, x)| mentions_param(x, p)),
        Type::Fn { params, ret, .. } => params.iter().any(|x| mentions_param(x, p)) || mentions_param(ret, p),
        Type::Dynamic(n) => n.args.iter().any(|a| mentions_param(a, p)),
        _ => false,
    }
}

/// The type parameters of an interface the program declares that its signatures use only as a lent parameter
/// (`a: A`, read or editable: `&A` or `&mut A` in the trait), never by value, in a result or inside another type. The
/// trait declares them `?Sized`, so an instantiation with a `dynamic M` can pass `&dyn M` straight through (D-88 (c)).
pub fn unsized_params(prog: &Program, i: &str) -> Vec<String> {
    let Some(d) = prog.type_def(i) else { return vec![] };
    if !matches!(d.kind, TypeKind::Interface { .. }) || !is_user_module(&d.module) {
        return vec![];
    }
    d.type_params
        .iter()
        .filter(|p| {
            let lent_only = d.sigs.iter().all(|s| {
                !mentions_param(&s.ret, p)
                    && s.params.iter().all(|(_, m, t)| matches!(strip(t), Type::TypeParam(x) if x == *p) && *m != Mode::Owned || !mentions_param(t, p))
                    && !s.bounds.iter().any(|(_, _, a)| a.iter().any(|x| mentions_param(x, p)))
            });
            lent_only && !d.iface_args.iter().any(|(_, a)| a.iter().any(|x| mentions_param(x, p)))
        })
        .cloned()
        .collect()
}

/// An interface's type arguments as its trait takes them: a `dynamic M` at a `?Sized` position is `dyn M` (lent as
/// `&dyn M`), any other argument its Rust type.
pub fn trait_args(prog: &Program, i: &str, args: &[Type]) -> Vec<String> {
    trait_args_lt(prog, i, args, None).0
}

/// The same, and whether an argument is such a lent `dynamic M`; with `lt` that argument is `(dyn M + 'lt)`, so a
/// bound or an impl can range over the loan's lifetime instead of taking the `'static` a bare `dyn M` means there.
pub fn trait_args_lt(prog: &Program, i: &str, args: &[Type], lt: Option<&str>) -> (Vec<String>, bool) {
    let lent = unsized_params(prog, i);
    let mut any = false;
    let out = args
        .iter()
        .enumerate()
        .map(|(k, a)| match strip(a) {
            Type::Dynamic(n) if prog.type_def(i).map_or(false, |d| d.type_params.get(k).map_or(false, |p| lent.contains(p))) => {
                any = true;
                match lt {
                    Some(l) => format!("(dyn {} + {l})", dyn_name(n, prog)),
                    None => format!("dyn {}", dyn_name(n, prog)),
                }
            }
            _ => rust_ty(a, prog),
        })
        .collect();
    (out, any)
}

/// A trait bound with its arguments, higher-ranked over the lifetime of a lent `dynamic M` argument
/// (`for<'x> Taker<(dyn Measured + 'x)>`), so a `&dyn M` of any lifetime can be passed through the bound (D-88 (c)).
pub fn trait_bound(prog: &Program, i: &str, args: &[Type]) -> String {
    if args.is_empty() {
        return ident(i);
    }
    let (ts, any) = trait_args_lt(prog, i, args, Some("'x"));
    if any {
        format!("for<'x> {}<{}>", ident(i), ts.join(", "))
    } else {
        format!("{}<{}>", ident(i), ts.join(", "))
    }
}

/// An interface the program's own modules declare (its trait is built from the declaration, D-82).
pub fn is_user_iface(prog: &Program, i: &str) -> bool {
    prog.type_def(i).map_or(false, |d| matches!(d.kind, TypeKind::Interface { .. }) && is_user_module(&d.module))
}

/// An interface and those it requires, transitively, the interface first.
pub fn iface_closure(prog: &Program, i: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut work = vec![i.to_string()];
    while let Some(x) = work.pop() {
        if out.contains(&x) {
            continue;
        }
        if let Some(d) = prog.type_def(&x) {
            work.extend(d.requires.iter().rev().cloned());
        }
        out.push(x);
    }
    out
}

/// The core interface `name` (`Comparable`), as an IR name of the core library may spell it.
pub fn is_core_iface(prog: &Program, i: &str, name: &str) -> bool {
    !is_user_iface(prog, i) && i.rsplit('.').next() == Some(name)
}

/// A trait reference with its type arguments (`Part<i64>`).
pub fn trait_ref(prog: &Program, i: &str, args: &[Type]) -> String {
    if args.is_empty() {
        ident(i)
    } else {
        format!("{}<{}>", ident(i), trait_args(prog, i, args).join(", "))
    }
}

/// The Rust bounds of a type parameter: those its `where` clauses name and nothing more (6.10, X-515; D-396). A core
/// interface is the trait that lowers it (`Copyable` is `Clone`; `Equatable` is `PartialEq`; `Comparable` adds
/// `PartialOrd`; `Hashable` adds `Eq`, `Hash` and the `Clone` the runtime's maps take of a key; `Displayable` is
/// `rt::Show`; the codec interfaces `rt::Codec`; `Default` is `Default`), and an interface the program declares is its
/// trait (D-82). Empty when the parameter has no bound, so a use at a type that satisfies the bounds always builds.
pub fn param_bounds(prog: &Program, bounds: &[(String, String, Vec<Type>)], t: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let push = |s: &str, out: &mut Vec<String>| {
        if !out.iter().any(|x| x == s) {
            out.push(s.to_string());
        }
    };
    for (_, i, args) in bounds.iter().filter(|(p, _, _)| p == t) {
        if is_user_iface(prog, i) {
            // higher-ranked over a lent `dynamic M` argument's lifetime, so a `&dyn M` passes through the bound (D-88 (c))
            push(&trait_bound(prog, i, args), &mut out);
        }
        for x in iface_closure(prog, i) {
            let core = |n: &str| is_core_iface(prog, &x, n);
            if core("Copyable") {
                push("::std::clone::Clone", &mut out);
            }
            if core("Equatable") || core("Comparable") || core("Hashable") {
                push("::std::cmp::PartialEq", &mut out);
            }
            if core("Comparable") {
                push("::std::cmp::PartialOrd", &mut out);
            }
            if core("Hashable") {
                push("::std::cmp::Eq", &mut out);
                push("::std::hash::Hash", &mut out);
                push("::std::clone::Clone", &mut out);
            }
            if core("Displayable") {
                push("rt::Show", &mut out);
            }
            if core("Encodable") || core("Decodable") || core("Codec") {
                push("rt::Codec", &mut out);
            }
            if core("Default") {
                push("::std::default::Default", &mut out);
            }
        }
    }
    out.join(" + ")
}

/// A generic parameter with its bounds, as a Rust generic list writes it: `T` alone when it has none.
pub fn generic_param(prog: &Program, bounds: &[(String, String, Vec<Type>)], t: &str) -> String {
    let b = param_bounds(prog, bounds, t);
    if b.is_empty() {
        ident(t)
    } else {
        format!("{}: {b}", ident(t))
    }
}

/// A Rust identifier for a Fortis name.
pub fn ident(n: &str) -> String {
    // Type names, type parameters and variants occupy a namespace disjoint from Rust's prelude
    // and our generated helpers. Encode the complete spelling, including module disambiguators,
    // so the mapping is injective rather than a list of reserved Rust names (D-244).
    if n.starts_with(|c: char| c.is_uppercase()) {
        return format!("__ft_{}", n.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>());
    }
    let mut base: String = n.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if base.starts_with(|c: char| c.is_ascii_digit()) {
        base.insert(0, '_');
    }
    if RUST_KEYWORDS.contains(&base.as_str()) {
        format!("{base}_")
    } else {
        base
    }
}

/// snake_case for runtime member names (`scalarCount` -> `scalar_count`).
pub fn snake(n: &str) -> String {
    let mut out = String::new();
    for (i, c) in n.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    ident(&out)
}

pub fn name_of(t: &Type) -> Option<&str> {
    match strip(t) {
        Type::Named(n) => Some(n.name.as_str()),
        Type::Refined { base, .. } => Some(base.name.as_str()),
        _ => None,
    }
}

pub fn args_of(t: &Type) -> Vec<Type> {
    match strip(t) {
        Type::Named(n) => n.args.clone(),
        Type::Refined { base, .. } => base.args.clone(),
        _ => vec![],
    }
}

/// A loan that is a value in Rust: of a trivial value (copied), of a class with a lifetime parameter (which holds
/// its loan inside), or a text view with no origin (owned text).
pub fn loan_is_value(t: &Type, prog: &Program) -> bool {
    match t {
        Type::Loan { inner, origin, .. } => {
            trivial(inner, prog)
                || name_of(inner) == Some("ListCursor")
                || name_of(inner).and_then(|n| prog.type_def(n)).map_or(false, |d| !d.lifetimes.is_empty())
                || ((origin.is_empty() || origin == "static") && is_string(inner, prog))
        }
        _ => false,
    }
}

/// A type the user's module declares (it shadows a runtime type of the same name).
pub fn user_type(n: &str, prog: &Program) -> bool {
    prog.types.iter().any(|t| t.name == n && is_user_module(&t.module) && matches!(t.kind, TypeKind::Class { .. } | TypeKind::Enum { .. }))
}

/// The Rust type of a Fortis value type (owned form).
pub fn rust_ty(t: &Type, prog: &Program) -> String {
    rust_ty_in(t, prog, false)
}

/// A function result may hide a concrete closure inside a generic holder without boxing it (LR 8.4).
/// Rust permits `Vec<impl Fn()>`, and the caller infers that one concrete environment type.
pub fn result_ty(t: &Type, prog: &Program) -> String {
    rust_ty_in(t, prog, true)
}

fn rust_ty_in(t: &Type, prog: &Program, result: bool) -> String {
    match t {
        Type::Loan { inner, mode, origin } => {
            // a class with a lifetime parameter holds its loan inside: `Probe from c` is `Probe<'c>`, not a reference
            if let Some(n) = name_of(inner) {
                if prog.type_def(n).map_or(false, |d| !d.lifetimes.is_empty()) {
                    let lt = if origin.is_empty() || origin == "self" { "'_".to_string() } else if origin == "static" { "'static".to_string() } else { format!("'{}", ident(origin)) };
                    return format!("{}<{lt}>", ident(n));
                }
            }
            // a loan of a trivial value is the value itself; a text view with no origin owns its text
            if trivial(inner, prog) {
                return rust_ty_in(inner, prog, result);
            }
            // a cursor holds its loan inside, as `Cursor<'_, T>`: `ListCursor[T] from self`, which `iterate()` of a class
            // that implements `Iterable` returns (5.3, D-545), is the cursor, not a reference to one
            if name_of(inner) == Some("ListCursor") {
                return rust_ty_in(inner, prog, result);
            }
            if (origin.is_empty() || origin == "static") && is_string(inner, prog) {
                return "String".into();
            }
            let lt = if origin.is_empty() || origin == "static" { "'static ".to_string() } else if origin == "self" { String::new() } else { format!("'{} ", ident(origin)) };
            let inner_t = match &**inner {
                Type::Dynamic(n) => format!("dyn {}", dyn_name(n, prog)),
                other => {
                    if is_string(other, prog) {
                        "str".to_string()
                    } else if name_of(other) == Some("BytesView") {
                        // a bytes view is a slice (D-390): `&[u8]`, which the runtime's bytes functions take
                        // and a static byte literal is, where `&Vec<u8>` fits neither
                        "[u8]".to_string()
                    } else {
                        rust_ty_in(other, prog, result)
                    }
                }
            };
            return if mode == "editable" { format!("&{lt}mut {inner_t}") } else { format!("&{lt}{inner_t}") };
        }
        Type::Optional(i) => return format!("Option<{}>", rust_ty_in(i, prog, result)),
        Type::Tuple(ts) => return format!("({})", ts.iter().map(|x| rust_ty_in(x, prog, result)).collect::<Vec<_>>().join(", ") + if ts.len() == 1 { "," } else { "" }),
        Type::Record(fs) => return record_name(fs),
        Type::Fn { params, param_modes, ret, kind, dynamic, .. } => {
            if result && !dynamic { return param_ty(t, Mode::Owned, prog); }
            let ps: Vec<String> = params.iter().enumerate().map(|(i, p)| param_ty_in_fn(p, fn_param_mode(param_modes, i), prog)).collect();
            let tr = match kind.as_str() {
                "fn mut" => "FnMut",
                "fn once" => "FnOnce",
                _ => "Fn",
            };
            return format!("::std::boxed::Box<dyn {tr}({}) -> {}>", ps.join(", "), rust_ty(ret, prog));
        }
        Type::Dynamic(n) => return format!("::std::boxed::Box<dyn {}>", dyn_name(n, prog)),
        Type::TypeParam(p) => return ident(p),
        Type::Unknown(_) => return "()".into(),
        _ => {}
    }
    let n = name_of(t).unwrap_or("");
    let a = args_of(t);
    let arg = |i: usize| a.get(i).map_or("()".to_string(), |x| rust_ty_in(x, prog, result));
    // a standard-library type keeps its runtime spelling when a user type has the same name (D-142:
    // `clock.Duration` is `rt::Duration`, not the program's `class Duration`)
    let standard = match strip(t) {
        Type::Named(n) => fortis_types::decls::is_standard(&n.package),
        Type::Refined { base, .. } => fortis_types::decls::is_standard(&base.package),
        _ => false,
    };
    if !standard {
        if let Some(d) = prog.type_def(n) {
            if let TypeKind::Newtype { repr } = &d.kind {
                return rust_ty_in(repr, prog, result);
            }
        }
    }
    if user_type(n, prog) && !standard {
        let lts = prog.type_def(n).map_or(0, |d| d.lifetimes.len());
        return if a.is_empty() && lts > 0 {
            format!("{}<'_>", ident(n))
        } else if a.is_empty() {
            ident(n)
        } else {
            format!("{}<{}>", ident(n), a.iter().map(|x| rust_ty_in(x, prog, result)).collect::<Vec<_>>().join(", "))
        };
    }
    match n {
        "Int" | "Int64" => "i64".into(),
        "Int32" => "i32".into(),
        "Int16" => "i16".into(),
        "Int8" => "i8".into(),
        "UInt" | "UInt64" => "u64".into(),
        "UInt32" => "u32".into(),
        "UInt16" => "u16".into(),
        "UInt8" | "Byte" => "u8".into(),
        "Bool" => "bool".into(),
        "Float" => "f64".into(),
        "Float32" => "f32".into(),
        "Decimal" => "rt::Decimal".into(),
        "Duration" => "rt::Duration".into(),
        "Timestamp" => "rt::Timestamp".into(),
        "String" | "StringView" => "String".into(),
        n if n.starts_with('$') && (n.contains("str") || n.contains("interp")) => "String".into(),
        "Scalar" => "char".into(),
        "Bytes" | "BytesView" => "Vec<u8>".into(),
        "Void" | "Never" => "()".into(),
        "List" | "ListView" | "Array" | "FixedList" => format!("Vec<{}>", arg(0)),
        "Map" | "OrderedMap" | "FixedMap" => format!("rt::OrderedMap<{}, {}>", arg(0), arg(1)),
        "Set" => format!("rt::OrderedSet<{}>", arg(0)),
        "Outcome" => format!("rt::Outcome<{}>", arg(0)),
        "Failure" => "rt::Failure".into(),
        "Report" => "rt::Report".into(),
        "Shared" => format!("rt::Shared<{}>", arg(0)),
        "Weak" => format!("rt::WeakRef<{}>", arg(0)),
        "Mutex" => format!("rt::Mutex<{}>", arg(0)),
        "MutexGuard" => format!("rt::Guard<'_, {}>", arg(0)),
        "AtomicInt" => "rt::AtomicInt".into(),
        "AtomicBool" => "rt::AtomicBool".into(),
        "Builder" => "rt::text::Builder".into(),
        "ListCursor" => format!("rt::collections::Cursor<'_, {}>", arg(0)),
        "Capabilities" => "rt::host::Capabilities".into(),
        "Console" | "Clock" | "Random" | "Network" | "Process" | "Environment" | "Supervisor" | "SecretAccess" | "RawSql" | "FileSystem" | "Path" | "File" => format!("rt::host::{n}"),
        "Secret" => format!("rt::host::Secret<{}>", arg(0)),
        // a task names its scope; a task stored in a declaration outlives every local scope (a parameter's is
        // elided, see `param_ty`)
        "Task" => format!("rt::task::Task<'static, 'static, {}>", arg(0)),
        "Must" => "()".into(),
        "VariantMap" => format!("Vec<({}, {})>", arg(0), arg(1)),
        _ => {
            match prog.type_def(n).map(|d| &d.kind) {
                Some(TypeKind::Newtype { repr }) => return rust_ty_in(repr, prog, result),
                Some(TypeKind::Error { .. }) => return "rt::Failure".into(),
                _ => {}
            }
            let lts = prog.type_def(n).map_or(0, |d| d.lifetimes.len());
            if a.is_empty() && lts > 0 {
                format!("{}<'_>", ident(n))
            } else if a.is_empty() {
                ident(n)
            } else {
                format!("{}<{}>", ident(n), a.iter().map(|x| rust_ty_in(x, prog, result)).collect::<Vec<_>>().join(", "))
            }
        }
    }
}

/// A canonical representation key: refinements erase, field order does not matter,
/// and nominal identities, nested types and loan permissions remain distinct.
fn record_type_key(t: &Type) -> Type {
    match t {
        Type::Refined { base, .. } => record_type_key(&Type::Named(base.clone())),
        Type::Named(n) | Type::Dynamic(n) => {
            let mut n = n.clone();
            n.origin.clear();
            n.args = n.args.iter().map(record_type_key).collect();
            if matches!(t, Type::Dynamic(_)) { Type::Dynamic(n) } else { Type::Named(n) }
        }
        Type::Record(fs) => {
            let mut fs: Vec<_> = fs.iter().map(|(n, t)| (n.clone(), record_type_key(t))).collect();
            fs.sort_by(|a, b| a.0.cmp(&b.0));
            Type::Record(fs)
        }
        Type::Optional(t) => Type::Optional(Box::new(record_type_key(t))),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(record_type_key).collect()),
        Type::Loan { inner, mode, origin } => Type::Loan { inner: Box::new(record_type_key(inner)), mode: mode.clone(), origin: origin.clone() },
        _ => t.clone(),
    }
}

/// Encode the complete canonical shape, without a lossy hash or field-name collisions.
pub fn record_name(fs: &[(String, Type)]) -> String {
    let key = format!("{:?}", record_type_key(&Type::Record(fs.to_vec())));
    let mut name = String::from("Rec_");
    for byte in key.bytes() {
        use std::fmt::Write;
        write!(&mut name, "{byte:02x}").unwrap();
    }
    name
}

/// A value type whose Rust form is `Copy` (passed and read by value).
pub fn trivial(t: &Type, prog: &Program) -> bool {
    trivial_seen(t, prog, &mut Vec::new())
}

/// A type that reaches itself is not trivial: its recursive fields are boxed (a boxed
/// recursive enum), so it is neither `Copy` nor read by value. The checker decides the
/// same way (decls.rs `is_trivial_seen`).
fn trivial_seen(t: &Type, prog: &Program, seen: &mut Vec<String>) -> bool {
    match strip(t) {
        Type::Optional(i) => trivial_seen(i, prog, seen),
        Type::Tuple(ts) => ts.iter().all(|x| trivial_seen(x, prog, seen)),
        // a record of trivial fields is trivial, as the checker decides (decls.rs `is_trivial_seen`; D-415)
        Type::Record(fs) => fs.iter().all(|(_, t)| trivial_seen(t, prog, seen)),
        Type::Named(_) | Type::Refined { .. } => {
            let n = name_of(t).unwrap_or("");
            // a capability is a Copy handle (C.1), passed by value like a trivial value
            if matches!(n, "Int" | "Int64" | "Int32" | "Int16" | "Int8" | "UInt" | "UInt64" | "UInt32" | "UInt16" | "UInt8" | "Byte" | "Bool" | "Float" | "Float32" | "Decimal" | "Duration" | "Timestamp" | "Scalar" | "Void" | "Never" | "Must")
                || matches!(n, "Console" | "Clock" | "Random" | "Network" | "Process" | "Environment" | "Supervisor" | "SecretAccess" | "RawSql" | "FileSystem" | "Capabilities")
            {
                return true;
            }
            if seen.iter().any(|s| s == n) {
                return false;
            }
            match prog.type_def(n) {
                Some(d) => {
                    seen.push(n.to_string());
                    let r = match &d.kind {
                        TypeKind::Class { .. } => d.trivial,
                        TypeKind::Enum { variants } => variants.iter().all(|v| v.fields.iter().all(|(_, ft)| trivial_seen(ft, prog, seen))),
                        TypeKind::Newtype { repr } => trivial_seen(repr, prog, seen),
                        _ => false,
                    };
                    seen.pop();
                    r
                }
                None => false,
            }
        }
        _ => false,
    }
}

pub fn is_string(t: &Type, prog: &Program) -> bool {
    let standard = match strip(t) {
        Type::Named(n) | Type::Refined { base: n, .. } => fortis_types::decls::is_standard(&n.package),
        _ => false,
    };
    if !standard && name_of(t).and_then(|n| prog.type_def(n)).is_some_and(|d| is_user_module(&d.module)) {
        return false;
    }
    matches!(name_of(t), Some("String") | Some("StringView")) || name_of(t).map_or(false, |n| n.starts_with('$') && (n.contains("str") || n.contains("interp")))
}

/// A read-mode `fn` (not `fn mut` or `fn once`) is borrowed. `Fn::call` takes `&self`, so the argument is not
/// moved and the caller's block still destroys the closure (7.9, D-181). `fn mut` stays by value (`call_mut`
/// needs `&mut self` from an owned binding) and `fn once` stays by value (`call_once` takes `self`).
pub fn fn_read_borrowed(t: &Type) -> bool {
    matches!(strip(t), Type::Fn { kind, .. } if kind == "fn")
}

fn mentions_tparam(t: &Type) -> bool {
    match t {
        Type::TypeParam(_) => true,
        Type::Named(n) => n.args.iter().any(mentions_tparam),
        Type::Refined { base, .. } => base.args.iter().any(mentions_tparam),
        Type::Optional(i) | Type::Loan { inner: i, .. } => mentions_tparam(i),
        Type::Tuple(ts) => ts.iter().any(mentions_tparam),
        Type::Record(fs) => fs.iter().any(|(_, a)| mentions_tparam(a)),
        Type::Fn { params, ret, .. } => params.iter().any(mentions_tparam) || mentions_tparam(ret),
        Type::Dynamic(n) => n.args.iter().any(mentions_tparam),
        Type::Unknown(_) => false,
    }
}

/// A concrete plain `fn` as a Rust function pointer. A function item does not implement the bounds on a
/// generic parameter (`PartialEq`, `Show`); the pointer does, and it is still callable. A `dynamic` fn, an
/// `async` fn, `fn mut` / `fn once`, or a signature that still names a type parameter is left as it is.
pub fn fn_pointer_ty(t: &Type, prog: &Program) -> Option<String> {
    let Type::Fn { params, param_modes, ret, kind, modifiers, dynamic, .. } = strip(t) else { return None };
    if *dynamic || kind != "fn" || modifiers.iter().any(|m| m == "async") || mentions_tparam(t) {
        return None;
    }
    let ps: Vec<String> = params.iter().enumerate().map(|(i, p)| param_ty_in_fn(p, fn_param_mode(param_modes, i), prog)).collect();
    let ret_s = match strip(ret) {
        Type::Named(n) if n.name == "Void" => "()".to_string(),
        other => rust_ty(other, prog),
    };
    Some(format!("fn({}) -> {ret_s}", ps.join(", ")))
}

/// A parameter's Rust type by mode: a read loan of a non-trivial value is `&T` (`&str` for text), editable `&mut T`.
/// A read-mode `fn` is `&impl Fn` (see `fn_read_borrowed`). `Mode::Owned` of a function type is `impl Fn` by value,
/// which is also how a returned closure is spelled.
pub fn param_ty(t: &Type, mode: Mode, prog: &Program) -> String {
    if let Type::Fn { params, param_modes, ret, kind, modifiers, .. } = strip(t) {
        let ps: Vec<String> = params.iter().enumerate().map(|(i, p)| param_ty_in_fn(p, fn_param_mode(param_modes, i), prog)).collect();
        let pre = if mode == Mode::Editable {
            "&mut "
        } else if mode == Mode::Read && kind == "fn" {
            "&"
        } else {
            ""
        };
        // an async callback is an async closure: `async fn(T) U` is `AsyncFn(T) -> Async<U>` (12.3)
        if modifiers.iter().any(|m| m == "async") {
            let tr = match kind.as_str() {
                "fn mut" => "AsyncFnMut",
                "fn once" => "AsyncFnOnce",
                _ => "AsyncFn",
            };
            return format!("{pre}impl {tr}({}) -> rt::task::Async<{}>", ps.join(", "), rust_ty(ret, prog));
        }
        let tr = match kind.as_str() {
            "fn mut" => "FnMut",
            "fn once" => "FnOnce",
            _ => "Fn",
        };
        let r = match &**ret {
            Type::Named(n) if n.name == "Void" => String::new(),
            other => format!(" -> {}", rust_ty(other, prog)),
        };
        return format!("{pre}impl {tr}({}){r}", ps.join(", "));
    }
    if let Type::Loan { inner, .. } = t {
        if trivial(inner, prog) {
            return rust_ty(inner, prog);
        }
        return rust_ty(t, prog);
    }
    if name_of(t) == Some("Task") {
        let task = rust_ty(t, prog).replace("'static", "'_");
        return if mode == Mode::Editable { format!("&mut {task}") } else { task };
    }
    match mode {
        Mode::Owned => rust_ty(t, prog),
        Mode::Editable => format!("&mut {}", rust_ty(t, prog)),
        Mode::Read => {
            if let Some(inner) = opt_read_inner(t, prog) {
                opt_read_ty(&inner, prog)
            } else if trivial(t, prog) {
                rust_ty(t, prog)
            } else if is_string(t, prog) {
                "&str".into()
            } else if matches!(strip(t), Type::Dynamic(_)) {
                format!("&dyn {}", match strip(t) {
                    Type::Dynamic(n) => dyn_name(n, prog),
                    _ => unreachable!(),
                })
            } else {
                format!("&{}", rust_ty(t, prog))
            }
        }
    }
}

/// The present type of a read parameter of a non-trivial optional type, which lowers to `Option<&T>` (`Option<&str>`
/// for text) rather than `&Option<T>`: a non-optional place passed to it is lent, not moved into a new `Option` (D-2).
pub fn opt_read_inner(t: &Type, prog: &Program) -> Option<Type> {
    match strip(t) {
        // a type parameter's optional (`value: T?` of `Optional.map`) is `Option<&T>` too: the caller lends its place (X-479)
        Type::Optional(inner) if !trivial(t, prog) && (fortis_types::types::known(inner) || matches!(strip(inner), Type::TypeParam(_))) => match strip(inner) {
            Type::Loan { .. } | Type::Optional(_) | Type::Fn { .. } | Type::Dynamic(_) | Type::Unknown(_) => None,
            _ if matches!(name_of(inner), Some("Task") | Some("Future")) => None,
            _ => Some((**inner).clone()),
        },
        _ => None,
    }
}

/// `Option<&T>` for an optional read parameter whose present type is `inner` (D-2).
pub fn opt_read_ty(inner: &Type, prog: &Program) -> String {
    if is_string(inner, prog) {
        "Option<&str>".into()
    } else {
        format!("Option<&{}>", rust_ty(inner, prog))
    }
}

/// An optional loan of text with an origin: `Option<&str>` in a signature, while an intrinsic (`xs.get(0)`), a
/// generic prelude call (`xs.first()`) or a binding of one answers `Option<&String>` (D-353). A loan of text with
/// no origin owns its text (`Option<String>`), and a `TypeParam` inner is a generic body's, not this.
pub fn opt_loan_text(t: &Type, prog: &Program) -> bool {
    matches!(strip(t), Type::Optional(i) if matches!(&**i, Type::Loan { inner, origin, .. } if is_string(inner, prog) && !origin.is_empty() && origin != "static"))
}

/// Function parameter modes are retained separately from their value types (D-540).
pub fn fn_param_mode(modes: &[String], i: usize) -> Mode {
    match modes.get(i).map(String::as_str) {
        Some("owned") => Mode::Owned,
        Some("editable") => Mode::Editable,
        _ => Mode::Read,
    }
}

/// The parameter type of a function type: read `User` is `&User`, owned `User` is `User`.
pub fn param_ty_in_fn(t: &Type, mode: Mode, prog: &Program) -> String {
    match mode {
        Mode::Owned => return rust_ty(t, prog),
        Mode::Editable => return format!("&mut {}", rust_ty(t, prog)),
        Mode::Read => {}
    }
    if let Some(inner) = opt_read_inner(t, prog) {
        return opt_read_ty(&inner, prog);
    }
    if trivial(t, prog) {
        rust_ty(t, prog)
    } else if is_string(t, prog) {
        "&str".into()
    } else {
        format!("&{}", rust_ty(t, prog))
    }
}

#[cfg(test)]
mod record_shape_tests {
    use super::*;
    use fortis_types::types::{int, boolean, optional, refined, nt};

    #[test]
    fn fields_and_types_both_identify_records() {
        assert_ne!(record_name(&[("n".into(), int())]), record_name(&[("n".into(), optional(int()))]));
        assert_ne!(record_name(&[("n_x".into(), int())]), record_name(&[("n".into(), int()), ("x".into(), int())]));
    }

    #[test]
    fn order_and_refinement_do_not_change_representation() {
        let a = vec![("n".into(), int()), ("b".into(), boolean())];
        let b = vec![("b".into(), boolean()), ("n".into(), refined(nt("Int"), Some(0), Some(3), "range"))];
        assert_eq!(record_name(&a), record_name(&b));
        assert_eq!(record_name(&[("inner".into(), Type::Record(a))]), record_name(&[("inner".into(), Type::Record(b))]));
    }
}
