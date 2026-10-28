//! The checks. Each walks the typed IR and reports violations under its registry id; none consults the checker:
//! the validator re-establishes every frontend-checked property from the IR alone (PROG-CMP-07).

use std::collections::{BTreeSet, HashMap, HashSet};

use fortis_mir::model::*;
use fortis_types::types::{strip, Type};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub check: &'static str,
    pub function: String,
    pub line: u32,
    pub message: String,
}

const HOST_IO: &[&str] = &["Console.print", "Network.resolve", "Clock.sleep", "Environment.get", "Process.run", "RawSql.execute"];
const NONDET: &[&str] = &["Random.below", "Clock.now", "Clock.elapsedSince"];
const GROWTH: &[&str] = &[
    "List.of", "List.push", "List.default", "Map.default", "Map.set", "Set.default", "Set.add", "String.from", "Builder.create", "Builder.append", "Builder.appendLine", "Builder.appendScalar",
    "Builder.finish", "Text.words", "List.iterate", "Array.filled",
];

/// The inline task storage, in bytes, that the v1 runtime host declares for a scope (Appendix C.3): none, so a
/// cooperative launch always boxes its future (SP-04-async, X-125).
const HOST_INLINE_TASK_STORAGE: usize = 0;

pub fn type_name(t: &Type) -> Option<&str> {
    match strip(t) {
        Type::Named(n) => Some(n.name.as_str()),
        Type::Refined { base, .. } => Some(base.name.as_str()),
        _ => None,
    }
}

fn is_outcome(t: &Type) -> bool {
    type_name(t) == Some("Outcome")
}
fn is_optional(t: &Type) -> bool {
    matches!(strip(t), Type::Optional(_))
}

pub struct Ctx<'p> {
    pub prog: &'p Program,
    pub funcs: HashMap<&'p str, &'p Function>,
    pub types: HashMap<&'p str, &'p TypeDef>,
    pub out: Vec<Violation>,
}

impl<'p> Ctx<'p> {
    pub fn new(prog: &'p Program) -> Ctx<'p> {
        Ctx { prog, funcs: prog.functions.iter().map(|f| (f.name.as_str(), f)).collect(), types: prog.types.iter().map(|t| (t.name.as_str(), t)).collect(), out: vec![] }
    }

    fn v(&mut self, check: &'static str, f: &Function, line: u32, message: impl Into<String>) {
        self.out.push(Violation { check, function: f.name.clone(), line, message: message.into() });
    }

    /// Trivial values lower to `Copy` (the trivial table, 6.11): primitives; classes marked trivial; enums, newtypes,
    /// tuples, records and optionals whose parts are all trivial.
    pub fn trivial(&self, t: &Type) -> bool {
        self.trivial_seen(t, &mut Vec::new())
    }

    fn trivial_seen(&self, t: &Type, seen: &mut Vec<String>) -> bool {
        match strip(t) {
            Type::Optional(i) => self.trivial_seen(i, seen),
            Type::Tuple(ts) => ts.iter().all(|x| self.trivial_seen(x, seen)),
            Type::Record(fs) => fs.iter().all(|(_, x)| self.trivial_seen(x, seen)),
            Type::Named(_) | Type::Refined { .. } => {
                let n = type_name(t).unwrap_or("").to_string();
                if fortis_types::types::is_trivial_builtin(&n) || matches!(n.as_str(), "Void" | "Never" | "Bool" | "Float" | "Float32" | "Decimal" | "Duration" | "Timestamp" | "Ordering") {
                    return true;
                }
                if seen.contains(&n) {
                    return false;
                }
                let d = match self.types.get(n.as_str()) {
                    Some(d) => *d,
                    None => return false,
                };
                seen.push(n);
                let r = match &d.kind {
                    TypeKind::Class { .. } => d.trivial,
                    TypeKind::Enum { variants } => variants.iter().all(|v| v.fields.iter().all(|(_, ft)| self.trivial_seen(ft, seen))),
                    TypeKind::Newtype { repr } => self.trivial_seen(repr, seen),
                    _ => false,
                };
                seen.pop();
                r
            }
            _ => false,
        }
    }

    /// A value that owns heap storage: anything not trivial except the inline containers.
    pub fn heap(&self, t: &Type) -> bool {
        if matches!(t, Type::Loan { .. }) {
            return false; // a loan owns nothing
        }
        let n = type_name(t).unwrap_or("");
        if matches!(n, "Array" | "FixedList" | "FixedMap") {
            return false;
        }
        !self.trivial(t) && !matches!(strip(t), Type::TypeParam(_) | Type::Unknown(_) | Type::Fn { .. } | Type::Loan { .. })
    }

    fn copyable(&self, t: &Type) -> bool {
        match strip(t) {
            Type::Named(_) | Type::Refined { .. } => {
                let n = type_name(t).unwrap_or("");
                if matches!(n, "Shared" | "Weak" | "Mutex" | "Failure" | "Task" | "Must" | "File") {
                    return false;
                }
                self.types.get(n).map_or(true, |d| d.copyable || d.trivial)
            }
            Type::Optional(i) => self.copyable(i),
            _ => true,
        }
    }
}

// ---- generic traversal ------------------------------------------------------------------------------------------

pub fn walk_block<'a>(b: &'a Block, f: &mut dyn FnMut(Node<'a>)) {
    f(Node::Block(b));
    for d in &b.defers {
        walk_block(d, f);
    }
    for s in &b.stmts {
        walk_stmt(s, f);
    }
}

#[derive(Clone, Copy)]
pub enum Node<'a> {
    Block(&'a Block),
    Stmt(&'a Stmt),
    Expr(&'a Expr),
}

pub fn walk_stmt<'a>(s: &'a Stmt, f: &mut dyn FnMut(Node<'a>)) {
    f(Node::Stmt(s));
    match &s.kind {
        StmtKind::Let { init, .. } | StmtKind::LetPattern { init, .. } => walk_expr(init, f),
        StmtKind::Assign { value, .. } | StmtKind::Compound { value, .. } => walk_expr(value, f),
        StmtKind::Expr(e) | StmtKind::Discard(e) | StmtKind::Give(e) | StmtKind::Recover(e) | StmtKind::Fail(e) => walk_expr(e, f),
        StmtKind::Return(e) => {
            if let Some(e) = e {
                walk_expr(e, f)
            }
        }
        StmtKind::If { cond, then, els } => {
            walk_expr(cond, f);
            walk_block(then, f);
            if let Some(b) = els {
                walk_block(b, f);
            }
        }
        StmtKind::Loop { body, .. } => walk_block(body, f),
        StmtKind::For { source, body, .. } => {
            match source {
                ForSource::Range { lo, hi, .. } => {
                    walk_expr(lo, f);
                    walk_expr(hi, f);
                }
                ForSource::Items(e) => walk_expr(e, f),
            }
            walk_block(body, f);
        }
        StmtKind::Block(b) | StmtKind::Scope { body: b, .. } => walk_block(b, f),
        StmtKind::Defer(_) => {}
        StmtKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, f);
            walk_arms(arms, f);
        }
        StmtKind::Capture { value, .. } => walk_expr(value, f),
        StmtKind::Record { fields, .. } => fields.iter().for_each(|(_, e)| walk_expr(e, f)),
        StmtKind::Report { value, .. } => walk_expr(value, f),
        StmtKind::Expect { cond, evidence } => {
            walk_expr(cond, f);
            if let Some((a, _, b)) = evidence {
                walk_expr(a, f);
                walk_expr(b, f);
            }
        }
        StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Drop(_) => {}
    }
}

fn walk_arms<'a>(arms: &'a [Arm], f: &mut dyn FnMut(Node<'a>)) {
    for a in arms {
        if let Some(g) = &a.guard {
            walk_expr(g, f);
        }
        match &a.body {
            ArmBody::Expr(e) => walk_expr(e, f),
            ArmBody::Block(b) => walk_block(b, f),
        }
    }
}

pub fn walk_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(Node<'a>)) {
    f(Node::Expr(e));
    match &e.kind {
        ExprKind::Const(_) | ExprKind::Place(..) | ExprKind::Global(_) | ExprKind::FuncRef(_) => {}
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) | ExprKind::Index(a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        ExprKind::Unary(_, a)
        | ExprKind::Codec { arg: a, .. }
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
        | ExprKind::Temp { value: a, .. } => walk_expr(a, f),
        ExprKind::Call { callee, args } => {
            if let Callee::Value(v) = callee {
                walk_expr(v, f);
            }
            args.iter().for_each(|a| walk_expr(&a.value, f));
        }
        ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => fields.iter().for_each(|(_, x)| walk_expr(x, f)),
        ExprKind::Tuple(es) | ExprKind::Closure { captures: es, .. } => es.iter().for_each(|x| walk_expr(x, f)),
        ExprKind::Interpolate(parts) => parts.iter().for_each(|p| {
            if let Part::Show(x) = p {
                walk_expr(x, f)
            }
        }),
        ExprKind::Catch { value, handler, .. } => {
            walk_expr(value, f);
            walk_block(handler, f);
        }
        ExprKind::If { cond, then, els } => {
            walk_expr(cond, f);
            walk_block(then, f);
            walk_block(els, f);
        }
        ExprKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, f);
            walk_arms(arms, f);
        }
        ExprKind::Block(b) => walk_block(b, f),
    }
}

