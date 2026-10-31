//! The production native lowering (Delivery Programme 5.3; DEL-36): a validated typed-IR program (fortis-mir,
//! accepted by fortis-validate) becomes one safe Rust crate (`#![forbid(unsafe_code)]`) over the native runtime
//! (fortis-native). Classes become structs, enums enums, interfaces traits, `cleanup` an `impl Drop`; parameter
//! modes become `&T`, `&mut T` and `T`, so rustc re-checks every loan; closures stay concrete closures; every
//! `.clone()` and reference-count operation the crate contains is attributed to a source `copy` or `retain`.

pub mod codec;
pub mod gen;
pub mod intrinsics;
pub mod types;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use fortis_mir::model::*;
use fortis_types::types::{strip, NamedT, Type};

use gen::{indent, FnGen};
use types::*;

#[derive(Clone, Debug, PartialEq)]
pub struct LowerError {
    pub function: String,
    pub line: u32,
    pub message: String,
}

/// The copy and reference-count audit (DEL-36 criterion 1): every clone the crate contains, with its source.
#[derive(Clone, Debug, Default)]
pub struct Audit {
    /// (function, line, source construct): `copy` for a clone, `retain`/`upgrade` for a reference count
    pub clones: Vec<(String, u32, &'static str)>,
    pub refcounts: Vec<(String, u32, &'static str)>,
    /// closures boxed (only a `dynamic fn` may be)
    pub boxed_closures: usize,
    /// values returned by a closure whose result is a function type, boxed because a Rust closure cannot name an
    /// `impl Fn` result (Appendix C.1); each is `alloc` in the checker's summary of that closure (C.3; D-34)
    pub boxed_results: usize,
    pub records: Vec<Vec<(String, Type)>>,
}

impl Audit {
    pub fn clone_at(&mut self, f: &str, line: u32, why: &'static str) {
        self.clones.push((f.to_string(), line, why));
    }
    pub fn refcount_at(&mut self, f: &str, line: u32, why: &'static str) {
        self.refcounts.push((f.to_string(), line, why));
    }
}

pub struct Lowered {
    pub main_rs: String,
    pub cargo_toml: String,
    pub audit: Audit,
}

pub struct Options {
    /// the path of the fortis-native crate, for the generated crate's dependency
    pub runtime: String,
    pub crate_name: String,
    /// the file a failure names when its function is the entry module, or the path of a standalone module
    pub file: String,
    /// source name -> declaring module file, first name wins (interpreter `decl_file`)
    pub decl_files: Vec<(String, String)>,
    /// run the program's tests instead of `main`
    pub tests: bool,
}

/// `rt::records::set_decl_files` before the run, so a failure names the module that declares it.
fn decl_files_prelude(opts: &Options) -> String {
    if opts.decl_files.is_empty() {
        return String::new();
    }
    let pairs = opts.decl_files.iter().map(|(n, f)| format!("({n:?}, {f:?})")).collect::<Vec<_>>().join(", ");
    format!("    rt::records::set_decl_files(&[{pairs}]);\n")
}

/// A block whose every path leaves it (return, fail, a diverging expression).
pub fn terminates(b: &Block) -> bool {
    match b.stmts.last() {
        None => false,
        Some(s) => match &s.kind {
            StmtKind::Return(_) | StmtKind::Fail(_) | StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Give(_) | StmtKind::Recover(_) => true,
            // a Never call is followed by `unreachable!` in the lowered code
            StmtKind::Expr(e) => matches!(e.kind, ExprKind::Fail(_)) || name_of(&e.ty) == Some("Never"),
            StmtKind::If { then, els: Some(e), .. } => terminates(then) && terminates(e),
            StmtKind::Block(b) | StmtKind::Scope { body: b, .. } => terminates(b),
            StmtKind::Loop { body, .. } => !has_break(body),
            StmtKind::Match { arms, .. } => !arms.is_empty() && arms.iter().all(|a| matches!(&a.body, ArmBody::Block(b) if terminates(b))),
            _ => false,
        },
    }
}

fn has_break(b: &Block) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Break(_) => true,
        StmtKind::If { then, els, .. } => has_break(then) || els.as_ref().map_or(false, has_break),
        StmtKind::Block(b) => has_break(b),
        StmtKind::Match { arms, .. } => arms.iter().any(|a| matches!(&a.body, ArmBody::Block(b) if has_break(b))),
        _ => false,
    })
}

/// Functions, types and constants reachable from the user module (its functions, tests and types).
fn reachable(prog: &Program, tests: bool) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut fns: BTreeSet<String> = BTreeSet::new();
    let mut tys: BTreeSet<String> = BTreeSet::new();
    // a single file lowers every declaration (each accepted program is checked whole); a package lowers what
    // its entry, or with --tests its tests, reach (a package's main may use what its tests do not)
    let package = prog.functions.iter().any(|f| is_user_module(&f.module) && f.module != "main");
    let mut work: Vec<String> = if !package {
        prog.functions.iter().filter(|f| is_user_module(&f.module)).map(|f| f.name.clone()).collect()
    } else if tests {
        prog.tests.iter().map(|t| t.function.clone()).collect()
    } else {
        prog.entry.iter().cloned().collect()
    };
    if !package {
        for t in prog.types.iter().filter(|t| is_user_module(&t.module)) {
            tys.insert(t.name.clone());
            for (_, f) in &t.methods {
                work.push(f.clone());
            }
            if let Some(c) = &t.cleanup {
                work.push(c.clone());
            }
        }
    }
    fn note_ty(t: &Type, tys: &mut BTreeSet<String>) {
        match t {
            Type::Named(n) => {
                tys.insert(n.name.clone());
                n.args.iter().for_each(|a| note_ty(a, tys));
            }
            Type::Refined { base, .. } => {
                tys.insert(base.name.clone());
            }
            Type::Optional(i) => note_ty(i, tys),
            Type::Tuple(ts) => ts.iter().for_each(|a| note_ty(a, tys)),
            Type::Record(fs) => fs.iter().for_each(|(_, a)| note_ty(a, tys)),
            Type::Fn { params, ret, .. } => {
                params.iter().for_each(|a| note_ty(a, tys));
                note_ty(ret, tys);
            }
            Type::Loan { inner, .. } => note_ty(inner, tys),
            Type::Dynamic(n) => {
                tys.insert(n.name.clone());
            }
            _ => {}
        }
    }
    // a reachable type brings its methods (dispatch through an interface may reach any) and its cleanup; their
    // bodies are walked like any reachable function's, until nothing new is reached
    loop {
        while let Some(n) = work.pop() {
            if !fns.insert(n.clone()) {
                continue;
            }
            let f = match prog.function(&n) {
                Some(f) => f,
                None => continue,
            };
            for l in &f.locals {
                note_ty(&l.ty, &mut tys);
            }
            note_ty(&f.ret, &mut tys);
            // an interface of the program's that a `where` bound names becomes a trait, with those it requires, so
            // the bound has something to name (D-82)
            for (_, i, args) in &f.bounds {
                if is_user_iface(prog, i) {
                    for x in iface_closure(prog, i) {
                        tys.insert(x);
                    }
                }
                args.iter().for_each(|a| note_ty(a, &mut tys));
            }
            // a reachable associated function brings its type, even one never used as a value (`C.make()`)
            for t in prog.types.iter().filter(|t| t.methods.iter().any(|(_, m)| *m == n)) {
                tys.insert(t.name.clone());
            }
            if let Some(b) = &f.body {
                visit_block(b, &mut |e| {
                    note_ty(&e.ty, &mut tys);
                    match &e.kind {
                        ExprKind::Call { callee: Callee::Func(g), .. } | ExprKind::FuncRef(g) | ExprKind::Closure { func: g, .. } => work.push(g.clone()),
                        ExprKind::Construct { ty, .. } | ExprKind::Variant { ty, .. } | ExprKind::MakeFailure { ty, .. } => {
                            tys.insert(ty.clone());
                        }
                        _ => {}
                    }
                });
            }
        }
        let types_before = tys.len();
        let snapshot: Vec<String> = tys.iter().cloned().collect();
        for t in prog.types.iter().filter(|t| snapshot.contains(&t.name)) {
            for (_, f) in &t.methods {
                if !fns.contains(f) && prog.function(f).is_some() && is_user_module(&t.module) {
                    work.push(f.clone());
                }
            }
            if let Some(c) = &t.cleanup {
                if !fns.contains(c) {
                    work.push(c.clone());
                }
            }
            match &t.kind {
                TypeKind::Class { fields } => fields.iter().for_each(|fd| note_ty(&fd.ty, &mut tys)),
                TypeKind::Enum { variants } => variants.iter().for_each(|v| v.fields.iter().for_each(|(_, ft)| note_ty(ft, &mut tys))),
                // an interface's trait is written from its declaration: the types its signatures name, and the
                // interfaces of the program's that it requires (its supertraits)
                TypeKind::Interface { .. } if is_user_module(&t.module) => {
                    for s in &t.sigs {
                        s.params.iter().for_each(|(_, _, pt)| note_ty(pt, &mut tys));
                        note_ty(&s.ret, &mut tys);
                    }
                    for r in t.requires.iter().filter(|r| is_user_iface(prog, r)) {
                        tys.insert(r.clone());
                    }
                    t.iface_args.iter().for_each(|(_, a)| a.iter().for_each(|x| note_ty(x, &mut tys)));
                }
                _ => {}
            }
            t.iface_args.iter().for_each(|(_, a)| a.iter().for_each(|x| note_ty(x, &mut tys)));
        }
        // done when no new function is queued and no new type was noted
        if work.is_empty() && tys.len() == types_before {
            break;
        }
    }
    (fns, tys)
}

pub fn visit_block(b: &Block, f: &mut dyn FnMut(&Expr)) {
    for d in &b.defers {
        visit_block(d, f);
    }
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Let { init: e, .. } | StmtKind::LetPattern { init: e, .. } | StmtKind::Assign { value: e, .. } | StmtKind::Compound { value: e, .. } | StmtKind::Expr(e) | StmtKind::Discard(e) | StmtKind::Give(e) | StmtKind::Recover(e) | StmtKind::Fail(e) | StmtKind::Capture { value: e, .. } | StmtKind::Report { value: e, .. } => visit_expr(e, f),
            StmtKind::Return(Some(e)) => visit_expr(e, f),
            StmtKind::If { cond, then, els } => {
                visit_expr(cond, f);
                visit_block(then, f);
                if let Some(e) = els {
                    visit_block(e, f);
                }
            }
            // a `scope` body: a package lowers only what is reachable, and a scope's tasks reach their functions
            StmtKind::Loop { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => visit_block(body, f),
            StmtKind::For { source, body, .. } => {
                match source {
                    ForSource::Range { lo, hi, .. } => {
                        visit_expr(lo, f);
                        visit_expr(hi, f);
                    }
                    ForSource::Items(e) => visit_expr(e, f),
                }
                visit_block(body, f);
            }
            StmtKind::Match { scrutinee, arms } => {
                visit_expr(scrutinee, f);
                for a in arms {
                    if let Some(g) = &a.guard {
                        visit_expr(g, f);
                    }
                    match &a.body {
                        ArmBody::Expr(e) => visit_expr(e, f),
                        ArmBody::Block(b) => visit_block(b, f),
                    }
                }
            }
            StmtKind::Record { fields, .. } => fields.iter().for_each(|(_, e)| visit_expr(e, f)),
            StmtKind::Expect { cond, evidence } => {
                visit_expr(cond, f);
                if let Some((a, _, b)) = evidence {
                    visit_expr(a, f);
                    visit_expr(b, f);
                }
            }
            _ => {}
        }
    }
}

pub fn visit_expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) | ExprKind::Index(a, b) => {
            visit_expr(a, f);
            visit_expr(b, f);
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
        | ExprKind::Temp { value: a, .. }
        | ExprKind::Validate { value: a, .. }
        | ExprKind::Wrap { value: a, .. }
        | ExprKind::ToDynamic { value: a, .. } => visit_expr(a, f),
        ExprKind::Call { callee, args } => {
            if let Callee::Value(v) = callee {
                visit_expr(v, f);
            }
            args.iter().for_each(|a| visit_expr(&a.value, f));
        }
        ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => fields.iter().for_each(|(_, x)| visit_expr(x, f)),
        ExprKind::Tuple(es) | ExprKind::Closure { captures: es, .. } => es.iter().for_each(|x| visit_expr(x, f)),
        ExprKind::Interpolate(parts) => parts.iter().for_each(|p| {
            if let Part::Show(x) = p {
                visit_expr(x, f)
            }
        }),
        ExprKind::Catch { value, handler, .. } => {
            visit_expr(value, f);
            visit_block(handler, f);
        }
        ExprKind::If { cond, then, els } => {
            visit_expr(cond, f);
            visit_block(then, f);
            visit_block(els, f);
        }
        ExprKind::Match { scrutinee, arms } => {
            visit_expr(scrutinee, f);
            for a in arms {
                if let Some(g) = &a.guard {
                    visit_expr(g, f);
                }
                match &a.body {
                    ArmBody::Expr(x) => visit_expr(x, f),
                    ArmBody::Block(b) => visit_block(b, f),
                }
            }
        }
        ExprKind::Block(b) => visit_block(b, f),
        _ => {}
    }
}

const BUILTIN_TYPES: &[&str] = &[
    "Void", "Never", "Bool", "Int", "Int8", "Int16", "Int32", "Int64", "UInt", "UInt8", "UInt16", "UInt32", "UInt64", "Byte", "Float", "Float32", "Decimal", "Duration", "Timestamp", "String", "StringView",
    "Scalar", "Bytes", "BytesView", "List", "ListView", "Array", "FixedList", "Map", "OrderedMap", "FixedMap", "Set", "Outcome", "Failure", "Report", "Shared", "Weak", "Mutex", "AtomicInt", "AtomicBool",
    "Builder", "ListCursor", "Capabilities", "Console", "Clock", "Random", "Network", "Process", "Environment", "Supervisor", "SecretAccess", "RawSql", "FileSystem", "Must", "Text", "Host", "Path",
    "Failures", "Task", "Scope", "Guard", "Secret", "File",
];

/// The Rust name of each function: free functions by name, methods as `Type::method`, library members prefixed.
/// The program without the tests whose reachable functions fail to lower; each becomes an unbuilt test with the
/// first problem.
fn prune_tests(prog: &Program) -> Program {
    let (fns, _) = reachable(prog, true);
    let names = rust_names(prog, &fns);
    let mut bad: HashMap<String, String> = HashMap::new();
    for n in &fns {
        let f = match prog.function(n) {
            Some(f) if f.closure.is_none() && f.body.is_some() => f,
            _ => continue,
        };
        let mut audit = Audit::default();
        let mut g = FnGen::new(prog, f, &mut audit, &names);
        if native_body(f).is_none() {
            let _ = g.fn_body(f.body.as_ref().unwrap());
        }
        if let Some(e) = g.errors.first() {
            bad.insert(n.clone(), format!("line {}: {}", e.line, e.message));
        }
    }
    let mut out = prog.clone();
    if bad.is_empty() {
        return out;
    }
    let mut kept = vec![];
    for t in &prog.tests {
        let single = Program { tests: vec![t.clone()], ..prog.clone() };
        let (reach, _) = reachable(&single, true);
        match reach.iter().find_map(|n| bad.get(n)) {
            Some(why) => out.unbuilt_tests.push((t.title.clone(), why.clone())),
            None => kept.push(t.clone()),
        }
    }
    out.tests = kept;
    out
}

