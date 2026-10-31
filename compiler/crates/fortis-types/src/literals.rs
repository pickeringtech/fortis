//! Literal typing and assignability (DEL-11): a port of checker/literals.py.

use fortis_syntax::{Kind, Token};

use crate::types::*;

pub fn strlit() -> NamedT {
    nt("$strlit")
}
pub fn interp() -> Type {
    named("$interp")
}

pub fn is_strlit(t: &Type) -> bool {
    match t {
        Type::Named(n) => n.name == "$strlit" && n.args.is_empty() && n.package.is_empty(),
        Type::Refined { base, .. } => base.name == "$strlit" && base.args.is_empty() && base.package.is_empty(),
        _ => false,
    }
}
pub fn is_interp(t: &Type) -> bool {
    *t == interp()
}

const VIEW_OF: &[(&str, &str)] = &[("String", "StringView"), ("Bytes", "BytesView"), ("List", "ListView"), ("Array", "ListView"), ("FixedList", "ListView")];

/// Whether `view` is the view type taken of an owned `owner` (6.6, 6.7).
pub fn view_of(owner: &str, view: &str) -> bool {
    VIEW_OF.contains(&(owner, view))
}

/// An integer refinement spanning its base's whole range is the base itself: `Int<min..max>` is `Int` (6.3).
pub fn unrefined_full(t: &Type) -> Type {
    if let Type::Refined { base, kind, .. } = t {
        if kind == "range" && int_types(&base.name).is_some() && int_bounds(t) == int_types(&base.name) {
            return Type::Named(base.clone());
        }
    }
    t.clone()
}

/// Two type arguments that are assignable one way but differ in a refinement (6.3, X-514): `Int<0..3>` against
/// `Int<0..5>` or `Int`. A literal's exact type and a string literal are not refinements a value carries.
pub fn refinement_differs(a: &Type, b: &Type) -> bool {
    let (a, b) = (unrefined_full(strip(a)), unrefined_full(strip(b)));
    let (a, b) = (&a, &b);
    if a == b || is_strlit(a) || is_strlit(b) {
        return false;
    }
    let refined = |t: &Type| matches!(t, Type::Refined { kind, .. } if kind != "exact");
    if refined(a) || refined(b) { return true; }
    match (a, b) {
        (Type::Optional(x), Type::Optional(y)) => refinement_differs(x, y),
        (Type::Tuple(xs), Type::Tuple(ys)) => xs.iter().zip(ys).any(|(x, y)| refinement_differs(x, y)),
        (Type::Record(xs), Type::Record(ys)) => xs.iter().any(|(k, x)| ys.iter().find(|(l, _)| k == l).is_some_and(|(_, y)| refinement_differs(x, y))),
        (Type::Named(x), Type::Named(y)) => x.args.iter().zip(&y.args).any(|(x, y)| refinement_differs(x, y)),
        _ => false,
    }
}

/// Unicode scalars of a string literal's content (escapes resolved; no interpolation).
pub fn scalar_count(text: &str) -> Int {
    let cs: Vec<char> = text.chars().collect();
    let (mut i, mut n) = (0usize, 0);
    while i < cs.len() {
        if cs[i] == '\\' {
            i += 2;
        } else {
            i += 1;
        }
        n += 1;
    }
    n
}

/// Python's int(text, 0) for an INT token (underscores already removed): decimal, 0x, 0b, 0o.
pub fn parse_int(text: &str) -> Int {
    let t: String = text.chars().filter(|c| *c != '_').collect();
    let lower = t.to_ascii_lowercase();
    let r = if let Some(h) = lower.strip_prefix("0x") {
        Int::from_str_radix(h, 16)
    } else if let Some(b) = lower.strip_prefix("0b") {
        Int::from_str_radix(b, 2)
    } else if let Some(o) = lower.strip_prefix("0o") {
        Int::from_str_radix(o, 8)
    } else {
        t.parse::<Int>()
    };
    r.unwrap_or(Int::MAX)
}

