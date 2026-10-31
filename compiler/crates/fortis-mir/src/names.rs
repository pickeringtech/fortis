//! Program-unique names in the typed IR (Language Reference 4.1, LANG-04-04; X-136).
//!
//! The checker tells types apart by their identity (`package@version/module`); the typed IR names a function
//! `module::label` and a type by its declared name. Two things would collide: a package linked at two versions (the
//! same modules, twice), and a type name declared by two modules. Only those are qualified, so every other program's
//! IR is unchanged:
//!
//! - a module of a package linked at more than one version is `module@version` (`acme.util.util@2.0.0`);
//! - a type whose name more than one module declares is `Name@module` (`Item@same.left.items`), with the version
//!   when its package is linked at more than one (`Helper@acme.util.util@2.0.0`).
//!
//! The builder names what it declares and what it calls through these rules; `map_types` then gives every type in
//! the program the same name.

use std::collections::{BTreeMap, BTreeSet};

use fortis_types::types::{NamedT, Type};

use crate::model::*;

#[derive(Clone, Debug, Default)]
pub struct Qualify {
    /// identities (`name@version`) of packages linked at more than one version
    pub versioned: BTreeSet<String>,
    /// type names more than one module declares
    pub types: BTreeSet<String>,
}

/// The package identity and the module of a type identity (`acme.util@2.0.0/acme.util.util`).
fn split_identity(identity: &str) -> (&str, &str) {
    identity.split_once('/').unwrap_or((identity, ""))
}

impl Qualify {
    /// The rules for a program of these modules, each with its type identity (`package@version/module`) and the type
    /// names it declares.
    pub fn of<'p>(modules: impl IntoIterator<Item = (&'p str, BTreeSet<String>)>) -> Qualify {
        let mut versions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (identity, types) in modules {
            let package = split_identity(identity).0;
            let name = package.split('@').next().unwrap_or(package);
            if name != "fortis" { versions.entry(name.to_string()).or_default().insert(package.to_string()); }
            for t in types {
                declared.entry(t).or_default().insert(identity.to_string());
            }
        }
        Qualify {
            versioned: versions.into_values().filter(|v| v.len() > 1).flatten().collect(),
            types: declared.into_iter().filter(|(_, ids)| ids.len() > 1).map(|(t, _)| t).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.versioned.is_empty() && self.types.is_empty()
    }

    /// The IR name of a module of the package with this identity.
    pub fn module(&self, module: &str, package: &str) -> String {
        match package.split_once('@') {
            Some((_, version)) if self.versioned.contains(package) => format!("{module}@{version}"),
            _ => module.to_string(),
        }
    }

    /// The IR name of a type with this identity (`package@version/module`).
    pub fn type_name(&self, name: &str, identity: &str) -> String {
        let (package, module) = split_identity(identity);
        if module.is_empty() || fortis_types::decls::is_standard(package) || !self.types.contains(name) {
            return name.to_string();
        }
        format!("{name}@{}", self.module(module, package))
    }

    fn named(&self, n: &mut NamedT) {
        n.name = self.type_name(&n.name, &n.package);
        for a in &mut n.args {
            self.apply(a);
        }
    }

    /// A type with every named type in it given its IR name.
    pub fn apply(&self, t: &mut Type) {
        match t {
            Type::Named(n) | Type::Dynamic(n) => self.named(n),
            Type::Refined { base, .. } => self.named(base),
            Type::Optional(i) => self.apply(i),
            Type::Loan { inner, .. } => self.apply(inner),
            Type::Tuple(ts) => ts.iter_mut().for_each(|t| self.apply(t)),
            Type::Record(fs) => fs.iter_mut().for_each(|(_, t)| self.apply(t)),
            Type::Fn { params, ret, .. } => {
                params.iter_mut().for_each(|t| self.apply(t));
                self.apply(ret);
            }
            Type::TypeParam(_) | Type::Unknown(_) => {}
        }
    }
}