/// The functions that can reach themselves through calls (direct, mutual, or through an interface method, taken
/// conservatively as a call of every method of that name).
fn recursive_functions(prog: &Program, fns: &BTreeSet<String>) -> BTreeSet<String> {
    let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();
    for n in fns {
        let f = match prog.function(n) {
            Some(f) => f,
            None => continue,
        };
        let mut out = BTreeSet::new();
        if let Some(b) = &f.body {
            callees(b, prog, &mut out);
        }
        edges.insert(n.clone(), out);
    }
    let mut rec = BTreeSet::new();
    for start in fns {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut stack: Vec<&str> = edges.get(start).map(|e| e.iter().map(|s| s.as_str()).collect()).unwrap_or_default();
        while let Some(n) = stack.pop() {
            if n == start {
                rec.insert(start.clone());
                break;
            }
            if seen.insert(n) {
                if let Some(e) = edges.get(n) {
                    stack.extend(e.iter().map(|s| s.as_str()));
                }
            }
        }
    }
    rec
}

/// Whether the optimiser could turn a function's recursion into a loop (a tail call, or an accumulator such as
/// `deep(n + 1) + 1`), so unbounded recursion would never exhaust the stack: true when some statement holds exactly
/// one call into the function's cycle. When every recursive call shares its statement with another (`fib(n - 1) +
/// fib(n - 2)`), one of them stays a real recursion and the platform's guard page bounds it.
fn tail_recursive(f: &Function, recursive: &BTreeSet<String>) -> bool {
    fn count(e: &Expr, rec: &BTreeSet<String>) -> usize {
        let own = match &e.kind {
            ExprKind::Call { callee: Callee::Func(n), .. } if rec.contains(n) => 1,
            ExprKind::Call { callee: Callee::Virtual { .. }, .. } => 1,
            _ => 0,
        };
        let mut n = own;
        let mut sub = |x: &Expr| n += count(x, rec);
        match &e.kind {
            ExprKind::Call { callee, args } => {
                if let Callee::Value(v) = callee {
                    sub(v);
                }
                args.iter().for_each(|a| sub(&a.value));
            }
            ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) | ExprKind::Index(a, b) => {
                sub(a);
                sub(b);
            }
            ExprKind::Unary(_, a) | ExprKind::Field(a, _) | ExprKind::TupleField(a, _) | ExprKind::OptField(a, _) | ExprKind::Present(a) | ExprKind::Success(a) | ExprKind::OrFail(a) | ExprKind::Fail(a) | ExprKind::Await(a) | ExprKind::Temp { value: a, .. } | ExprKind::Codec { arg: a, .. } | ExprKind::Catch { value: a, .. } => sub(a),
            ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::Record(fields) => fields.iter().for_each(|(_, x)| sub(x)),
            ExprKind::Tuple(es) => es.iter().for_each(|x| sub(x)),
            ExprKind::Interpolate(parts) => parts.iter().for_each(|p| {
                if let Part::Show(x) = p {
                    sub(x)
                }
            }),
            _ => {}
        }
        n
    }
    fn single(e: &Expr, rec: &BTreeSet<String>) -> bool {
        // an `if`/`match` expression's branches are separate statements of their own
        match &e.kind {
            ExprKind::If { cond, then, els } => single(cond, rec) || block(then, rec) || block(els, rec),
            ExprKind::Match { scrutinee, arms } => {
                single(scrutinee, rec)
                    || arms.iter().any(|a| match &a.body {
                        ArmBody::Expr(e) => single(e, rec),
                        ArmBody::Block(b) => block(b, rec),
                    })
            }
            ExprKind::Block(b) => block(b, rec),
            _ => count(e, rec) == 1,
        }
    }
    fn block(b: &Block, rec: &BTreeSet<String>) -> bool {
        b.defers.iter().any(|d| block(d, rec))
            || b.stmts.iter().any(|s| match &s.kind {
                StmtKind::Let { init: e, .. } | StmtKind::LetPattern { init: e, .. } | StmtKind::Expr(e) | StmtKind::Discard(e) | StmtKind::Give(e) | StmtKind::Recover(e) | StmtKind::Fail(e) | StmtKind::Return(Some(e)) => single(e, rec),
                StmtKind::Assign { value, .. } | StmtKind::Compound { value, .. } | StmtKind::Capture { value, .. } | StmtKind::Report { value, .. } => single(value, rec),
                StmtKind::If { cond, then, els } => single(cond, rec) || block(then, rec) || els.as_ref().map_or(false, |e| block(e, rec)),
                StmtKind::Loop { body, .. } | StmtKind::For { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => block(body, rec),
                StmtKind::Match { scrutinee, arms } => {
                    single(scrutinee, rec)
                        || arms.iter().any(|a| match &a.body {
                            ArmBody::Expr(e) => single(e, rec),
                            ArmBody::Block(b) => block(b, rec),
                        })
                }
                _ => false,
            })
    }
    f.body.as_ref().map_or(false, |b| block(b, recursive))
}

fn callees(b: &Block, prog: &Program, out: &mut BTreeSet<String>) {
    fn expr(e: &Expr, prog: &Program, out: &mut BTreeSet<String>) {
        match &e.kind {
            ExprKind::Call { callee, args } => {
                match callee {
                    Callee::Func(n) => {
                        out.insert(n.clone());
                    }
                    Callee::Virtual { method } => {
                        for t in &prog.types {
                            for (m, f) in &t.methods {
                                if m == method {
                                    out.insert(f.clone());
                                }
                            }
                        }
                    }
                    Callee::Value(v) => expr(v, prog, out),
                    Callee::Intrinsic(_) => {}
                }
                args.iter().for_each(|a| expr(&a.value, prog, out));
            }
            ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) | ExprKind::Index(a, b) => {
                expr(a, prog, out);
                expr(b, prog, out);
            }
            ExprKind::Unary(_, a) | ExprKind::Field(a, _) | ExprKind::TupleField(a, _) | ExprKind::OptField(a, _) | ExprKind::Present(a) | ExprKind::Success(a) | ExprKind::OrFail(a) | ExprKind::Fail(a) | ExprKind::Await(a) | ExprKind::Temp { value: a, .. } | ExprKind::Codec { arg: a, .. } => expr(a, prog, out),
            ExprKind::Catch { value, handler, .. } => {
                expr(value, prog, out);
                callees(handler, prog, out);
            }
            ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::Record(fields) => fields.iter().for_each(|(_, x)| expr(x, prog, out)),
            ExprKind::Tuple(es) => es.iter().for_each(|x| expr(x, prog, out)),
            ExprKind::Interpolate(parts) => parts.iter().for_each(|p| {
                if let Part::Show(x) = p {
                    expr(x, prog, out)
                }
            }),
            ExprKind::If { cond, then, els } => {
                expr(cond, prog, out);
                callees(then, prog, out);
                callees(els, prog, out);
            }
            ExprKind::Match { scrutinee, arms } => {
                expr(scrutinee, prog, out);
                for a in arms {
                    match &a.body {
                        ArmBody::Expr(e) => expr(e, prog, out),
                        ArmBody::Block(b) => callees(b, prog, out),
                    }
                }
            }
            ExprKind::Block(b) => callees(b, prog, out),
            ExprKind::Closure { func, .. } => {
                out.insert(func.clone());
            }
            _ => {}
        }
    }
    for d in &b.defers {
        callees(d, prog, out);
    }
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Let { init, .. } | StmtKind::LetPattern { init, .. } => expr(init, prog, out),
            StmtKind::Assign { value, .. } | StmtKind::Compound { value, .. } | StmtKind::Expr(value) | StmtKind::Discard(value) | StmtKind::Give(value) | StmtKind::Recover(value) | StmtKind::Fail(value) | StmtKind::Capture { value, .. } | StmtKind::Report { value, .. } => expr(value, prog, out),
            StmtKind::Return(Some(e)) => expr(e, prog, out),
            StmtKind::If { cond, then, els } => {
                expr(cond, prog, out);
                callees(then, prog, out);
                if let Some(e) = els {
                    callees(e, prog, out);
                }
            }
            StmtKind::Loop { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => callees(body, prog, out),
            StmtKind::For { source, body, .. } => {
                if let ForSource::Items(e) = source {
                    expr(e, prog, out);
                }
                callees(body, prog, out);
            }
            StmtKind::Match { scrutinee, arms } => {
                expr(scrutinee, prog, out);
                for a in arms {
                    match &a.body {
                        ArmBody::Expr(e) => expr(e, prog, out),
                        ArmBody::Block(b) => callees(b, prog, out),
                    }
                }
            }
            StmtKind::Record { fields, .. } => fields.iter().for_each(|(_, e)| expr(e, prog, out)),
            StmtKind::Expect { cond, .. } => expr(cond, prog, out),
            _ => {}
        }
    }
}

/// Library functions whose native body is hand-written Rust with the same contract as the Fortis body in
/// library/fortis (the signature is still lowered from the declaration, so callers and the audit are
/// unchanged). Each is checked against the lowered Fortis body by native_check's library differential.
fn native_body(f: &Function) -> Option<&'static str> {
    if std::env::var_os("FORTIS_NATIVE_LIBRARY_BODIES").map_or(false, |v| v == "0") {
        return None;
    }
    Some(match (f.module.as_str(), f.label.as_str()) {
        ("fortis.prelude", "method List.map") => "return self_.iter().map(|x| (f)(x)).collect();",
        ("fortis.prelude", "method List.filter") => "return self_.iter().filter(|x| (keep)(x)).cloned().collect();",
        ("fortis.prelude", "method List.countWhere") => "return self_.iter().filter(|x| (keep)(x)).count() as i64;",
        ("fortis.prelude", "method List.reversed") | ("fortis.prelude", "method ListView.reversed") => "return self_.iter().rev().cloned().collect();",
        ("fortis.prelude", "method List.concat") => "let mut out = Vec::with_capacity(rt::collections::joined_len(self_, other));\nout.extend_from_slice(self_);\nout.extend_from_slice(other);\nreturn out;",
        ("fortis.collections", "function sorted") => "let mut out = items.to_vec();\nout.sort();\nreturn out;",
        _ => return None,
    })
}

fn rust_names(prog: &Program, fns: &BTreeSet<String>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for n in fns {
        let f = match prog.function(n) {
            Some(f) => f,
            None => continue,
        };
        let module = ident(&f.module.replace('.', "_"));
        let base = f.label.replace('#', "__");
        let rust = if let Some(m) = base.strip_prefix("method ") {
            let (t, meth) = m.split_once('.').unwrap_or((m, ""));
            let user_type = prog.type_def(t).map_or(false, |d| matches!(d.kind, TypeKind::Class { .. } | TypeKind::Enum { .. })) && (!BUILTIN_TYPES.contains(&t) || types::user_type(t, prog));
            if user_type && !meth.contains("__") {
                format!("{}::{}", ident(t), ident(meth))
            } else {
                format!("{module}_{}_{}", ident(t), ident(&meth.replace("__", "_")))
            }
        } else if prog.entry.as_deref() == Some(f.name.as_str()) {
            "fortis_main".to_string()
        } else if let Some(fname) = base.strip_prefix("function ") {
            if f.module == "main" {
                if fname == "main" {
                    "fortis_main".to_string()
                } else {
                    ident(&fname.replace("__", "_body_"))
                }
            } else {
                format!("{module}_{}", ident(fname))
            }
        } else {
            format!("{module}_{}", ident(&base))
        };
        out.insert(n.clone(), rust);
    }
    out
}

fn type_params_in(t: &Type, out: &mut BTreeSet<String>) {
    match t {
        Type::TypeParam(p) => {
            out.insert(p.clone());
        }
        Type::Named(n) => n.args.iter().for_each(|a| type_params_in(a, out)),
        Type::Optional(i) | Type::Loan { inner: i, .. } => type_params_in(i, out),
        Type::Tuple(ts) => ts.iter().for_each(|a| type_params_in(a, out)),
        Type::Record(fs) => fs.iter().for_each(|(_, a)| type_params_in(a, out)),
        Type::Fn { params, ret, .. } => {
            params.iter().for_each(|a| type_params_in(a, out));
            type_params_in(ret, out);
        }
        Type::Dynamic(n) => n.args.iter().for_each(|a| type_params_in(a, out)),
        _ => {}
    }
}

/// Every type parameter's fixed bound. The standard traits are named by path, so a type or interface a program
/// declares under the same name (`Clone`) cannot capture it.
/// A function's Rust signature: generic parameters, parameters by mode, the result (wrapped for `try`).
fn signature(prog: &Program, f: &Function, rust_name: &str, in_impl: bool, frame: bool) -> String {
    signature_for(prog, f, rust_name, in_impl, frame, false)
}

/// `for_trait`: a trait declaration keeps `&self` for every read receiver (so the trait stays usable as `dyn`).
fn signature_for(prog: &Program, f: &Function, rust_name: &str, in_impl: bool, frame: bool, for_trait: bool) -> String {
    signature_with(prog, f, rust_name, in_impl, frame, for_trait, &BTreeSet::new(), false)
}

