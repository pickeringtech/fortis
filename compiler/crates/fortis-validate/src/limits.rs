//! The build's resource limits (Programme 7.1, class Resource limit, `E-LIM`): the instantiation-depth budget.
//!
//! A generic function is compiled once per instantiation. A generic function that calls itself (directly or
//! through others) at a larger type, `nest(Box(value: x))` inside `nest[T]`, has no last instantiation:
//! `nest[Int]`, `nest[Box[Int]]`, `nest[Box[Box[Int]]]` and so on. Starting from the program's non-generic
//! functions, every instantiation reachable through calls is enumerated; one whose type arguments nest deeper than
//! `DEPTH`, or more than `COUNT` instantiations in all, exhausts the budget, and the build stops with the
//! instantiation chain instead of handing the backend an unbounded program.

use std::collections::{BTreeMap, HashMap, HashSet};

use fortis_types::types::Type;

use fortis_mir::model::{Callee, Expr, ExprKind, Function, Program, TypeKind};

/// The deepest type argument an instantiation may have.
pub const DEPTH: usize = 32;
/// The most instantiations one program may have.
pub const COUNT: usize = 100_000;

/// The instantiation that exhausted the budget, and how it was reached.
#[derive(Debug, Clone)]
pub struct Exhausted {
    /// the function and line of the call that instantiated past the budget
    pub function: String,
    pub line: u32,
    pub budget: String,
    /// the instantiations from a non-generic function to the one past the budget, the last few
    pub chain: Vec<String>,
}

fn type_params(t: &Type, out: &mut Vec<String>) {
    match t {
        Type::TypeParam(n) => {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
        Type::Named(n) | Type::Dynamic(n) => n.args.iter().for_each(|a| type_params(a, out)),
        Type::Refined { base, .. } => base.args.iter().for_each(|a| type_params(a, out)),
        Type::Optional(i) | Type::Loan { inner: i, .. } => type_params(i, out),
        Type::Tuple(ts) => ts.iter().for_each(|x| type_params(x, out)),
        Type::Record(fs) => fs.iter().for_each(|(_, x)| type_params(x, out)),
        Type::Fn { params, ret, .. } => {
            params.iter().for_each(|x| type_params(x, out));
            type_params(ret, out);
        }
        Type::Unknown(_) => {}
    }
}

fn generic(f: &Function) -> bool {
    let mut ps = Vec::new();
    for p in &f.params {
        type_params(&f.locals[p.local as usize].ty, &mut ps);
    }
    !ps.is_empty()
}

fn unify(pattern: &Type, actual: &Type, out: &mut BTreeMap<String, Type>) {
    match (pattern, actual) {
        (Type::TypeParam(n), a) => {
            out.entry(n.clone()).or_insert_with(|| a.clone());
        }
        (Type::Named(p), Type::Named(a)) | (Type::Named(p), Type::Refined { base: a, .. }) | (Type::Dynamic(p), Type::Dynamic(a)) if p.name == a.name => {
            p.args.iter().zip(a.args.iter()).for_each(|(x, y)| unify(x, y, out));
        }
        (Type::Optional(p), Type::Optional(a)) => unify(p, a, out),
        (Type::Loan { inner: p, .. }, Type::Loan { inner: a, .. }) => unify(p, a, out),
        (Type::Loan { inner: p, .. }, a) => unify(p, a, out),
        (Type::Tuple(p), Type::Tuple(a)) => p.iter().zip(a.iter()).for_each(|(x, y)| unify(x, y, out)),
        _ => {}
    }
}

fn subst(t: &Type, b: &BTreeMap<String, Type>) -> Type {
    match t {
        Type::TypeParam(n) => b.get(n).cloned().unwrap_or_else(|| t.clone()),
        Type::Named(n) => {
            let mut n = n.clone();
            n.args = n.args.iter().map(|a| subst(a, b)).collect();
            Type::Named(n)
        }
        Type::Optional(i) => Type::Optional(Box::new(subst(i, b))),
        Type::Loan { inner, mode, origin } => Type::Loan { inner: Box::new(subst(inner, b)), mode: mode.clone(), origin: origin.clone() },
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|x| subst(x, b)).collect()),
        other => other.clone(),
    }
}