/// Calls `f` on every type the program holds: declarations, signatures, locals and every expression's type.
pub fn map_types(prog: &mut Program, f: &mut dyn FnMut(&mut Type)) {
    for t in &mut prog.types {
        match &mut t.kind {
            TypeKind::Class { fields } | TypeKind::Error { payload: fields, .. } => {
                for fd in fields {
                    f(&mut fd.ty);
                    if let Some(d) = &mut fd.default {
                        expr(d, f);
                    }
                }
            }
            TypeKind::Enum { variants } => variants.iter_mut().flat_map(|v| v.fields.iter_mut()).for_each(|(_, t)| f(t)),
            TypeKind::Newtype { repr } => f(repr),
            TypeKind::Observation { fields, .. } => fields.iter_mut().for_each(|(_, t, _)| f(t)),
            TypeKind::Interface { .. } | TypeKind::Mixin { .. } => {}
        }
        // an interface's declared signatures and every type's interface arguments (D-82)
        for s in &mut t.sigs {
            s.params.iter_mut().for_each(|(_, _, pt)| f(pt));
            f(&mut s.ret);
            s.bounds.iter_mut().for_each(|(_, _, a)| a.iter_mut().for_each(|x| f(x)));
        }
        t.iface_args.iter_mut().for_each(|(_, a)| a.iter_mut().for_each(|x| f(x)));
    }
    for imp in &mut prog.impls {
        imp.args.iter_mut().for_each(|x| f(x));
    }
    for func in &mut prog.functions {
        func.bounds.iter_mut().for_each(|(_, _, a)| a.iter_mut().for_each(|x| f(x)));
        f(&mut func.ret);
        func.locals.iter_mut().for_each(|l| f(&mut l.ty));
        for p in &mut func.params {
            if let Some(d) = &mut p.default {
                expr(d, f);
            }
        }
        if let Some(b) = &mut func.body {
            block(b, f);
        }
    }
    for c in &mut prog.consts {
        f(&mut c.ty);
        expr(&mut c.value, f);
    }
    for t in &mut prog.tests {
        t.cases.iter_mut().for_each(|(_, e)| expr(e, f));
        t.generators.iter_mut().for_each(|(_, t)| f(t));
    }
}

fn block(b: &mut Block, f: &mut dyn FnMut(&mut Type)) {
    b.stmts.iter_mut().for_each(|s| stmt(s, f));
    b.defers.iter_mut().for_each(|d| block(d, f));
}