/// `exclude`: type parameters the enclosing trait or impl declares; `boxed_async`: an async method of a trait
/// returns a boxed future whose lifetime covers its loans, so the trait stays usable as `dyn` (12.3).
fn signature_with(prog: &Program, f: &Function, rust_name: &str, in_impl: bool, frame: bool, for_trait: bool, exclude: &BTreeSet<String>, boxed_async: bool) -> String {
    let boxed = boxed_async && f.has("async");
    let mut tparams: BTreeSet<String> = BTreeSet::new();
    for p in &f.params {
        type_params_in(&f.locals[p.local as usize].ty, &mut tparams);
    }
    type_params_in(&f.ret, &mut tparams);
    // a parameter only a bound's arguments name (`where T implements Part[A]`) is the function's too
    for (_, _, args) in &f.bounds {
        args.iter().for_each(|a| type_params_in(a, &mut tparams));
    }
    // an optional loan (`(alias T from xs)?`) names its origin as a loan does (D-338)
    let ret_loan = match &f.ret {
        Type::Optional(i) if matches!(&**i, Type::Loan { .. }) => &**i,
        t => t,
    };
    let origin = match ret_loan {
        Type::Loan { origin, .. } if f.params.iter().any(|p| &p.name == origin) => Some(origin.clone()),
        Type::Fn { origin, .. } if f.params.iter().any(|p| &p.name == origin) => Some(origin.clone()),
        _ => None,
    };
    let mut gens: Vec<String> = Vec::new();
    if boxed {
        gens.push("'__f".into());
    }
    if let Some(o) = &origin {
        gens.push(format!("'{}", ident(o)));
    }
    // a method of a generic class gets its class's parameters from the impl block
    let class_params: BTreeSet<String> = if in_impl {
        f.label.strip_prefix("method ").and_then(|m| m.split('.').next()).and_then(|t| prog.type_def(t)).map(|d| d.type_params.iter().cloned().collect()).unwrap_or_default()
    } else {
        BTreeSet::new()
    };
    for t in tparams.iter().filter(|t| !class_params.contains(*t) && !exclude.contains(*t) && t.as_str() != "Self") {
        // a `where T implements I` bound on an interface the program declares is a trait bound, so a method of `I`
        // called on a `T` resolves statically (D-82); an ordered bound adds `PartialOrd`
        // a `T` the body boxes into an owned `dynamic I` (`move x` where `dynamic I` is expected) outlives the call,
        // which `Box<dyn I>` states as `'static` (D-88 (b))
        let b = param_bounds(prog, &f.bounds, t);
        let all: Vec<&str> = [b.as_str(), if boxed_tparams(f).contains(t) { "'static" } else { "" }].into_iter().filter(|s| !s.is_empty()).collect();
        gens.push(if all.is_empty() { ident(t) } else { format!("{}: {}", ident(t), all.join(" + ")) });
    }
    // a bound on a parameter of the method's class (the class's own `where`, or the method's): a `where` clause on
    // the method, since the class's impl gives its parameters only the fixed bound
    let class_where: Vec<String> = class_params
        .iter()
        .filter_map(|p| {
            let b = param_bounds(prog, &f.bounds, p);
            if b.is_empty() {
                None
            } else {
                Some(format!("{}: {b}", ident(p)))
            }
        })
        .collect();
    let mut ps: Vec<String> = Vec::new();
    for p in &f.params {
        let t = &f.locals[p.local as usize].ty;
        if f.locals[p.local as usize].kind == LocalKind::SelfParam && in_impl {
            // a trivial (Copy) receiver is taken by value, as a Rust programmer would write it
            let by_value = !for_trait && trivial(t, prog);
            ps.push(match p.mode {
                Mode::Read if by_value => "self".into(),
                Mode::Read if boxed => "&'__f self".into(),
                Mode::Read => "&self".into(),
                Mode::Editable if boxed => "&'__f mut self".into(),
                Mode::Editable => "&mut self".into(),
                Mode::Owned => "self".into(),
            });
            continue;
        }
        let mut pt = param_ty(t, p.mode, prog);
        if origin.as_deref() == Some(p.name.as_str()) && pt.starts_with('&') {
            pt = format!("&'{} {}", ident(&p.name), &pt[1..]);
        } else if origin.as_deref() == Some(p.name.as_str()) && pt.starts_with("Option<&") {
            pt = pt.replacen("Option<&", &format!("Option<&'{} ", ident(&p.name)), 1);
        } else if boxed && pt.starts_with('&') {
            pt = format!("&'__f {}", &pt[1..]);
        }
        if p.mode == Mode::Owned {
            if let Type::Fn { origin: region, .. } = strip(t) {
                if !region.is_empty() && (region == "static" || origin.as_ref() == Some(region)) {
                    pt.push_str(&format!(" + '{}", if region == "static" { region.clone() } else { ident(region) }));
                }
            }
        }
        let mutable = matches!(p.mode, Mode::Owned) && f.locals[p.local as usize].mutable;
        ps.push(format!("{}{}: {pt}", if mutable { "mut " } else { "" }, ident(&f.locals[p.local as usize].name)));
    }
    if frame {
        ps.push("__frame: &mut Vec<(&'static str, String)>".into());
    }
    // a function type in result position is `impl Fn…`: the closure is returned unboxed
    let ret = if matches!(strip(&f.ret), Type::Fn { .. }) {
        // a closure lent from a parameter (`fn() Int from base`) carries that parameter's lifetime
        match &origin {
            // a returned closure is owned (`impl Fn`), not the borrowed form of a read-mode parameter
            Some(o) => format!("{} + '{}", param_ty(&f.ret, Mode::Owned, prog), ident(o)),
            None => param_ty(&f.ret, Mode::Owned, prog),
        }
    } else {
        let r = result_ty(&f.ret, prog);
        // a loan of the receiver (`(alias T from self)?`) of a method lowered as a free function names the receiver's
        // lifetime: with another reference parameter, such as a callback, Rust has no single lifetime to elide to
        // (`List.find`; X-479). A method in an `impl` takes `&self`, whose lifetime elides to the result's.
        match &origin {
            Some(o) if o == "self" && !for_trait && !in_impl && (r.starts_with('&') || r.starts_with("Option<&")) => r.replacen('&', "&'self_ ", 1),
            _ => r,
        }
    };
    let ret = if f.label.starts_with("test ") {
        "Result<(), TestFailure>".to_string()
    } else if f.is_try() {
        format!("rt::Outcome<{ret}>")
    } else {
        ret
    };
    let g = if gens.is_empty() { String::new() } else { format!("<{}>", gens.join(", ")) };
    let short = rust_name.rsplit("::").next().unwrap_or(rust_name);
    let w = if class_where.is_empty() { String::new() } else { format!(" where {}", class_where.join(", ")) };
    if boxed {
        return format!("fn {short}{g}({}) -> ::std::pin::Pin<::std::boxed::Box<dyn ::std::future::Future<Output = rt::task::Async<{ret}>> + '__f>>{w}", ps.join(", "));
    }
    // an async function is a Rust `async fn` state machine whose `Err` is the cancellation exit (C.1)
    if f.has("async") && !f.label.starts_with("test ") {
        return format!("async fn {short}{g}({}) -> rt::task::Async<{ret}>{w}", ps.join(", "));
    }
    format!("fn {short}{g}({}) -> {ret}{w}", ps.join(", "))
}

/// The type parameters whose values the body converts to an owned `dynamic I` (boxed), which Rust bounds `'static`.
fn boxed_tparams(f: &Function) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(b) = &f.body {
        visit_block(b, &mut |e| {
            if let ExprKind::ToDynamic { value, borrowed: false, .. } = &e.kind {
                if let Type::TypeParam(p) = strip(&value.ty) {
                    out.insert(p.clone());
                }
            }
        });
    }
    out
}

/// A marker type parameter for an interface's own parameter (or `Self`) in a signature written for its trait or an
/// impl of it: substituted in the written signature, so the trait and every impl pass it the same way.
fn marker(i: usize) -> String {
    // A compiler placeholder cannot share a user spelling, or be a prefix of another placeholder.
    format!("__ft_signature_{i}_end")
}

/// An interface method's signature for its trait (`subst` maps each parameter and `Self` to the Rust it stands for)
/// or for an impl (the implementing type's arguments): the written signature, and each parameter's written type.
fn iface_method_sig(prog: &Program, iface: &TypeDef, ms: &MethodSig, subst: &BTreeMap<String, String>, boxed: bool, name: &str) -> (String, Vec<String>) {
    let mut names: Vec<String> = iface.type_params.clone();
    names.push("Self".into());
    let marks: BTreeMap<String, Type> = names.iter().enumerate().map(|(i, n)| (n.clone(), Type::TypeParam(marker(i)))).collect();
    let mark = |t: &Type| subst_tparams(t, &marks);
    let mut locals = vec![LocalDecl { name: "self".into(), ty: Type::TypeParam(marker(names.len() - 1)), mutable: ms.receiver == Some(Mode::Editable), kind: LocalKind::SelfParam, boxed: false }];
    let mut params = vec![Param { local: 0, name: "self".into(), mode: ms.receiver.unwrap_or(Mode::Read), default: None }];
    for (i, (n, m, t)) in ms.params.iter().enumerate() {
        locals.push(LocalDecl { name: n.clone(), ty: mark(t), mutable: false, kind: LocalKind::Param, boxed: false });
        params.push(Param { local: (i + 1) as u32, name: n.clone(), mode: *m, default: None });
    }
    let f = Function {
        name: format!("{}::iface {}.{}", iface.module, iface.name, ms.name),
        label: format!("iface {}.{}", iface.name, ms.name),
        module: iface.module.clone(),
        body_module: None,
        line: 0,
        params,
        ret: mark(&ms.ret),
        modifiers: ms.modifiers.clone(),
        locals,
        body: None,
        effects: Effects::default(),
        observe: None,
        closure: None,
        bounds: ms.bounds.iter().map(|(p, i, a)| (p.clone(), i.clone(), a.iter().map(|x| mark(x)).collect())).collect(),
    };
    let exclude: BTreeSet<String> = (0..names.len()).map(marker).collect();
    let unmark = |text: String| -> String {
        let mut text = text;
        for (i, n) in names.iter().enumerate() {
            let to = subst.get(n).cloned().unwrap_or_else(|| if n == "Self" { "Self".into() } else { ident(n) });
            text = text.replace(&ident(&marker(i)), &to);
        }
        text
    };
    // a read `fn` parameter is `&impl Fn` on an inherent method; the trait takes it as `&dyn Fn`, which a `dyn`
    // value can dispatch (D-88 (e)); the impl passes it on as `&f`, which is itself `Fn`
    let dyn_fn = |s: String| s.replace("&impl Fn(", "&dyn Fn(");
    let sig = dyn_fn(unmark(signature_with(prog, &f, name, true, false, true, &exclude, boxed)));
    let ptys: Vec<String> = ms.params.iter().map(|(_, m, t)| dyn_fn(unmark(param_ty(&mark(t), *m, prog)))).collect();
    (sig, ptys)
}

/// A type with the type parameters `m` names replaced (a named type without arguments of that name too).
fn subst_tparams(t: &Type, m: &BTreeMap<String, Type>) -> Type {
    match t {
        Type::TypeParam(p) => m.get(p).cloned().unwrap_or_else(|| t.clone()),
        Type::Named(n) if n.args.is_empty() && m.contains_key(&n.name) => m[&n.name].clone(),
        Type::Named(n) => Type::Named(fortis_types::types::NamedT { name: n.name.clone(), args: n.args.iter().map(|a| subst_tparams(a, m)).collect(), package: n.package.clone(), origin: n.origin.clone() }),
        Type::Optional(i) => Type::Optional(Box::new(subst_tparams(i, m))),
        Type::Loan { inner, mode, origin } => Type::Loan { inner: Box::new(subst_tparams(inner, m)), mode: mode.clone(), origin: origin.clone() },
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|x| subst_tparams(x, m)).collect()),
        Type::Record(fs) => Type::Record(fs.iter().map(|(k, x)| (k.clone(), subst_tparams(x, m))).collect()),
        Type::Fn { capture_origins, param_modes, params, ret, kind, modifiers, origin, dynamic, identity } => Type::Fn { capture_origins: capture_origins.clone(),  param_modes: param_modes.clone(), params: params.iter().map(|x| subst_tparams(x, m)).collect(), ret: Box::new(subst_tparams(ret, m)), kind: kind.clone(), modifiers: modifiers.clone(), origin: origin.clone(), dynamic: *dynamic, identity: identity.clone() },
        Type::Dynamic(n) => Type::Dynamic(fortis_types::types::NamedT { name: n.name.clone(), args: n.args.iter().map(|a| subst_tparams(a, m)).collect(), package: n.package.clone(), origin: n.origin.clone() }),
        other => other.clone(),
    }
}

fn mentions_self(t: &Type) -> bool {
    match t {
        Type::TypeParam(p) => p == "Self",
        Type::Named(n) => n.name == "Self" || n.args.iter().any(mentions_self),
        Type::Optional(i) | Type::Loan { inner: i, .. } => mentions_self(i),
        Type::Tuple(ts) => ts.iter().any(mentions_self),
        Type::Record(fs) => fs.iter().any(|(_, x)| mentions_self(x)),
        Type::Fn { params, ret, .. } => params.iter().any(mentions_self) || mentions_self(ret),
        Type::Dynamic(n) => n.args.iter().any(mentions_self),
        _ => false,
    }
}

/// A method a `dyn` value of the trait can dispatch: no `Self` but the receiver, no type parameters of its own, a
/// borrowed receiver. Any other is `where Self: Sized`, so the trait stays usable as `dyn I`.
fn dyn_dispatchable(ms: &MethodSig) -> bool {
    // a read plain `fn` parameter is `&dyn Fn` in the trait (D-88 (e)); `fn mut`, `fn once`, an owned or an async
    // callback are taken by value or are not `dyn`-compatible
    let fn_ok = |m: &Mode, t: &Type| match strip(t) {
        Type::Fn { kind, modifiers, .. } => *m == Mode::Read && kind == "fn" && !modifiers.iter().any(|x| x == "async"),
        _ => true,
    };
    ms.type_params.is_empty()
        && ms.receiver != Some(Mode::Owned)
        && !ms.params.iter().any(|(_, m, t)| mentions_self(t) || !fn_ok(m, t))
        && !mentions_self(&ms.ret)
}

fn with_where(sig: String, clause: &str) -> String {
    if sig.contains(" where ") {
        format!("{sig}, {clause}")
    } else {
        format!("{sig} where {clause}")
    }
}

/// The type arguments with which `c` implements the interface `iface`: those it lists, or, for an interface it
/// implements through another's `requires`, those that interface states with `c`'s arguments for it substituted.
fn implements_args(prog: &Program, c: &TypeDef, iface: &str, depth: usize) -> Option<Vec<Type>> {
    if depth > 16 {
        return None;
    }
    if c.implements.iter().any(|i| i == iface) {
        return Some(c.iface_args.iter().find(|(i, _)| i == iface).map(|(_, a)| a.clone()).unwrap_or_default());
    }
    for j in &c.implements {
        let jd = match prog.type_def(j) {
            Some(d) if matches!(d.kind, TypeKind::Interface { .. }) => d,
            _ => continue,
        };
        if j != iface && iface_closure(prog, j).iter().any(|x| x == iface) {
            let jargs = implements_args(prog, c, j, depth + 1).unwrap_or_default();
            let m: BTreeMap<String, Type> = jd.type_params.iter().cloned().zip(jargs).collect();
            // `iface` as `j` states it (directly, or through an interface `j` requires)
            let stated = if jd.requires.iter().any(|r| r == iface) {
                jd.iface_args.iter().find(|(i, _)| i == iface).map(|(_, a)| a.clone()).unwrap_or_default()
            } else {
                implements_args(prog, jd, iface, depth + 1).unwrap_or_default()
            };
            return Some(stated.iter().map(|a| subst_tparams(a, &m)).collect());
        }
    }
    // an interface's own requirements, for the recursion above
    if matches!(c.kind, TypeKind::Interface { .. }) && c.requires.iter().any(|r| r == iface) {
        return Some(c.iface_args.iter().find(|(i, _)| i == iface).map(|(_, a)| a.clone()).unwrap_or_default());
    }
    None
}