/// How deeply a type nests.
pub fn depth(t: &Type) -> usize {
    match t {
        Type::Named(n) | Type::Dynamic(n) => 1 + n.args.iter().map(depth).max().unwrap_or(0),
        Type::Refined { base, .. } => 1 + base.args.iter().map(depth).max().unwrap_or(0),
        Type::Optional(i) => 1 + depth(i),
        Type::Loan { inner, .. } => depth(inner),
        Type::Tuple(ts) => 1 + ts.iter().map(depth).max().unwrap_or(0),
        Type::Record(fs) => 1 + fs.iter().map(|(_, x)| depth(x)).max().unwrap_or(0),
        Type::Fn { params, ret, .. } => 1 + params.iter().chain(std::iter::once(&**ret)).map(depth).max().unwrap_or(0),
        _ => 1,
    }
}

fn show(name: &str, b: &BTreeMap<String, Type>) -> String {
    let short = name.rsplit("::").next().unwrap_or(name).trim_start_matches("function ").trim_start_matches("method ");
    if b.is_empty() {
        short.to_string()
    } else {
        format!("{short}[{}]", b.values().map(|t| t.show()).collect::<Vec<_>>().join(", "))
    }
}

/// An expression's type with the type arguments of a generic constructor rebuilt from its fields (the checker
/// types `Box(value: x)` as `Box`; its arguments are what the declared field types bind).
fn full_type(prog: &Program, e: &Expr) -> Type {
    if let ExprKind::Construct { ty, fields } = &e.kind {
        if let Some(td) = prog.type_def(ty) {
            if !td.type_params.is_empty() {
                if let TypeKind::Class { fields: decl } = &td.kind {
                    let mut b = BTreeMap::new();
                    for (name, value) in fields {
                        if let Some(fd) = decl.iter().find(|f| f.name == *name) {
                            unify(&fd.ty, &full_type(prog, value), &mut b);
                        }
                    }
                    if let Type::Named(n) = &e.ty {
                        let mut n = n.clone();
                        n.args = td.type_params.iter().map(|p| b.get(p).cloned().unwrap_or_else(|| Type::TypeParam(p.clone()))).collect();
                        return Type::Named(n);
                    }
                }
            }
        }
    }
    e.ty.clone()
}

/// The generic calls of a function body: (callee, line, argument types bound to parameter types).
fn generic_calls<'p>(prog: &'p Program, f: &'p Function) -> Vec<(&'p Function, u32, Vec<(Type, Type)>)> {
    let mut out = Vec::new();
    let Some(b) = &f.body else { return out };
    crate::validate::walk_block(b, &mut |n| {
        let crate::validate::Node::Expr(e) = n else { return };
        let e: &Expr = e;
        if let ExprKind::Call { callee: Callee::Func(name), args } = &e.kind {
            if let Some(g) = prog.function(name) {
                if generic(g) {
                    let pairs = args
                        .iter()
                        .filter_map(|a| g.params.iter().find(|p| p.name == a.param).map(|p| (g.locals[p.local as usize].ty.clone(), full_type(prog, &a.value))))
                        .collect();
                    out.push((g, e.line, pairs));
                }
            }
        }
    });
    out
}

/// Enumerate the program's instantiations within the budget.
pub fn instantiations(prog: &Program) -> Result<usize, Exhausted> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut todo: Vec<(&Function, BTreeMap<String, Type>, String)> = Vec::new();
    for f in prog.functions.iter().filter(|f| f.body.is_some() && !generic(f)) {
        let k = show(&f.name, &BTreeMap::new());
        todo.push((f, BTreeMap::new(), k));
    }
    while let Some((f, b, key)) = todo.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        if seen.len() > COUNT {
            return Err(Exhausted { function: f.name.clone(), line: f.line, budget: format!("{COUNT} instantiations"), chain: vec![key] });
        }
        for (g, line, pairs) in generic_calls(prog, f) {
            let mut gb = BTreeMap::new();
            for (pt, at) in &pairs {
                unify(pt, &subst(at, &b), &mut gb);
            }
            let gkey = show(&g.name, &gb);
            if gb.values().any(|t| depth(t) > DEPTH) {
                // the chain from the non-generic function that started it: its first steps, then the depth reached
                let mut full = vec![key.clone()];
                while let Some(p) = parent.get(full.last().unwrap()) {
                    full.push(p.clone());
                }
                full.reverse();
                let mut chain: Vec<String> = full.iter().take(4).cloned().collect();
                if full.len() > 4 {
                    chain.push("…".into());
                }
                chain.push(format!("{} with type arguments {} deep", gkey.split('[').next().unwrap_or(&gkey), gb.values().map(depth).max().unwrap_or(0)));
                return Err(Exhausted { function: f.name.clone(), line, budget: format!("instantiation depth {DEPTH}"), chain });
            }
            if !seen.contains(&gkey) {
                parent.entry(gkey.clone()).or_insert_with(|| key.clone());
                todo.push((g, gb, gkey));
            }
        }
    }
    Ok(seen.len())
}