pub fn literal_type(tok: &Token) -> Type {
    match tok.kind {
        Kind::Int => {
            let v = parse_int(&tok.text);
            refined(nt("Int"), Some(v), Some(v), "exact")
        }
        Kind::Float => named(if tok.text.ends_with("f32") { "Float32" } else { "Float" }),
        Kind::Dec => named("Decimal"),
        Kind::Dur => named("Duration"),
        Kind::Str | Kind::Raw => {
            if tok.kind == Kind::Str && tok.text.contains("${") {
                return interp();
            }
            let cs: Vec<char> = tok.text.chars().collect();
            let body: String = if tok.kind == Kind::Str { cs[1..cs.len() - 1].iter().collect() } else { cs[3..cs.len() - 3].iter().collect() };
            let n = scalar_count(&body);
            refined(strlit(), Some(n), Some(n), "scalars")
        }
        Kind::Bytes => named("BytesView"),
        Kind::Kw if tok.text == "true" || tok.text == "false" => boolean(),
        _ => unknown("literal"),
    }
}

pub fn negate(t: &Type) -> Type {
    if let Type::Refined { base, lo: Some(lo), hi: Some(hi), .. } = t {
        if base.name == "Int" && base.args.is_empty() && base.package.is_empty() && lo == hi {
            return refined(nt("Int"), Some(-lo), Some(-lo), "exact");
        }
    }
    t.clone()
}

/// A literal's type once the context has not decided: a plain literal is a view, an interpolated one a String.
pub fn concrete(t: &Type) -> Type {
    if is_strlit(t) {
        return string_view();
    }
    if is_interp(t) {
        return string();
    }
    t.clone()
}

pub type Verdict = (Option<bool>, String);

fn yes() -> Verdict {
    (Some(true), String::new())
}
fn unknown_v() -> Verdict {
    (None, String::new())
}
fn no(why: String) -> Verdict {
    (Some(false), why)
}

pub const CLOSURE_WHY: &str = "each lambda has its own closure type (8.4): this position already holds one lambda's closure, and a second, different lambda needs a `dynamic fn(...)` position";

fn closure_id(t: &Type) -> Option<&str> {
    match strip(t) {
        Type::Fn { identity, dynamic: false, .. } if !identity.is_empty() => Some(identity.as_str()),
        _ => None,
    }
}

fn empty_declared_fn(t: &Type) -> bool {
    match strip(t) {
        Type::Fn { dynamic: false, identity, .. } if identity.is_empty() => true,
        _ => false,
    }
}

/// True when `want` is one lambda's closure and `have` is a declared function at that position (8.4).
/// A lambda may still fill a declared `fn`. Two lambdas stay with `closure_conflict`.
pub fn declared_vs_lambda(have: &Type, want: &Type) -> bool {
    let have = strip(have);
    let want = strip(want);
    if let Type::Optional(w) = want {
        let h = match have {
            Type::Optional(h) => h.as_ref(),
            other => other,
        };
        return declared_vs_lambda(h, w);
    }
    if let Type::Loan { inner: w, .. } = want {
        let h = match have {
            Type::Loan { inner, .. } => inner.as_ref(),
            other => other,
        };
        return declared_vs_lambda(h, w);
    }
    if closure_id(want).is_some() && empty_declared_fn(have) {
        return true;
    }
    match (have, want) {
        (Type::Named(h), Type::Named(w)) if h.name == w.name => h.args.iter().zip(w.args.iter()).any(|(a, b)| declared_vs_lambda(a, b)),
        (Type::Tuple(h), Type::Tuple(w)) => h.iter().zip(w.iter()).any(|(a, b)| declared_vs_lambda(a, b)),
        (Type::Record(h), Type::Record(w)) => {
            let wm: std::collections::BTreeMap<&str, &Type> = w.iter().map(|(k, t)| (k.as_str(), t)).collect();
            h.iter().any(|(k, ft)| wm.get(k.as_str()).map_or(false, |wt| declared_vs_lambda(ft, wt)))
        }
        _ => false,
    }
}