/// Every place an expression tree uses, with its use.
fn places_in_expr(e: &Expr) -> Vec<(&Place, Use)> {
    let mut out = Vec::new();
    walk_expr(e, &mut |n| {
        if let Node::Expr(x) = n {
            if let ExprKind::Place(p, u) = &x.kind {
                out.push((p, *u));
            }
        }
    });
    out
}

fn mentions(s: &Stmt, l: LocalId) -> bool {
    let mut hit = false;
    walk_stmt(s, &mut |n| match n {
        Node::Expr(x) => {
            if let ExprKind::Place(p, _) = &x.kind {
                if p.local == l {
                    hit = true;
                }
            }
        }
        Node::Stmt(st) => match &st.kind {
            StmtKind::Assign { place, .. } | StmtKind::Compound { place, .. } if place.local == l => hit = true,
            _ => {}
        },
        _ => {}
    });
    hit
}

/// A block (or arm) whose every path ends in `return`, `fail` or `recover` (or another exit).
pub fn terminates(b: &Block) -> bool {
    match b.stmts.last() {
        None => false,
        Some(s) => stmt_terminates(s),
    }
}

fn stmt_terminates(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Return(_) | StmtKind::Fail(_) | StmtKind::Recover(_) | StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Give(_) => true,
        StmtKind::Expr(e) | StmtKind::Discard(e) => matches!(e.kind, ExprKind::Fail(_)) || type_name(&e.ty) == Some("Never"),
        StmtKind::If { then, els: Some(e), .. } => terminates(then) && terminates(e),
        StmtKind::Block(b) => terminates(b),
        StmtKind::Match { arms, .. } => !arms.is_empty() && arms.iter().all(|a| match &a.body {
            ArmBody::Block(b) => terminates(b),
            ArmBody::Expr(e) => matches!(e.kind, ExprKind::Fail(_)),
        }),
        StmtKind::Loop { body, .. } => !contains_break(body),
        _ => false,
    }
}

fn contains_break(b: &Block) -> bool {
    let mut hit = false;
    walk_block(b, &mut |n| {
        if let Node::Stmt(s) = n {
            if matches!(s.kind, StmtKind::Break(_)) {
                hit = true;
            }
        }
    });
    hit
}

// ---- the checks -------------------------------------------------------------------------------------------------

pub fn run(ctx: &mut Ctx) {
    let prog = ctx.prog;
    types(ctx);
    for f in &prog.functions {
        function(ctx, f);
    }
    ctx.out.sort();
    ctx.out.dedup();
}

/// Type-level rows: the trivial table, one lifetime parameter, mixin conflicts and requirements, interface
/// substitutability.
fn types(ctx: &mut Ctx) {
    let prog = ctx.prog;
    for t in &prog.types {
        let anchor = Function { name: format!("type {}::{}", t.module, t.name), label: String::new(), module: t.module.clone(), line: 0, params: vec![], ret: fortis_types::types::void(), modifiers: vec![], locals: vec![], body: None, effects: Effects::default(), observe: None, closure: None };
        if t.trivial {
            if let TypeKind::Class { fields } = &t.kind {
                for fd in fields {
                    if !ctx.trivial(&fd.ty) {
                        ctx.v("C1.trivial", &anchor, 0, format!("trivial class {} has the non-trivial field `{}: {}`", t.name, fd.name, fd.ty.show()));
                    }
                }
            }
            if let TypeKind::Enum { variants } = &t.kind {
                for v in variants {
                    for (n, ft) in &v.fields {
                        if !ctx.trivial(ft) {
                            ctx.v("C1.trivial", &anchor, 0, format!("trivial enum {} has the non-trivial payload `{}.{n}: {}`", t.name, v.name, ft.show()));
                        }
                    }
                }
            }
        }
        if !t.lifetimes.is_empty() {
            ctx.v("C1.standard-loans", &anchor, 0, format!("type {} declares a lifetime parameter; only the standard views and cursors store a loan (Appendix C.1, X-137)", t.name));
        }
        let mut seen = HashSet::new();
        for (m, _) in &t.methods {
            if !seen.insert(m.clone()) {
                ctx.v("C1.mixin", &anchor, 0, format!("{} receives method `{m}` twice (a mixin conflict)", t.name));
            }
        }
        for mx in &t.mixins {
            if let Some(TypeKind::Mixin { requires, .. }) = ctx.types.get(mx.as_str()).map(|d| &d.kind) {
                for r in requires.clone() {
                    let has = t.methods.iter().any(|(m, _)| *m == r) || matches!(&t.kind, TypeKind::Class { fields } if fields.iter().any(|fd| fd.name == r));
                    if !has {
                        ctx.v("C1.mixin", &anchor, 0, format!("{} uses mixin {mx}, which requires `{r}`", t.name));
                    }
                }
            }
        }
        for iname in &t.implements {
            let methods = match ctx.types.get(iname.as_str()).map(|d| &d.kind) {
                Some(TypeKind::Interface { methods }) => methods.clone(),
                _ => continue,
            };
            for (m, mods) in methods {
                let fname = match t.methods.iter().find(|(x, _)| *x == m) {
                    Some((_, f)) => f.clone(),
                    None => continue,
                };
                let f = match ctx.funcs.get(fname.as_str()) {
                    Some(f) => *f,
                    None => continue,
                };
                if mods.iter().any(|x| x == "pure") {
                    for e in ["io", "mutate", "nondet", "block", "suspend"] {
                        if f.effects.has(e) {
                            ctx.v("C1.interface", f, f.line, format!("{}.{m} has `{e}` but {iname}.{m} is pure: an implementation may not exceed its interface", t.name));
                        }
                    }
                }
                if !mods.iter().any(|x| x == "try") && f.is_try() {
                    ctx.v("C1.interface", f, f.line, format!("{}.{m} is `try` but {iname}.{m} is not", t.name));
                }
            }
        }
    }
}

struct Walk<'f> {
    f: &'f Function,
    /// the path of block ids declaring each local
    decl: HashMap<LocalId, Vec<usize>>,
    path: Vec<usize>,
    next_block: usize,
    loops: Vec<Option<String>>,
    in_value_block: usize,
    in_catch: usize,
}

fn function(ctx: &mut Ctx, f: &Function) {
    let body = match &f.body {
        Some(b) => b,
        None => return,
    };
    // ---- wf.locals: every local referenced exists; each is declared by at most one block
    let n = f.locals.len() as LocalId;
    let mut declared: HashMap<LocalId, usize> = HashMap::new();
    walk_block(body, &mut |node| {
        if let Node::Block(b) = node {
            for l in &b.locals {
                *declared.entry(*l).or_insert(0) += 1;
            }
        }
    });
    for (l, c) in &declared {
        if *c > 1 {
            ctx.v("wf.locals", f, f.line, format!("local _{l} is declared by {c} blocks"));
        }
    }
    let mut bad_locals = Vec::new();
    walk_block(body, &mut |node| match node {
        Node::Expr(e) => {
            if let ExprKind::Place(p, _) = &e.kind {
                if p.local >= n {
                    bad_locals.push((e.line, p.local));
                }
            }
        }
        Node::Stmt(s) => match &s.kind {
            StmtKind::Let { local, .. } if *local >= n => bad_locals.push((s.line, *local)),
            StmtKind::Assign { place, .. } | StmtKind::Compound { place, .. } if place.local >= n => bad_locals.push((s.line, place.local)),
            _ => {}
        },
        _ => {}
    });
    for (line, l) in bad_locals {
        ctx.v("wf.locals", f, line, format!("local _{l} is not declared (the function has {n})"));
    }
    if declared.keys().any(|l| *l >= n) {
        ctx.v("wf.locals", f, f.line, "a block declares a local the function does not have");
        return;
    }
    // ---- wf.calls and per-expression checks
    let mut w = Walk { f, decl: HashMap::new(), path: vec![], next_block: 0, loops: vec![], in_value_block: 0, in_catch: 0 };
    for p in &f.params {
        w.decl.insert(p.local, vec![]);
    }
    if let Some(c) = &f.closure {
        for cd in &c.captures {
            w.decl.insert(cd.local, vec![]);
        }
    }
    block(ctx, &mut w, body);
    effects(ctx, f, body);
    lambda_kind(ctx, f, body);
    observation(ctx, f, body);
    use_after_move(ctx, f, body);
    noalloc(ctx, f, body);
    cleanup_effects(ctx, f, body);
    async_rows(ctx, f, body);
}

