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

/// A Rust identifier for a Fortis name.
pub fn ident(n: &str) -> String {
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
                || ((origin.is_empty() || origin == "static") && is_string(inner))
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
                return rust_ty(inner, prog);
            }
            if (origin.is_empty() || origin == "static") && matches!(name_of(inner), Some("String") | Some("StringView")) {
                return "String".into();
            }
            let lt = if origin.is_empty() || origin == "static" { "'static ".to_string() } else if origin == "self" { String::new() } else { format!("'{} ", ident(origin)) };
            let inner_t = match &**inner {
                Type::Dynamic(n) => format!("dyn {}", ident(&n.name)),
                other => {
                    if name_of(other) == Some("StringView") || name_of(other) == Some("String") {
                        "str".to_string()
                    } else {
                        rust_ty(other, prog)
                    }
                }
            };
            return if mode == "editable" { format!("&{lt}mut {inner_t}") } else { format!("&{lt}{inner_t}") };
        }
        Type::Optional(i) => return format!("Option<{}>", rust_ty(i, prog)),
        Type::Tuple(ts) => return format!("({})", ts.iter().map(|x| rust_ty(x, prog)).collect::<Vec<_>>().join(", ") + if ts.len() == 1 { "," } else { "" }),
        Type::Record(fs) => return record_name(fs),
        Type::Fn { params, ret, kind, .. } => {
            let ps: Vec<String> = params.iter().map(|p| param_ty_in_fn(p, prog)).collect();
            let tr = match kind.as_str() {
                "fn mut" => "FnMut",
                "fn once" => "FnOnce",
                _ => "Fn",
            };
            return format!("Box<dyn {tr}({}) -> {}>", ps.join(", "), rust_ty(ret, prog));
        }
        Type::Dynamic(n) => return format!("Box<dyn {}>", ident(&n.name)),
        Type::TypeParam(p) => return ident(p),
        Type::Unknown(_) => return "()".into(),
        _ => {}
    }
    let n = name_of(t).unwrap_or("");
    let a = args_of(t);
    let arg = |i: usize| a.get(i).map_or("()".to_string(), |x| rust_ty(x, prog));
    // a standard-library type keeps its runtime spelling when a user type has the same name (D-142:
    // `clock.Duration` is `rt::Duration`, not the program's `class Duration`)
    let standard = match strip(t) {
        Type::Named(n) => fortis_types::decls::is_standard(&n.package),
        Type::Refined { base, .. } => fortis_types::decls::is_standard(&base.package),
        _ => false,
    };
    if user_type(n, prog) && !standard {
        let lts = prog.type_def(n).map_or(0, |d| d.lifetimes.len());
        return if a.is_empty() && lts > 0 {
            format!("{}<'_>", ident(n))
        } else if a.is_empty() {
            ident(n)
        } else {
            format!("{}<{}>", ident(n), a.iter().map(|x| rust_ty(x, prog)).collect::<Vec<_>>().join(", "))
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
        "ListCursor" => format!("rt::collections::Cursor<{}>", arg(0)),
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
                Some(TypeKind::Newtype { repr }) => return rust_ty(repr, prog),
                Some(TypeKind::Error { .. }) => return "rt::Failure".into(),
                _ => {}
            }
            let lts = prog.type_def(n).map_or(0, |d| d.lifetimes.len());
            if a.is_empty() && lts > 0 {
                format!("{}<'_>", ident(n))
            } else if a.is_empty() {
                ident(n)
            } else {
                format!("{}<{}>", ident(n), a.iter().map(|x| rust_ty(x, prog)).collect::<Vec<_>>().join(", "))
            }
        }
    }
}

/// The struct generated for an anonymous record shape.
pub fn record_name(fs: &[(String, Type)]) -> String {
    let mut n = String::from("Rec");
    for (f, _) in fs {
        n.push('_');
        n.push_str(&ident(f));
    }
    n
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

pub fn is_string(t: &Type) -> bool {
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
    let Type::Fn { params, ret, kind, modifiers, dynamic, .. } = strip(t) else { return None };
    if *dynamic || kind != "fn" || modifiers.iter().any(|m| m == "async") || mentions_tparam(t) {
        return None;
    }
    let ps: Vec<String> = params.iter().map(|p| param_ty_in_fn(p, prog)).collect();
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
    if let Type::Fn { params, ret, kind, modifiers, .. } = strip(t) {
        let ps: Vec<String> = params.iter().map(|p| param_ty_in_fn(p, prog)).collect();
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
            } else if is_string(t) {
                "&str".into()
            } else if matches!(strip(t), Type::Dynamic(_)) {
                format!("&dyn {}", match strip(t) {
                    Type::Dynamic(n) => ident(&n.name),
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
        Type::Optional(inner) if !trivial(t, prog) && fortis_types::types::known(inner) => match strip(inner) {
            Type::Loan { .. } | Type::Optional(_) | Type::Fn { .. } | Type::Dynamic(_) | Type::Unknown(_) => None,
            _ if matches!(name_of(inner), Some("Task") | Some("Future")) => None,
            _ => Some((**inner).clone()),
        },
        _ => None,
    }
}

/// `Option<&T>` for an optional read parameter whose present type is `inner` (D-2).
pub fn opt_read_ty(inner: &Type, prog: &Program) -> String {
    if is_string(inner) {
        "Option<&str>".into()
    } else {
        format!("Option<&{}>", rust_ty(inner, prog))
    }
}

/// The parameter type of a function type (`fn(Int, User) Bool` takes `i64` and `&User`).
pub fn param_ty_in_fn(t: &Type, prog: &Program) -> String {
    if let Some(inner) = opt_read_inner(t, prog) {
        return opt_read_ty(&inner, prog);
    }
    if trivial(t, prog) {
        rust_ty(t, prog)
    } else if is_string(t) {
        "&str".into()
    } else {
        format!("&{}", rust_ty(t, prog))
    }
}
