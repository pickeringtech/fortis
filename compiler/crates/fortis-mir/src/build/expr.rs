//! Expressions: names, places, operators, calls (with dispatch, modes, defaults), constructors, lambdas, `match`
//! and `if` expressions, outcome handling, literals and interpolation.

use std::collections::BTreeMap;

use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{C, N};
use fortis_types::decls::{named_of_decl, DeclRef, Sig};
use fortis_types::types::*;

use super::fb::{Fb, LambdaCtx};
use super::{err, line_of, tok_line, BResult, BuildError, Builder};
use crate::model::*;

fn bx(e: Expr) -> Box<Expr> {
    Box::new(e)
}

/// The temporary under a chain of tuple fields, and the indexes from the outside in (`.0.1` is `[0, 1]`).
fn tuple_temp_proj(e: &Expr, suffix: Vec<u32>) -> Option<(LocalId, Vec<u32>)> {
    match &e.kind {
        ExprKind::Temp { local, .. } => Some((*local, suffix)),
        ExprKind::TupleField(inner, i) => {
            let mut path = vec![*i];
            path.extend(suffix);
            tuple_temp_proj(inner, path)
        }
        _ => None,
    }
}

/// Split that chain into the temporary expression itself and the same indexes. The temporary is what a
/// consuming use discards, so the slot is filled; an intermediate projection is not a temporary.
fn split_tuple_temp(e: Expr, suffix: Vec<u32>) -> Option<(Expr, Vec<u32>)> {
    match e.kind {
        ExprKind::Temp { .. } => Some((e, suffix)),
        ExprKind::TupleField(inner, i) => {
            let mut path = vec![i];
            path.extend(suffix);
            split_tuple_temp(*inner, path)
        }
        _ => None,
    }
}

/// `ir` is `bare@module` and `bare_ty` is that bare nominal: the call's result is the disambiguated enum (D-142).
fn nominal_shadow(bare_ty: &Type, ir: &Type) -> bool {
    match (strip(bare_ty), strip(ir)) {
        (Type::Named(b), Type::Named(i)) => i.name.split_once('@').is_some_and(|(n, _)| n == b.name),
        _ => false,
    }
}

/// Mark the root of a core signature's type as a standard-library type. A single-file program's class has an empty
/// identity, the same as a core type, so `rust_ty` would otherwise spell both `Duration`. Type arguments stay as
/// written: `List[Duration].default` carries the program's `Duration`, not `rt::Duration` (D-142).
fn mark_standard(t: &Type, module: &str) -> Type {
    if !fortis_types::decls::is_standard(module) {
        return t.clone();
    }
    match t {
        Type::Named(n) => {
            let mut n = n.clone();
            if n.package.is_empty() {
                n.package = module.to_string();
            }
            Type::Named(n)
        }
        Type::Refined { base, lo, hi, kind } => {
            let mut base = base.clone();
            if base.package.is_empty() {
                base.package = module.to_string();
            }
            Type::Refined { base, lo: *lo, hi: *hi, kind: kind.clone() }
        }
        Type::Optional(i) => Type::Optional(Box::new(mark_standard(i, module))),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|x| mark_standard(x, module)).collect()),
        Type::Loan { inner, mode, origin } => Type::Loan { inner: Box::new(mark_standard(inner, module)), mode: mode.clone(), origin: origin.clone() },
        other => other.clone(),
    }
}

/// The type a value of `t` has when read: refinements and loans fall away.
pub fn value_ty(t: &Type) -> Type {
    match strip(t) {
        Type::Refined { base, .. } => Type::Named(base.clone()),
        other => other.clone(),
    }
}

impl<'t, 'a> Builder<'t, 'a> {
    /// A value where `want` is expected: `T` into `T?` is wrapped explicitly.
    pub fn coerce(&self, e: Expr, want: &Type) -> Expr {
        // a class value where `dynamic I` (owned) or `alias dynamic I` is expected: an explicit conversion
        let dynamic = match want {
            Type::Dynamic(n) => Some((self.tname(&n.name, &n.package), false)),
            Type::Loan { inner, .. } => match &**inner {
                Type::Dynamic(n) => Some((self.tname(&n.name, &n.package), true)),
                _ => None,
            },
            _ => None,
        };
        if let Some((iface, borrowed)) = dynamic {
            let have = strip(&e.ty);
            if matches!(have, Type::Named(_)) {
                let line = e.line;
                return Expr::new(ExprKind::ToDynamic { iface, value: Box::new(e), borrowed }, want.clone(), line);
            }
        }
        let w = strip(want);
        if let Type::Optional(_) = w {
            let have = strip(&e.ty);
            let is_null = matches!(e.kind, ExprKind::Const(Const::Null));
            if !is_null && !have.is_optional() && known(have) && *have != never() {
                let line = e.line;
                return Expr::new(ExprKind::Present(bx(e)), w.clone(), line);
            }
        }
        e
    }

    pub fn expr_c(&mut self, c: C<'a>) -> BResult<Expr> {
        match c {
            C::N(n) => self.expr(n),
            C::T(t) => self.literal_tok(t, None),
        }
    }

    /// A value in a consuming position (a binding, an owned argument, a returned value) is not itself a
    /// statement temporary: its consumer owns it (7.9).
    pub fn expr_moved(&mut self, n: N<'a>) -> BResult<Expr> {
        self.with_consume(n, true)
    }

    pub fn expr(&mut self, n: N<'a>) -> BResult<Expr> {
        self.with_consume(n, false)
    }

    fn with_consume(&mut self, n: N<'a>, consuming: bool) -> BResult<Expr> {
        let saved = self.consuming;
        self.consuming = consuming;
        let built = self.expr_produce(n);
        self.consuming = saved;
        let e = built?;
        // a consuming position takes ownership, including a field moved out of a temporary and a value
        // forwarded by `??` or `or fail` (7.9). The temporary stays, and does not destroy what moved out.
        Ok(if consuming { self.take_owned(e) } else { self.hold(e) })
    }

    /// A single-child wrapper is the same value as its child, so a consuming position stays consuming.
    fn propagate(&mut self, c: C<'a>) -> BResult<Expr> {
        let consuming = self.consuming;
        match c {
            C::N(n) => self.with_consume(n, consuming),
            C::T(t) => {
                let e = self.literal_tok(t, None)?;
                Ok(if consuming { e } else { self.hold(e) })
            }
        }
    }

    /// A newly created owned value that nothing takes ownership of is a statement temporary, destroyed at the
    /// end of the full statement in reverse creation order (5.7, 7.9). A projection, a place or a constant is
    /// not one: it does not create a value, and dropping it as well as its source would run one cleanup twice.
    fn hold(&mut self, e: Expr) -> Expr {
        if self.fbs.is_empty() || self.fb().stmt_temps.is_empty() {
            return e;
        }
        let create = matches!(
            e.kind,
            ExprKind::Call { .. }
                | ExprKind::Construct { .. }
                | ExprKind::Variant { .. }
                | ExprKind::Tuple(_)
                | ExprKind::Record(_)
                | ExprKind::Block(_)
                | ExprKind::If { .. }
                | ExprKind::Match { .. }
                | ExprKind::Await(_)
        );
        if !create || !self.ty.dc.has_drop(&e.ty) {
            return e;
        }
        let ty = e.ty.clone();
        let line = e.line;
        let local = self.fb().stmt_temp(ty.clone());
        Expr::new(ExprKind::Temp { local, value: bx(e) }, ty, line)
    }

    fn untrack_temp(&mut self, local: LocalId) {
        if let Some(v) = self.fb().stmt_temps.last_mut() {
            v.retain(|id| *id != local);
        }
    }

    /// An owned use consumes the temporary: the consumer owns the value (7.9). A field of a temporary is
    /// moved out of it; the temporary stays, and must not destroy that field again. `??`, `or fail`,
    /// `or recover`, `present` and `success` forward one value, so the same transfer applies to the operand
    /// that is selected. The operand that does not run is never created.
    fn take_owned(&mut self, recv: Expr) -> Expr {
        let line = recv.line;
        let ty = recv.ty.clone();
        match recv.kind {
            ExprKind::Temp { local, value } => {
                self.untrack_temp(local);
                *value
            }
            ExprKind::Field(inner, name) => {
                if let ExprKind::Temp { local, .. } = &inner.kind {
                    let local = *local;
                    let ft = ty;
                    // The temporary stays tracked: it is destroyed at the end of the statement (7.9). A trivial
                    // field is read and a copyable field is copied, so cleanup still sees them. Only a
                    // non-copyable field is moved out, and the temporary must not destroy that field again.
                    let use_of = if self.ty.dc.is_trivial(&ft) {
                        Use::Read
                    } else if self.ty.dc.is_copyable(&ft) {
                        Use::Copy
                    } else {
                        Use::Move
                    };
                    let moved = Expr::new(ExprKind::Place(Place::local(local).field(&name), use_of), ft.clone(), line);
                    let discard = Stmt { line, temps: vec![], kind: StmtKind::Discard(*inner) };
                    let give = Stmt { line, temps: vec![], kind: StmtKind::Give(moved) };
                    let body = Block { stmts: vec![discard, give], locals: vec![], line, defers: vec![] };
                    Expr::new(ExprKind::Block(body), ft, line)
                } else {
                    Expr { kind: ExprKind::Field(inner, name), ..recv }
                }
            }
            ExprKind::TupleField(inner, index) => {
                // `.0` of a temporary is the same transfer as a field. The temporary stays tracked and is
                // destroyed at the end of the statement, after the consumer (7.9). A trivial element is read
                // and a copyable one is copied, so the other elements stay in the tuple. Only a non-copyable
                // element is moved out. A chain `.0.1` is one place with both projections. The discarded
                // expression is the temporary itself: discarding an intermediate projection would leave the
                // slot unfilled, and the place move would then read an empty temporary.
                if tuple_temp_proj(&inner, vec![index]).is_some() {
                    let (temp, proj) = split_tuple_temp(*inner, vec![index]).expect("tuple temp");
                    let local = match &temp.kind {
                        ExprKind::Temp { local, .. } => *local,
                        _ => unreachable!("split_tuple_temp returns the temporary"),
                    };
                    let ft = ty;
                    let mut place = Place::local(local);
                    for i in proj {
                        place = place.tuple(i);
                    }
                    let use_of = if self.ty.dc.is_trivial(&ft) {
                        Use::Read
                    } else if self.ty.dc.is_copyable(&ft) {
                        Use::Copy
                    } else {
                        Use::Move
                    };
                    let moved = Expr::new(ExprKind::Place(place, use_of), ft.clone(), line);
                    let discard = Stmt { line, temps: vec![], kind: StmtKind::Discard(temp) };
                    let give = Stmt { line, temps: vec![], kind: StmtKind::Give(moved) };
                    let body = Block { stmts: vec![discard, give], locals: vec![], line, defers: vec![] };
                    Expr::new(ExprKind::Block(body), ft, line)
                } else {
                    Expr { kind: ExprKind::TupleField(inner, index), ty, line }
                }            }
            ExprKind::Coalesce(a, b) => {
                let a = self.take_owned(*a);
                let b = self.take_owned(*b);
                Expr::new(ExprKind::Coalesce(bx(a), bx(b)), ty, line)
            }
            ExprKind::OrFail(a) => Expr::new(ExprKind::OrFail(bx(self.take_owned(*a))), ty, line),
            ExprKind::OrRecover(a, b) => {
                let a = self.take_owned(*a);
                let b = self.take_owned(*b);
                Expr::new(ExprKind::OrRecover(bx(a), bx(b)), ty, line)
            }
            ExprKind::Present(a) => Expr::new(ExprKind::Present(bx(self.take_owned(*a))), ty, line),
            ExprKind::Success(a) => Expr::new(ExprKind::Success(bx(self.take_owned(*a))), ty, line),
            kind => Expr { kind, ty, line },
        }
    }