/// The trait of an interface the program declares, from its declaration, and an impl of it for each reachable class
/// or enum that implements it (directly, or through an interface that requires it) delegating to its methods. An
/// interface without methods of its own (a bundle of requirements) is implemented by every type that meets them.
fn declared_trait(prog: &Program, t: &TypeDef, tys: &BTreeSet<String>) -> String {
    let mut out = String::new();
    let tn = ident(&t.name);
    // the interface's own parameters carry no bound (an argument may be a function or a `dynamic` value); one its
    // signatures only lend is `?Sized`, so `dyn M` can instantiate it (D-88 (c))
    let lent = unsized_params(prog, &t.name);
    let tgens: Vec<String> = t.type_params.iter().map(|p| ident(p)).collect();
    let tg = if tgens.is_empty() { String::new() } else { format!("<{}>", tgens.join(", ")) };
    let tgens_decl: Vec<String> = t.type_params.iter().map(|p| if lent.contains(p) { format!("{}: ?::std::marker::Sized", ident(p)) } else { ident(p) }).collect();
    let tg_decl = if tgens_decl.is_empty() { String::new() } else { format!("<{}>", tgens_decl.join(", ")) };
    let own: BTreeMap<String, String> = t.type_params.iter().map(|p| (p.clone(), ident(p))).collect();
    let mut supers = vec!["rt::Show".to_string()];
    for r in t.requires.iter().filter(|r| is_user_iface(prog, r) && tys.contains(*r)) {
        let args = t.iface_args.iter().find(|(i, _)| i == r).map(|(_, a)| a.clone()).unwrap_or_default();
        supers.push(trait_ref(prog, r, &args));
    }
    let methods: Vec<&MethodSig> = t.sigs.iter().filter(|m| m.receiver.is_some()).collect();
    let mut items = Vec::new();
    for ms in &methods {
        let is_async = ms.modifiers.iter().any(|m| m == "async");
        let (sig, _) = iface_method_sig(prog, t, ms, &own, false, &ident(&ms.name));
        // an async method: an `async fn` for static dispatch (no allocation), and a boxed-future twin a `dyn` value
        // dispatches (12.3)
        if is_async || !dyn_dispatchable(ms) {
            items.push(format!("{};", with_where(sig, "Self: ::std::marker::Sized")));
        } else {
            items.push(format!("{sig};"));
        }
        if is_async && dyn_dispatchable(ms) {
            let (dsig, _) = iface_method_sig(prog, t, ms, &own, true, &format!("{}__dyn", ident(&ms.name)));
            items.push(format!("{dsig};"));
        }
    }
    out.push_str(&format!("pub trait {tn}{tg_decl}: {} {{\n{}\n}}\n", supers.join(" + "), indent(&items.join("\n"), 1)));
    if methods.is_empty() {
        // a bundle: every type that meets its requirements has it (as the checker judges it, structurally)
        let mut gens: Vec<String> = tgens_decl.clone();
        gens.push(format!("__X: {} + ?::std::marker::Sized", supers.join(" + ")));
        out.push_str(&format!("impl<{}> {tn}{tg} for __X {{}}\n\n", gens.join(", ")));
        return out;
    }
    for c in prog.types.iter().filter(|c| tys.contains(&c.name) && matches!(c.kind, TypeKind::Class { .. } | TypeKind::Enum { .. })) {
        let args = match implements_args(prog, c, &t.name, 0) {
            Some(a) => a,
            None => continue,
        };
        // a lent `dynamic M` argument is `(dyn M + 'x)`, and the impl ranges over `'x` (D-88 (c))
        let (targs, lent_dyn) = trait_args_lt(prog, &t.name, &args, Some("'x"));
        let mut subst: BTreeMap<String, String> = t.type_params.iter().cloned().zip(targs.iter().cloned()).collect();
        subst.insert("Self".into(), "Self".into());
        // the bounds the implementing methods need on the type's parameters, those of the interfaces this one
        // requires included (their impls are this impl's supertraits)
        let mut class_bounds: Vec<(String, String, Vec<Type>)> = Vec::new();
        for x in iface_closure(prog, &t.name) {
            let xd = match prog.type_def(&x) {
                Some(d) if is_user_iface(prog, &x) => d,
                _ => continue,
            };
            for ms in xd.sigs.iter().filter(|m| m.receiver.is_some()) {
                if let Some(f) = c.methods.iter().find(|(m, _)| *m == ms.name).and_then(|(_, n)| prog.function(n)) {
                    for b in f.bounds.iter().filter(|(p, _, _)| c.type_params.contains(p)) {
                        if !class_bounds.contains(b) {
                            class_bounds.push(b.clone());
                        }
                    }
                }
            }
        }
        let mut defs = Vec::new();
        for ms in &methods {
            let f = match c.methods.iter().find(|(x, _)| *x == ms.name).and_then(|(_, n)| prog.function(n)) {
                Some(f) => f,
                None => continue,
            };
            let (sig, ptys) = iface_method_sig(prog, t, ms, &subst, false, &ident(&ms.name));
            let triv_self = trivial(&fortis_types::types::named(&c.name), prog);
            let mut call_args: Vec<String> = Vec::new();
            let mut k = 0;
            for p in &f.params {
                let l = &f.locals[p.local as usize];
                if l.kind == LocalKind::SelfParam {
                    call_args.push(if triv_self && p.mode == Mode::Read { "*self".to_string() } else { "self".to_string() });
                    continue;
                }
                // the trait passes a parameter as the interface writes it; the method may take it otherwise
                let name = ms.params.get(k).map(|(n, _, _)| ident(n)).unwrap_or_else(|| ident(&l.name));
                let tr = ptys.get(k).cloned().unwrap_or_default();
                let inh = param_ty(&l.ty, p.mode, prog);
                k += 1;
                let by_ref = |x: &str| x.starts_with('&') && !x.starts_with("&mut");
                call_args.push(if tr == inh || (tr.starts_with("&(dyn ") && inh.starts_with("&dyn ")) {
                    // the same type, or the trait's `&(dyn M + 'x)` where the method reads `&dyn M` (a subtype)
                    name
                } else if tr.starts_with("&dyn Fn(") && inh.starts_with("&impl Fn(") {
                    // a function the trait lends as `&dyn Fn` where the method takes `&impl Fn`: `&&dyn Fn` is `Fn` too
                    format!("&{name}")
                } else if by_ref(&tr) && inh.starts_with("impl ") {
                    // a function the trait lends boxed (`&Box<dyn Fn…>`) where the method takes `impl Fn…`
                    format!("&**{name}")
                } else if by_ref(&tr) && inh.starts_with("&dyn ") {
                    // a `dynamic` value the trait lends boxed where the method reads `&dyn I`
                    format!("::std::borrow::Borrow::<{}>::borrow({name})", &inh[1..])
                } else if by_ref(&tr) && !by_ref(&inh) && inh != "&str" {
                    format!("{name}.clone()")
                } else if !by_ref(&tr) && by_ref(&inh) {
                    format!("&{name}")
                } else {
                    name
                });
            }
            let call = format!("{}::{}({})", ident(&c.name), ident(&ms.name), call_args.join(", "));
            // the method's result as the interface declares it: an implementation without `async` or `try` is
            // lifted into the interface's (9.5: an implementation may do less)
            let (m_async, m_try) = (ms.modifiers.iter().any(|m| m == "async"), ms.modifiers.iter().any(|m| m == "try"));
            let (f_async, f_try) = (f.has("async"), f.is_try());
            let lift = |v: String| if m_try && !f_try { format!("Ok({v})") } else { v };
            let body = if m_async {
                if f_async {
                    if m_try && !f_try { format!("{call}.await.map(Ok)") } else { format!("{call}.await") }
                } else {
                    format!("Ok({})", lift(call.clone()))
                }
            } else if !m_try {
                // a function type the interface's parameter stands for is boxed in the trait; the method returns it
                // unboxed
                let arg_map: BTreeMap<String, Type> = t.type_params.iter().cloned().zip(args.iter().cloned()).collect();
                let boxed_ret = matches!(strip(&ms.ret), Type::TypeParam(_)) && matches!(strip(&subst_tparams(&ms.ret, &arg_map)), Type::Fn { .. }) && matches!(strip(&f.ret), Type::Fn { .. });
                if boxed_ret {
                    format!("::std::boxed::Box::new({call})")
                } else {
                    call.clone()
                }
            } else {
                lift(call.clone())
            };
            if m_async {
                // the static `async fn` and its boxed twin
                let static_sig = sig.replacen("fn ", "async fn ", if sig.starts_with("async fn ") { 0 } else { 1 });
                defs.push(format!("{static_sig} {{\n    {body}\n}}"));
                if dyn_dispatchable(ms) {
                    let (dsig, _) = iface_method_sig(prog, t, ms, &subst, true, &format!("{}__dyn", ident(&ms.name)));
                    defs.push(format!("{dsig} {{\n    ::std::boxed::Box::pin(async move {{ {body} }})\n}}"));
                }
            } else {
                defs.push(format!("{sig} {{\n    {body}\n}}"));
            }
        }
        // every declared trait requires `rt::Show`, and a generic type shows only when its parameters do, so the impl
        // asks that of them (as the type's own structural `Show` does) and of nothing else (X-515; D-396)
        let mut cps: Vec<String> = c
            .type_params
            .iter()
            .map(|p| {
                let b = param_bounds(prog, &class_bounds, p);
                if b.split(" + ").any(|x| x == "rt::Show") {
                    format!("{}: {b}", ident(p))
                } else if b.is_empty() {
                    format!("{}: rt::Show", ident(p))
                } else {
                    format!("{}: {b} + rt::Show", ident(p))
                }
            })
            .collect();
        if lent_dyn {
            cps.insert(0, "'x".into());
        }
        let cg = if cps.is_empty() { String::new() } else { format!("<{}>", cps.join(", ")) };
        let ca = if c.type_params.is_empty() { String::new() } else { format!("<{}>", c.type_params.iter().map(|p| ident(p)).collect::<Vec<_>>().join(", ")) };
        let tref = if targs.is_empty() { tn.clone() } else { format!("{tn}<{}>", targs.join(", ")) };
        out.push_str(&format!("impl{cg} {tref} for {}{ca} {{\n{}\n}}\n", ident(&c.name), indent(&defs.join("\n"), 1)));
    }
    out.push('\n');
    out
}

/// The statement registering the program's observed `async` functions with the runtime, so that a task cancelled
/// before it starts writes its `not_started` frame (13; D-27); empty when there are none.
fn observed_table(prog: &Program) -> String {
    let rows: Vec<String> = prog
        .functions
        .iter()
        .filter(|f| f.has("async") && f.body.is_some())
        .filter_map(|f| f.observe.as_ref().map(|s| format!("({:?}, {s:?}, {})", gen::source_name(f), schema_version(prog, s))))
        .collect();
    if rows.is_empty() {
        String::new()
    } else {
        format!("    rt::records::observed(&[{}]);\n", rows.join(", "))
    }
}

/// An observation schema's `version` (1 when it declares none), carried by every record (Platform 6.1).
pub(crate) fn schema_version(prog: &Program, schema: &str) -> u32 {
    prog.types
        .iter()
        .find_map(|t| match &t.kind {
            TypeKind::Observation { version, .. } if t.name == schema => Some(*version),
            _ => None,
        })
        .unwrap_or(1)
}