fn block(ctx: &mut Ctx, w: &mut Walk, b: &Block) {
    let id = w.next_block;
    w.next_block += 1;
    w.path.push(id);
    for l in &b.locals {
        w.decl.insert(*l, w.path.clone());
    }
    for d in &b.defers {
        block(ctx, w, d);
    }
    for (i, s) in b.stmts.iter().enumerate() {
        stmt(ctx, w, s);
        exclusive_loan(ctx, w.f, b, i);
    }
    // C1.try: a Failure held by a local of this block is consumed in it, never dropped
    for l in &b.locals {
        let d = &w.f.locals[*l as usize];
        if type_name(&d.ty) == Some("Failure") && d.kind != LocalKind::Temp {
            let consumed = b.stmts.iter().any(|s| consumes(s, *l));
            if !consumed {
                ctx.v("C1.try", w.f, b.line, format!("the Failure in `{}` is dropped at the end of its block (no Drop of a live Failure is emitted)", d.name));
            }
        }
    }
    w.path.pop();
}

/// The statement moves, reports, fails with, returns or propagates the local.
fn consumes(s: &Stmt, l: LocalId) -> bool {
    let mut hit = false;
    walk_stmt(s, &mut |n| match n {
        Node::Expr(e) => match &e.kind {
            ExprKind::Place(p, Use::Move) if p.local == l => hit = true,
            ExprKind::Place(p, _) if p.local == l => hit = true,
            _ => {}
        },
        _ => {}
    });
    hit
}

fn stmt(ctx: &mut Ctx, w: &mut Walk, s: &Stmt) {
    let f = w.f;
    match &s.kind {
        StmtKind::Let { local, init } => {
            expr(ctx, w, init);
            // C1.read-loan: a local holding a loan of a place lives no longer than that place's block
            if let ExprKind::Place(p, Use::Alias | Use::Editable) = &init.kind {
                if let (Some(lp), Some(op)) = (w.decl.get(local), w.decl.get(&p.local)) {
                    if !lp.starts_with(op) {
                        ctx.v("C1.read-loan", f, s.line, format!("`{}` holds a loan of `{}` beyond the block that owns it", f.local(*local).name, f.local(p.local).name));
                    }
                }
            }
            // C1.try: an outcome bound and never used is dropped
            if is_outcome(&init.ty) && f.local(*local).kind != LocalKind::Temp {
                // checked by the enclosing block's statements after this one
            }
            shared_dup(ctx, f, init, s.line);
        }
        StmtKind::LetPattern { pattern, init } => {
            expr(ctx, w, init);
            pattern_check(ctx, f, pattern, &init.ty, s.line);
        }
        StmtKind::Assign { value, .. } => {
            expr(ctx, w, value);
            shared_dup(ctx, f, value, s.line);
        }
        StmtKind::Compound { value, .. } => expr(ctx, w, value),
        StmtKind::Expr(e) => {
            expr(ctx, w, e);
            if is_outcome(&e.ty) && !matches!(e.kind, ExprKind::Catch { .. } | ExprKind::OrFail(_) | ExprKind::OrRecover(..)) {
                ctx.v("C1.try", f, s.line, "an outcome is dropped at the end of its statement (or fail, recover, catch or match it)");
            }
        }
        StmtKind::Discard(e) => {
            expr(ctx, w, e);
            if is_outcome(&e.ty) {
                ctx.v("C1.try", f, s.line, "`discard` of an outcome: a linear value is consumed, never discarded");
            }
        }
        StmtKind::If { cond, then, els } => {
            expr(ctx, w, cond);
            block(ctx, w, then);
            if let Some(b) = els {
                block(ctx, w, b);
            }
        }
        StmtKind::Loop { label, body } => {
            w.loops.push(label.clone());
            block(ctx, w, body);
            w.loops.pop();
        }
        StmtKind::For { label, binding, source, body } => {
            match source {
                ForSource::Range { lo, hi, .. } => {
                    expr(ctx, w, lo);
                    expr(ctx, w, hi);
                }
                ForSource::Items(e) => expr(ctx, w, e),
            }
            if let Some(b) = binding {
                w.decl.insert(*b, w.path.clone());
            }
            w.loops.push(label.clone());
            block(ctx, w, body);
            w.loops.pop();
        }
        StmtKind::Break(l) | StmtKind::Continue(l) => {
            let ok = match l {
                None => !w.loops.is_empty(),
                Some(l) => w.loops.iter().any(|x| x.as_deref() == Some(l.as_str())),
            };
            if !ok {
                ctx.v("wf.control", f, s.line, format!("`break`/`continue` outside a loop{}", l.as_ref().map_or(String::new(), |l| format!(" labelled `{l}`"))));
            }
        }
        StmtKind::Return(e) => {
            if let Some(e) = e {
                expr(ctx, w, e);
                from_return(ctx, f, e, s.line);
            }
        }
        StmtKind::Give(e) => {
            expr(ctx, w, e);
            if w.in_value_block == 0 {
                ctx.v("wf.control", f, s.line, "`give` outside a block expression");
            }
        }
        StmtKind::Recover(e) => {
            expr(ctx, w, e);
            if w.in_catch == 0 {
                ctx.v("wf.control", f, s.line, "`recover` outside a `catch` block");
            }
        }
        StmtKind::Fail(e) => expr(ctx, w, e),
        StmtKind::Block(b) | StmtKind::Scope { body: b, .. } => block(ctx, w, b),
        StmtKind::Defer(_) | StmtKind::Drop(_) => {}
        StmtKind::Match { scrutinee, arms } => {
            expr(ctx, w, scrutinee);
            match_arms(ctx, w, scrutinee, arms, false);
        }
        StmtKind::Capture { value, .. } => expr(ctx, w, value),
        StmtKind::Record { fields, .. } => fields.iter().for_each(|(_, e)| expr(ctx, w, e)),
        StmtKind::Report { value, bind } => {
            expr(ctx, w, value);
            if let Some(b) = bind {
                w.decl.insert(*b, w.path.clone());
            }
        }
        StmtKind::Expect { cond, evidence } => {
            expr(ctx, w, cond);
            if let Some((a, _, b)) = evidence {
                expr(ctx, w, a);
                expr(ctx, w, b);
            }
        }
    }
}

fn match_arms(ctx: &mut Ctx, w: &mut Walk, scrutinee: &Expr, arms: &[Arm], value: bool) {
    let f = w.f;
    let moving = matches!(scrutinee.kind, ExprKind::Place(_, Use::Move)) || !matches!(scrutinee.kind, ExprKind::Place(..));
    for a in arms {
        pattern_check(ctx, f, &a.pattern, &scrutinee.ty, scrutinee.line);
        for l in &a.locals {
            w.decl.insert(*l, w.path.clone());
        }
        if let Some(g) = &a.guard {
            expr(ctx, w, g);
        }
        match &a.body {
            ArmBody::Expr(e) => expr(ctx, w, e),
            ArmBody::Block(b) => {
                if value {
                    w.in_value_block += 1;
                }
                block(ctx, w, b);
                if value {
                    w.in_value_block -= 1;
                }
            }
        }
        // C1.match: the bindings of a match over a borrowed scrutinee are loans; none is moved out
        if !moving {
            let bound: Vec<LocalId> = a.locals.iter().copied().filter(|l| ctx.heap(&f.local(*l).ty)).collect();
            let mut moved_out = Vec::new();
            let mut check = |e: &Expr| {
                for (p, u) in places_in_expr(e) {
                    if u == Use::Move && p.proj.is_empty() && bound.contains(&p.local) {
                        moved_out.push(p.local);
                    }
                }
            };
            match &a.body {
                ArmBody::Expr(e) => check(e),
                ArmBody::Block(b) => walk_block(b, &mut |n| {
                    if let Node::Expr(e) = n {
                        if let ExprKind::Place(p, Use::Move) = &e.kind {
                            if p.proj.is_empty() && bound.contains(&p.local) {
                                moved_out.push(p.local);
                            }
                        }
                    }
                }),
            }
            for l in moved_out {
                ctx.v("C1.match", f, scrutinee.line, format!("`{}` is bound by a match over a borrowed value and moved out of it", f.local(l).name));
            }
        }
    }
}

fn pattern_check(ctx: &mut Ctx, f: &Function, p: &Pattern, t: &Type, line: u32) {
    match p {
        Pattern::Null | Pattern::Present(_) if is_outcome(t) => ctx.v("C1.optional", f, line, format!("an absence pattern on an outcome ({})", t.show())),
        Pattern::Success(_) | Pattern::Failure(_) if is_optional(t) => ctx.v("C1.optional", f, line, format!("an outcome pattern on an optional ({})", t.show())),
        _ => {}
    }
}