fn stmt(s: &mut Stmt, f: &mut dyn FnMut(&mut Type)) {
    match &mut s.kind {
        StmtKind::Let { init: e, .. }
        | StmtKind::LetPattern { init: e, .. }
        | StmtKind::Assign { value: e, .. }
        | StmtKind::Compound { value: e, .. }
        | StmtKind::Expr(e)
        | StmtKind::Discard(e)
        | StmtKind::Give(e)
        | StmtKind::Recover(e)
        | StmtKind::Fail(e)
        | StmtKind::Capture { value: e, .. }
        | StmtKind::Report { value: e, .. } => expr(e, f),
        StmtKind::Return(e) => {
            if let Some(e) = e {
                expr(e, f);
            }
        }
        StmtKind::If { cond, then, els } => {
            expr(cond, f);
            block(then, f);
            if let Some(e) = els {
                block(e, f);
            }
        }
        StmtKind::Loop { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => block(body, f),
        StmtKind::For { source, body, .. } => {
            match source {
                ForSource::Range { lo, hi, .. } => {
                    expr(lo, f);
                    expr(hi, f);
                }
                ForSource::Items(e) => expr(e, f),
            }
            block(body, f);
        }
        StmtKind::Match { scrutinee, arms } => {
            expr(scrutinee, f);
            arms.iter_mut().for_each(|a| arm(a, f));
        }
        StmtKind::Record { fields, .. } => fields.iter_mut().for_each(|(_, e)| expr(e, f)),
        StmtKind::Expect { cond, evidence } => {
            expr(cond, f);
            if let Some((a, _, b)) = evidence {
                expr(a, f);
                expr(b, f);
            }
        }
        StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Defer(_) | StmtKind::Drop(_) => {}
    }
}

fn arm(a: &mut Arm, f: &mut dyn FnMut(&mut Type)) {
    if let Some(g) = &mut a.guard {
        expr(g, f);
    }
    match &mut a.body {
        ArmBody::Expr(e) => expr(e, f),
        ArmBody::Block(b) => block(b, f),
    }
}

fn expr(e: &mut Expr, f: &mut dyn FnMut(&mut Type)) {
    f(&mut e.ty);
    match &mut e.kind {
        ExprKind::Const(_) | ExprKind::Place(..) | ExprKind::Global(_) | ExprKind::FuncRef(_) => {}
        ExprKind::Codec { target, arg, .. } => {
            f(target);
            expr(arg, f);
        }
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Index(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) => {
            expr(a, f);
            expr(b, f);
        }
        ExprKind::Unary(_, a)
        | ExprKind::Field(a, _)
        | ExprKind::TupleField(a, _)
        | ExprKind::OptField(a, _)
        | ExprKind::Present(a)
        | ExprKind::Success(a)
        | ExprKind::OrFail(a)
        | ExprKind::Fail(a)
        | ExprKind::Await(a)
        | ExprKind::Validate { value: a, .. }
        | ExprKind::Wrap { value: a, .. }
        | ExprKind::ToDynamic { value: a, .. }
        | ExprKind::Temp { value: a, .. } => expr(a, f),
        ExprKind::Call { callee, args } => {
            if let Callee::Value(c) = callee {
                expr(c, f);
            }
            args.iter_mut().for_each(|a| expr(&mut a.value, f));
        }
        ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => {
            fields.iter_mut().for_each(|(_, e)| expr(e, f))
        }
        ExprKind::Tuple(es) | ExprKind::Closure { captures: es, .. } => es.iter_mut().for_each(|e| expr(e, f)),
        ExprKind::Interpolate(parts) => {
            for p in parts {
                if let Part::Show(e) = p {
                    expr(e, f);
                }
            }
        }
        ExprKind::Catch { value, handler, .. } => {
            expr(value, f);
            block(handler, f);
        }
        ExprKind::If { cond, then, els } => {
            expr(cond, f);
            block(then, f);
            block(els, f);
        }
        ExprKind::Match { scrutinee, arms } => {
            expr(scrutinee, f);
            arms.iter_mut().for_each(|a| arm(a, f));
        }
        ExprKind::Block(b) => block(b, f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn q() -> Qualify {
        Qualify::of([
            ("acme.util@1.0.0/acme.util.util", set(&["Helper"])),
            ("acme.util@2.0.0/acme.util.util", set(&["Helper"])),
            ("acme.left@1.0.0/acme.left.items", set(&["Item", "Only"])),
            ("acme.right@1.0.0/acme.right.items", set(&["Item"])),
            ("acme.two@1.0.0/acme.two.one", set(&["Node"])),
            ("acme.two@1.0.0/acme.two.two", set(&["Node"])),
            ("fortis@1.0/fortis.prelude", set(&["Item"])),
        ])
    }

    #[test]
    fn only_what_collides_is_qualified() {
        let q = q();
        assert_eq!(q.module("acme.util.util", "acme.util@2.0.0"), "acme.util.util@2.0.0");
        assert_eq!(q.module("acme.left.items", "acme.left@1.0.0"), "acme.left.items");
        assert_eq!(q.type_name("Helper", "acme.util@1.0.0/acme.util.util"), "Helper@acme.util.util@1.0.0");
        assert_eq!(q.type_name("Item", "acme.left@1.0.0/acme.left.items"), "Item@acme.left.items");
        assert_eq!(q.type_name("Node", "acme.two@1.0.0/acme.two.two"), "Node@acme.two.two");
        assert_eq!(q.type_name("Only", "acme.left@1.0.0/acme.left.items"), "Only");
        assert_eq!(q.type_name("Item", ""), "Item");
        assert!(Qualify::of([("acme.a@1.0.0/acme.a.m", set(&["T"]))]).is_empty());
    }

    #[test]
    fn types_are_renamed_inside_other_types() {
        let q = q();
        let item = Type::Named(NamedT { name: "Item".into(), args: vec![], package: "acme.right@1.0.0/acme.right.items".into(), origin: String::new() });
        let mut t = Type::Optional(Box::new(Type::Named(NamedT { name: "List".into(), args: vec![item], package: String::new(), origin: String::new() })));
        q.apply(&mut t);
        assert_eq!(format!("{t:?}").matches("Item@acme.right.items").count(), 1);
    }
}