/// An observation schema's bounded `String` fields and their bounds, which the frame is cut to (LANG-13-03).
fn schema_bounds(prog: &Program, schema: &str) -> Vec<(String, usize)> {
    prog.types
        .iter()
        .filter(|t| t.name == schema)
        .flat_map(|t| match &t.kind {
            TypeKind::Observation { fields, .. } => fields
                .iter()
                .filter_map(|(n, ty, _)| match ty {
                    Type::Refined { hi: Some(hi), kind, .. } if kind == "scalars" => Some((n.clone(), *hi as usize)),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        })
        .collect()
}

pub fn lower(prog: &Program, opts: &Options) -> Result<Lowered, Vec<LowerError>> {
    // a test build reports a test that reaches a construct without a native lowering yet as `unbuilt`, as the
    // builder does, instead of refusing the whole program
    let pruned;
    let prog = if opts.tests {
        pruned = prune_tests(prog);
        &pruned
    } else {
        prog
    };
    let (fns, tys) = reachable(prog, opts.tests);
    let names = rust_names(prog, &fns);
    let mut audit = Audit::default();
    let mut errors: Vec<LowerError> = Vec::new();
    let mut out = String::new();
    out.push_str("// Generated by the Fortis production lowering (DEL-36) from the validated typed IR. Do not edit.\n");
    out.push_str("#![forbid(unsafe_code)]\n#![allow(dead_code, non_snake_case, non_camel_case_types, unused_imports, unused_variables, unused_mut, unused_parens, unreachable_code, unused_labels, unused_braces, irrefutable_let_patterns, clippy::all)]\n");
    out.push_str("use fortis_native as rt;\nuse rt::Show;\n\n");
    out.push_str("pub enum TestFailure {\n    Failure(rt::Failure),\n    Expect(::std::string::String),\n}\nimpl ::std::convert::From<rt::Failure> for TestFailure {\n    fn from(f: rt::Failure) -> Self {\n        TestFailure::Failure(f)\n    }\n}\n\n");
    // ---- types
    let codec_set = codec::codec_types(prog);
    let recursive = recursive_functions(prog, &fns);
    let mut impls: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in &prog.types {
        if !tys.contains(&t.name) || (BUILTIN_TYPES.contains(&t.name.as_str()) && !is_user_module(&t.module)) {
            continue;
        }
        // The prelude and the standard module expose the same type (for example time.Instant).
        // Emit its declaring module's definition once, as the checker's core identity does (D-593).
        if t.module == "fortis.prelude" && prog.types.iter().any(|other| other.name == t.name && other.module != "fortis.prelude" && !is_user_module(&other.module)) {
            continue;
        }
        out.push_str(&type_decl(prog, t));
        // the canonical codec for a class or enum the program encodes or decodes (6.10)
        if codec_set.contains(&t.name) {
            let mut lower_default = |e: &Expr| -> String {
                let dummy = Function { name: "main::field default".into(), label: "field default".into(), module: "main".into(), body_module: None, line: 0, params: vec![], ret: fortis_types::types::void(), modifiers: vec![], locals: vec![], body: None, effects: Effects::default(), observe: None, closure: None, bounds: vec![] };
                let mut audit = Audit::default();
                let mut g = FnGen::new(prog, &dummy, &mut audit, &names);
                g.expr(e, gen::Want::Value)
            };
            if let Some(code) = codec::impl_for(prog, t, &mut lower_default) {
                out.push_str(&code);
            }
        }
    }
    // ---- functions
    let observed: BTreeSet<String> = prog.functions.iter().filter(|f| f.observe.is_some() && fns.contains(&f.name)).map(|f| f.name.clone()).collect();
    for n in &fns {
        let f = match prog.function(n) {
            Some(f) => f,
            None => continue,
        };
        if f.closure.is_some() || f.body.is_none() {
            continue;
        }
        let rust = match names.get(n) {
            Some(r) => r.clone(),
            None => continue,
        };
        let is_cleanup = f.label.starts_with("cleanup ");
        let in_impl = rust.contains("::") && !is_cleanup;
        let frame = observed.contains(n);
        let mut g = FnGen::new(prog, f, &mut audit, &names);
        let body = match native_body(f) {
            Some(b) => b.to_string(),
            None => g.fn_body(f.body.as_ref().unwrap()),
        };
        // a function in a cycle of the call graph guards its activation: unbounded recursion is the
        // stack_exhaustion defect, never a loop the optimiser made of it (10.8)
        // (recursion that is not a tail call consumes stack: the platform's guard page aborts it, which is the
        // native behaviour of stack exhaustion, at no cost per call)
        let body = if recursive.contains(n) && !f.has("async") && tail_recursive(f, &recursive) { format!("let __depth = rt::stack::enter();\n{body}") } else { body };
        errors.extend(std::mem::take(&mut g.errors));
        if is_cleanup {
            // The label is source spelling; its declaring type may have a qualified IR name (D-592).
            let t = prog.types.iter().find(|d| d.cleanup.as_deref() == Some(f.name.as_str()))
                .map(|d| d.name.as_str()).unwrap_or_else(|| f.label.trim_start_matches("cleanup "));
            let tparams = prog.type_def(t).map(|d| d.type_params.clone()).unwrap_or_default();
            let tbounds = prog.type_def(t).map(|d| d.bounds.clone()).unwrap_or_default();
            let lts = prog.type_def(t).map(|d| d.lifetimes.clone()).unwrap_or_default();
            let gen_list: Vec<String> = lts.iter().map(|l| format!("'{}", ident(l))).chain(tparams.iter().map(|p| generic_param(prog, &tbounds, p))).collect();
            let gens = if gen_list.is_empty() { String::new() } else { format!("<{}>", gen_list.join(", ")) };
            let args: Vec<String> = lts.iter().map(|l| format!("'{}", ident(l))).chain(tparams.iter().map(|p| ident(p))).collect();
            let targs = if args.is_empty() { String::new() } else { format!("<{}>", args.join(", ")) };
            out.push_str(&format!(
                "impl{gens} Drop for {}{targs} {{\n    fn drop(&mut self) {{\n        let self_ = self;\n        rt::records::enter_cleanup();\n{}\n        rt::records::leave_cleanup();\n    }}\n}}\n\n",
                ident(t),
                indent(&format!("(|| {{\n{}\n}})();", indent(&body, 1)), 2)
            ));
            continue;
        }
        let sig = signature(prog, f, &rust, in_impl, frame);
        // fault injection for the backend-error tests (PROG-CMP-05): `FORTIS_FAULT=lower:NAME` makes the named
        // function's Rust ill-typed, as a lowering defect would
        let body = match std::env::var("FORTIS_FAULT").ok().and_then(|v| v.strip_prefix("lower:").map(|n| n.to_string())) {
            Some(n) if f.name.ends_with(&n) || f.label == n => format!("let __fortis_fault: () = 1u8;\n{body}"),
            _ => body,
        };
        let def = if frame {
            // an observed function: the body fills a frame, emitted at every exit (13)
            let schema = f.observe.clone().unwrap();
            let inner_sig = sig.replacen("fn ", "fn __observed_", 1);
            let call_args: Vec<String> = f.params.iter().map(|p| ident(&f.locals[p.local as usize].name)).chain(std::iter::once("&mut __frame".to_string())).collect();
            let outer_sig = sig.replace(", __frame: &mut Vec<(&'static str, String)>", "").replace("(__frame: &mut Vec<(&'static str, String)>)", "()");
            let short = rust.rsplit("::").next().unwrap_or(&rust);
            let prefix = if in_impl { "Self::" } else { "" };
            let is_async = f.has("async");
            let outcome = match (is_async, f.is_try()) {
                (false, true) => "if __r.is_ok() { \"success\" } else { \"failure\" }",
                (false, false) => "\"success\"",
                // an async activation's `Err` is its cancellation (13: the outcome class)
                (true, true) => "match &__r { Ok(Ok(_)) => \"success\", Ok(Err(_)) => \"failure\", Err(_) => \"cancelled\" }",
                (true, false) => "if __r.is_ok() { \"success\" } else { \"cancelled\" }",
            };
            let wait = if is_async { ".await" } else { "" };
            format!(
                "pub {inner_sig} {{\n{}\n}}\npub {outer_sig} {{\n    let mut __frame: Vec<(&'static str, String)> = Vec::new();\n    let __r = {prefix}__observed_{short}({}){wait};\n    rt::records::frame({schema:?}, {}, {:?}, &__frame, {outcome}, &[{}]);\n    __r\n}}\n",
                indent(&body, 1),
                call_args.join(", "),
                schema_version(prog, &schema),
                gen::source_name(f),
                schema_bounds(prog, &schema).iter().map(|(n, b)| format!("({n:?}, {b})")).collect::<Vec<_>>().join(", ")
            )
        } else if alloc_probe() && probed(f, &sig, in_impl) {
            // the allocator probe (SP-04-async): the function's future is counted per phase (rt::probe)
            let short = rust.rsplit("::").next().unwrap_or(&rust);
            let inner_sig = sig.replacen(&format!("fn {short}"), &format!("fn __probe_{short}"), 1);
            let outer_full = sig.replacen("async fn ", "fn ", 1);
            let (outer, wh) = match outer_full.find(" where ") {
                Some(i) => (outer_full[..i].to_string(), outer_full[i..].to_string()),
                None => (outer_full.clone(), String::new()),
            };
            let (head, ret) = outer.rsplit_once(" -> ").unwrap_or((&outer, "()"));
            let call_args: Vec<String> = f.params.iter().map(|p| ident(&f.locals[p.local as usize].name)).collect();
            format!(
                "{inner_sig} {{\n{}\n}}\npub {head} -> impl std::future::Future<Output = {ret}>{wh} {{\n    rt::probe::construct({:?}, move || __probe_{short}({}))\n}}\n",
                indent(&body, 1),
                gen::source_name(f),
                call_args.join(", ")
            )
        } else {
            format!("pub {sig} {{\n{}\n}}\n", indent(&body, 1))
        };
        // where the function came from: a backend error in it maps back to its source (PROG-CMP-05)
        let def = format!("// fortis: {} line {}\n{def}", f.name, f.line);
        if in_impl {
            let t = rust.split("::").next().unwrap().to_string();
            impls.entry(t).or_default().push(def);
        } else {
            out.push_str(&def);
            out.push('\n');
        }
    }
    for (t, defs) in &impls {
        let td = prog.types.iter().find(|x| ident(&x.name) == *t);
        let tparams = td.map(|d| d.type_params.clone()).unwrap_or_default();
        let tbounds = td.map(|d| d.bounds.clone()).unwrap_or_default();
        let lts = td.map(|d| d.lifetimes.clone()).unwrap_or_default();
        let gen_list: Vec<String> = lts.iter().map(|l| format!("'{}", ident(l))).chain(tparams.iter().map(|p| generic_param(prog, &tbounds, p))).collect();
        let gens = if gen_list.is_empty() { String::new() } else { format!("<{}>", gen_list.join(", ")) };
        let args: Vec<String> = lts.iter().map(|l| format!("'{}", ident(l))).chain(tparams.iter().map(|p| ident(p))).collect();
        let targs = if args.is_empty() { String::new() } else { format!("<{}>", args.join(", ")) };
        out.push_str(&format!("impl{gens} {t}{targs} {{\n{}\n}}\n\n", indent(&defs.join("\n"), 1)));
    }
    // ---- interfaces: traits, and each implementing type's impl delegating to its inherent method. An interface the
    // program declares is written from its declaration (D-82): its methods whether or not a type implements it, `Self`
    // as `Self`, its parameters as the trait's, the interfaces it requires as supertraits
    for t in &prog.types {
        if let TypeKind::Interface { methods } = &t.kind {
            if !tys.contains(&t.name) {
                continue;
            }
            if is_user_module(&t.module) && (!t.sigs.is_empty() || methods.is_empty()) {
                out.push_str(&declared_trait(prog, t, &tys));
                continue;
            }
            let mut items = Vec::new();
            for (m, _) in methods {
                let imp = prog.types.iter().find_map(|c| c.methods.iter().find(|(x, _)| x == m).filter(|_| c.implements.contains(&t.name)).map(|(_, f)| f.clone()));
                if let Some(f) = imp.and_then(|n| prog.function(&n)) {
                    let sig = signature_for(prog, f, m, true, false, true);
                    items.push(format!("{sig};"));
                }
            }
            // every generated type shows itself, so a `dynamic` value of the interface shows its value (`rt::Show`)
            out.push_str(&format!("pub trait {}: rt::Show {{\n{}\n}}\n", ident(&t.name), indent(&items.join("\n"), 1)));
            for c in prog.types.iter().filter(|c| c.implements.contains(&t.name) && tys.contains(&c.name)) {
                let mut defs = Vec::new();
                for (m, _) in methods {
                    if let Some((_, fname)) = c.methods.iter().find(|(x, _)| x == m) {
                        if let Some(f) = prog.function(fname) {
                            let sig = signature_for(prog, f, m, true, false, true);
                            let triv_self = trivial(&fortis_types::types::named(&c.name), prog);
                            let args: Vec<String> = f.params.iter().map(|p| if f.locals[p.local as usize].kind == LocalKind::SelfParam { if triv_self && p.mode == Mode::Read { "*self".to_string() } else { "self".to_string() } } else { ident(&f.locals[p.local as usize].name) }).collect();
                            defs.push(format!("{sig} {{\n    {}::{}({})\n}}", ident(&c.name), ident(m), args.join(", ")));
                        }
                    }
                }
                out.push_str(&format!("impl {} for {} {{\n{}\n}}\n", ident(&t.name), ident(&c.name), indent(&defs.join("\n"), 1)));
            }
            out.push('\n');
        }
    }
    // ---- constants
    for c in &prog.consts {
        let mut dummy = Function { name: format!("{}::const {}", c.module, c.name), label: format!("const {}", c.name), module: c.module.clone(), body_module: None, line: 0, params: vec![], ret: c.ty.clone(), modifiers: vec![], locals: vec![], body: None, effects: Effects::default(), observe: None, closure: None, bounds: vec![] };
        dummy.ret = c.ty.clone();
        let mut g = FnGen::new(prog, &dummy, &mut audit, &names);
        let v = g.expr_as(&c.value, &c.ty);
        errors.extend(std::mem::take(&mut g.errors));
        out.push_str(&format!("pub fn {}() -> {} {{\n    {v}\n}}\n", gen::global_fn(&c.module, &c.name), rust_ty(&c.ty, prog)));
    }
    // ---- anonymous record shapes: those built in functions and in test rows
    let mut shapes: BTreeMap<String, Vec<(String, Type)>> = BTreeMap::new();
    for r in &audit.records {
        shapes.insert(record_name(r), r.clone());
    }
    // record types written in signatures and bindings
    fn records_in(t: &Type, out: &mut BTreeMap<String, Vec<(String, Type)>>) {
        match t {
            Type::Record(fs) => {
                out.insert(record_name(fs), fs.clone());
                fs.iter().for_each(|(_, x)| records_in(x, out));
            }
            Type::Named(n) => n.args.iter().for_each(|a| records_in(a, out)),
            Type::Optional(i) | Type::Loan { inner: i, .. } => records_in(i, out),
            Type::Tuple(ts) => ts.iter().for_each(|x| records_in(x, out)),
            Type::Fn { params, ret, .. } => {
                params.iter().for_each(|x| records_in(x, out));
                records_in(ret, out);
            }
            _ => {}
        }
    }
    for n in &fns {
        if let Some(f) = prog.function(n) {
            f.locals.iter().for_each(|l| records_in(&l.ty, &mut shapes));
            records_in(&f.ret, &mut shapes);
        }
    }
    for t in &prog.tests {
        for (_, rec) in &t.cases {
            if let ExprKind::Record(fs) = &rec.kind {
                let shape: Vec<(String, Type)> = fs.iter().map(|(n, v)| (n.clone(), v.ty.clone())).collect();
                shapes.insert(record_name(&shape), shape);
            }
        }
    }
    for (n, fs) in &shapes {
        let fields: Vec<String> = fs.iter().map(|(f, t)| format!("pub {}: {}", ident(f), rust_ty(t, prog))).collect();
        let shows: Vec<String> = fs.iter().map(|(f, _)| format!("format!(\"'{f}': {{}}\", rt::Show::repr(&self.{}))", ident(f))).collect();
        // a record of a class that has no `Clone` or `PartialEq` (a cleanup class, a `Shared` it holds) cannot
        // derive them; the same rules as a class declaration. `can_clone` also refuses a record of a recursive
        // enum that is not itself `Clone`, and accepts a string literal lowered to `String` (D-55).
        let mut derives = Vec::new();
        if can_clone(prog, &Type::Record(fs.clone()), &mut Vec::new()) {
            derives.push("Clone");
            // a record of trivial fields is trivial (6.11) and copies like a trivial class, so `Copy` makes a read
            // of a binding a copy, not a move: `return r` past a defer that reads it (D-415)
            if trivial(&Type::Record(fs.clone()), prog) {
                derives.push("Copy");
            }
        }
        if fs.iter().all(|(_, t)| record_can(prog, t, false)) {
            derives.push("PartialEq");
        }
        let derive_attr = if derives.is_empty() { String::new() } else { format!("#[derive({})]\n", derives.join(", ")) };
        out.push_str(&format!("{derive_attr}pub struct {n} {{ {} }}\nimpl rt::Show for {n} {{\n    fn show(&self) -> ::std::string::String {{\n        format!(\"{{{{{{}}}}}}\", vec![{}].join(\", \"))\n    }}\n}}\n", fields.join(", "), shows.join(", ")));
    }
    // ---- entry
    out.push_str(&entry(prog, opts, &names));
    if !errors.is_empty() {
        return Err(errors);
    }
    // a test build unwinds, so a false `expect` inside a closure reaches the test runner (14); a program aborts
    let panic = if opts.tests { "unwind" } else { "abort" };
    let cargo_toml = format!(
        "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nfortis-native = {{ path = \"{}\"{} }}\n\n[profile.release]\noverflow-checks = true\npanic = \"{panic}\"\ncodegen-units = 1\n\n[workspace]\n",
        opts.crate_name,
        opts.runtime,
        if alloc_probe() { ", features = [\"alloc-probe\"]" } else { "" }
    );
    Ok(Lowered { main_rs: out, cargo_toml, audit })
}

/// A class, enum or interface declaration with its display form and equality.
/// The type parameters a declaration's fields or payloads never mention (Rust needs a `PhantomData` for them).
pub fn unused_params(t: &TypeDef) -> Vec<String> {
    let mut used: BTreeSet<String> = BTreeSet::new();
    match &t.kind {
        TypeKind::Class { fields } => fields.iter().for_each(|f| type_params_in(&f.ty, &mut used)),
        TypeKind::Enum { variants } => variants.iter().for_each(|v| v.fields.iter().for_each(|(_, ft)| type_params_in(ft, &mut used))),
        _ => {}
    }
    t.type_params.iter().filter(|p| !used.contains(*p)).cloned().collect()
}

/// The `PhantomData` a declaration needs, if any: its type parameters no field mentions, and its lifetimes no
/// lowered field carries (a loan of a capability is a value, so a lifetime class may hold none).
pub fn phantom_ty(prog: &Program, t: &TypeDef) -> Option<String> {
    let mut parts: Vec<String> = vec![];
    let field_tys: Vec<String> = match &t.kind {
        TypeKind::Class { fields } => fields.iter().map(|f| rust_ty(&f.ty, prog)).collect(),
        TypeKind::Enum { variants } => variants.iter().flat_map(|v| v.fields.iter().map(|(_, ft)| rust_ty(ft, prog))).collect(),
        _ => vec![],
    };
    for l in &t.lifetimes {
        let lt = format!("'{}", ident(l));
        if !field_tys.iter().any(|f| f.contains(&lt)) {
            parts.push(format!("&{lt} ()"));
        }
    }
    parts.extend(unused_params(t).iter().map(|p| ident(p)));
    if parts.is_empty() {
        None
    } else {
        Some(format!("std::marker::PhantomData<({},)>", parts.join(", ")))
    }
}

/// The locals an expression reads (a guard's, to know which bindings it needs before an arm's prelude runs; D-3).
pub(crate) fn locals_read(e: &Expr, out: &mut Vec<LocalId>) {
    visit_expr(e, &mut |x: &Expr| {
        if let ExprKind::Place(p, _) = &x.kind {
            if !out.contains(&p.local) {
                out.push(p.local);
            }
        }
    });
}

/// Whether a method is Displayable's `display(self, into: editable Builder) Void` (LR 11.1; D-83).
pub fn displayable_impl(f: &Function) -> bool {
    !f.has("async")
        && !f.is_try()
        && f.params.len() == 2
        && f.params[0].mode == Mode::Read
        && f.params[1].mode == Mode::Editable
        && name_of(&f.locals[f.params[1].local as usize].ty) == Some("Builder")
        && matches!(strip(&f.ret), Type::Named(n) if n.name == "Void")
}

/// The `Show` body of a type whose `display` is hand-written (Displayable, LR 11.1): interpolation writes what that
/// method appends to a fresh Builder (D-83).
const SHOW_BY_DISPLAY: &str = "let mut __into = rt::text::Builder::create();\n        self.display(&mut __into);\n        __into.finish()";

/// A heap collection or pointer is the indirection boundary: a `List[Tree]` is `Vec<Tree>`, which has a size, so a
/// field of that type is not boxed again. `Outcome[T]` is not here: it lowers to `Result<T, Failure>`, which stores
/// `T` inline, so a recursive field of that type is boxed (D-55).
const HEAP_BOUNDARY: &[&str] = &["List", "ListView", "Map", "OrderedMap", "Set", "Array", "FixedList", "FixedMap", "Shared", "Weak", "Mutex"];

/// `t` with each type parameter in `map` replaced (a generic wrapper's fields mention the parameter, not the argument).
pub(crate) fn subst_type(t: &Type, map: &[(String, Type)]) -> Type {
    match t {
        Type::TypeParam(p) => map.iter().find(|(n, _)| n == p).map(|(_, ty)| ty.clone()).unwrap_or_else(|| t.clone()),
        Type::Named(n) => Type::Named(NamedT { name: n.name.clone(), args: n.args.iter().map(|a| subst_type(a, map)).collect(), package: n.package.clone(), origin: n.origin.clone() }),
        Type::Refined { base, lo, hi, kind } => Type::Refined {
            base: NamedT { name: base.name.clone(), args: base.args.iter().map(|a| subst_type(a, map)).collect(), package: base.package.clone(), origin: base.origin.clone() },
            lo: *lo,
            hi: *hi,
            kind: kind.clone(),
        },
        Type::Optional(i) => Type::Optional(Box::new(subst_type(i, map))),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|x| subst_type(x, map)).collect()),
        Type::Record(fs) => Type::Record(fs.iter().map(|(n, x)| (n.clone(), subst_type(x, map))).collect()),
        Type::Loan { inner, mode, origin } => Type::Loan { inner: Box::new(subst_type(inner, map)), mode: mode.clone(), origin: origin.clone() },
        Type::Fn { capture_origins, param_modes, params, ret, kind, modifiers, origin, dynamic, identity } => Type::Fn {
            capture_origins: capture_origins.clone(),
            param_modes: param_modes.clone(),
            params: params.iter().map(|p| subst_type(p, map)).collect(),
            ret: Box::new(subst_type(ret, map)),
            kind: kind.clone(),
            modifiers: modifiers.clone(),
            origin: origin.clone(),
            dynamic: *dynamic,
            identity: identity.clone(),
        },
        Type::Dynamic(n) => Type::Dynamic(NamedT { name: n.name.clone(), args: n.args.iter().map(|a| subst_type(a, map)).collect(), package: n.package.clone(), origin: n.origin.clone() }),
        Type::Unknown(_) => t.clone(),
    }
}