/// True when both sides are one closure type and they are different lambdas (8.4).
/// A declared function type has no identity, and `dynamic fn` may hold any lambda of the signature.
pub fn closure_conflict(have: &Type, want: &Type) -> bool {
    let have = strip(have);
    let want = strip(want);
    // present injection (6.8): an optional closure position conflicts when its inner closure does
    if let Type::Optional(w) = want {
        let h = match have {
            Type::Optional(h) => h.as_ref(),
            other => other,
        };
        return closure_conflict(h, w);
    }
    if let (Some(h), Some(w)) = (closure_id(have), closure_id(want)) {
        if h != w {
            return true;
        }
    }
    match (have, want) {
        (Type::Named(h), Type::Named(w)) if h.name == w.name => h.args.iter().zip(w.args.iter()).any(|(a, b)| closure_conflict(a, b)),
        (Type::Tuple(h), Type::Tuple(w)) => h.iter().zip(w.iter()).any(|(a, b)| closure_conflict(a, b)),
        (Type::Record(h), Type::Record(w)) => {
            let wm: std::collections::BTreeMap<&str, &Type> = w.iter().map(|(k, t)| (k.as_str(), t)).collect();
            h.iter().any(|(k, ft)| wm.get(k.as_str()).map_or(false, |wt| closure_conflict(ft, wt)))
        }
        _ => false,
    }
}

pub fn contains_dynamic_fn(t: &Type) -> bool {
    match strip(t) {
        Type::Fn { dynamic, params, ret, .. } => *dynamic || params.iter().any(contains_dynamic_fn) || contains_dynamic_fn(ret),
        Type::Named(n) => n.args.iter().any(contains_dynamic_fn),
        Type::Optional(i) | Type::Loan { inner: i, .. } => contains_dynamic_fn(i),
        Type::Tuple(xs) => xs.iter().any(contains_dynamic_fn),
        Type::Record(fs) => fs.iter().any(|(_, x)| contains_dynamic_fn(x)),
        _ => false,
    }
}

fn nested_identity(t: &Type) -> bool {
    match strip(t) {
        Type::Fn { identity, params, ret, .. } => !identity.is_empty() || params.iter().any(nested_identity) || nested_identity(ret),
        Type::Named(n) => n.args.iter().any(nested_identity),
        Type::Optional(i) | Type::Loan { inner: i, .. } => nested_identity(i),
        Type::Tuple(xs) => xs.iter().any(nested_identity),
        Type::Record(fs) => fs.iter().any(|(_, x)| nested_identity(x)),
        _ => false,
    }
}

/// True when every closure identity in `value` sits in a `dynamic fn` position of `expected`.
pub fn dynamic_covers(expected: Option<&Type>, value: &Type) -> bool {
    let e = match expected {
        Some(e) => strip(e),
        None => return false,
    };
    let v = strip(value);
    // a present value covers an optional expected type by covering its inner (6.8)
    if let Type::Optional(ei) = e {
        if !matches!(v, Type::Optional(_)) {
            return dynamic_covers(Some(ei), v);
        }
    }
    if let Type::Fn { identity, dynamic: false, .. } = v {
        if !identity.is_empty() {
            return matches!(e, Type::Fn { dynamic: true, .. });
        }
    }
    match (e, v) {
        (Type::Optional(ei), Type::Optional(vi)) => dynamic_covers(Some(ei), vi),
        (Type::Named(en), Type::Named(vn)) if en.name == vn.name && en.args.len() == vn.args.len() => en.args.iter().zip(vn.args.iter()).all(|(a, b)| dynamic_covers(Some(a), b)),
        (Type::Tuple(ei), Type::Tuple(vi)) if ei.len() == vi.len() => ei.iter().zip(vi.iter()).all(|(a, b)| dynamic_covers(Some(a), b)),
        (Type::Record(ef), Type::Record(vf)) => {
            let em: std::collections::BTreeMap<&str, &Type> = ef.iter().map(|(k, t)| (k.as_str(), t)).collect();
            vf.iter().all(|(k, ft)| em.get(k.as_str()).map_or(false, |et| dynamic_covers(Some(et), ft)))
        }
        _ => closure_id(v).is_none() && !nested_identity(v),
    }
}