    /// A condition has no full-statement end of its own (decision for the integrator): its temporaries die when
    /// the condition finishes, before the branch or the next iteration. Native code drops them there too.
    pub fn seal_cond(&mut self, cond: N<'a>) -> BResult<Expr> {
        let line = line_of(cond);
        self.fb().stmt_temps.push(vec![]);
        let init = self.expr(cond);
        let temps = self.fb().stmt_temps.pop().unwrap();
        let init = init?;
        let n = self.fb().locals.len();
        let local = self.fb().add_local(&format!("__cond{n}"), boolean(), false, LocalKind::Temp);
        let give_v = Expr::new(ExprKind::Place(Place::local(local), Use::Read), boolean(), line);
        let body = Block {
            stmts: vec![Stmt { line, temps, kind: StmtKind::Let { local, init } }, Stmt { line, temps: vec![], kind: StmtKind::Give(give_v) }],
            locals: vec![],
            line,
            defers: vec![],
        };
        Ok(Expr::new(ExprKind::Block(body), boolean(), line))
    }

    fn expr_produce(&mut self, n: N<'a>) -> BResult<Expr> {
        let line = line_of(n);
        let kids = n.kids();
        let ty = self.node_ty(n);
        match n.name() {
            "expr" | "exprStmt" | "literal" if kids.len() == 1 => self.propagate(kids[0]),
            "outcomeExpr" => self.outcome(n),
            "coalesceExpr" => {
                if kids.len() == 1 {
                    return self.propagate(kids[0]);
                }
                // the selected operand is the result. A consuming position owns it, so that operand is not
                // also a statement temporary (7.9). The operand that is not selected is still built here and
                // is not evaluated (the VM and native short-circuit).
                let consuming = self.consuming;
                let side = |b: &mut Self, c: C<'a>| -> BResult<Expr> {
                    match c {
                        C::N(n) => b.with_consume(n, consuming),
                        C::T(t) => {
                            let e = b.literal_tok(t, None)?;
                            Ok(if consuming { e } else { b.hold(e) })
                        }
                    }
                };
                let a = side(self, kids[0])?;
                let b = side(self, kids[2])?;
                Ok(Expr::new(ExprKind::Coalesce(bx(a), bx(b)), ty, line))
            }
            "orExpr" | "andExpr" => {
                if kids.len() == 1 {
                    return self.propagate(kids[0]);
                }
                let mut v = self.expr_c(kids[0])?;
                let mut i = 1;
                while i < kids.len() {
                    let r = self.expr_c(kids[i + 1])?;
                    v = Expr::new(if n.name() == "orExpr" { ExprKind::Or(bx(v), bx(r)) } else { ExprKind::And(bx(v), bx(r)) }, boolean(), line);
                    i += 2;
                }
                Ok(v)
            }
            "bitOrExpr" | "bitXorExpr" | "bitAndExpr" | "shiftExpr" | "additiveExpr" | "multiplicativeExpr" | "equalityExpr" | "relationalExpr" | "constExpr" => {
                if kids.len() == 1 {
                    return self.propagate(kids[0]);
                }
                let mut v = self.expr_c(kids[0])?;
                let mut i = 1;
                while i < kids.len() {
                    let op = kids[i].tok().map(|t| t.text.clone()).unwrap_or_default();
                    let r = self.expr_c(kids[i + 1])?;
                    let bop = BinOp::from_symbol(&op).ok_or_else(|| BuildError { line, message: format!("operator {op}") })?;
                    // the checker types a constant expression by its first term only: arithmetic falls back to its operands
                    let rt = if bop.is_comparison() {
                        boolean()
                    } else if i + 2 >= kids.len() && known(&value_ty(&ty)) {
                        value_ty(&ty)
                    } else if known(&value_ty(&v.ty)) {
                        value_ty(&v.ty)
                    } else {
                        value_ty(&r.ty)
                    };
                    v = Expr::new(ExprKind::Binary(bop, bx(v), bx(r)), rt, line);
                    i += 2;
                }
                Ok(v)
            }
            "unaryExpr" => {
                if let C::T(op) = kids[0] {
                    let v = self.expr_c(kids[1])?;
                    return Ok(match op.text.as_str() {
                        // a negation the checker did not type (a test row) has the type of its operand
                        "-" => {
                            let nt = if known(&value_ty(&ty)) { value_ty(&ty) } else { value_ty(&v.ty) };
                            Expr::new(ExprKind::Unary(UnOp::Neg, bx(v)), nt, line)
                        }
                        "!" => Expr::new(ExprKind::Unary(UnOp::Not, bx(v)), boolean(), line),
                        "await" => Expr::new(ExprKind::Await(bx(v)), ty, line),
                        o => return err(line, format!("unary {o}")),
                    });
                }
                self.propagate(kids[0])
            }
            "markerExpr" => self.marker(n),
            "postfixExpr" => self.postfix_expr(n),
            "primaryExpr" => self.primary(n),
            "constTerm" => self.const_term(n),
            "literal" => self.literal_tok(n.tokens()[0], Some(ty)),
            "place" => self.place_expr(n, Use::Read),
            "lambda" => self.lambda(n),
            "matchExpr" => {
                let (scrutinee, arms) = self.match_parts(n, true)?;
                Ok(Expr::new(ExprKind::Match { scrutinee: bx(scrutinee), arms }, ty, line))
            }
            "ifExpr" => self.if_expr(n),
            "block" => {
                let b = self.block(n)?;
                Ok(Expr::new(ExprKind::Block(b), ty, line))
            }
            "tupleLit" => {
                let mut es = Vec::new();
                for e in n.child_nodes("expr") {
                    es.push(self.expr_moved(e)?);
                }
                Ok(Expr::new(ExprKind::Tuple(es), ty, line))
            }
            "recordLit" => {
                let keys: Vec<String> = kids.iter().filter(|k| k.is("ident")).map(|k| k.node().unwrap().tokens()[0].text.clone()).collect();
                let keys = if keys.is_empty() { kids.iter().filter_map(|k| k.tok()).filter(|t| matches!(t.kind, Kind::Lower | Kind::Upper | Kind::Kw)).map(|t| t.text.clone()).collect() } else { keys };
                let mut fields = Vec::new();
                for (k, e) in keys.iter().zip(n.child_nodes("expr")) {
                    fields.push((k.clone(), self.expr_moved(e)?));
                }
                // a record the checker did not type (a test row) has the type of its fields
                let ty = if known(&ty) { ty } else { Type::Record(fields.iter().map(|(k, v)| (k.clone(), value_ty(&v.ty))).collect()) };
                Ok(Expr::new(ExprKind::Record(fields), ty, line))
            }
            "rangeExpr" => err(line, "a range outside `for`"),
            other => {
                if kids.len() == 1 {
                    return self.propagate(kids[0]);
                }
                err(line, format!("expression {other}"))
            }
        }
    }

    // ---- outcomes --------------------------------------------------------------------------------------------

    fn outcome(&mut self, n: N<'a>) -> BResult<Expr> {
        let kids = n.kids();
        if kids.len() == 1 {
            return self.propagate(kids[0]);
        }
        let line = line_of(n);
        let ty = self.node_ty(n);
        let words: Vec<String> = kids.iter().filter_map(|k| k.tok()).map(|t| t.text.clone()).collect();
        // a consuming position owns the selected success, so that operand is not also a statement temporary (7.9)
        let consuming = self.consuming;
        let owned = |b: &mut Self, c: C<'a>| -> BResult<Expr> {
            match c {
                C::N(node) => b.with_consume(node, consuming),
                C::T(t) => {
                    let e = b.literal_tok(t, None)?;
                    Ok(if consuming { e } else { b.hold(e) })
                }
            }
        };
        let v = owned(self, kids[0])?;
        if words.len() >= 2 && words[0] == "or" && words[1] == "fail" {
            return Ok(Expr::new(ExprKind::OrFail(bx(v)), ty, line));
        }
        if words.len() >= 2 && words[0] == "or" && words[1] == "recover" {
            let r = owned(self, *kids.last().unwrap())?;
            return Ok(Expr::new(ExprKind::OrRecover(bx(v), bx(r)), ty, line));
        }
        if words.first().map_or(false, |w| w == "catch") {
            let ident = kids.iter().filter_map(|k| k.tok()).find(|t| t.kind == Kind::Lower).unwrap().text.clone();
            self.fb().push_scope();
            let bind = self.fb().declare(&ident, named("Failure"), false, LocalKind::Binding);
            let handler = self.block(n.find("block").unwrap());
            self.fb().pop_scope();
            let handler = handler?;
            return Ok(Expr::new(ExprKind::Catch { value: bx(v), bind, handler }, ty, line));
        }
        err(line, "outcome expression")
    }

    // ---- names and places -----------------------------------------------------------------------------------

    /// Resolve a name to a local of the current function, capturing it from enclosing functions when the current
    /// function is a lambda without a capture list (by reference: the local moves to a shared cell).
    pub fn resolve(&mut self, name: &str, line: u32) -> Option<LocalId> {
        let d = self.fbs.len() - 1;
        self.resolve_at(d, name, line)
    }

    fn resolve_at(&mut self, d: usize, name: &str, line: u32) -> Option<LocalId> {
        if let Some(l) = self.fbs[d].lookup(name) {
            return Some(l);
        }
        let explicit = match &self.fbs[d].lambda {
            Some(l) => l.explicit,
            None => return None,
        };
        if explicit || d == 0 {
            return None;
        }
        let outer = self.resolve_at(d - 1, name, line)?;
        let (ty, mutable) = {
            let od = &mut self.fbs[d - 1].locals[outer as usize];
            od.boxed = true;
            (od.ty.clone(), od.mutable)
        };
        let fb = &mut self.fbs[d];
        let cl = fb.add_local(name, ty.clone(), mutable, LocalKind::Capture);
        fb.locals[cl as usize].boxed = true;
        fb.captures.push(CaptureDef { local: cl, name: name.to_string(), mode: CaptureMode::Implicit });
        fb.capture_values.push(Expr::new(ExprKind::Place(Place::local(outer), Use::Alias), ty, line));
        fb.scopes[0].push((name.to_string(), cl));
        Some(cl)
    }

    pub fn local_ty(&self, l: LocalId) -> Type {
        self.fbs.last().unwrap().locals[l as usize].ty.clone()
    }

    /// The type of a member of a value of type `t`: a field, a variant payload field, a tuple item.
    pub fn member_ty(&mut self, t: &Type, member: &str) -> Type {
        let s = value_ty(t);
        if let Type::Tuple(ts) = &s {
            if let Ok(i) = member.parse::<usize>() {
                return ts.get(i).cloned().unwrap_or_else(|| unknown("tuple"));
            }
        }
        if let Type::Record(fs) = &s {
            return fs.iter().find(|(n, _)| n == member).map(|(_, t)| t.clone()).unwrap_or_else(|| unknown("record"));
        }
        if let Some(d) = self.ty.decl_of(&s) {
            if let Some((ft, _, _)) = d.fields.get(member) {
                let mut m = BTreeMap::new();
                if let Type::Named(nt) = &s {
                    for (p, a) in d.type_params.iter().zip(nt.args.iter()) {
                        m.insert(p.clone(), a.clone());
                    }
                }
                return self.ty.subst(ft, &m);
            }
            if let Some(p) = d.payload.get(member) {
                return p.clone();
            }
        }
        unknown("member")
    }

    /// A `place` node: its root local and projections, and the place's type.
    pub fn place_of(&mut self, pl: N<'a>) -> BResult<(Place, Type)> {
        let ts = pl.tokens();
        let line = tok_line(ts[0]);
        let root = &ts[0].text;
        let local = self.resolve(root, line).ok_or_else(|| BuildError { line, message: format!("`{root}` is not a local") })?;
        let mut ty = self.local_ty(local);
        let mut proj = Vec::new();
        let mut i = 1;
        while i < ts.len() {
            let t = ts[i];
            if t.text == "." {
                let m = ts[i + 1];
                if m.kind == Kind::Int {
                    let k: u32 = m.text.parse().unwrap_or(0);
                    ty = self.member_ty(&ty, &m.text);
                    proj.push(Proj::Tuple(k));
                } else {
                    ty = self.member_ty(&ty, &m.text);
                    proj.push(Proj::Field(m.text.clone()));
                }
                i += 2;
            } else if t.text == "[" {
                let k: u32 = ts[i + 1].text.replace('_', "").parse().unwrap_or(0);
                ty = match value_ty(&ty) {
                    Type::Named(n) if !n.args.is_empty() => n.args[0].clone(),
                    Type::Tuple(ts2) => ts2.get(k as usize).cloned().unwrap_or_else(|| unknown("")),
                    _ => unknown("index"),
                };
                proj.push(Proj::Index(k));
                i += 3;
            } else {
                i += 1;
            }
        }
        Ok((Place { local, proj }, ty))
    }

    pub fn place_expr(&mut self, pl: N<'a>, u: Use) -> BResult<Expr> {
        let line = line_of(pl);
        let ts = pl.tokens();
        if ts.len() == 1 && self.resolve(&ts[0].text, line).is_none() {
            let hint = self.node_ty(pl);
            return self.name_value(ts[0], line, Some(&hint));
        }
        let (p, ty) = self.place_of(pl)?;
        Ok(Expr::new(ExprKind::Place(p, u), ty, line))
    }

    /// The type of a constant at a use: the initialiser's type when the constant is unannotated (D-54.4), else the
    /// declared type, else the type the checker recorded at this use.
    fn const_use_ty(&self, d: &DeclRef<'a>, hint: Option<&Type>) -> Type {
        self.ty
            .inferred_const(&d.module, &d.name)
            .filter(known)
            .or_else(|| d.alias.clone().filter(known))
            .or_else(|| hint.cloned().filter(known))
            .or_else(|| d.alias.clone())
            .unwrap_or_else(|| unknown("const"))
    }

    /// A name that is not a local: a module constant or a function used as a value.
    fn name_value(&mut self, t: &Token, line: u32, hint: Option<&Type>) -> BResult<Expr> {
        let name = &t.text;
        if let Some(d) = self.ty.dc.table.get(name).cloned() {
            match d.kind.as_str() {
                "const" => {
                    let ty = self.const_use_ty(&d, hint);
                    return Ok(Expr::new(ExprKind::Global(self.qualified(&self.ir_mod(&d), &d.name)), ty, line));
                }
                "func" => {
                    let sig = d.sig.clone().unwrap();
                    let fty = Type::Fn { params: sig.params.iter().map(|p| p.1.clone()).collect(), ret: Box::new(sig.ret.clone()), kind: "fn".into(), modifiers: sig.modifiers.clone(), origin: String::new(), dynamic: false, identity: String::new() };
                    return match self.free_function(&d) {
                        Some(f) => Ok(Expr::new(ExprKind::FuncRef(f), fty, line)),
                        None => Ok(Expr::new(ExprKind::FuncRef(format!("intrinsic:{}", d.name)), fty, line)),
                    };
                }
                _ => {}
            }
        }
        err(line, format!("`{name}` is not a local, a constant or a function"))
    }

    // ---- primaries -------------------------------------------------------------------------------------------

    fn primary(&mut self, n: N<'a>) -> BResult<Expr> {
        let kids = n.kids();
        let line = line_of(n);
        let ty = self.node_ty(n);
        match kids[0] {
            C::T(first) => {
                if first.text == "self" {
                    let l = self.resolve("self", line).ok_or_else(|| BuildError { line, message: "`self` outside a method".into() })?;
                    let t = self.local_ty(l);
                    return Ok(Expr::new(ExprKind::Place(Place::local(l), Use::Read), t, line));
                }
                if first.kind == Kind::Lower {
                    if let Some(l) = self.resolve(&first.text, line) {
                        // the checker's type here carries its flow facts (a null check narrows `T?` to `T`)
                        let t = if known(&ty) { ty.clone() } else { self.local_ty(l) };
                        return Ok(Expr::new(ExprKind::Place(Place::local(l), Use::Read), t, line));
                    }
                    return self.name_value(first, line, Some(&ty));
                }
                match first.text.as_str() {
                    "null" => Ok(Expr::new(ExprKind::Const(Const::Null), ty, line)),
                    "present" => {
                        // `present(v)` forwards `v`. A consuming position owns that value (7.9).
                        let v = self.with_consume(n.child_node("expr").unwrap(), self.consuming)?;
                        Ok(Expr::new(ExprKind::Present(bx(v)), ty, line))
                    }
                    "fail" => {
                        let v = self.expr_moved(n.child_node("expr").unwrap())?;
                        Ok(Expr::new(ExprKind::Fail(bx(v)), never(), line))
                    }
                    "(" => self.propagate(C::N(n.child_node("expr").unwrap())),
                    _ => self.literal_tok(first, Some(ty)),
                }
            }
            C::N(f) => match f.name() {
                "literal" => self.literal_tok(f.tokens()[0], Some(ty)),
                "typeRef" => {
                    let member = kids[1..].iter().find_map(|k| match k {
                        C::N(m) if m.name() == "memberName" => Some(m.tokens()[0].clone()),
                        C::T(t) if t.kind == Kind::Upper => Some((*t).clone()),
                        _ => None,
                    });
                    let called = kids.iter().any(|k| k.text_is("("));
                    self.type_ref(f, member, n.child_node("argList"), called, ty, line)
                }
                // a record, a tuple, a match or an if in a consuming position is owned by that position (7.9).
                // Forcing `expr` here held it as a statement temporary as well as binding it.
                _ => self.with_consume(f, self.consuming),
            },
        }
    }

    fn const_term(&mut self, n: N<'a>) -> BResult<Expr> {
        let kids = n.kids();
        let line = line_of(n);
        let ty = self.node_ty(n);
        match kids[0] {
            C::T(t) if t.text == "-" => {
                let v = self.expr_c(kids[1])?;
                let vt = v.ty.clone();
                Ok(Expr::new(ExprKind::Unary(UnOp::Neg, bx(v)), value_ty(&vt), line))
            }
            C::T(t) if t.text == "(" => self.propagate(kids[1]),
            C::T(t) if t.text == "null" => Ok(Expr::new(ExprKind::Const(Const::Null), ty, line)),
            C::T(t) if t.text == "{" => {
                let keys: Vec<String> = kids.iter().filter(|k| k.is("ident")).map(|k| k.node().unwrap().tokens()[0].text.clone()).collect();
                let mut fields = Vec::new();
                for (k, e) in keys.iter().zip(n.child_nodes("constExpr")) {
                    fields.push((k.clone(), self.expr_moved(e)?));
                }
                Ok(Expr::new(ExprKind::Record(fields), ty, line))
            }
            C::T(t) if t.kind == Kind::Lower => {
                let path: Vec<String> = n.tokens().iter().filter(|x| x.text != ".").map(|x| x.text.clone()).collect();
                if path.len() == 1 {
                    if let Some(l) = self.resolve(&path[0], line) {
                        let lt = self.local_ty(l);
                        return Ok(Expr::new(ExprKind::Place(Place::local(l), Use::Read), lt, line));
                    }
                    return self.name_value(t, line, Some(&ty));
                }
                let q = path.join(".");
                if let Some(d) = self.ty.dc.table.get(&q).cloned() {
                    if d.kind == "const" {
                        let cty = self.const_use_ty(&d, Some(&ty));
                        return Ok(Expr::new(ExprKind::Global(self.qualified(&self.ir_mod(&d), d.name.rsplit('.').next().unwrap())), cty, line));
                    }
                }
                err(line, format!("constant `{q}`"))
            }
            C::T(t) => self.literal_tok(t, Some(ty)),
            C::N(f) => match f.name() {
                "literal" => self.literal_tok(f.tokens()[0], Some(ty)),
                "typeRef" => {
                    let member = kids[1..].iter().find_map(|k| match k {
                        C::N(m) if m.name() == "memberName" => Some(m.tokens()[0].clone()),
                        C::T(t) if t.kind == Kind::Upper => Some((*t).clone()),
                        _ => None,
                    });
                    let called = kids.iter().any(|k| k.text_is("("));
                    self.type_ref(f, member, n.child_node("constArgList").or_else(|| n.child_node("argList")), called, ty, line)
                }
                _ => self.with_consume(f, self.consuming),
            },
        }
    }

    /// `Type`, `Type.member`, `Type(args)`, `Type.member(args)`, `alias.Type...`, `Int<0..9>.validate(x)`.
    fn type_ref(&mut self, tr: N<'a>, member: Option<Token>, al: Option<N<'a>>, called: bool, ty: Type, line: u32) -> BResult<Expr> {
        let t = self.ty.dc.type_of_ref(tr);
        let mut d = self.ty.decl_of(&t);
        let rts = tr.direct_tokens();
        if rts.first().map(|tk| tk.kind) == Some(Kind::Lower) && rts.len() > 2 {
            // `alias.Type` is the declaration the alias reaches, not a program type of the same bare name (D-142).
            // A core type and a single-file type share the empty identity, so bare lookup cannot tell them apart.
            if let Some(dq) = self.ty.dc.table.get(&format!("{}.{}", rts[0].text, rts[2].text)).cloned() {
                if dq.kind != "unknown" {
                    d = Some(dq);
                }
            }
        }
        let tname = tr.tokens().into_iter().find(|x| x.kind == Kind::Upper).map(|x| x.text.clone()).unwrap_or_default();
        // a bare variant name typed by its expected enum
        if d.is_none() && member.is_none() {
            if let Some(ed) = self.ty.decl_of(&ty) {
                if ed.kind == "enum" && ed.variants.contains(&tname) {
                    let fields = self.variant_fields(&ed, &tname, al)?;
                    let ir = self.enum_ir_name(&ed);
                    return Ok(Expr::new(ExprKind::Variant { ty: ir.clone(), variant: tname, fields }, self.rename_shadow(&ty, &ir), line));
                }
            }
        }
        let d = match d {
            Some(d) => d,
            None => return err(line, format!("type `{tname}` has no declaration")),
        };
        if let Some(m) = &member {
            if (m.text == "fromText" || m.text == "fromBytes") && called {
                // `T.fromText(s)`: decoded by the referenced type, with its arguments and refinement (6.10)
                let arg = al.and_then(|a| arg_nodes(a).into_iter().next()).and_then(|a| a.child_node("expr").or_else(|| a.child_node("constExpr")));
                let v = match arg {
                    Some(e) => self.expr(e)?,
                    None => return err(line, format!("`{}.{}` without an argument", d.name, m.text)),
                };
                let op = CodecOp::of(&m.text).unwrap();
                return Ok(Expr::new(ExprKind::Codec { op, target: t.clone(), arg: bx(v) }, ty, line));
            }
            if m.text == "validate" && called {
                let arg = al.and_then(|a| arg_nodes(a).into_iter().next()).and_then(|a| a.child_node("expr").or_else(|| a.child_node("constExpr")));
                let v = match arg {
                    Some(e) => self.expr(e)?,
                    None => return err(line, "`validate` without an argument"),
                };
                let refined = match (&t, d.kind.as_str()) {
                    (Type::Refined { .. }, _) => Some(t.clone()),
                    (_, "type") => d.alias.clone(),
                    _ => None,
                };
                if let Some(Type::Refined { base, lo, hi, .. }) = &refined {
                    let length = base.name != "Int" && !is_int_type(&base.name);
                    return Ok(Expr::new(ExprKind::Validate { value: bx(v), lo: *lo, hi: *hi, length }, ty, line));
                }
                if d.kind == "type" {
                    return Ok(Expr::new(ExprKind::Validate { value: bx(v), lo: None, hi: None, length: false }, ty, line));
                }
            }
        }
        let member = match member {
            None => {
                // `Void` is the unit type's only value (`recover Void`)
                if !called && d.name == "Void" {
                    return Ok(Expr::void(line));
                }
                if !called {
                    return err(line, format!("type `{}` used as a value", d.name));
                }
                return match d.kind.as_str() {
                    "class" => self.construct(&d, al, ty, line),
                    "type" => {
                        let a = al.and_then(|a| arg_nodes(a).into_iter().next()).and_then(|a| a.child_node("expr").or_else(|| a.child_node("constExpr")));
                        let v = match a {
                            Some(e) => self.expr(e)?,
                            None => return err(line, "a newtype without its value"),
                        };
                        Ok(Expr::new(ExprKind::Wrap { ty: self.tname(&d.name, &d.type_identity()), value: bx(v) }, ty, line))
                    }
                    "error" => {
                        let mut fields = Vec::new();
                        let payload: Vec<String> = d.payload.keys().cloned().collect();
                        for (i, a) in al.map(arg_nodes).unwrap_or_default().into_iter().enumerate() {
                            let at = a.direct_tokens();
                            let name = if at.len() > 1 && at[1].text == ":" { at[0].text.clone() } else { payload.get(i).cloned().unwrap_or_default() };
                            let want = d.payload.get(&name).cloned().unwrap_or_else(|| unknown(""));
                            let v = self.expr(a.child_node("expr").or_else(|| a.child_node("constExpr")).unwrap())?;
                            fields.push((name, self.coerce(v, &want)));
                        }
                        Ok(Expr::new(ExprKind::MakeFailure { ty: self.tname(&d.name, &d.type_identity()), fields }, ty, line))
                    }
                    k => err(line, format!("call of {k} `{}`", d.name)),
                };
            }
            Some(m) => m,
        };
        let m = member.text.clone();
        if d.kind == "enum" && d.variants.contains(&m) {
            // an uncalled variant whose type is a function is the constructor (LR 9.7; D-54.5): a zero-capture
            // function the VM and native can call. A call, or a use as the enum value, stays a variant.
            if !called {
                if let Type::Fn { .. } = &ty {
                    return self.variant_as_function(&d, &m, &ty, &member, line);
                }
            }
            let fields = self.variant_fields(&d, &m, al)?;
            // a variant of a generic enum takes its type arguments from the type reference (`Level[Int].Low`)
            let vty = if known(&ty) { ty } else { t.clone() };
            // a standard enum the program also declares is `Name@module`, not the program's enum (D-142)
            let ir = self.enum_ir_name(&d);
            let vty = self.rename_shadow(&vty, &ir);
            return Ok(Expr::new(ExprKind::Variant { ty: ir, variant: m, fields }, vty, line));
        }
        if let Some(sig) = d.methods.get(&m).cloned() {
            if !called {
                return match self.member_function(&d, &m, &sig) {
                    Some(f) => Ok(Expr::new(ExprKind::FuncRef(f), ty, line)),
                    // an associated intrinsic kept as a value (D-142): a closure the native lowering can call
                    None if sig.receiver.is_none() => self.intrinsic_as_function(&d, &m, &sig, &ty, &member, line),
                    None => Ok(Expr::new(ExprKind::FuncRef(format!("intrinsic:{}.{m}", d.name)), ty, line)),
                };
            }
            let mut subst: BTreeMap<String, Type> = BTreeMap::new();
            subst.insert("Self".into(), named_of_decl(&d, vec![]));
            if let Type::Named(tn) = &t {
                for (p, a) in d.type_params.iter().zip(tn.args.iter()) {
                    subst.insert(p.clone(), a.clone());
                }
            }
            let mut args = self.call_args(&sig, al, &subst, line)?;
            let callee = match self.member_function(&d, &m, &sig) {
                Some(f) => Callee::Func(f),
                None if super::is_kit_module(&d.module) => Callee::Intrinsic(format!("kit:{}:{}", d.module, d.name)),
                None => Callee::Intrinsic(format!("{}.{m}", d.name)),
            };
            if d.name == "Array" && m == "filled" {
                // the size of `Array[T, N].filled(v)` comes from the type (6.7)
                let n = match &t {
                    Type::Named(tn) => tn.args.get(1).and_then(|a| a.show().trim_start_matches('#').parse::<i128>().ok()),
                    _ => None,
                };
                let n = n.ok_or_else(|| BuildError { line, message: "Array.filled without a size in its type".into() })?;
                args.push(Arg { param: "#size".into(), mode: Mode::Read, value: Expr::new(ExprKind::Const(Const::Int(n)), int(), line) });
            }
            return Ok(Expr::new(ExprKind::Call { callee, args }, ty, line));
        }
        if d.constants.get(&m).is_some() {
            return Ok(Expr::new(ExprKind::Global(self.qualified(&self.ir_mod(&d), &format!("{}.{m}", d.name))), ty, line));
        }
        // `T.default()` of a class deriving Default: every field at its default (9.2)
        if m == "default" && called && d.kind == "class" && d.derives.iter().any(|(n, _)| n == "Default") {
            return self.construct(&d, None, ty, line);
        }
        err(line, format!("`{}.{m}`", d.name))
    }

    /// `Enum.Variant` used as a function value (LR 9.7): a pure function of the payload fields that returns the variant.
    fn variant_as_function(&mut self, d: &DeclRef<'a>, vname: &str, ty: &Type, at: &Token, line: u32) -> BResult<Expr> {
        let payload = d.variants.get(vname).cloned().unwrap_or_default();
        let (params, ret) = match ty {
            Type::Fn { params, ret, .. } => (params.clone(), (**ret).clone()),
            _ => return err(line, format!("`{}.{vname}` is not a function", d.name)),
        };
        // the constructor's result is the standard enum, even when the program declares the same bare name (D-142)
        let ir = self.enum_ir_name(d);
        let ret = self.rename_shadow(&ret, &ir);
        let label = format!("variant {}.{vname}@{}:{}", d.name, at.line, at.col);
        let mut fb = Fb::new(&label, &self.ir_module, ret.clone(), false);
        // an explicit, empty capture list: the constructor closes over nothing
        fb.lambda = Some(LambdaCtx { kind: "fn".into(), explicit: true });
        let mut fields = Vec::new();
        let mut copies = false;
        for (i, (fname, raw)) in payload.iter().enumerate() {
            let ft = params.get(i).cloned().unwrap_or_else(|| raw.clone());
            let l = fb.add_local(fname, ft.clone(), false, LocalKind::Param);
            fb.params.push(Param { local: l, name: fname.clone(), mode: Mode::Read, default: None });
            fb.bind(fname, l);
            // a non-trivial payload is copied in: the parameter of `fn(String)` is a read loan (`&str`), and the
            // variant field wants the value (LR 9.7). A trivial payload is read, so an `Int` constructor copies nothing.
            // The copy allocates, so the constructor's summary says `alloc` (C1.copy).
            let use_of = if self.ty.dc.is_trivial(&ft) { Use::Read } else { copies = true; Use::Copy };
            fields.push((fname.clone(), Expr::new(ExprKind::Place(Place::local(l), use_of), ft, line)));
        }
        let vexpr = Expr::new(ExprKind::Variant { ty: ir.clone(), variant: vname.to_string(), fields }, ret, line);
        let body = Block { stmts: vec![Stmt::new(line, StmtKind::Return(Some(vexpr)))], locals: vec![], line, defers: vec![] };
        let name = fb.name.clone();
        let effects = if copies { Effects { effects: vec![("alloc".into(), "proved".into())], ..Effects::default() } } else { Effects::default() };
        self.prog.functions.push(fb.finish(line, Some(body), vec!["pure".into()], effects, None, Some(ClosureDef { kind: "fn".into(), captures: vec![] })));
        Ok(Expr::new(ExprKind::Closure { func: name, captures: vec![] }, self.rename_fn_ret(ty, &ir), line))
    }

    /// An associated intrinsic used as a function value (`clock.Duration.seconds`): a zero-capture function whose
    /// body is that intrinsic, so the VM and native call the core method rather than a `FuncRef` native cannot name.
    fn intrinsic_as_function(&mut self, d: &DeclRef<'a>, m: &str, sig: &Sig<'a>, ty: &Type, at: &Token, line: u32) -> BResult<Expr> {
        let fallback = Expr::new(ExprKind::FuncRef(format!("intrinsic:{}.{m}", d.name)), ty.clone(), line);
        let (params, ret) = match ty {
            Type::Fn { params, ret, .. } => (params.clone(), (**ret).clone()),
            _ => return Ok(fallback),
        };
        if params.len() != sig.params.len() {
            return Ok(fallback);
        }
        // the closure's Rust types are the core method's, even when the program declares the same names
        let ret = mark_standard(&ret, &d.module);
        let label = format!("intrinsic {}.{m}@{}:{}", d.name, at.line, at.col);
        let mut fb = Fb::new(&label, &self.ir_module, ret.clone(), false);
        fb.lambda = Some(LambdaCtx { kind: "fn".into(), explicit: true });
        let mut args = Vec::new();
        for (i, (pname, pty, _)) in sig.params.iter().enumerate() {
            let ft = mark_standard(&params.get(i).cloned().unwrap_or_else(|| pty.clone()), &d.module);
            let l = fb.add_local(pname, ft.clone(), false, LocalKind::Param);
            fb.params.push(Param { local: l, name: pname.clone(), mode: Mode::Read, default: None });
            fb.bind(pname, l);
            args.push(Arg { param: pname.clone(), mode: Mode::Read, value: Expr::new(ExprKind::Place(Place::local(l), Use::Read), ft, line) });
        }
        let callee = if super::is_kit_module(&d.module) {
            Callee::Intrinsic(format!("kit:{}:{}", d.module, d.name))
        } else {
            Callee::Intrinsic(format!("{}.{}", d.name, m))
        };
        let call = Expr::new(ExprKind::Call { callee, args }, ret, line);
        let body = Block { stmts: vec![Stmt::new(line, StmtKind::Return(Some(call)))], locals: vec![], line, defers: vec![] };
        let name = fb.name.clone();
        let effects = self.effects_of(&format!("method {}.{}", d.name, m));
        self.prog.functions.push(fb.finish(line, Some(body), vec!["pure".into()], effects, None, Some(ClosureDef { kind: "fn".into(), captures: vec![] })));
        Ok(Expr::new(ExprKind::Closure { func: name, captures: vec![] }, ty.clone(), line))
    }

    fn variant_fields(&mut self, d: &DeclRef<'a>, vname: &str, al: Option<N<'a>>) -> BResult<Vec<(String, Expr)>> {
        let payload = d.variants.get(vname).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for (i, a) in al.map(arg_nodes).unwrap_or_default().into_iter().enumerate() {
            let at = a.direct_tokens();
            let name = if at.len() > 1 && at[1].text == ":" { at[0].text.clone() } else { payload.get(i).map(|p| p.0.clone()).unwrap_or_default() };
            let want = payload.iter().find(|p| p.0 == name).map(|p| p.1.clone()).unwrap_or_else(|| unknown(""));
            let e = a.child_node("expr").or_else(|| a.child_node("constExpr")).unwrap();
            let v = self.expr_moved(e)?;
            out.push((name, self.coerce(v, &want)));
        }
        Ok(out)
    }

    /// A class value: the arguments in source order, then a spread's remaining fields, then defaults (9.2).
    fn construct(&mut self, d: &DeclRef<'a>, al: Option<N<'a>>, ty: Type, line: u32) -> BResult<Expr> {
        let ty = if known(&ty) { ty } else { named_of_decl(d, vec![]) };
        let names: Vec<String> = d.fields.keys().cloned().collect();
        let mut given: Vec<(String, Expr)> = Vec::new();
        let mut spread: Option<Expr> = None;
        let mut spread_move = false;
        let mut pos = 0usize;
        for a in al.map(arg_nodes).unwrap_or_default() {
            let at = a.direct_tokens();
            if at.first().map_or(false, |t| t.text == "...") {
                let marker = at.get(1).map(|t| t.text.clone()).unwrap_or_default();
                let pl = a.child_node("place").unwrap();
                spread_move = marker == "move";
                spread = Some(self.place_expr(pl, if spread_move { Use::Move } else { Use::Copy })?);
                continue;
            }
            let name = if at.len() > 1 && at[1].text == ":" {
                at[0].text.clone()
            } else {
                let n = names.get(pos).cloned().unwrap_or_default();
                pos += 1;
                n
            };
            let want = d.fields.get(&name).map(|f| f.0.clone()).unwrap_or_else(|| unknown(""));
            let e = a.child_node("expr").or_else(|| a.child_node("constExpr")).unwrap();
            let v = self.expr_moved(e)?;
            given.push((name, self.coerce(v, &want)));
        }
        if let Some(sp) = spread {
            // the spread value is bound once and each field it supplies is moved out of it; like the interpreter, the
            // emptied shell is not destroyed, so the temporary belongs to no block
            let tmp = self.fb().add_local("spread", sp.ty.clone(), false, LocalKind::Temp);
            let st = sp.ty.clone();
            let mut fields = given;
            for f in &names {
                if fields.iter().any(|(n, _)| n == f) {
                    continue;
                }
                let ft = self.member_ty(&st, f);
                if known(&ft) {
                    fields.push((f.clone(), Expr::new(ExprKind::Place(Place::local(tmp).field(f), Use::Move), ft, line)));
                } else if let Some(ce) = self.class_default(d, f) {
                    // a field the spread source lacks takes its default (9.2)
                    let want = d.fields.get(f).unwrap().0.clone();
                    let v = self.const_expr(ce)?;
                    fields.push((f.clone(), self.coerce(v, &want)));
                }
            }
            let c = Expr::new(ExprKind::Construct { ty: self.tname(&d.name, &d.type_identity()), fields }, ty.clone(), line);
            // `...move p` consumes the whole source: the fields the target lacks are dropped at the spread, in the
            // source's declaration order, before the new value exists (9.2, 7.9). `...copy` leaves the source.
            let mut stmts = vec![Stmt { line, temps: vec![], kind: StmtKind::Let { local: tmp, init: sp } }];
            if spread_move {
                // a class source, in declaration order, or a record source, in written order (9.2). A record
                // has no declaration, so `decl_of` does not see it.
                let absent: Vec<String> = if let Some(sd) = self.ty.decl_of(&st) {
                    sd.fields.keys().filter(|f| !names.iter().any(|n| n == *f)).cloned().collect()
                } else if let Type::Record(fs) = strip(&st) {
                    fs.iter().map(|(f, _)| f.clone()).filter(|f| !names.iter().any(|n| n == f)).collect()
                } else {
                    Vec::new()
                };
                for f in absent {
                    stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Drop(Place::local(tmp).field(&f)) });
                }
            }
            stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Give(c) });
            let body = Block { stmts, locals: vec![], line, defers: vec![] };
            return Ok(Expr::new(ExprKind::Block(body), ty, line));
        }
        let mut fields = given;
        for f in &names {
            if !fields.iter().any(|(n, _)| n == f) {
                let ce = self.class_default(d, f);
                match ce {
                    Some(ce) => {
                        let ft = d.fields.get(f).unwrap().0.clone();
                        let v = self.const_expr(ce)?;
                        fields.push((f.clone(), self.coerce(v, &ft)));
                    }
                    None => return err(line, format!("{}: field `{f}` not given", d.name)),
                }
            }
        }
        Ok(Expr::new(ExprKind::Construct { ty: self.tname(&d.name, &d.type_identity()), fields }, ty, line))
    }

    fn class_default(&self, d: &DeclRef<'a>, fname: &str) -> Option<N<'a>> {
        let node = d.node?;
        for m in node.kids().into_iter().filter_map(|c| c.node()) {
            if let Some(fd) = m.find("fieldDecl") {
                if fd.direct_tokens().first().map_or(false, |t| t.text == fname) {
                    return fd.child_node("constExpr");
                }
            }
        }
        None
    }

    // ---- calls -----------------------------------------------------------------------------------------------

    fn param_mode(m: &str) -> Mode {
        match m.trim_start_matches("...") {
            "editable" => Mode::Editable,
            "owned" => Mode::Owned,
            _ => Mode::Read,
        }
    }

    /// The arguments of a call in source order, each naming its parameter, then the defaults of omitted ones.
    pub fn call_args(&mut self, sig: &Sig<'a>, al: Option<N<'a>>, subst: &BTreeMap<String, Type>, line: u32) -> BResult<Vec<Arg>> {
        let params = sig.params.clone();
        let mut out: Vec<Arg> = Vec::new();
        let args = al.map(arg_nodes).unwrap_or_default();
        for (i, a) in args.iter().enumerate() {
            let at = a.direct_tokens();
            if at.first().map_or(false, |t| t.text == "...") {
                return err(line, "a spread argument outside a constructor");
            }
            let e = a.child_node("expr").or_else(|| a.child_node("constExpr")).unwrap();
            let p = if at.len() > 1 && at[0].kind == Kind::Lower && at[1].text == ":" {
                params.iter().find(|p| p.0 == at[0].text).cloned()
            } else if !params.is_empty() && params.last().unwrap().2.starts_with("...") && i + 1 >= params.len() {
                params.last().cloned()
            } else {
                params.get(i).cloned()
            };
            let (pname, pty, pmode) = match p {
                Some(p) => p,
                None => return err(line, "an argument without a parameter"),
            };
            let want = self.ty.subst(&pty, subst);
            let mode = Self::param_mode(&pmode);
            let v = if mode == Mode::Owned { self.expr_moved(e)? } else { self.expr(e)? };
            out.push(Arg { param: pname, mode, value: self.coerce(v, &want) });
        }
        // omitted parameters take their defaults, evaluated after the given arguments
        let defaults = self.sig_defaults(sig);
        for (pname, pty, pmode) in &params {
            if pmode.starts_with("...") || out.iter().any(|a| &a.param == pname) {
                continue;
            }
            if let Some(ce) = defaults.iter().find(|(n, _)| n == pname).map(|(_, c)| *c) {
                let want = self.ty.subst(pty, subst);
                let mode = Self::param_mode(pmode);
                let v = if mode == Mode::Owned { self.expr_moved(ce)? } else { self.const_expr(ce)? };
                out.push(Arg { param: pname.clone(), mode, value: self.coerce(v, &want) });
            }
        }
        Ok(out)
    }

    fn sig_defaults(&self, sig: &Sig<'a>) -> Vec<(String, N<'a>)> {
        let mut out = Vec::new();
        if let Some(head) = sig.node {
            if let Some(pl) = head.find("paramList") {
                for p in pl.child_nodes("param") {
                    if let (Some(pn), Some(ce)) = (p.direct_tokens().first(), p.child_node("constExpr")) {
                        out.push((pn.text.clone(), ce));
                    }
                }
            }
        }
        out
    }

    /// The signature of `m` on a receiver of type `s` and the declaration that provides it.
    fn find_method(&self, s: &Type, m: &str) -> Option<(DeclRef<'a>, Sig<'a>, bool)> {
        let d = self.ty.decl_of(s)?;
        if let Some(sig) = d.methods.get(m) {
            return Some((d.clone(), sig.clone(), false));
        }
        for mx in &d.mixins {
            if let Some(md) = self.ty.dc.table.get(&mx.name) {
                if let Some(sig) = md.methods.get(m) {
                    return Some((d.clone(), sig.clone(), true));
                }
            }
        }
        for i in &d.implements {
            if let Some(id) = self.ty.dc.table.get(&i.name) {
                if let Some(sig) = id.methods.get(m) {
                    return Some((d.clone(), sig.clone(), false));
                }
            }
        }
        // an owner type offers its view's methods (String: StringView, List: ListView, Bytes: BytesView)
        let view = match d.name.as_str() {
            "String" => "StringView",
            "List" => "ListView",
            "Bytes" => "BytesView",
            _ => return None,
        };
        let vd = self.ty.dc.table.get(view)?;
        vd.methods.get(m).map(|sig| (vd.clone(), sig.clone(), false))
    }

    /// A method call on a receiver value. `recv_use` is the receiver's marker, when it has one.
    pub fn method_call(&mut self, recv: Expr, mtok: &Token, al: Option<N<'a>>, ty: Type, line: u32) -> BResult<Expr> {
        let m = mtok.text.clone();
        let mut s = value_ty(&recv.ty);
        if let Type::Named(nt) = &s {
            if nt.name == "Shared" && !nt.args.is_empty() && m != "retain" && m != "downgrade" {
                s = value_ty(&nt.args[0]);
            }
        }
        let recv_arg = |mode: Mode, recv: Expr, b: &mut Self| -> Arg {
            let value = match recv.kind {
                ExprKind::Place(p, Use::Read) => {
                    let u = match mode {
                        Mode::Editable => Use::Editable,
                        Mode::Owned => Use::Move,
                        Mode::Read => Use::Read,
                    };
                    Expr { kind: ExprKind::Place(p, u), ..recv }
                }
                _ if mode == Mode::Owned => b.take_owned(recv),
                _ => recv,
            };
            Arg { param: "self".into(), mode, value }
        };
        // a type parameter or an interface: dispatched on the receiver's type at run time
        let dynamic = matches!(&s, Type::TypeParam(_) | Type::Dynamic(_));
        if dynamic {
            let iface = match &s {
                Type::Dynamic(n) => self.ty.dc.table.get(&n.name).cloned(),
                _ => None,
            };
            let sig = iface.as_ref().and_then(|d| d.methods.get(&m).cloned()).or_else(|| self.any_interface_method(&m));
            let (mode, args) = match &sig {
                Some(sig) => (Self::recv_mode(sig), self.call_args(sig, al, &BTreeMap::new(), line)?),
                None => (Mode::Read, self.untyped_args(al)?),
            };
            let mut all = vec![recv_arg(mode, recv, self)];
            all.extend(args);
            return Ok(Expr::new(ExprKind::Call { callee: Callee::Virtual { method: m }, args: all }, ty, line));
        }
        let (d, sig, from_mixin) = match self.find_method(&s, &m) {
            Some(x) => x,
            None => {
                // `v.toText()`/`v.toBytes()`: the canonical codec on any Encodable value (6.10)
                if let Some(op @ (CodecOp::ToText | CodecOp::ToBytes)) = CodecOp::of(&m) {
                    return Ok(Expr::new(ExprKind::Codec { op, target: s.clone(), arg: bx(recv) }, ty, line));
                }
                return err(line, format!("no method `{m}` on {}", s.show()));
            }
        };
        let mut subst = self.ty.recv_subst(&s, &d);
        subst.insert("Self".into(), s.clone());
        let mode = Self::recv_mode(&sig);
        let args = self.call_args(&sig, al, &subst, line)?;
        let callee = if from_mixin {
            Callee::Func(self.qualified(&self.ir_mod(&d), &format!("method {}.{m}", d.name)))
        } else {
            match self.member_function(&d, &m, &sig) {
                Some(f) => Callee::Func(f),
                None => Callee::Intrinsic(format!("{}.{m}", d.name)),
            }
        };
        let mut all = Vec::new();
        if sig.receiver.is_some() {
            all.push(recv_arg(mode, recv, self));
        }
        all.extend(args);
        Ok(Expr::new(ExprKind::Call { callee, args: all }, ty, line))
    }

    fn any_interface_method(&self, m: &str) -> Option<Sig<'a>> {
        self.ty.dc.table.decls.values().filter(|d| d.kind == "interface").find_map(|d| d.methods.get(m).cloned())
    }

    fn recv_mode(sig: &Sig<'a>) -> Mode {
        match sig.receiver.as_deref() {
            Some("editable self") => Mode::Editable,
            Some("owned self") => Mode::Owned,
            _ => Mode::Read,
        }
    }

    /// Arguments of a call whose parameters are positional only (a closure value): p0, p1, ...
    fn untyped_args(&mut self, al: Option<N<'a>>) -> BResult<Vec<Arg>> {
        let mut out = Vec::new();
        for (i, a) in al.map(arg_nodes).unwrap_or_default().into_iter().enumerate() {
            let e = a.child_node("expr").or_else(|| a.child_node("constExpr")).unwrap();
            let v = self.expr(e)?;
            let mode = match &v.kind {
                ExprKind::Place(_, Use::Editable) => Mode::Editable,
                ExprKind::Place(_, Use::Move) => Mode::Owned,
                _ => Mode::Read,
            };
            out.push(Arg { param: format!("p{i}"), mode, value: v });
        }
        Ok(out)
    }

    /// A call of a local whose type is `fn(...) Name@module` yields that enum, not the program's type of the bare name.
    fn shadow_call_ty(&self, f: &Expr, ty: Type) -> Type {
        let ret = match &f.kind {
            ExprKind::Place(p, _) => self.fbs.last().and_then(|fb| fb.locals.get(p.local as usize)).and_then(|l| match &l.ty {
                Type::Fn { ret, .. } => Some((**ret).clone()),
                _ => None,
            }),
            ExprKind::Closure { .. } => match &f.ty {
                Type::Fn { ret, .. } => Some((**ret).clone()),
                _ => None,
            },
            _ => None,
        };
        match ret {
            Some(ret) if nominal_shadow(&ty, &ret) => ret,
            _ => ty,
        }
    }

    /// A call of a value: a closure, a function value.
    fn call_value(&mut self, f: Expr, al: Option<N<'a>>, ty: Type, line: u32) -> BResult<Expr> {
        let args = self.untyped_args(al)?;
        if let ExprKind::FuncRef(name) = &f.kind {
            if let Some(intr) = name.strip_prefix("intrinsic:") {
                return Ok(Expr::new(ExprKind::Call { callee: Callee::Intrinsic(intr.to_string()), args }, ty, line));
            }
        }
        let ty = self.shadow_call_ty(&f, ty);
        Ok(Expr::new(ExprKind::Call { callee: Callee::Value(bx(f)), args }, ty, line))
    }

    // ---- postfix chains and markers -------------------------------------------------------------------------

    fn postfix_expr(&mut self, n: N<'a>) -> BResult<Expr> {
        let kids = n.kids();
        let line = line_of(n);
        let prim = kids[0].node().unwrap();
        let pfs: Vec<N<'a>> = kids[1..].iter().filter_map(|k| k.node()).collect();
        if pfs.is_empty() {
            return self.propagate(C::N(prim));
        }
        let pk = prim.kids();
        let mut start = 0usize;
        let mut cur: Expr;
        let first_lower = match pk.first() {
            Some(C::T(t)) if pk.len() == 1 && t.kind == Kind::Lower && self.resolve(&t.text, line).is_none() => Some((*t).clone()),
            _ => None,
        };
        if let Some(t0) = &first_lower {
            let pf0 = pfs[0];
            if self.ty.aliases.contains_key(&t0.text) {
                // `alias.member(...)`: a function of an imported module
                let mn = pf0.find("memberName").map(|m| m.tokens()[0].clone());
                let mn = match mn {
                    Some(m) => m,
                    None => return err(line, format!("module `{}` used as a value", t0.text)),
                };
                let q = format!("{}.{}", t0.text, mn.text);
                let d = self.ty.dc.table.get(&q).cloned().ok_or_else(|| BuildError { line, message: format!("`{q}` is not declared") })?;
                let ty = self.node_ty(pf0);
                cur = match d.kind.as_str() {
                    "func" => {
                        let sig = d.sig.clone().unwrap();
                        let args = self.call_args(&sig, pf0.find("argList"), &BTreeMap::new(), line)?;
                        let callee = match self.free_function(&d) {
                            Some(f) => Callee::Func(f),
                            None => Callee::Intrinsic(mn.text.clone()),
                        };
                        Expr::new(ExprKind::Call { callee, args }, ty, line)
                    }
                    "const" => {
                        let cty = self.const_use_ty(&d, Some(&ty));
                        Expr::new(ExprKind::Global(self.qualified(&self.ir_mod(&d), &mn.text)), cty, line)
                    }
                    k => return err(line, format!("module member of kind {k}")),
                };
                start = 1;
            } else if pf0.kids().first().map_or(false, |k| k.text_is("(")) {
                if let Some(d) = self.ty.dc.table.get(&t0.text).cloned() {
                    if d.kind == "func" {
                        let sig = d.sig.clone().unwrap();
                        let ty = self.node_ty(pf0);
                        let args = self.call_args(&sig, pf0.find("argList"), &BTreeMap::new(), line)?;
                        let callee = match self.free_function(&d) {
                            Some(f) => Callee::Func(f),
                            None => Callee::Intrinsic(t0.text.clone()),
                        };
                        cur = Expr::new(ExprKind::Call { callee, args }, ty, line);
                        start = 1;
                    } else {
                        cur = self.expr(prim)?;
                    }
                } else {
                    cur = self.expr(prim)?;
                }
            } else {
                cur = self.expr(prim)?;
            }
        } else {
            cur = self.expr(prim)?;
        }
        for pf in &pfs[start..] {
            // a call used as the receiver of the next postfix is a fresh value. Hold it when nothing owns it
            // yet, so `make().ping()` destroys the handle at the end of the statement (7.9). An owned method
            // takes that temporary back out.
            cur = self.hold(cur);
            cur = self.apply_postfix(cur, *pf)?;
        }
        Ok(cur)
    }

    pub fn apply_postfix(&mut self, cur: Expr, pf: N<'a>) -> BResult<Expr> {
        let line = line_of(pf);
        let ty = self.node_ty(pf);
        let k = pf.kids();
        if pf.name() == "methodCall" {
            let mn = pf.find("memberName").unwrap().tokens()[0].clone();
            return self.method_call(cur, &mn, pf.find("argList"), ty, line);
        }
        let first = k[0];
        if let C::N(mc) = first {
            if mc.name() == "methodCall" {
                let mn = mc.find("memberName").unwrap().tokens()[0].clone();
                return self.method_call(cur, &mn, mc.find("argList"), ty, line);
            }
        }
        let ft = first.tok().map(|t| t.text.clone()).unwrap_or_default();
        match ft.as_str() {
            "." => {
                let second = k[1];
                let (name, is_int) = match second {
                    C::T(t) => (t.text.clone(), t.kind == Kind::Int),
                    C::N(m) => (m.tokens()[0].text.clone(), false),
                };
                let mty = if known(&ty) { ty.clone() } else { self.member_ty(&cur.ty, &name) };
                match cur.kind {
                    ExprKind::Place(mut p, Use::Read) => {
                        p.proj.push(if is_int { Proj::Tuple(name.parse().unwrap_or(0)) } else { Proj::Field(name) });
                        Ok(Expr::new(ExprKind::Place(p, Use::Read), mty, line))
                    }
                    _ if is_int => Ok(Expr::new(ExprKind::TupleField(bx(cur), name.parse().unwrap_or(0)), mty, line)),
                    _ => Ok(Expr::new(ExprKind::Field(bx(cur), name), mty, line)),
                }
            }
            "?." => {
                let name = k[1].node().map(|m| m.tokens()[0].text.clone()).unwrap_or_default();
                Ok(Expr::new(ExprKind::OptField(bx(cur), name), ty, line))
            }
            "(" => self.call_value(cur, pf.find("argList"), ty, line),
            "[" => {
                let i = self.expr_c(k[1])?;
                Ok(Expr::new(ExprKind::Index(bx(cur), bx(i)), ty, line))
            }
            _ => err(line, "postfix"),
        }
    }

    fn marker(&mut self, n: N<'a>) -> BResult<Expr> {
        let kids = n.kids();
        let line = line_of(n);
        let ty = self.node_ty(n);
        let mk = kids[0].node().map(|m| m.tokens()[0].text.clone()).unwrap_or_default();
        let u = match mk.as_str() {
            "move" => Use::Move,
            "copy" => Use::Copy,
            "alias" => Use::Alias,
            _ => Use::Editable,
        };
        let pl = kids[1].node().unwrap();
        if kids.len() == 2 {
            return self.place_expr(pl, u);
        }
        let rest = &kids[2..];
        let mut cur: Expr;
        let mut idx = 0usize;
        if rest[0].is("methodCall") {
            let recv = self.place_expr(pl, Use::Read)?;
            let recv = Expr { kind: match recv.kind { ExprKind::Place(p, _) => ExprKind::Place(p, u), k => k }, ..recv };
            let mc = rest[0].node().unwrap();
            let mn = mc.find("memberName").unwrap().tokens()[0].clone();
            let mty = self.node_ty(mc);
            cur = self.marked_method(recv, &mn, mc.find("argList"), if rest.len() == 1 { ty.clone() } else { mty }, line, u)?;
            idx = 1;
        } else if rest[0].text_is("(") {
            let al = rest.iter().find_map(|k| if k.is("argList") { k.node() } else { None });
            let pts = pl.tokens();
            if pts.len() >= 3 && pts[pts.len() - 2].text == "." {
                // `editable p.m(...)`: the place's last member names the method; the marker applies to `p`
                let (p, pty) = self.place_of(pl)?;
                let mut recv_p = p.clone();
                recv_p.proj.pop();
                // the checker records the receiver type on the place, narrowed when a null test says so (6.8)
                let recorded = self.node_ty(pl);
                let recv_ty = if known(&recorded) { recorded } else { self.place_prefix_ty(&p, pty) };
                let recv = Expr::new(ExprKind::Place(recv_p, u), recv_ty, line);
                let mn = pts[pts.len() - 1].clone();
                cur = self.marked_method(recv, &mn, al, ty.clone(), line, u)?;
            } else {
                let f = self.place_expr(pl, u)?;
                cur = self.call_value(f, al, ty.clone(), line)?;
            }
            while idx < rest.len() && !rest[idx].is("postfix") {
                idx += 1;
            }
        } else {
            cur = self.place_expr(pl, u)?;
        }
        for k in &rest[idx..] {
            if let C::N(pf) = k {
                if pf.name() == "postfix" || pf.name() == "methodCall" {
                    cur = self.apply_postfix(cur, *pf)?;
                }
            }
        }
        Ok(cur)
    }

    /// The type of a place without its last projection (the receiver of `editable p.m(...)`).
    fn place_prefix_ty(&mut self, p: &Place, _full: Type) -> Type {
        let mut t = self.local_ty(p.local);
        for pr in &p.proj[..p.proj.len().saturating_sub(1)] {
            t = match pr {
                Proj::Field(f) => self.member_ty(&t, f),
                Proj::Tuple(i) | Proj::Index(i) => self.member_ty(&t, &i.to_string()),
            };
        }
        t
    }

    /// A method call whose receiver carries an explicit marker: the marker decides the receiver's use.
    fn marked_method(&mut self, recv: Expr, mtok: &Token, al: Option<N<'a>>, ty: Type, line: u32, u: Use) -> BResult<Expr> {
        let plain = Expr { kind: match recv.kind.clone() { ExprKind::Place(p, _) => ExprKind::Place(p, Use::Read), k => k }, ..recv.clone() };
        let mut call = self.method_call(plain, mtok, al, ty, line)?;
        if let ExprKind::Call { args, .. } = &mut call.kind {
            if let Some(a) = args.first_mut() {
                if a.param == "self" {
                    if let ExprKind::Place(p, _) = &a.value.kind {
                        a.value.kind = ExprKind::Place(p.clone(), u);
                    }
                }
            }
        }
        Ok(call)
    }

    // ---- lambdas ---------------------------------------------------------------------------------------------

    fn lambda(&mut self, n: N<'a>) -> BResult<Expr> {
        let line = line_of(n);
        let ty = self.node_ty(n);
        let words: Vec<&Token> = n.direct_tokens();
        let fn_tok = words.iter().find(|t| t.text == "fn").map(|t| (*t).clone()).unwrap_or_else(|| n.tokens()[0].clone());
        let kind = if words.iter().any(|t| t.text == "once") { "fn once" } else if words.iter().any(|t| t.text == "mut") { "fn mut" } else { "fn" };
        // the lambda's modifiers (`async`, `pure`, `try`) are a child node, not direct tokens
        let mut modifiers: Vec<String> = words.iter().take_while(|t| t.text != "fn").map(|t| t.text.clone()).collect();
        if let Some(m) = n.child_node("modifiers") {
            modifiers.extend(m.tokens().iter().map(|t| t.text.clone()));
        }
        let label = format!("lambda@{}:{}", fn_tok.line, fn_tok.col);
        let (pts, ret) = match &ty {
            Type::Fn { params, ret, .. } => (params.clone(), (**ret).clone()),
            _ => (vec![], n.child_node("type").map(|t| self.ty.dc.type_of(Some(t))).unwrap_or_else(|| unknown(""))),
        };
        let ret = match n.child_node("type") {
            Some(t) => self.ty.dc.type_of(Some(t)),
            None => ret,
        };
        // the lambda's own parts, not a nested lambda's (D-34)
        let cl = n.child_node("captureList");
        let mut fb = Fb::new(&label, &self.ir_module, ret, modifiers.iter().any(|m| m == "try"));
        fb.lambda = Some(LambdaCtx { kind: kind.into(), explicit: cl.is_some() });
        // explicit captures are evaluated in the enclosing function
        let mut explicit: Vec<(String, CaptureMode, Expr)> = Vec::new();
        if let Some(cl) = cl {
            for cap in cl.child_nodes("capture") {
                let ck = cap.kids();
                match ck[0] {
                    C::T(t) => {
                        let v = self.place_expr_tok(t, Use::Copy)?;
                        explicit.push((t.text.clone(), CaptureMode::Value, v));
                    }
                    C::N(mk) => {
                        let marker = mk.tokens()[0].text.clone();
                        let pl = ck[1].node().unwrap();
                        let root = pl.tokens()[0].text.clone();
                        let (mode, u) = match marker.as_str() {
                            "alias" => (CaptureMode::Alias, Use::Alias),
                            "editable" => (CaptureMode::Editable, Use::Editable),
                            "copy" => (CaptureMode::Copy, Use::Copy),
                            _ => (CaptureMode::Move, Use::Move),
                        };
                        let v = self.place_expr(pl, u)?;
                        if matches!(mode, CaptureMode::Alias | CaptureMode::Editable) {
                            if let ExprKind::Place(p, _) = &v.kind {
                                let l = p.local;
                                self.fb().locals[l as usize].boxed = true;
                            }
                        }
                        explicit.push((root, mode, v));
                    }
                }
            }
        }
        for (name, mode, v) in &explicit {
            let l = fb.add_local(name, v.ty.clone(), matches!(mode, CaptureMode::Move | CaptureMode::Editable), LocalKind::Capture);
            if matches!(mode, CaptureMode::Alias | CaptureMode::Editable) {
                fb.locals[l as usize].boxed = true;
            }
            fb.captures.push(CaptureDef { local: l, name: name.clone(), mode: *mode });
            fb.capture_values.push(v.clone());
            fb.bind(name, l);
        }
        if let Some(lp) = n.child_node("lambdaParams") {
            for (i, p) in lp.child_nodes("lambdaParam").into_iter().enumerate() {
                let pn = p.direct_tokens()[0].text.clone();
                let pty = match p.find("paramType") {
                    Some(pt) => self.ty.dc.type_of(pt.find("type")),
                    None => pts.get(i).cloned().unwrap_or_else(|| unknown("lambda parameter")),
                };
                let mode = match p.find("paramType").map(|pt| pt.tokens()[0].text.clone()).as_deref() {
                    Some("editable") => Mode::Editable,
                    Some("owned") => Mode::Owned,
                    _ => Mode::Read,
                };
                let l = fb.add_local(&pn, pty, mode == Mode::Editable, LocalKind::Param);
                fb.params.push(Param { local: l, name: pn.clone(), mode, default: None });
                fb.bind(&pn, l);
            }
        }
        self.fbs.push(fb);
        let body = n.child_node("funcBody").ok_or_else(|| BuildError { line, message: "a lambda without a body".into() });
        let built = body.and_then(|b| self.func_body(b));
        let fb = self.fbs.pop().unwrap();
        let built = built?;
        let effects = self
            .ty
            .closure_info
            .get(&n.id())
            .and_then(|c| c.eff.as_ref())
            .map(|s| Effects { effects: s.effects.clone(), footprint: s.footprint.iter().cloned().collect(), authorities: s.authorities.iter().cloned().collect() })
            .unwrap_or_default();
        let captures = fb.captures.clone();
        let values = fb.capture_values.clone();
        let name = fb.name.clone();
        let mut mods = modifiers.clone();
        if !mods.iter().any(|m| m == "try") && matches!(&ty, Type::Fn { modifiers: tm, .. } if tm.iter().any(|m| m == "try")) {
            mods.push("try".into());
        }
        self.prog.functions.push(fb.finish(line, Some(built), mods, effects, None, Some(ClosureDef { kind: kind.into(), captures })));
        Ok(Expr::new(ExprKind::Closure { func: name, captures: values }, ty, line))
    }

    fn place_expr_tok(&mut self, t: &Token, u: Use) -> BResult<Expr> {
        let line = tok_line(t);
        let l = self.resolve(&t.text, line).ok_or_else(|| BuildError { line, message: format!("capture of `{}`, which is not a local", t.text) })?;
        let lt = self.local_ty(l);
        Ok(Expr::new(ExprKind::Place(Place::local(l), u), lt, line))
    }

    // ---- match and if ----------------------------------------------------------------------------------------

    pub fn match_parts(&mut self, n: N<'a>, _as_expr: bool) -> BResult<(Expr, Vec<Arm>)> {
        let kids = n.kids();
        let moving = kids.iter().take(3).any(|k| k.text_is("move"));
        let sn = kids.iter().filter_map(|k| k.node()).find(|c| c.name() == "expr" || c.name() == "place").unwrap();
        let scrutinee = if moving {
            match Self::bare(sn, "place") {
                Some(pl) => self.place_expr(pl, Use::Move)?,
                None => {
                    let e = self.expr(sn)?;
                    match e.kind {
                        ExprKind::Place(p, Use::Read) => Expr { kind: ExprKind::Place(p, Use::Move), ..e },
                        _ => e,
                    }
                }
            }
        } else {
            self.expr(sn)?
        };
        let st = scrutinee.ty.clone();
        let mut arms = Vec::new();
        for arm in n.child_nodes("matchArm") {
            self.fb().push_scope();
            let r = self.arm(arm, &st);
            let locals = self.fb().pop_scope();
            let mut a = r?;
            a.locals = locals;
            arms.push(a);
        }
        Ok((scrutinee, arms))
    }

    fn arm(&mut self, arm: N<'a>, st: &Type) -> BResult<Arm> {
        let pat = arm.find("pattern").unwrap();
        let pattern = self.pattern(pat, st, false)?;
        let ak = arm.kids();
        let guard = if ak.iter().any(|k| k.text_is("when")) {
            let g = ak.iter().filter_map(|k| k.node()).find(|c| c.name() == "expr").unwrap();
            Some(self.expr(g)?)
        } else {
            None
        };
        let body_node = ak.last().unwrap().node().unwrap();
        let body = if body_node.name() == "block" { ArmBody::Block(self.block(body_node)?) } else { ArmBody::Expr(self.expr_moved(body_node)?) };
        Ok(Arm { pattern, guard, locals: vec![], body })
    }

    fn if_expr(&mut self, n: N<'a>) -> BResult<Expr> {
        let line = line_of(n);
        let ty = self.node_ty(n);
        let kids = n.kids();
        let cond = self.seal_cond(kids[1].node().unwrap())?;
        let then = self.block(kids[2].node().unwrap())?;
        let tail = kids[4].node().unwrap();
        let els = if tail.name() == "ifExpr" {
            let e = self.if_expr(tail)?;
            let el = e.line;
            Block { stmts: vec![Stmt { line: el, temps: vec![], kind: StmtKind::Give(e) }], line: el, ..Default::default() }
        } else {
            self.block(tail)?
        };
        Ok(Expr::new(ExprKind::If { cond: bx(cond), then, els }, ty, line))
    }

    // ---- literals --------------------------------------------------------------------------------------------

    pub fn literal_tok(&mut self, tk: &'a Token, ty: Option<Type>) -> BResult<Expr> {
        let line = tok_line(tk);
        let t = |d: Type| match &ty {
            Some(x) if known(x) => value_ty(x),
            _ => d,
        };
        let text = tk.text.replace('_', "");
        let c = match tk.kind {
            Kind::Int => {
                let v = if text.len() > 2 && matches!(&text[..2], "0x" | "0X") {
                    i128::from_str_radix(&text[2..], 16)
                } else if text.len() > 2 && matches!(&text[..2], "0b" | "0B") {
                    i128::from_str_radix(&text[2..], 2)
                } else if text.len() > 2 && matches!(&text[..2], "0o" | "0O") {
                    i128::from_str_radix(&text[2..], 8)
                } else {
                    text.parse::<i128>()
                };
                let v = v.map_err(|e| BuildError { line, message: format!("integer literal {}: {e}", tk.text) })?;
                return Ok(Expr::new(ExprKind::Const(Const::Int(v)), t(int()), line));
            }
            Kind::Float => {
                if let Some(x) = text.strip_suffix("f32") {
                    let v: f32 = x.parse().map_err(|_| BuildError { line, message: format!("float literal {}", tk.text) })?;
                    return Ok(Expr::new(ExprKind::Const(Const::Float32(v.to_bits())), named("Float32"), line));
                }
                let v: f64 = text.parse().map_err(|_| BuildError { line, message: format!("float literal {}", tk.text) })?;
                return Ok(Expr::new(ExprKind::Const(Const::Float(v.to_bits())), named("Float"), line));
            }
            Kind::Dec => Const::Decimal(text.trim_end_matches('d').to_string()),
            Kind::Dur => {
                let (num, unit) = text.split_at(text.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(text.len()));
                let n: i64 = num.parse().map_err(|_| BuildError { line, message: format!("duration literal {}", tk.text) })?;
                let mult: i64 = match unit {
                    "ns" => 1,
                    "us" => 1_000,
                    "ms" => 1_000_000,
                    "s" => 1_000_000_000,
                    "min" => 60_000_000_000,
                    "h" => 3_600_000_000_000,
                    u => return err(line, format!("duration unit {u}")),
                };
                Const::Duration(n.checked_mul(mult).ok_or_else(|| BuildError { line, message: "duration literal out of range".into() })?)
            }
            Kind::Str => return self.string_literal(tk, line),
            Kind::Raw => Const::Str(tk.text[3..tk.text.len() - 3].to_string()),
            Kind::Bytes => Const::Bytes(unescape(&tk.text[2..tk.text.len() - 1]).into_bytes()),
            Kind::Kw => match tk.text.as_str() {
                "true" => Const::Bool(true),
                "false" => Const::Bool(false),
                "null" => Const::Null,
                w => return err(line, format!("keyword literal {w}")),
            },
            _ => return err(line, format!("literal {}", tk.text)),
        };
        let dt = match &c {
            Const::Decimal(_) => named("Decimal"),
            Const::Duration(_) => named("Duration"),
            Const::Str(_) => string(),
            Const::Bytes(_) => named("Bytes"),
            Const::Bool(_) => boolean(),
            Const::Null => null_t(),
            _ => unknown("literal"),
        };
        Ok(Expr::new(ExprKind::Const(c), t(dt), line))
    }

    fn string_literal(&mut self, tk: &'a Token, line: u32) -> BResult<Expr> {
        let cs: Vec<char> = tk.text.chars().collect();
        let body: Vec<char> = cs[1..cs.len() - 1].to_vec();
        let mut parts: Vec<Part> = Vec::new();
        let mut lit = String::new();
        let mut i = 0;
        let mut hole = 0usize;
        while i < body.len() {
            let c = body[i];
            if c == '\\' && i + 1 < body.len() {
                lit.push(match body[i + 1] {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    '0' => '\0',
                    o => o,
                });
                i += 2;
            } else if c == '$' && i + 1 < body.len() && body[i + 1] == '{' {
                let (mut depth, mut j) = (1, i + 2);
                while j < body.len() && depth > 0 {
                    if body[j] == '{' {
                        depth += 1;
                    } else if body[j] == '}' {
                        depth -= 1;
                    }
                    j += 1;
                }
                let src: String = body[i + 2..j - 1].iter().collect();
                if !lit.is_empty() {
                    parts.push(Part::Lit(std::mem::take(&mut lit)));
                }
                let parsed = fortis_types::typer::expr::hole_tree_at(tk, hole, &src).ok_or_else(|| BuildError { line, message: format!("interpolation `{src}` does not parse") })?;
                hole += 1;
                let root = N::new(&parsed.root, &parsed.toks);
                let e = self.expr(root)?;
                parts.push(Part::Show(e));
                i = j;
            } else {
                lit.push(c);
                i += 1;
            }
        }
        if parts.is_empty() {
            return Ok(Expr::new(ExprKind::Const(Const::Str(lit)), string(), line));
        }
        if !lit.is_empty() {
            parts.push(Part::Lit(lit));
        }
        Ok(Expr::new(ExprKind::Interpolate(parts), string(), line))
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The arguments of an argument list: `arg`s, or the `constArg`s of a constant expression's call (a field default
/// `List.of(10, 20)`, a constant `Money.of(…)`), whose arguments were once dropped (defect D-22).
fn arg_nodes<'a>(al: N<'a>) -> Vec<N<'a>> {
    let args = al.child_nodes("arg");
    if args.is_empty() {
        al.child_nodes("constArg")
    } else {
        args
    }
}