/// Whether `t` reaches `target` by value: through optionals, tuples, records, classes, enums and newtypes (a
/// newtype's Rust form is its representation's, types.rs `rust_ty`), not through a heap collection. A generic
/// wrapper contributes its fields with the wrapper's type arguments substituted, so `Hold[Tree]` reaches `Tree`
/// through `value: T`. The same wrapper at two instantiations (`Hold[Hold[Tree]]`) is two steps, not one cycle:
/// `seen` records the instantiated type, not the declaration's name (D-55).
pub fn reaches_type(prog: &Program, t: &Type, target: &str, seen: &mut Vec<String>) -> bool {
    match strip(t) {
        Type::Optional(i) => reaches_type(prog, i, target, seen),
        Type::Tuple(ts) => ts.iter().any(|x| reaches_type(prog, x, target, seen)),
        Type::Record(fs) => fs.iter().any(|(_, x)| reaches_type(prog, x, target, seen)),
        Type::Named(_) | Type::Refined { .. } => {
            let n = match name_of(t) {
                Some(n) => n.to_string(),
                None => return false,
            };
            if n == target {
                return true;
            }
            // `Outcome[Tree]` is `Result<Tree, Failure>`: the success value is inline, so it reaches `Tree` (D-55)
            if n == "Outcome" {
                return args_of(t).iter().any(|a| reaches_type(prog, a, target, seen));
            }
            // `List[Tree]` does not reach `Tree`: the collection is the box (D-55)
            if HEAP_BOUNDARY.contains(&n.as_str()) {
                return false;
            }
            // the instantiated type, so `Hold[Hold[Tree]]` is not the same step as the `Hold[Tree]` it contains
            let key = strip(t).show();
            if seen.contains(&key) {
                return false;
            }
            seen.push(key);
            // clone the nested types out of the definition before recursing: the walk borrows `prog` again.
            // substitute the use's type arguments so a field `value: T` is seen as the argument.
            let nested: Vec<Type> = match prog.type_def(&n) {
                Some(d) => {
                    let map: Vec<(String, Type)> = d.type_params.iter().cloned().zip(args_of(t)).collect();
                    match &d.kind {
                        TypeKind::Class { fields } => fields.iter().map(|f| subst_type(&f.ty, &map)).collect(),
                        TypeKind::Enum { variants } => variants.iter().flat_map(|v| v.fields.iter().map(|(_, ft)| subst_type(ft, &map))).collect(),
                        TypeKind::Newtype { repr } => vec![subst_type(repr, &map)],
                        _ => vec![],
                    }
                }
                None => vec![],
            };
            let hit = nested.iter().any(|ft| reaches_type(prog, ft, target, seen));
            seen.pop();
            hit
        }
        _ => false,
    }
}

/// Whether a value of type `t` owns a `cleanup` (7.9): its own class declares one, or an owned field, enum
/// payload, newtype representation or owning container does. A loan does not own the borrowed value. A cycle
/// does not own a second copy. Used to drop the local with its block, before an outer `defer` (D-181).
pub fn owns_cleanup(prog: &Program, t: &Type, seen: &mut Vec<String>) -> bool {
    if matches!(t, Type::Loan { .. }) {
        return false;
    }
    // a tuple or record part whose type is a loan may still be a value of the aggregate's own, held as a `MaybeOwned`
    // (a `??` whose fallback was taken, 7.9; D-366): the aggregate is destroyed explicitly, in its turn, so that the
    // value it may own goes in reverse initialisation order with the block's other locals (D-370)
    let part = |x: &Type| match x {
        Type::Loan { inner, .. } => (**inner).clone(),
        other => other.clone(),
    };
    match strip(t) {
        Type::Optional(i) => owns_cleanup(prog, i, seen),
        Type::Tuple(ts) => ts.iter().any(|x| owns_cleanup(prog, &part(x), seen)),
        Type::Record(fs) => fs.iter().any(|(_, x)| owns_cleanup(prog, &part(x), seen)),
        Type::Named(_) | Type::Refined { .. } => {
            let n = match name_of(t) {
                Some(n) => n.to_string(),
                None => return false,
            };
            // an owning container destroys its values (7.9); its name is not a user type on the cycle
            if matches!(n.as_str(), "List" | "ListView" | "Map" | "OrderedMap" | "Set" | "Array" | "FixedList" | "FixedMap" | "Shared" | "Mutex" | "Outcome") {
                return args_of(t).iter().any(|a| owns_cleanup(prog, a, seen));
            }
            if seen.iter().any(|s| s == &n) {
                return false;
            }
            if prog.type_def(&n).map_or(false, |d| d.cleanup.is_some()) {
                return true;
            }
            seen.push(n.clone());
            let nested: Vec<Type> = match prog.type_def(&n).map(|d| &d.kind) {
                Some(TypeKind::Class { fields }) => fields.iter().map(|f| f.ty.clone()).collect(),
                Some(TypeKind::Enum { variants }) => variants.iter().flat_map(|v| v.fields.iter().map(|(_, ft)| ft.clone())).collect(),
                Some(TypeKind::Newtype { repr }) => vec![repr.clone()],
                _ => vec![],
            };
            let hit = nested.iter().any(|ft| owns_cleanup(prog, ft, seen));
            seen.pop();
            hit
        }
        _ => false,
    }
}

/// The local's represented class declares `cleanup`. Newtypes are erased at run time, so their representation
/// supplies the trace name. Fields and containers remain indirect owners, with no second drop event (7.9).
pub fn cleanup_name<'p>(prog: &'p Program, t: &Type) -> Option<&'p str> {
    let mut ty = t;
    let mut seen = Vec::new();
    loop {
        let d = prog.type_def(name_of(ty)?)?;
        if seen.contains(&d.name.as_str()) {
            return None;
        }
        seen.push(d.name.as_str());
        if d.cleanup.is_some() {
            return Some(d.name.as_str());
        }
        match &d.kind {
            TypeKind::Newtype { repr } => ty = repr,
            _ => return None,
        }
    }
}

pub fn declares_cleanup(prog: &Program, t: &Type) -> bool {
    cleanup_name(prog, t).is_some()
}

/// A class that contains itself by value (through optionals, tuples and other classes, not through a heap
/// collection): Rust needs a box to give it a size. Such a class lowers to a boxed newtype that dereferences to its
/// fields (`pub struct Node(pub Box<Node_>)`), so field reads and writes are unchanged and only construction wraps.
pub fn recursive_class(prog: &Program, name: &str) -> bool {
    match prog.type_def(name) {
        Some(d) if d.type_params.is_empty() && d.lifetimes.is_empty() => match &d.kind {
            TypeKind::Class { fields } => fields.iter().any(|f| reaches_type(prog, &f.ty, name, &mut vec![name.to_string()])),
            _ => false,
        },
        _ => false,
    }
}

/// An enum that contains itself by value (through optionals, tuples, records, classes, newtypes and other
/// enums, including a generic enum and a generic wrapper): Rust needs a box to give it a size (D-55). Every
/// variant field that reaches the enum lowers as `Box<T>`: construction wraps, a name binding dereferences in
/// the arm's prelude, and a nested pattern selects through a guard fragment over the dereferenced box and
/// extracts its bindings in the prelude (Rust patterns do not look inside a box). A generic class is not boxed
/// this way: its newtype form does not carry the class's type arguments.
pub fn recursive_enum(prog: &Program, name: &str) -> bool {
    match prog.type_def(name).map(|d| &d.kind) {
        Some(TypeKind::Enum { variants }) => variants.iter().any(|v| v.fields.iter().any(|(_, ft)| reaches_type(prog, ft, name, &mut vec![name.to_string()]))),
        _ => false,
    }
}

/// Whether a variant field of the enum `name` lowers as `Box<T>` (see `recursive_enum`).
pub fn boxes_field(prog: &Program, name: &str, ft: &Type) -> bool {
    recursive_enum(prog, name) && reaches_type(prog, ft, name, &mut vec![name.to_string()])
}