/// Why these types name different declarations, or "" when every origin agrees.
/// Equality ignores origin. Assignability asks this before it accepts on equality, so a type
/// argument or a callback parameter cannot swap a user's class for the core type of the same name.
/// A type parameter or an unknown type agrees: the caller treats that as unproved.
pub fn origin_clash(have: &Type, want: &Type) -> String {
    fn named(h: &NamedT, w: &NamedT) -> String {
        if h.name.split('@').next() == w.name.split('@').next() && (h.package != w.package || h.origin != w.origin) {
            let shown = crate::types::diagnostic_types(&[&h.as_type(), &w.as_type()]);
            return format!("{} is not {}", shown[0], shown[1]);
        }
        if h.name.split('@').next() == w.name.split('@').next() {
            for (a, b) in h.args.iter().zip(w.args.iter()) {
                let found = origin_clash(a, b);
                if !found.is_empty() {
                    return found;
                }
            }
        }
        String::new()
    }
    let have = strip(have);
    let want = strip(want);
    if !known(have) || !known(want) {
        return String::new();
    }
    match (have, want) {
        (Type::Refined { base: h, .. }, Type::Refined { base: w, .. }) => named(h, w),
        (Type::Refined { base: h, .. }, Type::Named(w)) => named(h, w),
        (Type::Named(h), Type::Refined { base: w, .. }) => named(h, w),
        (Type::Named(h), Type::Named(w)) => named(h, w),
        (Type::Fn { params: hp, ret: hr, .. }, Type::Fn { params: wp, ret: wr, .. }) => {
            for (a, b) in hp.iter().zip(wp.iter()) {
                let found = origin_clash(a, b);
                if !found.is_empty() {
                    return found;
                }
            }
            origin_clash(hr, wr)
        }
        (Type::Optional(h), Type::Optional(w)) => origin_clash(h, w),
        (Type::Tuple(h), Type::Tuple(w)) => {
            for (a, b) in h.iter().zip(w.iter()) {
                let found = origin_clash(a, b);
                if !found.is_empty() {
                    return found;
                }
            }
            String::new()
        }
        (Type::Record(h), Type::Record(w)) => {
            for ((hk, ht), (wk, wt)) in h.iter().zip(w.iter()) {
                if hk != wk {
                    continue;
                }
                let found = origin_clash(ht, wt);
                if !found.is_empty() {
                    return found;
                }
            }
            String::new()
        }
        (Type::Loan { inner: h, .. }, Type::Loan { inner: w, .. }) => origin_clash(h, w),
        (Type::Dynamic(h), Type::Dynamic(w)) => named(h, w),
        _ => String::new(),
    }
}

/// (true) when have is assignable to want; (false, why) when not; (None) when unknown.
pub fn invariant_assignable(have: &Type, want: &Type) -> Verdict {
    let (have, want) = (strip(have), strip(want));
    let forward = assignable(have, want);
    if forward.0 == Some(false) { return forward; }
    let backward = assignable(want, have);
    if backward.0 == Some(false) {
        return no(format!("writable types are invariant: {} is not {} (6.9)", have.show(), want.show()));
    }
    let pairs: Option<Vec<(&Type, &Type)>> = match (have, want) {
        (Type::Tuple(xs), Type::Tuple(ys)) => Some(xs.iter().zip(ys).collect()),
        (Type::Record(xs), Type::Record(ys)) => Some(xs.iter().map(|(k, x)| (x, &ys.iter().find(|(l, _)| k == l).unwrap().1)).collect()),
        (Type::Optional(x), Type::Optional(y)) => Some(vec![(x, y)]),
        (Type::Named(x), Type::Named(y)) if x.name == y.name && x.args.len() == y.args.len() => Some(x.args.iter().zip(&y.args).collect()),
        (Type::Fn { params: hp, ret: hr, .. }, Type::Fn { params: wp, ret: wr, .. }) => Some(hp.iter().zip(wp).chain(std::iter::once((&**hr, &**wr))).collect()),
        _ => None,
    };
    if let Some(pairs) = pairs {
        let mut unknown = false;
        for (a, b) in pairs {
            let verdict = invariant_assignable(a, b);
            if verdict.0 == Some(false) { return verdict; }
            unknown |= verdict.0.is_none();
        }
        return if unknown { unknown_v() } else { yes() };
    }
    if matches!(have, Type::Dynamic(_)) || matches!(want, Type::Dynamic(_)) {
        if have == want { return yes(); }
        if !matches!(have, Type::Unknown(_) | Type::TypeParam(_)) && !matches!(want, Type::Unknown(_) | Type::TypeParam(_)) {
            return no(format!("writable types are invariant: {} is not {} (6.9)", have.show(), want.show()));
        }
    }
    if refinement_differs(have, want) {
        return no(format!("writable types are invariant: {} is not {} (6.9)", have.show(), want.show()));
    }
    if forward.0.is_none() || backward.0.is_none() { unknown_v() } else { yes() }
}