fn expr(ctx: &mut Ctx, w: &mut Walk, e: &Expr) {
    let f = w.f;
    match &e.kind {
        ExprKind::Place(p, Use::Copy) => {
            let t = &e.ty;
            if !ctx.copyable(t) {
                ctx.v("C1.copy", f, e.line, format!("`copy` of `{}`: {} is not copyable", f.local(p.local).name, t.show()));
            } else if ctx.heap(t) && !f.effects.has("alloc") {
                ctx.v("C1.copy", f, e.line, format!("`copy` of a heap-owning {} allocates, but the summary has no `alloc`", t.show()));
            }
            if type_name(t) == Some("Shared") || type_name(t) == Some("Weak") {
                ctx.v("C1.shared", f, e.line, "a Shared handle is duplicated only by `retain` (7.8)");
            }
        }
        ExprKind::Call { callee, args } => {
            match callee {
                Callee::Func(n) => {
                    if !ctx.funcs.contains_key(n.as_str()) {
                        ctx.v("wf.calls", f, e.line, format!("call of `{n}`, which the program does not define"));
                    }
                }
                Callee::Value(v) => expr(ctx, w, v),
                _ => {}
            }
            for a in args {
                expr(ctx, w, &a.value);
                if a.mode == Mode::Owned {
                    shared_dup(ctx, f, &a.value, e.line);
                }
            }
            return;
        }
        ExprKind::Construct { ty, fields } => {
            match ctx.types.get(ty.as_str()).copied() {
                None => ctx.v("wf.calls", f, e.line, format!("construction of `{ty}`, which the program does not define")),
                Some(d) => {
                    // C1.class: a class built outside its module has every field visible there
                    if d.module != f.module {
                        if let TypeKind::Class { fields: fds } = &d.kind {
                            for fd in fds {
                                if !fd.exported {
                                    ctx.v("C1.class", f, e.line, format!("{ty} is constructed outside {} but its field `{}` is not exported", d.module, fd.name));
                                }
                            }
                        }
                    }
                }
            }
            for (_, x) in fields {
                expr(ctx, w, x);
                shared_dup(ctx, f, x, e.line);
            }
            return;
        }
        ExprKind::Variant { ty, .. } | ExprKind::MakeFailure { ty, .. } => {
            if !ctx.types.contains_key(ty.as_str()) {
                ctx.v("wf.calls", f, e.line, format!("`{ty}` is not defined by the program"));
            }
        }
        ExprKind::Wrap { ty, value } => {
            // C1.newtype: a refined newtype is built from a constant within its refinement or from `validate`
            if let Some(TypeKind::Newtype { repr: Type::Refined { lo, hi, .. } }) = ctx.types.get(ty.as_str()).map(|d| &d.kind) {
                match &value.kind {
                    ExprKind::Const(Const::Int(n)) => {
                        if lo.map_or(false, |l| *n < l) || hi.map_or(false, |h| *n > h) {
                            ctx.v("C1.newtype", f, e.line, format!("{ty}({n}) lies outside its refinement"));
                        }
                    }
                    ExprKind::Validate { .. } | ExprKind::OrFail(_) => {}
                    ExprKind::Const(Const::Str(s)) => {
                        let n = s.chars().count() as i128;
                        if lo.map_or(false, |l| n < l) || hi.map_or(false, |h| n > h) {
                            ctx.v("C1.newtype", f, e.line, format!("{ty} built from a string of {n} scalars, outside its refinement"));
                        }
                    }
                    _ => {}
                }
            }
        }
        ExprKind::ToDynamic { iface, value, borrowed } => {
            if let Some(tn) = type_name(&value.ty) {
                let eligible = ctx.types.get(tn).map_or(true, |d| d.implements.iter().any(|i| i == iface));
                if !eligible {
                    ctx.v("C1.dynamic", f, e.line, format!("{tn} does not implement {iface}, so it is not eligible as `dynamic {iface}`"));
                }
            }
            if !borrowed && !f.effects.has("alloc") {
                ctx.v("C1.dynamic", f, e.line, format!("an owned `dynamic {iface}` is boxed, but the summary has no `alloc`"));
            }
        }
        ExprKind::OrFail(a) | ExprKind::OrRecover(a, _) | ExprKind::Catch { value: a, .. } => {
            if is_optional(&a.ty) {
                ctx.v("C1.optional", f, e.line, format!("outcome handling of an optional value ({}): absence is not failure", a.ty.show()));
            }
        }
        ExprKind::Coalesce(a, _) | ExprKind::OptField(a, _) => {
            if is_outcome(&a.ty) {
                ctx.v("C1.optional", f, e.line, format!("absence handling of an outcome ({}): failure is not absence", a.ty.show()));
            }
        }
        ExprKind::Present(a) if is_outcome(&a.ty) => ctx.v("C1.optional", f, e.line, "an outcome made present: failure is not absence"),
        _ => {}
    }
    // descend, with the context of value blocks and catch handlers
    match &e.kind {
        ExprKind::Catch { value, handler, .. } => {
            expr(ctx, w, value);
            if !terminates(handler) {
                ctx.v("C1.catch", f, e.line, "a `catch` block must end in `recover`, `return` or `fail` on every path");
            }
            w.in_catch += 1;
            block(ctx, w, handler);
            w.in_catch -= 1;
        }
        ExprKind::If { cond, then, els } => {
            expr(ctx, w, cond);
            w.in_value_block += 1;
            block(ctx, w, then);
            block(ctx, w, els);
            w.in_value_block -= 1;
        }
        ExprKind::Match { scrutinee, arms } => {
            expr(ctx, w, scrutinee);
            match_arms(ctx, w, scrutinee, arms, true);
        }
        ExprKind::Block(b) => {
            w.in_value_block += 1;
            block(ctx, w, b);
            w.in_value_block -= 1;
        }
        _ => {
            // the direct children only: nested statements are visited by `block`
            let mut kids: Vec<&Expr> = Vec::new();
            match &e.kind {
                ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) | ExprKind::Index(a, b) => {
                    kids.push(a);
                    kids.push(b);
                }
                ExprKind::Unary(_, a)
                | ExprKind::Codec { arg: a, .. }
                | ExprKind::Field(a, _)
                | ExprKind::TupleField(a, _)
                | ExprKind::OptField(a, _)
                | ExprKind::Present(a)
                | ExprKind::Success(a)
                | ExprKind::OrFail(a)
                | ExprKind::Fail(a)
                | ExprKind::Await(a)
                | ExprKind::Temp { value: a, .. }
                | ExprKind::Validate { value: a, .. }
                | ExprKind::Wrap { value: a, .. }
                | ExprKind::ToDynamic { value: a, .. } => kids.push(a),
                ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => kids.extend(fields.iter().map(|(_, x)| x)),
                ExprKind::Tuple(es) | ExprKind::Closure { captures: es, .. } => kids.extend(es.iter()),
                ExprKind::Interpolate(parts) => kids.extend(parts.iter().filter_map(|p| if let Part::Show(x) = p { Some(x) } else { None })),
                _ => {}
            }
            for k in kids {
                expr(ctx, w, k);
            }
        }
    }
}

/// C1.shared: a Shared or Weak handle stored into a new owner by a plain read duplicates it without `retain`.
fn shared_dup(ctx: &mut Ctx, f: &Function, e: &Expr, line: u32) {
    if let ExprKind::Place(_, Use::Read) = &e.kind {
        if matches!(type_name(&e.ty), Some("Shared") | Some("Weak")) {
            ctx.v("C1.shared", f, line, "a Shared handle stored into a new owner without `retain` or `move` (7.8)");
        }
    }
}

/// C1.from-return: a function whose result is a loan `from x` returns loans of `x` only.
fn from_return(ctx: &mut Ctx, f: &Function, e: &Expr, line: u32) {
    let origin = match &f.ret {
        Type::Loan { origin, .. } => origin.clone(),
        _ => return,
    };
    if origin == "self" || origin == "static" || !f.params.iter().any(|p| p.name == origin) {
        return;
    }
    if let Some(root) = loan_root(f, e) {
        if root != origin {
            ctx.v("C1.from-return", f, line, format!("the result is a loan from `{origin}`, but this return lends `{root}`"));
        }
    }
}

fn loan_root(f: &Function, e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Place(p, _) => {
            let d = f.local(p.local);
            if matches!(d.kind, LocalKind::Param | LocalKind::SelfParam) {
                Some(d.name.clone())
            } else {
                None
            }
        }
        ExprKind::Present(a) | ExprKind::Field(a, _) => loan_root(f, a),
        _ => None,
    }
}

/// C1.exclusive-loan: while a local holds an editable loan of a place (until its last use in the block), the
/// place is not used directly.
fn exclusive_loan(ctx: &mut Ctx, f: &Function, b: &Block, i: usize) {
    let (holder, owner) = match &b.stmts[i].kind {
        StmtKind::Let { local, init: Expr { kind: ExprKind::Place(p, Use::Editable), .. } } => (*local, p.clone()),
        _ => return,
    };
    let last = match b.stmts.iter().enumerate().rev().find(|(_, s)| mentions(s, holder)) {
        Some((j, _)) if j > i => j,
        _ => return,
    };
    for s in &b.stmts[i + 1..last] {
        let mut hit = false;
        walk_stmt(s, &mut |n| {
            if let Node::Expr(e) = n {
                if let ExprKind::Place(p, _) = &e.kind {
                    if p.local == owner.local && (p.proj.starts_with(&owner.proj) || owner.proj.starts_with(&p.proj)) {
                        hit = true;
                    }
                }
            }
        });
        if hit {
            ctx.v("C1.exclusive-loan", f, s.line, format!("`{}` is used while `{}` holds an editable loan of it", f.local(owner.local).name, f.local(holder).name));
        }
    }
}