fn type_decl(prog: &Program, t: &TypeDef) -> String {
    let lts: Vec<String> = t.lifetimes.iter().map(|l| format!("'{}", ident(l))).collect();
    let _phantom = unused_params(t);
    let tps: Vec<String> = t.type_params.iter().map(|p| ident(p)).collect();
    let all: Vec<String> = lts.iter().cloned().chain(tps.iter().cloned()).collect();
    let gens = if all.is_empty() { String::new() } else { format!("<{}>", all.join(", ")) };
    let bounded: Vec<String> = lts.iter().cloned().chain(t.type_params.iter().map(|p| generic_param(prog, &t.bounds, p))).collect();
    let gens_b = if bounded.is_empty() { String::new() } else { format!("<{}>", bounded.join(", ")) };
    // Structural `Show` only displays: `T: rt::Show`. A recursive enum is not `Clone`, and a generic wrapper around
    // it still has to show (D-55). A hand-written `display` lives on the inherent impl, which keeps `BOUND`, so that
    // `Show` uses the same bounds or it cannot call the method.
    let show_bounded: Vec<String> = lts.iter().cloned().chain(tps.iter().map(|p| format!("{p}: rt::Show"))).collect();
    let show_gens = if show_bounded.is_empty() { String::new() } else { format!("<{}>", show_bounded.join(", ")) };
    // a hand-written `display` that implements Displayable (its contract's signature), not any method of that name
    let has_display = t.methods.iter().any(|(m, f)| m == "display" && prog.function(f).map_or(false, displayable_impl));
    let show_impl_gens = if has_display { &gens_b } else { &show_gens };
    let has_equals = t.methods.iter().any(|(m, _)| m == "equals");
    // a hand-written `compare` (`Comparable`) orders the type: `<` and a generic body's comparisons call it (D-82)
    let compare_fn = t.methods.iter().find(|(m, _)| m == "compare").and_then(|(_, f)| prog.function(f));
    let ordered_by = |eq: bool| -> String {
        let f = match compare_fn {
            Some(f) if eq => f,
            _ => return String::new(),
        };
        let ord = rust_ty(&f.ret, prog);
        let triv = trivial(&fortis_types::types::named(&t.name), prog);
        let (a, b) = if triv { ("*self", "*o") } else { ("self", "o") };
        let (less, equal, greater) = (ident("Less"), ident("Equal"), ident("Greater"));
        format!(
            "impl{gens_b} ::std::cmp::PartialOrd for {}{gens} {{\n    fn partial_cmp(&self, o: &Self) -> Option<::std::cmp::Ordering> {{\n        Some(match {}::compare({a}, {b}) {{ {ord}::{less} => ::std::cmp::Ordering::Less, {ord}::{equal} => ::std::cmp::Ordering::Equal, {ord}::{greater} => ::std::cmp::Ordering::Greater }})\n    }}\n}}\n",
            ident(&t.name),
            ident(&t.name)
        )
    };
    // Core declarations such as time.Instant have built-in Hashable without a written derive (D-593).
    let derives_hash = t.derives.iter().any(|x| x == "Hashable") || (!is_user_module(&t.module) && fortis_types::decls::builtin_implements(&t.name, "Hashable"));
    let loans = |ty: &Type| matches!(ty, Type::Loan { .. }) || matches!(strip(ty), Type::Fn { .. });
    match &t.kind {
        TypeKind::Class { fields } => {
            let copyable = t.copyable && !fields.iter().any(|f| loans(&f.ty));
            // Recursive key wrappers need the same field-wise Clone derive as generic keys (D-594).
            let generic_clone = (!t.type_params.is_empty() || recursive_class(prog, &t.name)) && fields.iter().all(|f| clone_capable(prog, &f.ty, &mut vec![t.name.clone()]));
            let mut derives = vec![];
            if copyable || t.trivial || generic_clone {
                derives.push("Clone");
            }
            if t.trivial && !fields.iter().any(|f| loans(&f.ty)) {
                derives.push("Copy");
            }
            let eq = !has_equals && fields.iter().all(|f| has_eq(prog, &f.ty, &mut Vec::new()));
            if eq {
                derives.push("PartialEq");
            }
            // a derived `Comparable` orders the class by its fields in declaration order (X-516; D-54.1): Rust's
            // derive matches, when every field orders and no hand-written `compare` lowers instead (`ordered_by`)
            let der_cmp = t.derives.iter().any(|x| x == "Comparable");
            if der_cmp && compare_fn.is_none() && fields.iter().all(|f| has_ord(prog, &f.ty, &mut Vec::new())) {
                derives.push("PartialOrd");
            }
            // a derived `Hashable` that writes neither `equals` nor `hash` hashes by its fields; `Eq` marks the
            // derived equality total, so the type is a `Map` key or `Set` element (X-516)
            let has_hash_fn = t.methods.iter().any(|(m, _)| m == "hash");
            let hashable = derives_hash || t.implements.iter().any(|x| x == "Hashable");
            let derived_hash = derives_hash && !has_equals && !has_hash_fn && eq && fields.iter().all(|f| has_hash(prog, &f.ty, &mut Vec::new()));
            if derived_hash {
                derives.push("Eq");
                derives.push("Hash");
            }
            let mut fs: Vec<String> = fields.iter().map(|f| format!("pub {}: {}", ident(&f.name), rust_ty(&f.ty, prog))).collect();
            if let Some(pt) = phantom_ty(prog, t) {
                fs.push(format!("pub __phantom: {pt}"));
            }
            let show = if has_display {
                SHOW_BY_DISPLAY.to_string()
            } else if fields.is_empty() {
                format!("{:?}.to_string()", format!("{}()", t.name))
            } else {
                let parts: Vec<String> = fields.iter().map(|f| format!("format!(\"{}: {{}}\", rt::Show::show(&self.{}))", f.name, ident(&f.name))).collect();
                format!("format!(\"{}({{}})\", vec![{}].join(\", \"))", t.name, parts.join(", "))
            };
            let mut s = String::new();
            if recursive_class(prog, &t.name) {
                // the boxed newtype of a class that contains itself (see `recursive_class`)
                let n = ident(&t.name);
                let d = if derives.is_empty() { String::new() } else { format!("#[derive({})]\n", derives.iter().filter(|x| **x != "Copy").cloned().collect::<Vec<_>>().join(", ")) };
                s.push_str(&format!("{d}pub struct {n}_ {{\n{}\n}}\n", indent(&fs.join(",\n"), 1)));
                s.push_str(&format!("{d}pub struct {n}(pub Box<{n}_>);\n"));
                s.push_str(&format!("impl std::ops::Deref for {n} {{\n    type Target = {n}_;\n    fn deref(&self) -> &{n}_ {{\n        &self.0\n    }}\n}}\n"));
                s.push_str(&format!("impl std::ops::DerefMut for {n} {{\n    fn deref_mut(&mut self) -> &mut {n}_ {{\n        &mut self.0\n    }}\n}}\n"));
            } else {
                if !derives.is_empty() {
                    s.push_str(&format!("#[derive({})]\n", derives.join(", ")));
                }
                s.push_str(&format!("pub struct {}{gens} {{\n{}\n}}\n", ident(&t.name), indent(&fs.join(",\n"), 1)));
            }
            s.push_str(&format!("impl{show_impl_gens} rt::Show for {}{gens} {{\n    fn show(&self) -> ::std::string::String {{\n        {show}\n    }}\n}}\n", ident(&t.name)));
            if has_equals {
                if let Some((_, f)) = t.methods.iter().find(|(m, _)| m == "equals") {
                    let _ = f;
                    // a trivial class's methods take it by value
                    let (a, b) = if t.trivial && !fields.iter().any(|f| loans(&f.ty)) { ("*self", "*o") } else { ("self", "o") };
                    s.push_str(&format!("impl{gens_b} ::std::cmp::PartialEq for {}{gens} {{\n    fn eq(&self, o: &Self) -> bool {{\n        {}::equals({a}, {b})\n    }}\n}}\n", ident(&t.name), ident(&t.name)));
                }
            }
            s.push_str(&ordered_by(has_equals || eq));
            // a class that is `Hashable` and writes its own `equals` or `hash`: every value hashes alike (`Hasher`
            // has no members in Language 1.0, so a hand-written `hash` writes nothing) and the containers decide by
            // `equals` (X-516; D-84). `Eq` needs the `PartialEq` the hand-written `equals` or the derivation gives.
            if hashable && !derived_hash && (has_equals || has_hash_fn) && (has_equals || eq) {
                s.push_str(&format!("impl{gens_b} ::std::hash::Hash for {}{gens} {{\n    fn hash<__H: ::std::hash::Hasher>(&self, _state: &mut __H) {{}}\n}}\n", ident(&t.name)));
                s.push_str(&format!("impl{gens_b} Eq for {}{gens} {{}}\n", ident(&t.name)));
            }
            s.push('\n');
            s
        }
        TypeKind::Enum { variants } => {
            let mut vs: Vec<String> = variants
                .iter()
                .map(|v| {
                    if v.fields.is_empty() {
                        ident(&v.name)
                    } else {
                        // a field of a recursive enum that reaches it is boxed (see `boxes_field`)
                        format!("{} {{ {} }}", ident(&v.name), v.fields.iter().map(|(n, ft)| { let rt = rust_ty(ft, prog); if boxes_field(prog, &t.name, ft) { format!("{}: Box<{rt}>", ident(n)) } else { format!("{}: {rt}", ident(n)) } }).collect::<Vec<_>>().join(", "))
                    }
                })
                .collect();
            if let Some(pt) = phantom_ty(prog, t) {
                vs.push(format!("#[doc(hidden)]\n__Phantom({pt}, std::convert::Infallible)"));
            }
            // a hand-written `display` is the enum's display form (D-83)
            let arms_override: Option<String> = if has_display { Some(SHOW_BY_DISPLAY.to_string()) } else { None };
            // inside `impl<T> Show for Chain<T>`, `Chain::End` is missing its type arguments (E0107). `Self` names them.
            let vp = if t.type_params.is_empty() { ident(&t.name) } else { "Self".to_string() };
            let mut arms: Vec<String> = variants
                .iter()
                .map(|v| {
                    if v.fields.is_empty() {
                        format!("{vp}::{} => {:?}.to_string(),", ident(&v.name), v.name)
                    } else {
                        let binds: Vec<String> = v.fields.iter().map(|(n, _)| ident(n)).collect();
                        let parts: Vec<String> = v.fields.iter().map(|(n, ft)| { let arg = if boxes_field(prog, &t.name, ft) { format!("&**{}", ident(n)) } else { ident(n) }; format!("format!(\"{n}: {{}}\", rt::Show::show({arg}))") }).collect();
                        format!("{vp}::{} {{ {} }} => format!(\"{}({{}})\", vec![{}].join(\", \")),", ident(&v.name), binds.join(", "), v.name, parts.join(", "))
                    }
                })
                .collect();
            if phantom_ty(prog, t).is_some() {
                arms.push(format!("{vp}::__Phantom(..) => unreachable!()",));
            }
            let triv = trivial(&fortis_types::types::named(&t.name), prog) && phantom_ty(prog, t).is_none();
            let payload_free = variants.iter().all(|v| v.fields.is_empty());
            // a derived `Comparable` orders the enum by variant order, then its payload's fields (X-516; D-54.1):
            // Rust's derive matches, when every payload orders and no hand-written `compare` lowers instead. A
            // payload-free enum always orders by its variant order.
            let der_cmp = t.derives.iter().any(|x| x == "Comparable");
            let der_ord = der_cmp && compare_fn.is_none() && variants.iter().all(|v| v.fields.iter().all(|(_, ft)| has_ord(prog, ft, &mut vec![t.name.clone()])));
            let ord = if compare_fn.is_none() && (payload_free || der_ord) { ", PartialOrd" } else { "" };
            // `PartialEq` only when every payload has it (a payload of a class that cannot derive it, a Shared, a
            // dynamic value), as for classes; a hand-written `equals` is the type's equality instead (D-82)
            let eq = !has_equals && variants.iter().all(|v| v.fields.iter().all(|(_, ft)| has_eq(prog, ft, &mut vec![t.name.clone()])));
            // a derived `Hashable` that writes neither `equals` nor `hash` hashes by its payload (X-516)
            let has_hash_fn = t.methods.iter().any(|(m, _)| m == "hash");
            let hashable = derives_hash || t.implements.iter().any(|x| x == "Hashable");
            let derived_hash = derives_hash && !has_equals && !has_hash_fn && eq && variants.iter().all(|v| v.fields.iter().all(|(_, ft)| has_hash(prog, ft, &mut vec![t.name.clone()])));
            let eh = if derived_hash { ", Eq, Hash" } else { "" };
            let pe = format!("{}{}{}", if eq { ", PartialEq" } else { "" }, if eq || has_equals { ord } else { "" }, eh);
            let generic_clone = !t.type_params.is_empty() && variants.iter().all(|v| v.fields.iter().all(|(_, ft)| clone_capable(prog, ft, &mut vec![t.name.clone()])));
            let derives = if triv { format!("Clone, Copy{pe}") } else if t.copyable || generic_clone { format!("Clone{pe}") } else { pe.trim_start_matches(", ").to_string() };
            let derive_attr = if derives.is_empty() { String::new() } else { format!("#[derive({derives})]\n") };
            // a hand-written `equals` is the enum's equality (D-82)
            let equality = if has_equals {
                let triv = trivial(&fortis_types::types::named(&t.name), prog);
                let (a, b) = if triv { ("*self", "*o") } else { ("self", "o") };
                format!("impl{gens_b} ::std::cmp::PartialEq for {}{gens} {{\n    fn eq(&self, o: &Self) -> bool {{\n        {}::equals({a}, {b})\n    }}\n}}\n", ident(&t.name), ident(&t.name))
            } else {
                String::new()
            };
            // an enum that is `Hashable` and writes its own `equals` or `hash` hashes nothing, so equal values hash
            // alike and the containers decide by `equals` (X-516; D-84)
            let hash_impl = if hashable && !derived_hash && (has_equals || has_hash_fn) && (has_equals || eq) {
                format!("impl{gens_b} ::std::hash::Hash for {}{gens} {{\n    fn hash<__H: ::std::hash::Hasher>(&self, _state: &mut __H) {{}}\n}}\nimpl{gens_b} Eq for {}{gens} {{}}\n", ident(&t.name), ident(&t.name))
            } else {
                String::new()
            };
            let body = match &arms_override {
                Some(b) => b.clone(),
                None => format!("match self {{\n{}\n        }}", indent(&arms.join("\n"), 3)),
            };
            format!(
                "{derive_attr}pub enum {}{gens} {{\n{}\n}}\nimpl{show_impl_gens} rt::Show for {}{gens} {{\n    fn show(&self) -> ::std::string::String {{\n        {body}\n    }}\n}}\n{equality}{hash_impl}{}\n",
                ident(&t.name),
                indent(&vs.join(",\n"), 1),
                ident(&t.name),
                ordered_by(has_equals || eq)
            )
        }
        _ => String::new(),
    }
}

/// `main`: the program's entry, or its test runner.
/// Whether the allocator probe build is requested (`FORTIS_ALLOC_PROBE=1`, SP-04-async).
fn alloc_probe() -> bool {
    std::env::var_os("FORTIS_ALLOC_PROBE").map_or(false, |v| v == "1")
}

/// A function the probe wraps: a free `noalloc` async function whose parameters are all owned (the wrapper's
/// future then borrows nothing).
fn probed(f: &Function, sig: &str, in_impl: bool) -> bool {
    let params = sig.split_once('(').and_then(|(_, rest)| rest.rsplit_once(") ->")).map_or("", |(p, _)| p);
    // a lambda is lowered as a closure at its use, not as a function the probe could wrap
    f.closure.is_none() && f.has("noalloc") && f.has("async") && !in_impl && f.observe.is_none() && !params.contains('&') && !params.contains("impl ")
}

fn entry(prog: &Program, opts: &Options, names: &HashMap<String, String>) -> String {
    if opts.tests {
        let mut cases = Vec::new();
        for (ti, t) in prog.tests.iter().enumerate() {
            let rust = match names.get(&t.function) {
                Some(r) => r.clone(),
                None => continue,
            };
            let f = prog.function(&t.function).unwrap();
            let run = |call: String| format!("::std::boxed::Box::new(move || match {call} {{ Ok(()) => rt::CaseResult::Pass, Err(TestFailure::Expect(ev)) => rt::CaseResult::Fail(ev), Err(TestFailure::Failure(f)) => rt::CaseResult::Fail(format!(\"actual=failure {{}} expected=success\", f.code)) }})");
            if t.kind == "property" {
                let seed: u128 = t.title.chars().map(|c| c as u128).sum::<u128>() * 7919;
                let gens: Vec<String> = f.params.iter().map(|p| generator(&f.locals[p.local as usize].ty, prog)).collect();
                cases.push(format!(
                    "{{\n    let mut __rng = rt::pyrandom::PyRandom::new({seed});\n    for __i in 0..25 {{\n        {}\n        __cases.push(({:?}.to_string(), format!(\"case {{}}\", __i + 1), {}));\n    }}\n}}",
                    f.params.iter().zip(gens.iter()).map(|(p, g)| format!("let {} = {g};", ident(&f.locals[p.local as usize].name))).collect::<Vec<_>>().join(" "),
                    t.title,
                    run(format!("{rust}({})", f.params.iter().map(|p| arg_for(&f.locals[p.local as usize].ty, ident(&f.locals[p.local as usize].name), prog)).collect::<Vec<_>>().join(", ")))
                ));
            } else if t.cases.is_empty() {
                cases.push(format!("__cases.push(({:?}.to_string(), String::new(), {}));", t.title, run(format!("{rust}()"))));
            } else {
                for (ci, (cname, rec)) in t.cases.iter().enumerate() {
                    let mut audit = Audit::default();
                    let dummy = Function { name: format!("main::test case {ti}.{ci}"), label: "test case".into(), module: "main".into(), body_module: None, line: 0, params: vec![], ret: fortis_types::types::void(), modifiers: vec![], locals: vec![], body: None, effects: Effects::default(), observe: None, closure: None, bounds: vec![] };
                    let mut g = FnGen::new(prog, &dummy, &mut audit, names);
                    let v = g.expr(rec, gen::Want::Value);
                    cases.push(format!("{{ let __row = {v}; __cases.push(({:?}.to_string(), {:?}.to_string(), {})); }}", t.title, cname, run(format!("{rust}(__row)"))));
                }
            }
        }
        for (title, why) in &prog.unbuilt_tests {
            cases.push(format!("__cases.push(({title:?}.to_string(), String::new(), ::std::boxed::Box::new(|| rt::CaseResult::Unbuilt({why:?}.to_string()))));"));
        }
        let head = format!("{}{}", decl_files_prelude(opts), observed_table(prog));
        return format!(
            "fn main() {{\n{}    let mut __cases: Vec<(String, String, ::std::boxed::Box<dyn FnOnce() -> rt::CaseResult>)> = Vec::new();\n{}\n    std::process::exit(rt::run_tests({:?}, __cases));\n}}\n",
            head,
            indent(&cases.join("\n"), 1),
            opts.file
        );
    }
    let main = match prog.entry.as_ref().and_then(|e| prog.function(e)) {
        Some(m) => m,
        None => return "fn main() {}\n".into(),
    };
    // each parameter of the entry `main` is supplied by its type (4.6, X-345): the command-line arguments for
    // `List[String]`, and for a capability type that capability, all created once, before `main` runs (11.6)
    let kinds: Vec<String> = main.params.iter().map(|p| name_of(&main.locals[p.local as usize].ty).unwrap_or("").to_string()).collect();
    let caps = if kinds.iter().any(|k| k != "List") { "let __caps = rt::host::capabilities(); " } else { "" };
    let passed: Vec<String> = kinds.iter().map(|k| if k == "List" { "&__args".to_string() } else { format!("__caps.{}", capability_field(k)) }).collect();
    let call = format!("{{ {caps}fortis_main({}) }}", passed.join(", "));
    // an async main is the main task's future, run by the controlled scheduler (main is never cancelled)
    let call = if main.has("async") { format!("rt::task::block_on({call}).unwrap_or_else(|_| rt::dynamic(\"the main task was cancelled\"))") } else { call };
    let body = if main.is_try() { format!("{call}") } else { format!("{{ {call}; Ok(()) }}") };
    // the allocator probe also drops one never-polled future of each probed function whose parameters have
    // defaults (the drop-before-completion phase, as for a task cancelled before it starts)
    let mut early = String::new();
    if alloc_probe() {
        for f in &prog.functions {
            let Some(rust) = names.get(&f.name) else { continue };
            if rust.contains("::") {
                continue;
            }
            let sig = signature(prog, f, rust, false, false);
            let scalar = f.params.iter().all(|p| matches!(param_ty(&f.locals[p.local as usize].ty, p.mode, prog).as_str(), "i64" | "i32" | "i16" | "i8" | "u64" | "u32" | "u16" | "u8" | "bool" | "f64" | "f32"));
            if probed(f, &sig, false) && scalar {
                let args = vec!["Default::default()"; f.params.len()].join(", ");
                early.push_str(&format!("    drop({rust}({args}));\n"));
            }
        }
    }
    let observed = format!("{}{}", decl_files_prelude(opts), observed_table(prog));
    format!(
        "fn main() {{\n{observed}{early}    let __args: ::std::vec::Vec<::std::string::String> = std::env::args().skip(1).collect();\n    let code = rt::run_main({:?}, || {body});\n    std::process::exit(code);\n}}\n",
        opts.file
    )
}