fn contains_structural(t: &Type) -> bool {
    match strip(t) {
        Type::Record(_) | Type::Tuple(_) => true,
        Type::Refined { base, .. } => contains_structural(&base.as_type()),
        Type::Optional(inner) => contains_structural(inner),
        Type::Named(n) => n.args.iter().any(contains_structural),
        _ => false,
    }
}

pub fn assignable(have: &Type, want: &Type) -> Verdict {
    if matches!(have, Type::Loan { mode, .. } if mode == "editable") || matches!(want, Type::Loan { mode, .. } if mode == "editable") {
        // Length-refined collections already reject every editable loan with E-REF-003.
        if (contains_structural(have) || contains_structural(want)) && !matches!(strip(have), Type::Refined { kind, .. } if kind == "length") {
            return invariant_assignable(have, want);
        }
    }
    let have = strip(have);
    let want = strip(want);
    if matches!(have, Type::Unknown(_) | Type::TypeParam(_)) || matches!(want, Type::Unknown(_) | Type::TypeParam(_)) {
        return unknown_v();
    }
    let clash = origin_clash(have, want);
    if !clash.is_empty() { return no(clash); }
    if closure_conflict(have, want) {
        return no(CLOSURE_WHY.into());
    }
    if let Type::Optional(wi) = want {
        if let Type::Optional(hi) = have {
            return assignable(hi, wi);
        }
        if have.is_named("Never") {
            return yes();
        }
        return assignable(have, wi);
    }
    if have.is_named("Never") {
        return yes();
    }
    if is_strlit(have) {
        if let Type::Refined { base: wb, lo: wlo, hi: whi, kind: wk } = want {
            if wk == "scalars" && (wb.as_type() == string() || wb.as_type() == string_view()) {
                if let Type::Refined { lo: Some(hlo), hi: hhi, .. } = have {
                    let lo = wlo.unwrap_or(0);
                    let hi = whi.or(*hhi).unwrap_or(Int::MAX);
                    return if lo <= *hlo && *hlo <= hi { yes() } else { no(format!("the literal has {} scalars, outside {}", hlo, want.show())) };
                }
            }
        }
        if *want == string_view() || *want == string() {
            return yes();
        }
        if let Type::Refined { base, .. } = want {
            if base.as_type() == string_view() || base.as_type() == string() {
                return unknown_v();
            }
        }
        return no(format!("a string literal is a StringView (or a String where one is expected), not {}", want.show()));
    }
    if is_interp(have) {
        return assignable(&string(), want);
    }
    let hb = int_bounds(have);
    let wb = int_bounds(want);
    if let (Some(hb), Some(wb)) = (hb, wb) {
        if let Type::Refined { kind, .. } = want {
            if kind != "range" {
                return unknown_v();
            }
        }
        if wb.0 <= hb.0 && hb.1 <= wb.1 {
            return yes();
        }
        if let Type::Refined { kind, .. } = have {
            if kind == "exact" {
                return no(format!("interval [{}, {}] is outside [{}, {}]", hb.0, hb.1, wb.0, wb.1));
            }
        }
        if matches!(have, Type::Named(_)) && matches!(want, Type::Named(_)) {
            return no(format!("{} is not {} (distinct integer types)", have.show(), want.show()));
        }
        return unknown_v();
    }
    if hb.is_some() || wb.is_some() {
        if let (Type::Refined { base: hbase, kind: hk, .. }, Type::Refined { base: wbase, kind: wk, .. }) = (have, want) {
            if hk == wk && hbase == wbase {
                return unknown_v();
            }
        }
        if let Type::Refined { base, .. } = have {
            if base.as_type() == *want {
                return yes();
            }
        }
        if let (Type::Named(h), Type::Refined { base, .. }) = (have, want) {
            if h == base {
                return unknown_v();
            }
        }
        return no(format!("{} is not {}", have.show(), want.show()));
    }
    if let (Type::Refined { base: hb, lo: hl, hi: hh, kind: hk }, Type::Refined { base: wb, lo: wl, hi: wh, kind: wk }) = (have, want) {
        if hb == wb && hk == wk && matches!(hk.as_str(), "scalars" | "length")
            && wl.is_none_or(|w| hl.is_some_and(|h| h >= w)) && wh.is_none_or(|w| hh.is_some_and(|h| h <= w)) {
            return yes();
        }
        return unknown_v();
    }
    if let Type::Refined { base, .. } = have {
        return assignable(&base.as_type(), want);
    }
    if matches!(want, Type::Refined { .. }) {
        return unknown_v();
    }
    if let (Type::Named(h), Type::Named(w)) = (have, want) {
        if h.name == w.name && (h.package != w.package || h.origin != w.origin) {
            return no(origin_clash(have, want));
        }
        if h == w {
            // Equality ignores origins nested in the arguments. Reject before accepting.
            let found = origin_clash(have, want);
            if !found.is_empty() {
                return no(found);
            }
            return yes();
        }
        if h.name == "Never" && h.package.is_empty() {
            return yes();
        }
        if h.package.is_empty() && w.package.is_empty() && VIEW_OF.contains(&(h.name.as_str(), w.name.as_str())) && (h.args.is_empty() || w.args.is_empty() || h.args[0] == w.args[0]) {
            // an owned value where its view is expected (6.6, 7.4); the element still has an origin
            if !h.args.is_empty() && !w.args.is_empty() {
                let found = origin_clash(&h.args[0], &w.args[0]);
                if !found.is_empty() {
                    return no(found);
                }
            }
            return yes();
        }
        if h.name == w.name && h.origin == w.origin && h.package == w.package {
            // arguments are invariant in the proved direction: one false argument makes the whole type false,
            // so `List[Bool]` is not `List[Int]` even when another argument is still a type parameter (6.1, 6.7).
            if h.args.len() != w.args.len() {
                // A constructor's result names the class (`Hold`) while the signature names `Hold[Tree]`,
                // and those argument counts were never proved apart. Same-arity arguments are folded below,
                // so `List[Bool]` is still not `List[Int]`.
                return unknown_v();
            }
            let mut unknown = false;
            for (a, b) in h.args.iter().zip(w.args.iter()) {
                let found = origin_clash(a, b);
                if !found.is_empty() {
                    return no(found);
                }
                let (verdict, why) = assignable(a, b);
                if verdict == Some(false) {
                    return no(why);
                }
                if verdict == Some(true) && refinement_differs(a, b) {
                    // a type argument is invariant in its refinement (6.3, X-514; D-465): through an alias with the wider
                    // element type a value the narrower one excludes could be stored
                    return no(format!("type arguments are invariant in their refinement: {} is not {} (6.3)", a.show(), b.show()));
                }
                if verdict.is_none() {
                    unknown = true;
                }
            }
            return if unknown { unknown_v() } else { yes() };
        }
        if h.name == w.name {
            return no(format!(
                "{} of {} is not {} of {}",
                h.name,
                if h.package.is_empty() { "this package" } else { &h.package },
                w.name,
                if w.package.is_empty() { "this package" } else { &w.package }
            ));
        }
        if w.name == "Self" || h.name == "Self" {
            return unknown_v();
        }
        return no(format!("{} is not {}", have.show(), want.show()));
    }
    if let (Type::Fn { params: hp, ret: hr, kind: hk, modifiers: hm, param_modes: hmode, .. }, Type::Fn { params: wp, ret: wr, kind: wk, modifiers: wm, param_modes: wmode, .. }) = (have, want) {
        let order = |k: &str| match k {
            "fn" => 0,
            "fn mut" => 1,
            _ => 2,
        };
        if order(hk) > order(wk) {
            return no(format!("a `{hk}` closure cannot be used as `{wk}` (8.4)"));
        }
        if hp.len() != wp.len() {
            return no("parameter count differs".into());
        }
        for (i, (a, b)) in hp.iter().zip(wp.iter()).enumerate() {
            let h = hmode.get(i).map(String::as_str).unwrap_or("");
            let w = wmode.get(i).map(String::as_str).unwrap_or("");
            if h != w { return no(format!("parameter {} mode {} differs from {} (8.4)", i + 1, if h.is_empty() { "read" } else { h }, if w.is_empty() { "read" } else { w })); }
            if known(a) && known(b) && strip(a) != strip(b) {
                return no(format!("parameter {} differs from {}", a.show(), b.show()));
            }
            if known(a) && known(b) {
                let found = origin_clash(a, b); // equality ignores an origin nested in the parameter
                if !found.is_empty() {
                    return no(format!("parameter {found}"));
                }
            }
        }
        if wm.iter().any(|m| m == "async") && !hm.iter().any(|m| m == "async") {
            return no("the position requires an async callable: declare the function `async`, or wrap the call in an async lambda (8.4, 12.3)".into());
        }
        for claim in ["pure", "noalloc"] {
            if wm.iter().any(|m| m == claim) && !hm.iter().any(|m| m == claim) {
                return no(format!("the position requires `{claim}`"));
            }
        }
        for weak in ["try", "async", "recovers"] {
            if hm.iter().any(|m| m == weak) && !wm.iter().any(|m| m == weak) {
                return no(format!("the closure is `{weak}` but the position is not"));
            }
        }
        let r = assignable(hr, wr);
        return if r.0 != Some(false) { yes() } else { no(format!("result {}", r.1)) };
    }
    let pairs: Vec<(&Type, &Type)> = match (have, want) {
        (Type::Tuple(xs), Type::Tuple(ys)) => {
            if xs.len() != ys.len() { return no("tuple element count differs".into()); }
            xs.iter().zip(ys).collect()
        }
        (Type::Record(xs), Type::Record(ys)) => {
            if xs.len() != ys.len() || xs.iter().any(|(k, _)| !ys.iter().any(|(l, _)| k == l)) {
                return no("record field names differ".into());
            }
            xs.iter().map(|(k, x)| (x, &ys.iter().find(|(l, _)| k == l).unwrap().1)).collect()
        }
        _ => Vec::new(),
    };
    if matches!((have, want), (Type::Tuple(_), Type::Tuple(_)) | (Type::Record(_), Type::Record(_))) {
        let mut unknown = false;
        for (a, b) in pairs {
            let verdict = assignable(a, b);
            if verdict.0 == Some(false) { return verdict; }
            unknown |= verdict.0.is_none();
        }
        return if unknown { unknown_v() } else { yes() };
    }
    if have.is_optional() && !want.is_optional() {
        return no(format!("{} is optional; {} is not (6.8)", have.show(), want.show()));
    }
    if std::mem::discriminant(have) != std::mem::discriminant(want) {
        if matches!(have, Type::Dynamic(_)) || matches!(want, Type::Dynamic(_)) {
            return unknown_v();
        }
        // two known types of different kinds (`Light` against `fn`, `fn` against `Never?`) are not the same type (6.1)
        return no(format!("{} is not {}", have.show(), want.show()));
    }
    unknown_v()
}