/// C1.effects and C1.capability: effects the IR shows (I/O through host intrinsics or callees, nondeterminism,
/// failure) are in the declared summary; a pure function has no I/O, mutation or nondeterminism; I/O needs an
/// authority parameter.
fn effects(ctx: &mut Ctx, f: &Function, body: &Block) {
    let mut found: BTreeSet<&'static str> = BTreeSet::new();
    let mut lines: HashMap<&'static str, u32> = HashMap::new();
    // a host call whose receiver does not come from an authority: a parameter, `self`, a capture, or main's own
    let mut direct_io: Option<u32> = None;
    let funcs = &ctx.funcs;
    // locals that hold an authority or a value obtained through one (a File, a subtree, a frozen clock)
    let mut derived: HashSet<LocalId> = f.locals.iter().enumerate().filter(|(_, l)| matches!(l.kind, LocalKind::Param | LocalKind::SelfParam | LocalKind::Capture)).map(|(i, _)| i as LocalId).collect();
    fn through(e: &Expr, derived: &HashSet<LocalId>) -> bool {
        match &e.kind {
            ExprKind::Place(p, _) => derived.contains(&p.local),
            ExprKind::OrFail(a) | ExprKind::OrRecover(a, _) | ExprKind::Catch { value: a, .. } | ExprKind::Present(a) | ExprKind::Field(a, _) => through(a, derived),
            ExprKind::Call { callee: Callee::Intrinsic(n), args } => n == "Host.capabilities" || args.first().map_or(false, |a| through(&a.value, derived)),
            _ => false,
        }
    }
    for _ in 0..2 {
        walk_block(body, &mut |n| {
            if let Node::Stmt(s) = n {
                if let StmtKind::Let { local, init } = &s.kind {
                    if through(init, &derived) {
                        derived.insert(*local);
                    }
                }
            }
        });
    }
    let from_authority = |e: &Expr| -> bool { through(e, &derived) };
    walk_block(body, &mut |n| match n {
        Node::Expr(e) => match &e.kind {
            ExprKind::Call { callee: Callee::Intrinsic(name), args } => {
                let fam = name.split('.').next().unwrap_or("");
                if HOST_IO.contains(&name.as_str()) || matches!(fam, "FileSystem" | "File") {
                    found.insert("io");
                    lines.entry("io").or_insert(e.line);
                    if !args.first().map_or(false, |a| from_authority(&a.value)) {
                        direct_io.get_or_insert(e.line);
                    }
                }
                if NONDET.contains(&name.as_str()) {
                    found.insert("nondet");
                    lines.entry("nondet").or_insert(e.line);
                }
            }
            ExprKind::Call { callee: Callee::Func(name), .. } => {
                if let Some(g) = funcs.get(name.as_str()) {
                    if g.effects.has("io") {
                        found.insert("io");
                        lines.entry("io").or_insert(e.line);
                    }
                }
            }
            ExprKind::OrFail(_) | ExprKind::Fail(_) => {
                found.insert("fail");
                lines.entry("fail").or_insert(e.line);
            }
            _ => {}
        },
        Node::Stmt(s) => {
            if let StmtKind::Fail(_) = s.kind {
                found.insert("fail");
                lines.entry("fail").or_insert(s.line);
            }
        }
        _ => {}
    });
    let woven = f.label.contains('#') || f.label.starts_with("aspect ") || f.label.starts_with("test ");
    for e in &found {
        if !f.effects.has(e) && !woven && !(*e == "fail" && !f.effects.effects.is_empty() && f.is_try()) {
            if *e == "fail" && f.is_try() {
                continue;
            }
            ctx.v("C1.effects", f, lines[e], format!("the body has `{e}`, which the summary does not declare"));
        }
    }
    if f.has("pure") {
        for e in ["io", "mutate", "nondet", "block", "suspend"] {
            if f.effects.has(e) || found.contains(e) {
                ctx.v("C1.effects", f, f.line, format!("{} is pure but has `{e}`", f.label));
            }
        }
    }
    // I/O through a host capability needs an authority parameter (11.6); reporting goes to the task's reporter
    if let Some(line) = direct_io {
        if f.label != "function main" && !woven {
            ctx.v("C1.capability", f, line, format!("{} performs I/O through a capability that no parameter, receiver or capture supplies", f.label));
        }
    }
}

/// C1.lambda: a `fn` lambda neither assigns nor moves its captures; a `fn mut` lambda does not move them out.
fn lambda_kind(ctx: &mut Ctx, f: &Function, body: &Block) {
    let c = match &f.closure {
        Some(c) => c,
        None => return,
    };
    let caps: HashSet<LocalId> = c.captures.iter().map(|x| x.local).collect();
    let (mut mutates, mut moves) = (None, None);
    walk_block(body, &mut |n| match n {
        Node::Stmt(s) => match &s.kind {
            StmtKind::Assign { place, .. } | StmtKind::Compound { place, .. } if caps.contains(&place.local) => {
                mutates.get_or_insert(s.line);
            }
            _ => {}
        },
        Node::Expr(e) => match &e.kind {
            ExprKind::Place(p, Use::Editable) if caps.contains(&p.local) => {
                mutates.get_or_insert(e.line);
            }
            ExprKind::Place(p, Use::Move) if caps.contains(&p.local) && p.proj.is_empty() => {
                moves.get_or_insert(e.line);
            }
            _ => {}
        },
        _ => {}
    });
    if let Some(l) = moves {
        if c.kind != "fn once" {
            ctx.v("C1.lambda", f, l, format!("this `{}` lambda moves a capture out: it needs `fn once`", c.kind));
        }
    }
    if let Some(l) = mutates {
        if c.kind == "fn" {
            ctx.v("C1.lambda", f, l, "this `fn` lambda mutates a capture: it needs `fn mut`");
        }
    }
}

/// C1.observation: on every exit, the frame fields the schema requires on it are captured on every path.
fn observation(ctx: &mut Ctx, f: &Function, body: &Block) {
    let schema = match &f.observe {
        Some(s) => s,
        None => return,
    };
    let fields = match ctx.types.get(schema.as_str()).map(|d| &d.kind) {
        Some(TypeKind::Observation { fields, .. }) => fields.clone(),
        _ => return,
    };
    let need = |kind: &str| -> Vec<String> { fields.iter().filter(|(_, _, r)| r == "all" || r == kind).map(|(n, _, _)| n.clone()).collect() };
    let (succ, fail) = (need("success"), need("failure"));
    let mut problems: Vec<(u32, String)> = Vec::new();
    fn walk_obs(b: &Block, mut have: BTreeSet<String>, succ: &[String], fail: &[String], problems: &mut Vec<(u32, String)>) -> (BTreeSet<String>, bool) {
        for s in &b.stmts {
            let (h, term) = stmt_obs(s, have, succ, fail, problems);
            have = h;
            if term {
                return (have, true);
            }
        }
        (have, false)
    }
    fn exit_check(have: &BTreeSet<String>, need: &[String], line: u32, what: &str, problems: &mut Vec<(u32, String)>) {
        for n in need {
            if !have.contains(n) {
                problems.push((line, format!("`{n}` is not captured on every path to this {what} exit")));
            }
        }
    }
    fn fails_inside(e: &Expr) -> bool {
        let mut hit = false;
        walk_expr(e, &mut |n| {
            if let Node::Expr(x) = n {
                if matches!(x.kind, ExprKind::OrFail(_) | ExprKind::Fail(_)) {
                    hit = true;
                }
            }
        });
        hit
    }
    fn stmt_obs(s: &Stmt, mut have: BTreeSet<String>, succ: &[String], fail: &[String], problems: &mut Vec<(u32, String)>) -> (BTreeSet<String>, bool) {
        let exprs: Vec<&Expr> = match &s.kind {
            StmtKind::Let { init, .. } | StmtKind::LetPattern { init, .. } => vec![init],
            StmtKind::Assign { value, .. } | StmtKind::Compound { value, .. } | StmtKind::Expr(value) | StmtKind::Discard(value) => vec![value],
            _ => vec![],
        };
        if exprs.iter().any(|e| fails_inside(e)) {
            exit_check(&have, fail, s.line, "failure", problems);
        }
        match &s.kind {
            StmtKind::Capture { field, .. } => {
                have.insert(field.clone());
                (have, false)
            }
            StmtKind::Return(_) => {
                exit_check(&have, succ, s.line, "success", problems);
                (have, true)
            }
            StmtKind::Fail(_) => {
                exit_check(&have, fail, s.line, "failure", problems);
                (have, true)
            }
            StmtKind::If { then, els, .. } => {
                let (a, ta) = walk_obs(then, have.clone(), succ, fail, problems);
                let (b, tb) = match els {
                    Some(e) => walk_obs(e, have.clone(), succ, fail, problems),
                    None => (have.clone(), false),
                };
                let joined: BTreeSet<String> = match (ta, tb) {
                    (true, true) => return (a, true),
                    (true, false) => b,
                    (false, true) => a,
                    (false, false) => a.intersection(&b).cloned().collect(),
                };
                (joined, false)
            }
            StmtKind::Block(b) => walk_obs(b, have, succ, fail, problems),
            StmtKind::Loop { body, .. } | StmtKind::For { body, .. } => {
                walk_obs(body, have.clone(), succ, fail, problems);
                (have, false)
            }
            StmtKind::Match { arms, .. } => {
                let mut acc: Option<BTreeSet<String>> = None;
                let mut all_term = !arms.is_empty();
                for a in arms {
                    let (h, t) = match &a.body {
                        ArmBody::Block(b) => walk_obs(b, have.clone(), succ, fail, problems),
                        ArmBody::Expr(_) => (have.clone(), false),
                    };
                    if !t {
                        all_term = false;
                        acc = Some(match acc {
                            None => h,
                            Some(x) => x.intersection(&h).cloned().collect(),
                        });
                    }
                }
                (acc.unwrap_or(have), all_term)
            }
            _ => (have, false),
        }
    }
    let (have, term) = walk_obs(body, BTreeSet::new(), &succ, &fail, &mut problems);
    if !term {
        exit_check(&have, &succ, body.line, "success", &mut problems);
    }
    for (line, m) in problems {
        ctx.v("C1.observation", f, line, m);
    }
}