fn generator(t: &Type, prog: &Program) -> String {
    match name_of(t) {
        Some("Bool") => "__rng.random() < 0.5".into(),
        Some("String") => "{ let __n = __rng.randint(0, 8); let __pool: Vec<char> = \"abcxyz \".chars().collect(); (0..__n).map(|_| __pool[__rng.below(__pool.len() as u128) as usize]).collect::<String>() }".into(),
        Some("List") => {
            let inner = args_of(t).first().cloned().unwrap_or_else(fortis_types::types::int);
            format!("{{ let __k = __rng.randint(0, 8); (0..__k).map(|_| {}).collect::<Vec<_>>() }}", generator(&inner, prog))
        }
        _ => match strip(t) {
            Type::Refined { lo, hi, .. } => format!("__rng.randint({}, {}) as i64", lo.unwrap_or(-1000), hi.unwrap_or(1000)),
            _ => "__rng.randint(-1000, 1000) as i64".into(),
        },
    }
}

/// A property's generated value passed to its owned parameter.
fn arg_for(_t: &Type, name: String, _prog: &Program) -> String {
    name
}

/// Whether a record field can sit in a derived `Clone` (`clone`) or `PartialEq`. A user class or enum contributes
/// only what its own declaration derives. Builtins contribute what the runtime implements (`Shared` is not
/// `Clone`; a cleanup class is neither).
fn record_can(prog: &Program, ty: &Type, clone: bool) -> bool {
    fn walk(prog: &Program, ty: &Type, clone: bool, seen: &mut Vec<String>) -> bool {
        if matches!(ty, Type::Loan { .. }) || matches!(strip(ty), Type::Fn { .. } | Type::Dynamic(_)) {
            return false;
        }
        match strip(ty) {
            Type::Optional(i) => walk(prog, i, clone, seen),
            Type::Tuple(ts) => ts.iter().all(|t| walk(prog, t, clone, seen)),
            Type::Record(fs) => fs.iter().all(|(_, t)| walk(prog, t, clone, seen)),
            Type::TypeParam(_) => true,
            other => {
                let n = name_of(other).unwrap_or("");
                if clone && matches!(n, "Shared" | "Weak" | "Mutex" | "MutexGuard" | "Task" | "Failure") {
                    return false;
                }
                if !clone && matches!(n, "Mutex" | "MutexGuard" | "Task" | "Failure" | "Weak") {
                    return false;
                }
                if !n.is_empty() && seen.iter().any(|s| s == n) {
                    return true;
                }
                match prog.type_def(n) {
                    Some(d) => match &d.kind {
                        TypeKind::Class { .. } => {
                            if clone {
                                d.copyable || d.trivial
                            } else {
                                // `has_eq` treats a name already in `seen` as acyclic and true, so do not push first
                                has_eq(prog, other, seen)
                            }
                        }
                        TypeKind::Enum { variants } => {
                            if clone {
                                d.copyable || d.trivial
                            } else {
                                seen.push(n.to_string());
                                let ok = variants.iter().all(|v| v.fields.iter().all(|(_, ft)| walk(prog, ft, false, seen)));
                                seen.pop();
                                ok
                            }
                        }
                        TypeKind::Newtype { repr } => walk(prog, repr, clone, seen),
                        TypeKind::Error { .. } => false,
                        _ => args_of(other).iter().all(|a| walk(prog, a, clone, seen)),
                    },
                    None => args_of(other).iter().all(|a| walk(prog, a, clone, seen)),
                }
            }
        }
    }
    walk(prog, ty, clone, &mut Vec::new())
}

/// Whether a value of `ty` has `PartialEq` in the generated crate: a class derives it only when every field has it,
/// so the decision follows field types into other classes (a class holding a class that cannot derive it cannot
/// either); a class with its own `equals` implements it by hand.
/// Whether the generated Rust type implements `Clone`. A recursive enum is boxed, but it derives `Clone` only
/// when it is trivial or copyable, so a record that holds one cannot derive `Clone` itself (E0277). A string
/// literal is a refinement of `$strlit`, and `rust_ty` lowers that to `String`, which is `Clone`.
fn can_clone(prog: &Program, t: &Type, seen: &mut Vec<String>) -> bool {
    if matches!(t, Type::Loan { .. }) || matches!(strip(t), Type::Fn { .. } | Type::Dynamic(_)) {
        return false;
    }
    if trivial(t, prog) {
        return true;
    }
    match strip(t) {
        Type::Optional(i) => can_clone(prog, i, seen),
        Type::Tuple(ts) => ts.iter().all(|x| can_clone(prog, x, seen)),
        Type::Record(fs) => fs.iter().all(|(_, x)| can_clone(prog, x, seen)),
        Type::Named(_) | Type::Refined { .. } => {
            let n = name_of(t).unwrap_or("");
            // `$strlit` and `$interp…` lower to `String` (types.rs `rust_ty`); a record of one is copied with `.clone()`
            if is_string(t, prog) || matches!(n, "Bytes" | "BytesView" | "Failure") {
                return true;
            }
            if matches!(n, "List" | "ListView" | "Array" | "FixedList" | "Map" | "OrderedMap" | "FixedMap" | "Set") {
                return args_of(t).iter().all(|a| can_clone(prog, a, seen));
            }
            if seen.iter().any(|s| s == n) {
                return true;
            }
            match prog.type_def(n) {
                Some(d) => {
                    seen.push(n.to_string());
                    let r = match &d.kind {
                        TypeKind::Class { .. } | TypeKind::Enum { .. } => d.trivial || d.copyable,
                        TypeKind::Newtype { repr } => can_clone(prog, repr, seen),
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

/// A generic type's field can be cloned once the type's parameters can (`Clone` is in every parameter's fixed
/// bound): a generic class or enum then derives `Clone`, which Rust bounds on its parameters, so it can instantiate a
/// generic function (D-82). Loans, closures, `dynamic` values and the runtime's handles cannot.
fn clone_capable(prog: &Program, ty: &Type, seen: &mut Vec<String>) -> bool {
    if matches!(ty, Type::Loan { .. }) || matches!(strip(ty), Type::Fn { .. } | Type::Dynamic(_)) {
        return false;
    }
    match strip(ty) {
        Type::TypeParam(_) => true,
        Type::Optional(inner) => clone_capable(prog, inner, seen),
        Type::Tuple(ts) => ts.iter().all(|x| clone_capable(prog, x, seen)),
        Type::Record(fs) => fs.iter().all(|(_, x)| clone_capable(prog, x, seen)),
        t => {
            let n = name_of(t).unwrap_or("");
            if seen.iter().any(|x| x == n) {
                return true;
            }
            let args_ok = args_of(t).iter().all(|a| clone_capable(prog, a, seen));
            // the core types whose native forms are `Clone` (the runtime's handles, `Shared` among them, are not)
            const CLONE_CORE: &[&str] = &[
                "Int", "Int64", "Int32", "Int16", "Int8", "UInt", "UInt64", "UInt32", "UInt16", "UInt8", "Byte", "Bool", "Float", "Float32", "Decimal", "Duration", "Timestamp", "String", "StringView",
                "Scalar", "Bytes", "BytesView", "Void", "Never", "List", "ListView", "Array", "FixedList", "Map", "OrderedMap", "FixedMap", "Set", "VariantMap",
            ];
            if CLONE_CORE.contains(&n) {
                return args_ok;
            }
            match prog.type_def(n) {
                Some(d) if !is_user_module(&d.module) => (d.copyable || d.trivial) && args_ok,
                Some(d) if d.copyable || d.trivial => args_ok,
                Some(d) if !d.type_params.is_empty() => {
                    seen.push(n.to_string());
                    let ok = match &d.kind {
                        TypeKind::Class { fields } => fields.iter().all(|f| clone_capable(prog, &f.ty, seen)),
                        TypeKind::Enum { variants } => variants.iter().all(|v| v.fields.iter().all(|(_, ft)| clone_capable(prog, ft, seen))),
                        _ => false,
                    };
                    seen.pop();
                    ok && args_ok
                }
                Some(_) => false,
                None => false,
            }
        }
    }
}

fn has_eq(prog: &Program, ty: &Type, seen: &mut Vec<String>) -> bool {
    if matches!(ty, Type::Loan { .. }) || matches!(strip(ty), Type::Fn { .. } | Type::Dynamic(_)) {
        return false;
    }
    match strip(ty) {
        Type::Optional(inner) => has_eq(prog, inner, seen),
        t => {
            let n = name_of(t).unwrap_or("");
            if matches!(n, "Shared" | "Weak" | "Mutex" | "Failure" | "Builder" | "Capabilities" | "Console" | "Task") {
                return false;
            }
            if seen.iter().any(|x| x == n) {
                return true;
            }
            let args_ok = args_of(t).iter().all(|a| has_eq(prog, a, seen));
            match prog.type_def(n) {
                Some(d) => match &d.kind {
                    TypeKind::Class { fields } if !d.methods.iter().any(|(m, _)| m == "equals") => {
                        seen.push(n.to_string());
                        let ok = fields.iter().all(|f| has_eq(prog, &f.ty, seen));
                        seen.pop();
                        ok && args_ok
                    }
                    _ => args_ok,
                },
                None => args_ok,
            }
        }
    }
}

/// Whether a type's native form is `PartialOrd`, so a class or enum that derives `Comparable` can derive it too and
/// order by declaration order (X-516; D-54.1): a core `Comparable` type, or a program type that is `Comparable`.
fn has_ord(prog: &Program, ty: &Type, seen: &mut Vec<String>) -> bool {
    if matches!(ty, Type::Loan { .. }) || matches!(strip(ty), Type::Fn { .. } | Type::Dynamic(_)) {
        return false;
    }
    match strip(ty) {
        Type::Optional(_) => false,
        t => {
            let n = name_of(t).unwrap_or("");
            if seen.iter().any(|x| x == n) {
                return true;
            }
            let args_ok = args_of(t).iter().all(|a| has_ord(prog, a, seen));
            if fortis_types::decls::builtin_implements(n, "Comparable") {
                return args_ok;
            }
            match prog.type_def(n) {
                Some(d) => {
                    let cmp = d.derives.iter().any(|x| x == "Comparable") || d.implements.iter().any(|x| x == "Comparable") || d.methods.iter().any(|(m, _)| m == "compare");
                    if !cmp {
                        return false;
                    }
                    seen.push(n.to_string());
                    let ok = match &d.kind {
                        TypeKind::Class { fields } => fields.iter().all(|f| has_ord(prog, &f.ty, seen)),
                        TypeKind::Enum { variants } => variants.iter().all(|v| v.fields.iter().all(|(_, ft)| has_ord(prog, ft, seen))),
                        _ => false,
                    };
                    seen.pop();
                    ok && args_ok
                }
                None => false,
            }
        }
    }
}

/// Whether a type's native form is `Hash`, so a class or enum that derives `Hashable` can derive it too (X-516). A
/// type that writes its own `hash` hashes nothing, so it is `Hash` whatever its fields are.
fn has_hash(prog: &Program, ty: &Type, seen: &mut Vec<String>) -> bool {
    if matches!(ty, Type::Loan { .. }) || matches!(strip(ty), Type::Fn { .. } | Type::Dynamic(_)) {
        return false;
    }
    match strip(ty) {
        // Rust derive supplies conditional Eq/Hash bounds on the class or enum parameter (D-594).
        Type::TypeParam(_) => true,
        Type::Optional(inner) => has_hash(prog, inner, seen),
        t => {
            let n = name_of(t).unwrap_or("");
            if matches!(n, "Shared" | "Weak" | "Mutex" | "Failure" | "Builder" | "Capabilities" | "Console" | "Task") {
                return false;
            }
            if seen.iter().any(|x| x == n) {
                return true;
            }
            let args_ok = args_of(t).iter().all(|a| has_hash(prog, a, seen));
            if fortis_types::decls::builtin_implements(n, "Hashable") {
                return args_ok;
            }
            match prog.type_def(n) {
                Some(d) => {
                    let hand = d.methods.iter().any(|(m, _)| m == "hash" || m == "equals");
                    let hashable = hand || d.derives.iter().any(|x| x == "Hashable") || d.implements.iter().any(|x| x == "Hashable");
                    if !hashable {
                        return false;
                    }
                    if hand {
                        return args_ok;               // a hand-written hash writes nothing: its fields need not hash
                    }
                    seen.push(n.to_string());
                    let ok = match &d.kind {
                        TypeKind::Class { fields } => fields.iter().all(|f| has_hash(prog, &f.ty, seen)),
                        TypeKind::Enum { variants } => variants.iter().all(|v| v.fields.iter().all(|(_, ft)| has_hash(prog, ft, seen))),
                        _ => false,
                    };
                    seen.pop();
                    ok && args_ok
                }
                None => false,
            }
        }
    }
}
fn capability_field(ty: &str) -> &'static str {
    match ty {
        "FileSystem" => "fileSystem",
        "Network" => "network",
        "Clock" => "clock",
        "Random" => "random",
        "Environment" => "environment",
        "Process" => "process",
        "Supervisor" => "supervisor",
        "Console" => "console",
        "SecretAccess" => "secretAccess",
        _ => "rawSql",
    }
}

#[cfg(test)]
mod closure_tests;