/// C1.move: a whole local moved on some path is not used again until it is reassigned.
fn use_after_move(ctx: &mut Ctx, f: &Function, body: &Block) {
    let mut problems: Vec<(u32, String)> = Vec::new();
    fn note(p: &Place, u: Use, line: u32, moved: &mut BTreeSet<LocalId>, f: &Function, problems: &mut Vec<(u32, String)>) {
        if moved.contains(&p.local) {
            problems.push((line, format!("`{}` is used after it was moved", f.local(p.local).name)));
        }
        if u == Use::Move && p.proj.is_empty() && !f.local(p.local).boxed {
            moved.insert(p.local);
        }
    }
    /// One expression, in evaluation order. A `match` or `if` forks the moved set per arm: the arms are mutually
    /// exclusive, so a move in one is not a use after move in another (the same join as a statement `match`).
    fn ex(e: &Expr, moved: &mut BTreeSet<LocalId>, f: &Function, problems: &mut Vec<(u32, String)>) {
        match &e.kind {
            ExprKind::Const(_) | ExprKind::Global(_) | ExprKind::FuncRef(_) => {}
            ExprKind::Place(p, u) => note(p, *u, e.line, moved, f, problems),
            ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) | ExprKind::Index(a, b) => {
                ex(a, moved, f, problems);
                ex(b, moved, f, problems);
            }
            ExprKind::Unary(_, a)
            | ExprKind::Codec { arg: a, .. }
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
            | ExprKind::ToDynamic { value: a, .. } => ex(a, moved, f, problems),
            // A statement temporary evaluates its initialiser. Its local is not a user place, so destroying
            // it at the end of the statement is not a use of a moved place, and a value taken back out of it
            // (an owned method) is not destroyed again.
            ExprKind::Temp { value: a, .. } => ex(a, moved, f, problems),
            ExprKind::Call { callee, args } => {
                if let Callee::Value(v) = callee {
                    ex(v, moved, f, problems);
                }
                for a in args {
                    ex(&a.value, moved, f, problems);
                }
            }
            ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => {
                for (_, x) in fields {
                    ex(x, moved, f, problems);
                }
            }
            ExprKind::Tuple(es) | ExprKind::Closure { captures: es, .. } => {
                for x in es {
                    ex(x, moved, f, problems);
                }
            }
            ExprKind::Interpolate(parts) => {
                for p in parts {
                    if let Part::Show(x) = p {
                        ex(x, moved, f, problems);
                    }
                }
            }
            ExprKind::Catch { value, handler, .. } => {
                ex(value, moved, f, problems);
                bl(handler, moved, f, problems);
            }
            ExprKind::Block(b) => {
                bl(b, moved, f, problems);
            }
            ExprKind::If { cond, then, els } => {
                ex(cond, moved, f, problems);
                let mut a = moved.clone();
                let ta = bl(then, &mut a, f, problems);
                let mut b = moved.clone();
                let tb = bl(els, &mut b, f, problems);
                if !(ta && tb) {
                    *moved = match (ta, tb) {
                        (true, false) => b,
                        (false, true) => a,
                        _ => a.union(&b).cloned().collect(),
                    };
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                ex(scrutinee, moved, f, problems);
                let start = moved.clone();
                let mut acc: BTreeSet<LocalId> = BTreeSet::new();
                let mut all_term = !arms.is_empty();
                for a in arms {
                    let mut m = start.clone();
                    for l in &a.locals {
                        m.remove(l);
                    }
                    if let Some(g) = &a.guard {
                        ex(g, &mut m, f, problems);
                    }
                    let t = match &a.body {
                        ArmBody::Expr(e) => {
                            ex(e, &mut m, f, problems);
                            false
                        }
                        ArmBody::Block(b) => bl(b, &mut m, f, problems),
                    };
                    if !t {
                        all_term = false;
                        acc = acc.union(&m).cloned().collect();
                    }
                }
                if !all_term {
                    *moved = acc;
                }
            }
        }
    }
    fn bl(b: &Block, moved: &mut BTreeSet<LocalId>, f: &Function, problems: &mut Vec<(u32, String)>) -> bool {
        for s in &b.stmts {
            if st(s, moved, f, problems) {
                return true;
            }
        }
        false
    }
    fn st(s: &Stmt, moved: &mut BTreeSet<LocalId>, f: &Function, problems: &mut Vec<(u32, String)>) -> bool {
        match &s.kind {
            StmtKind::Let { local, init } => {
                ex(init, moved, f, problems);
                moved.remove(local);
                false
            }
            StmtKind::LetPattern { init, .. } => {
                ex(init, moved, f, problems);
                false
            }
            StmtKind::Assign { place, value, .. } => {
                ex(value, moved, f, problems);
                if place.proj.is_empty() {
                    moved.remove(&place.local);
                }
                false
            }
            StmtKind::Compound { value, .. } | StmtKind::Expr(value) | StmtKind::Discard(value) | StmtKind::Capture { value, .. } => {
                ex(value, moved, f, problems);
                false
            }
            StmtKind::Report { value, .. } => {
                ex(value, moved, f, problems);
                false
            }
            StmtKind::Record { fields, .. } => {
                fields.iter().for_each(|(_, e)| ex(e, moved, f, problems));
                false
            }
            StmtKind::Expect { cond, .. } => {
                ex(cond, moved, f, problems);
                false
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    ex(e, moved, f, problems);
                }
                true
            }
            StmtKind::Give(e) | StmtKind::Recover(e) | StmtKind::Fail(e) => {
                ex(e, moved, f, problems);
                true
            }
            StmtKind::Break(_) | StmtKind::Continue(_) => true,
            StmtKind::If { cond, then, els } => {
                ex(cond, moved, f, problems);
                let mut a = moved.clone();
                let ta = bl(then, &mut a, f, problems);
                let mut b = moved.clone();
                let tb = match els {
                    Some(e) => bl(e, &mut b, f, problems),
                    None => false,
                };
                *moved = match (ta, tb) {
                    (true, true) => return true,
                    (true, false) => b,
                    (false, true) => a,
                    (false, false) => a.union(&b).cloned().collect(),
                };
                false
            }
            StmtKind::Block(b) | StmtKind::Scope { body: b, .. } => bl(b, moved, f, problems),
            StmtKind::Defer(_) => false,
            StmtKind::Drop(p) => {
                if p.proj.is_empty() {
                    moved.insert(p.local);
                }
                false
            }
            StmtKind::Loop { body, .. } | StmtKind::For { body, .. } => {
                let mut fresh: Option<LocalId> = None;
                if let StmtKind::For { source, binding, .. } = &s.kind {
                    match source {
                        ForSource::Range { lo, hi, .. } => {
                            ex(lo, moved, f, problems);
                            ex(hi, moved, f, problems);
                        }
                        ForSource::Items(e) => ex(e, moved, f, problems),
                    }
                    fresh = *binding;
                }
                // twice: a move in one iteration reaches the next (the loop binding is fresh in each)
                let mut once = moved.clone();
                if let Some(b) = fresh {
                    once.remove(&b);
                }
                bl(body, &mut once, f, problems);
                let mut twice = once.clone();
                if let Some(b) = fresh {
                    twice.remove(&b);
                }
                bl(body, &mut twice, f, problems);
                *moved = moved.union(&once).cloned().collect();
                false
            }
            StmtKind::Match { scrutinee, arms } => {
                ex(scrutinee, moved, f, problems);
                let mut acc: BTreeSet<LocalId> = BTreeSet::new();
                let start = moved.clone();
                let mut all_term = !arms.is_empty();
                for a in arms {
                    let mut m = start.clone();
                    for l in &a.locals {
                        m.remove(l);
                    }
                    if let Some(g) = &a.guard {
                        ex(g, &mut m, f, problems);
                    }
                    let t = match &a.body {
                        ArmBody::Expr(e) => {
                            ex(e, &mut m, f, problems);
                            false
                        }
                        ArmBody::Block(b) => bl(b, &mut m, f, problems),
                    };
                    if !t {
                        all_term = false;
                        acc = acc.union(&m).cloned().collect();
                    }
                }
                if !all_term {
                    *moved = acc;
                }
                all_term
            }
        }
    }
    let mut moved = BTreeSet::new();
    bl(body, &mut moved, f, &mut problems);
    problems.sort();
    problems.dedup();
    for (line, m) in problems {
        ctx.v("C1.move", f, line, m);
    }
}

/// C1.noalloc and the C.3 patterns: every operation of a `noalloc` function is on the whitelist.
fn noalloc(ctx: &mut Ctx, f: &Function, body: &Block) {
    if !f.has("noalloc") {
        return;
    }
    if f.effects.has("alloc") {
        ctx.v("C1.noalloc", f, f.line, "a noalloc function whose summary has `alloc`");
    }
    let mut found: Vec<(&'static str, u32, String)> = Vec::new();
    let c: &Ctx = ctx;
    walk_block(body, &mut |n| {
        if let Node::Expr(e) = n {
            match &e.kind {
                ExprKind::Call { callee: Callee::Intrinsic(name), .. } => {
                    if GROWTH.contains(&name.as_str()) {
                        found.push(("C3.growth", e.line, format!("`{name}` grows a List, String or Map")));
                    }
                    if name == "Shared.create" {
                        found.push(("C3.shared-create", e.line, "`Shared.create` allocates".into()));
                    }
                }
                ExprKind::Call { callee: Callee::Func(name), .. } => {
                    if let Some(g) = c.funcs.get(name.as_str()) {
                        if g.effects.has("alloc") {
                            found.push(("C3.call", e.line, format!("calls `{name}`, whose summary has `alloc`")));
                        }
                    }
                }
                ExprKind::Interpolate(_) => found.push(("C3.growth", e.line, "an interpolated string grows a String".into())),
                ExprKind::ToDynamic { borrowed: false, iface, .. } => found.push(("C3.dynamic-create", e.line, format!("an owned `dynamic {iface}` is boxed"))),
                ExprKind::Place(_, Use::Copy) if c.heap(&e.ty) => found.push(("C3.copy-heap", e.line, format!("`copy` of a heap-owning {}", e.ty.show()))),
                ExprKind::MakeFailure { fields, ty } => {
                    if fields.iter().any(|(_, x)| c.heap(&x.ty)) {
                        found.push(("C3.failure-payload", e.line, format!("failure {ty} carries a heap payload (an extended payload allocates)")));
                    }
                }
                ExprKind::Construct { ty, fields } => {
                    // a class's storage is inline (C.3, X-137): constructing it allocates only when a field value is a
                    // heap value created here; a moved value, a retained handle, a fresh aggregate (visited on its own)
                    // and a call (C3.call) allocate nowhere in this expression
                    let created_here = |x: &Expr| {
                        c.heap(&x.ty)
                            && !matches!(&x.kind,
                                ExprKind::Place(_, Use::Move)
                                | ExprKind::Construct { .. }
                                | ExprKind::Variant { .. }
                                | ExprKind::Tuple(_)
                                | ExprKind::Record(_)
                                | ExprKind::Call { callee: Callee::Func(_), .. })
                            && !matches!(&x.kind, ExprKind::Call { callee: Callee::Intrinsic(n), .. } if n.ends_with("retain") || n.ends_with("downgrade"))
                    };
                    let inline = match c.types.get(ty.as_str()).map(|d| (d.trivial, &d.kind)) {
                        Some((true, _)) => true,
                        Some((false, TypeKind::Class { fields: fds })) => fds.iter().all(|fd| !c.heap(&fd.ty)) || !fields.iter().any(|(_, x)| created_here(x)),
                        _ => !fields.iter().any(|(_, x)| created_here(x)),
                    };
                    if !inline {
                        found.push(("C3.inline-construct", e.line, format!("{ty} is neither trivial nor inline: constructing it allocates")));
                    }
                }
                _ => {}
            }
        }
    });
    for (id, line, m) in found {
        ctx.v(id, f, line, format!("{m} in noalloc {}", f.label));
    }
}

/// C1.cleanup: a function that destroys a value whose class has `cleanup` includes the cleanup's effects.
fn cleanup_effects(ctx: &mut Ctx, f: &Function, body: &Block) {
    let mut dropped: Vec<(LocalId, String)> = Vec::new();
    walk_block(body, &mut |n| {
        if let Node::Block(b) = n {
            for l in &b.locals {
                if let Some(tn) = type_name(&f.local(*l).ty) {
                    dropped.push((*l, tn.to_string()));
                }
            }
        }
    });
    for (l, tn) in dropped {
        let cleanup = match ctx.types.get(tn.as_str()).and_then(|d| d.cleanup.clone()) {
            Some(c) => c,
            None => continue,
        };
        let moved = {
            let mut m = false;
            walk_block(body, &mut |n| {
                if let Node::Expr(e) = n {
                    if let ExprKind::Place(p, Use::Move) = &e.kind {
                        if p.local == l {
                            m = true;
                        }
                    }
                }
            });
            m
        };
        if moved {
            continue;
        }
        let ce: Vec<String> = ctx.funcs.get(cleanup.as_str()).map(|g| g.effects.effects.iter().map(|(e, _)| e.clone()).collect()).unwrap_or_default();
        for e in ce {
            if e != "mutate" && !f.effects.has(&e) && !f.label.contains('#') {
                ctx.v("C1.cleanup", f, f.line, format!("destroying `{}` runs the cleanup of {tn}, whose `{e}` the summary does not include", f.local(l).name));
            }
        }
    }
}

// ---- the asynchronous rows (DEL-39) ------------------------------------------------------------------------------

fn contains(b: &Block, pred: &dyn Fn(&Expr) -> bool) -> bool {
    let mut hit = false;
    walk_block(b, &mut |n| {
        if let Node::Expr(e) = n {
            if pred(e) {
                hit = true;
            }
        }
    });
    hit
}

fn stmt_contains(s: &Stmt, pred: &dyn Fn(&Expr) -> bool) -> bool {
    let mut hit = false;
    walk_stmt(s, &mut |n| {
        if let Node::Expr(e) = n {
            if pred(e) {
                hit = true;
            }
        }
    });
    hit
}

fn is_await(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Await(_))
}

/// The locals an expression tree reads, by place.
fn places_in(b: &Block) -> Vec<(LocalId, u32)> {
    let mut out = vec![];
    walk_block(b, &mut |n| {
        if let Node::Expr(e) = n {
            if let ExprKind::Place(p, _) = &e.kind {
                out.push((p.local, e.line));
            }
        }
    });
    out
}

fn launched_callee<'a>(e: &'a Expr) -> Option<(&'a str, &'a Expr, &'a [Arg])> {
    if let ExprKind::Call { callee: Callee::Intrinsic(n), args } = &e.kind {
        if n.starts_with("Scope.launch") {
            if let Some(fut) = args.get(1) {
                if let ExprKind::Call { args: fargs, .. } = &fut.value.kind {
                    return Some((n.as_str(), &fut.value, fargs.as_slice()));
                }
                return Some((n.as_str(), &fut.value, &[]));
            }
        }
    }
    None
}

/// C1.mutex, C1.async, C1.scope, C1.launch-parallel, C1.defer, and the C.3 patterns C3.defer, C3.launch,
/// C3.boxed-future, C3.parallel.
fn async_rows(ctx: &mut Ctx, f: &Function, body: &Block) {
    // C1.mutex: no MutexGuard is live across an await (12.3)
    fn guards(ctx: &mut Ctx, f: &Function, b: &Block, live: &mut Vec<LocalId>) {
        let base = live.len();
        for s in &b.stmts {
            if !live.is_empty() && stmt_contains(s, &is_await) && !matches!(s.kind, StmtKind::Block(_) | StmtKind::If { .. } | StmtKind::Loop { .. } | StmtKind::For { .. } | StmtKind::Scope { .. } | StmtKind::Match { .. }) {
                let g = live[live.len() - 1];
                ctx.v("C1.mutex", f, s.line, format!("the MutexGuard `{}` is live across an await", f.local(g).name));
            }
            match &s.kind {
                StmtKind::Let { local, .. } if type_name(&f.local(*local).ty) == Some("MutexGuard") => live.push(*local),
                StmtKind::Block(inner) | StmtKind::Loop { body: inner, .. } | StmtKind::For { body: inner, .. } | StmtKind::Scope { body: inner, .. } => guards(ctx, f, inner, live),
                StmtKind::If { then, els, .. } => {
                    guards(ctx, f, then, live);
                    if let Some(e) = els {
                        guards(ctx, f, e, live);
                    }
                }
                _ => {}
            }
        }
        live.truncate(base);
    }
    guards(ctx, f, body, &mut vec![]);

    // C1.async: a future is linear: a future-typed local is consumed (awaited, launched, returned) at least once
    let mut futures: Vec<(LocalId, u32)> = vec![];
    walk_block(body, &mut |n| {
        if let Node::Stmt(s) = n {
            if let StmtKind::Let { local, .. } = &s.kind {
                if type_name(&f.local(*local).ty) == Some("Future") {
                    futures.push((*local, s.line));
                }
            }
        }
    });
    let reads = places_in(body);
    for (l, line) in futures {
        if !reads.iter().any(|(r, _)| *r == l) {
            ctx.v("C1.async", f, line, format!("the future `{}` is never awaited, launched or returned (a future is linear)", f.local(l).name));
        }
    }

    // C1.scope: a task launched on a scope does not leave it (no return or outward store of a Task)
    fn scopes(ctx: &mut Ctx, f: &Function, b: &Block, outer: &[LocalId]) {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Scope { body, .. } => {
                    let mut tasks: Vec<LocalId> = vec![];
                    walk_block(body, &mut |n| {
                        if let Node::Stmt(st) = n {
                            if let StmtKind::Let { local, .. } = &st.kind {
                                if type_name(&f.local(*local).ty) == Some("Task") {
                                    tasks.push(*local);
                                }
                            }
                        }
                    });
                    let mut escapes: Vec<(u32, LocalId)> = vec![];
                    walk_block(body, &mut |n| {
                        if let Node::Stmt(st) = n {
                            match &st.kind {
                                StmtKind::Return(Some(Expr { kind: ExprKind::Place(p, _), .. })) if tasks.contains(&p.local) => escapes.push((st.line, p.local)),
                                StmtKind::Assign { place, value: Expr { kind: ExprKind::Place(p, _), .. }, .. } if tasks.contains(&p.local) && outer.contains(&place.local) => escapes.push((st.line, p.local)),
                                _ => {}
                            }
                        }
                    });
                    for (line, t) in escapes {
                        ctx.v("C1.scope", f, line, format!("the task `{}` leaves the scope it was launched on (it is joined at the scope's exit)", f.local(t).name));
                    }
                    let mut inner_outer = outer.to_vec();
                    inner_outer.extend(b.locals.iter().copied());
                    scopes(ctx, f, body, &inner_outer);
                }
                StmtKind::Block(inner) | StmtKind::Loop { body: inner, .. } | StmtKind::For { body: inner, .. } => scopes(ctx, f, inner, outer),
                StmtKind::If { then, els, .. } => {
                    scopes(ctx, f, then, outer);
                    if let Some(e) = els {
                        scopes(ctx, f, e, outer);
                    }
                }
                _ => {}
            }
        }
    }
    let params: Vec<LocalId> = f.params.iter().map(|p| p.local).collect();
    let mut outer = params.clone();
    outer.extend(body.locals.iter().copied());
    scopes(ctx, f, body, &outer);

    // C1.launch-parallel: a future launched on a parallel pool holds no loan (Shareable; C.2)
    let mut loans: Vec<(u32, String)> = vec![];
    walk_block(body, &mut |n| {
        if let Node::Expr(e) = n {
            if let Some((name, _, args)) = launched_callee(e) {
                if name == "Scope.launchParallel" {
                    for a in args {
                        let loan = matches!(a.value.kind, ExprKind::Place(_, Use::Alias | Use::Editable)) || matches!(a.value.ty, fortis_types::types::Type::Loan { .. });
                        if loan {
                            loans.push((e.line, a.param.clone()));
                        }
                    }
                }
            }
        }
    });
    for (line, p) in loans {
        ctx.v("C1.launch-parallel", f, line, format!("the future launched on a parallel pool holds a loan in `{p}` (it must own what it holds)"));
    }

    // C1.defer: a deferred block uses only places initialised where it is registered
    fn defers(ctx: &mut Ctx, f: &Function, b: &Block, bound: &mut Vec<LocalId>) {
        let base = bound.len();
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let { local, .. } => bound.push(*local),
                StmtKind::LetPattern { .. } => {}
                StmtKind::Defer(i) => {
                    if let Some(d) = b.defers.get(*i) {
                        let own: Vec<LocalId> = {
                            let mut v = vec![];
                            walk_block(d, &mut |n| {
                                if let Node::Block(x) = n {
                                    v.extend(x.locals.iter().copied());
                                }
                            });
                            v
                        };
                        for (l, line) in places_in(d) {
                            if !bound.contains(&l) && !own.contains(&l) && f.local(l).kind != LocalKind::Capture {
                                ctx.v("C1.defer", f, line, format!("the deferred block uses `{}`, which is not initialised where the block is registered", f.local(l).name));
                            }
                        }
                    }
                }
                StmtKind::Block(inner) | StmtKind::Loop { body: inner, .. } | StmtKind::Scope { body: inner, .. } => defers(ctx, f, inner, bound),
                StmtKind::For { binding, body: inner, .. } => {
                    if let Some(l) = binding {
                        bound.push(*l);
                    }
                    defers(ctx, f, inner, bound);
                }
                StmtKind::If { then, els, .. } => {
                    defers(ctx, f, then, bound);
                    if let Some(e) = els {
                        defers(ctx, f, e, bound);
                    }
                }
                _ => {}
            }
        }
        bound.truncate(base);
    }
    let mut bound: Vec<LocalId> = params;
    bound.extend(f.closure.as_ref().map(|c| c.captures.iter().map(|x| x.local).collect::<Vec<_>>()).unwrap_or_default());
    // pattern, catch and report bindings are bound where they are introduced; treat them as bound throughout
    bound.extend(f.locals.iter().enumerate().filter(|(_, l)| matches!(l.kind, LocalKind::Binding | LocalKind::Temp | LocalKind::SelfParam)).map(|(i, _)| i as LocalId));
    defers(ctx, f, body, &mut bound);

    // C3.defer (noalloc): a deferred block is on the whitelist only when its body is
    if f.has("noalloc") {
        let c: &Ctx = ctx;
        let mut found: Vec<u32> = vec![];
        walk_block(body, &mut |n| {
            if let Node::Block(b) = n {
                for d in &b.defers {
                    if contains(d, &|e| allocates(c, e)) {
                        found.push(d.line);
                    }
                }
            }
        });
        for line in found {
            ctx.v("C3.defer", f, line, format!("a deferred block of noalloc {} allocates", f.label));
        }
    }

    // C3.launch: a cooperative launch is allocation-free only for a future that fits the scope's inline storage
    // declared by the runtime host; the v1 host declares none (SP-04-async, X-125), so every launch allocates and
    // the summary must say so
    if HOST_INLINE_TASK_STORAGE == 0 && !f.effects.has("alloc") {
        let mut found: Vec<u32> = vec![];
        walk_block(body, &mut |n| {
            if let Node::Expr(e) = n {
                if let Some(("Scope.launch", _, _)) = launched_callee(e) {
                    found.push(e.line);
                }
            }
        });
        for line in found {
            ctx.v("C3.launch", f, line, format!("a cooperative launch allocates (the v1 runtime host declares no inline task storage), but the summary of {} has no `alloc`", f.label));
        }
    }

    // C3.boxed-future and C3.parallel: the operations allocate, so the summary has `alloc`
    if !f.effects.has("alloc") {
        let mut boxed: Vec<u32> = vec![];
        let mut pools: Vec<(u32, String)> = vec![];
        let c: &Ctx = ctx;
        walk_block(body, &mut |n| {
            if let Node::Expr(e) = n {
                if let ExprKind::Call { callee: Callee::Virtual { method }, args } = &e.kind {
                    let dynamic = args.first().map_or(false, |a| matches!(fortis_types::types::strip(&a.value.ty), fortis_types::types::Type::Dynamic(_)));
                    if dynamic && async_method(c, &args[0].value.ty, method) {
                        boxed.push(e.line);
                    }
                }
                if let ExprKind::Call { callee: Callee::Intrinsic(name), .. } = &e.kind {
                    if name == "Scope.launchParallel" || name == "Scope.launchBlocking" {
                        pools.push((e.line, name.clone()));
                    }
                }
            }
        });
        for line in boxed {
            ctx.v("C3.boxed-future", f, line, format!("an async call through `dynamic` boxes its future, but the summary of {} has no `alloc`", f.label));
        }
        for (line, name) in pools {
            ctx.v("C3.parallel", f, line, format!("`{name}` allocates, but the summary of {} has no `alloc`", f.label));
        }
    }
}

/// Whether an expression allocates (the C.3 list).
fn allocates(c: &Ctx, e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Call { callee: Callee::Intrinsic(name), .. } => GROWTH.contains(&name.as_str()) || name == "Shared.create",
        ExprKind::Call { callee: Callee::Func(name), .. } => c.funcs.get(name.as_str()).map_or(false, |g| g.effects.has("alloc")),
        ExprKind::Interpolate(_) => true,
        ExprKind::ToDynamic { borrowed: false, .. } => true,
        ExprKind::Place(_, Use::Copy) => c.heap(&e.ty),
        _ => false,
    }
}

/// Whether a dynamic receiver's interface method is `async`.
fn async_method(c: &Ctx, recv: &fortis_types::types::Type, method: &str) -> bool {
    let iface = match fortis_types::types::strip(recv) {
        fortis_types::types::Type::Dynamic(n) => n.name.clone(),
        _ => return false,
    };
    match c.types.get(iface.as_str()).map(|d| &d.kind) {
        Some(TypeKind::Interface { methods }) => methods.iter().any(|(m, mods)| m == method && mods.iter().any(|x| x == "async")),
        _ => false,
    }
}
