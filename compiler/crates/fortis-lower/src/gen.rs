//! Function bodies: statements and expressions of the typed IR as Rust. Every local has a Rust form (an owned
//! binding, a shared reference or an exclusive reference), and every operand is converted to the form its use
//! needs: `&`, `&mut`, a copy of a trivial value, a move, or an explicit `.clone()` for a source `copy` (the only
//! clones the lowering emits, each attributed in the audit).

use std::collections::{HashMap, HashSet};

use fortis_mir::model::*;
use fortis_types::types::{strip, Type};

use crate::intrinsics;
use crate::types::*;
use crate::{Audit, LowerError};

/// A `&{ ... }`, `&(|...|)` or `&(...)` borrows a temporary. Binding that borrow in a `let` drops the temporary at
/// the end of the statement (E0716). The owned value has to be bound, and the reference taken where it is used.
/// A place is `&name` (or a projection); that borrow outlives the call, so it is left as it is.
fn borrowed_temporary(code: &str) -> Option<&str> {
    let rest = code.strip_prefix('&')?.trim_start();
    match rest.chars().next() {
        Some('{' | '(' | '|') => Some(rest),
        _ => None,
    }
}

/// Whether a pattern can fail to match (a boxed class's contents then need a test, D-3).
fn refutable(p: &Pattern) -> bool {
    match p {
        Pattern::Wildcard | Pattern::Bind(_) | Pattern::Otherwise(_) => false,
        Pattern::Class { fields, .. } | Pattern::Record(fields) => fields.iter().any(|(_, sp)| refutable(sp)),
        Pattern::Tuple(ps) => ps.iter().any(refutable),
        _ => true,
    }
}

/// A pattern that binds nothing: every part is discarded. `..` with no name discards the rest.
fn pure_discard(p: &Pattern) -> bool {
    match p {
        Pattern::Wildcard | Pattern::Otherwise(None) => true,
        Pattern::Range { .. } => true, // a range pattern binds no name (9.8)
        Pattern::Present(i) | Pattern::Success(i) | Pattern::Failure(i) => pure_discard(i),
        Pattern::Tuple(ps) => ps.iter().all(pure_discard),
        Pattern::Variant { fields, .. } | Pattern::Class { fields, .. } | Pattern::Record(fields) => fields.iter().all(|(_, sp)| pure_discard(sp)),
        _ => false,
    }
}

/// Whether a by-value pattern takes ownership of some part of the scrutinee. `One { h: _ }` does not:
/// Rust leaves the scrutinee in place. A `Bind` does.
fn pattern_binds_owned(p: &Pattern) -> bool {
    match p {
        Pattern::Bind(_) | Pattern::Otherwise(Some(_)) => true,
        Pattern::Is { bind: Some(_), .. } => true,
        Pattern::Is { fields, bind: None, .. } => fields.iter().any(|(_, sp)| pattern_binds_owned(sp)),
        Pattern::Present(i) | Pattern::Success(i) | Pattern::Failure(i) => pattern_binds_owned(i),
        Pattern::Variant { fields, .. } | Pattern::Class { fields, .. } | Pattern::Record(fields) => fields.iter().any(|(_, sp)| pattern_binds_owned(sp)),
        Pattern::Tuple(ps) => ps.iter().any(pattern_binds_owned),
        _ => false,
    }
}

fn arms_bind_owned(arms: &[Arm]) -> bool {
    arms.iter().any(|a| pattern_binds_owned(&a.pattern))
}

/// A pattern that opens the scrutinee into fields or elements. A whole-value `_` does not.
/// `match move` of an opened pattern moves the value, so a field matched by `_` drops with the arm
/// (7.9, 9.8, D-315). A whole-value `_` leaves the local in place (D-300).
fn pattern_opens(p: &Pattern) -> bool {
    match p {
        Pattern::Present(i) | Pattern::Success(i) | Pattern::Failure(i) => pattern_opens(i),
        Pattern::Is { fields, .. } => !fields.is_empty(),
        Pattern::Variant { fields, .. } | Pattern::Class { fields, .. } | Pattern::Record(fields) => !fields.is_empty(),
        Pattern::Tuple(ps) => !ps.is_empty(),
        _ => false,
    }
}

/// The locals a pattern binds.
fn collect_binds(p: &Pattern, out: &mut Vec<LocalId>) {
    match p {
        Pattern::Bind(l) => out.push(*l),
        Pattern::Otherwise(Some(l)) => out.push(*l),
        Pattern::Is { fields, bind, .. } => {
            if let Some(l) = bind {
                out.push(*l);
            }
            fields.iter().for_each(|(_, sp)| collect_binds(sp, out));
        }
        Pattern::Present(i) | Pattern::Success(i) | Pattern::Failure(i) => collect_binds(i, out),
        Pattern::Variant { fields, .. } | Pattern::Class { fields, .. } | Pattern::Record(fields) => fields.iter().for_each(|(_, sp)| collect_binds(sp, out)),
        Pattern::Tuple(ps) => ps.iter().for_each(|sp| collect_binds(sp, out)),
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Owned,
    Ref,
    RefMut,
    /// a read parameter of a non-trivial optional type: `Option<&T>` (D-2)
    OptRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// an owned value (moved, copied, or a trivial value read)
    Value,
    Ref,
    RefMut,
    /// text as `&str`
    Str,
    /// any form: a receiver, an operand of display or comparison
    Any,
}

pub struct FnGen<'p> {
    pub prog: &'p Program,
    pub f: &'p Function,
    pub names: Vec<String>,
    pub forms: Vec<Form>,
    pub is_try: bool,
    /// an `async` function: its result is `rt::task::Async<…>`, whose `Err` is the cancellation exit
    pub is_async: bool,
    /// the scope variable each open block declares (aligned with `defers`): its exit finishes the scope
    scopes: Vec<Option<String>>,
    /// aligned with `defers`. A quiet frame traces its live cleanups and does not emit `loans-end`: a match
    /// arm's bindings are not a block (7.9, 9.8).
    quiet: Vec<bool>,
    /// a `scope` statement's local, for the block about to be lowered
    pending_scope: Option<String>,
    /// locals of a scope's block declared before its task set (their `let` becomes an assignment), and those
    /// declarations: a task may borrow them, so they must outlive the set in Rust's drop order (D-10)
    deferred_lets: Vec<LocalId>,
    /// the locals a scope block declares before its task set and initialises with a pattern (D-10)
    deferred_binds: Vec<LocalId>,
    /// a local bound to an unawaited call of an async function, and that function: a launch of the local names its
    /// task, and finds its observation schema, by it (D-27)
    pub future_fns: HashMap<LocalId, String>,
    deferred_decls: Vec<String>,
    /// task locals that are later detached: launched with a future that owns what it holds
    detachable: std::collections::HashSet<LocalId>,
    /// set while lowering the initialiser of a detachable task's `let`
    pub launch_detachable: bool,
    /// set while lowering the closure handed to `launchBlocking`: it becomes an `async move` block (its body may
    /// reach `checkpoint()`, a cancellation point)
    pub closure_as_future: bool,
    /// a closure whose result is a function type: Rust closures cannot return `impl Fn…`, so its result type is
    /// `Box<dyn Fn…>` and every value it returns is boxed (D-34: a lambda that returns a lambda)
    pub box_result: bool,
    pub test: bool,
    labels: usize,
    /// lets an awaited call borrows. The `await` places them around the poll: a block whose value was the
    /// future would drop them before the await (E0597).
    await_lets: Vec<String>,
    /// set while lowering an `await`, so a call's temporaries are recorded in `await_lets` instead of a block
    await_depth: u32,
    /// the label of the innermost value block (`give`) and catch handler (`recover`)
    give: Vec<String>,
    recover: Vec<String>,
    /// bindings a pattern needs as statements (payload fields of an `is` pattern), emitted at the arm's start
    arm_prelude: Vec<String>,
    /// conditions a pattern needs as the arm's guard (the refutable contents of a boxed class, D-3)
    arm_guards: Vec<String>,
    /// the bindings boxed class patterns make in the arm's prelude, and the outermost box (its name, the pattern of
    /// its contents, the guards of the boxes inside it, whether its own test was pushed): an arm's guard that reads
    /// those bindings runs inside that box's test, where they are references (D-3)
    boxed_locals: Vec<LocalId>,
    last_box: Option<(String, String, Vec<String>, bool)>,
    /// the borrow-only part of the arm prelude: a guard reads a boxed enum binding through the same dereference
    guard_prelude: Vec<String>,
    /// a nested pattern over a boxed field selects through a guard fragment: the dereferenced box and the
    /// pattern to match it with, outermost first (Rust patterns do not look inside a box). The guard fold
    /// applies that list in reverse, so the first fragment is the innermost `if let`: an inner binding is
    /// tested only after the outer fragment has bound it.
    pat_guards: Vec<(String, String)>,
    /// the pattern being generated matches by reference (the arm's bindings are `Form::Ref`)
    pat_ref: bool,
    /// the pattern currently being generated is the scrutinee of a guard fragment (`box.as_ref()`), so its
    /// bindings are references even when the arm itself consumes the enum
    in_guard: bool,
    /// locals the guard must read as `&T` (a boxed payload, or a binding introduced inside a guard fragment)
    guard_as_ref: Vec<LocalId>,
    /// the next closure literal takes these parameters by reference (a generic function type `fn(T) U`)
    closure_refs: Option<Vec<bool>>,
    /// for each closure parameter, whether the callee declares it with a type parameter (`fn(T) U`): such a
    /// parameter is exactly `&T` (a `String` element is `&String`, not `&str`)
    closure_generic: Option<Vec<bool>>,
    /// the value type of each open value block
    give_ty: Vec<Type>,
    /// the value type of each open `catch` (what its `recover` gives)
    recover_ty: Vec<Type>,
    recover_held: Vec<bool>,
    /// the Abandonable locals of each open block (abandoned when a failure leaves the function, 12.4)
    abandonable: Vec<Vec<LocalId>>,
    /// for each open block (aligned with `abandonable`): owned locals that own a cleanup, directly or through a
    /// field, payload or container. They are dropped with the block. The trace names only a class that itself
    /// declares `cleanup` (7.9, D-181).
    cleanups: Vec<Vec<LocalId>>,
    /// for each open block (aligned with `cleanups`): drops of fields a consuming pattern matched with `_`.
    /// They run after that frame's named bindings, in declaration order, and they emit no `drop` trace (7.9, 9.8).
    discards: Vec<Vec<String>>,
    /// `(field path, drop)` for the pattern currently being lowered. The path is the declaration index of
    /// each enclosing field, so a nested `_` sorts before a later sibling (7.9, 9.8).
    arm_discards: Vec<(Vec<u32>, String)>,
    /// declaration-index path of the pattern currently being lowered
    discard_ord: Vec<u32>,
    /// for each open block (aligned with `abandonable`): its deferred blocks' registration flags, with each body
    /// once the `defer` statement has been lowered (a defer textually after an exit cannot have run there)
    defers: Vec<Vec<(String, Option<String>)>>,
    /// the open loops: the Fortis label, the number of open blocks when the loop began, and the Rust label every
    /// loop gets (a `break` or `continue` inside a labeled value block or catch handler must name its loop: E0695)
    loops: Vec<(Option<String>, usize, String)>,
    /// the number of open blocks when each value block and each catch handler began
    give_depth: Vec<usize>,
    recover_depth: Vec<usize>,
    moved: HashSet<LocalId>,
    /// a local with a field moved or dropped. The parent is not dropped whole afterwards: that is a use of a
    /// partially moved value. The field drop already traced the destroyed field, and Rust drops a field that remains.
    partial: HashSet<LocalId>,
    /// a cleanup or function local that is moved on one path and left live on another. It is an `Option`, so the
    /// live path still drops it before an outer `defer` and a move on the other path leaves `None` (7.9, D-181).
    /// The `moved` set is flow-insensitive and would suppress that drop on every path. `Option` rather than
    /// `ManuallyDrop`: `take` on `ManuallyDrop` is unsafe, and the generated crate forbids unsafe code.
    manual: HashSet<LocalId>,
    /// initialisers of statement temporaries emitted while lowering the current statement, creation order (5.7).
    /// `hoist_owners` is the temporary each line declares. A nested statement leaves a temporary it does not own
    /// on this vec, so the statement that drops it declares it in that scope (7.9).
    hoist: Vec<String>,
    hoist_owners: Vec<LocalId>,
    /// statement temporaries already bound by a hoist: a second lowering of the same temporary (an intrinsic that
    /// renders an argument twice) names the local and does not evaluate it again (5.7)
    emitted_temps: Vec<LocalId>,
    /// the Rust of each statement temporary's value. The slot is `Option`, initialised where the operand is
    /// evaluated, so a skipped `&&`/`||` operand is never created and a second lowering initialises once (5.7)
    temp_inits: HashMap<LocalId, String>,
    /// statement temporaries that hold the `MaybeOwned` of an `or recover` whose try side succeeds with a loan
    /// (D-446): the slot owns the replacement its fallback made, and announces only that one when it drops
    mo_temps: HashSet<LocalId>,
    /// a `let` bound to a lent `??` that can give a value of its own (its fallback moved or made): the name of the
    /// `MaybeOwned` that holds that value beside the binding, so a move of the binding into a tuple moves what it
    /// owns with it (7.9; D-367)
    held: HashMap<LocalId, String>,
    /// statement temporaries whose non-copyable field move has already hoisted a hold slot per field (7.9).
    /// A tuple-element move uses the same set for its element slots (D-301).
    split_hoisted: HashSet<LocalId>,
    /// destruction of each statement being lowered, innermost last; a control transfer runs it before the jump
    transfer_drops: Vec<String>,
    in_transfer: bool,
    pub audit: &'p mut Audit,
    pub fn_names: &'p HashMap<String, String>,
    pub errors: Vec<LowerError>,
}

/// A loan's inner type; any other type itself.
fn loan_inner(t: &Type) -> &Type {
    match t {
        Type::Loan { inner, .. } => inner.as_ref(),
        t => t,
    }
}

pub fn indent(s: &str, n: usize) -> String {
    let pad = "    ".repeat(n);
    s.lines().map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") }).collect::<Vec<_>>().join("\n")
}

impl<'p> FnGen<'p> {
    pub fn new(prog: &'p Program, f: &'p Function, audit: &'p mut Audit, fn_names: &'p HashMap<String, String>) -> FnGen<'p> {
        let mut names = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        // `self` is the receiver only of a method generated in an impl block; elsewhere it is an ordinary parameter
        let receiver = fn_names.get(&f.name).map_or(false, |r| r.contains("::"));
        for (i, l) in f.locals.iter().enumerate() {
            let base = if l.kind == LocalKind::SelfParam && receiver { "self".to_string() } else { ident(&l.name) };
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            names.push(if *n == 1 || (l.kind == LocalKind::SelfParam && receiver) { base } else { format!("{base}_{i}") });
        }
        let mut forms = vec![Form::Owned; f.locals.len()];
        for p in &f.params {
            let t = &f.locals[p.local as usize].ty;
            forms[p.local as usize] = match p.mode {
                Mode::Owned => Form::Owned,
                Mode::Editable => Form::RefMut,
                Mode::Read => {
                    if opt_read_inner(t, prog).is_some() {
                        Form::OptRef
                    } else if trivial(t, prog) || matches!(strip(t), Type::Fn { kind, .. } if kind != "fn") {
                        // `fn mut` and `fn once` are still passed by value. A plain `fn` is `&impl Fn` (see `fn_read_borrowed`).
                        Form::Owned
                    } else {
                        Form::Ref
                    }
                }
            };
            let loan_of_value = matches!(t, Type::Loan { inner, .. } if trivial(inner, prog));
            if matches!(t, Type::Loan { mode, .. } if mode == "editable") && !loan_of_value {
                forms[p.local as usize] = Form::RefMut;
            } else if matches!(t, Type::Loan { .. }) && !loan_of_value {
                forms[p.local as usize] = Form::Ref;
            }
        }
        let manual = manual_locals(prog, f);
        FnGen { prog, f, names, forms, is_try: f.is_try(), is_async: f.has("async"), scopes: vec![None], quiet: vec![false], pending_scope: None, deferred_lets: vec![], deferred_binds: vec![], future_fns: HashMap::new(), deferred_decls: vec![], detachable: detached_tasks(f), launch_detachable: false, closure_as_future: false, box_result: false, test: f.label.starts_with("test "), labels: 0, await_lets: vec![], await_depth: 0, give: vec![], recover: vec![], arm_prelude: vec![], arm_guards: vec![], boxed_locals: vec![], last_box: None, guard_prelude: vec![], pat_guards: vec![], pat_ref: false, in_guard: false, guard_as_ref: vec![], closure_refs: None, closure_generic: None, give_ty: vec![], recover_ty: vec![], recover_held: vec![], abandonable: vec![vec![]], cleanups: vec![owned_cleanup_params(prog, f)], discards: vec![vec![]], arm_discards: vec![], discard_ord: vec![], defers: vec![vec![]], loops: vec![], give_depth: vec![], recover_depth: vec![], moved: Default::default(), partial: HashSet::new(), manual, hoist: vec![], hoist_owners: vec![], emitted_temps: vec![], temp_inits: HashMap::new(), mo_temps: HashSet::new(), held: HashMap::new(), split_hoisted: HashSet::new(), transfer_drops: vec![], in_transfer: false, audit, fn_names, errors: vec![] }
    }

    pub fn err_pub(&mut self, line: u32, m: impl Into<String>) -> String {
        self.err(line, m)
    }

    fn err(&mut self, line: u32, m: impl Into<String>) -> String {
        self.errors.push(LowerError { function: self.f.name.clone(), line, message: m.into() });
        "todo!()".into()
    }

    fn label(&mut self) -> String {
        self.labels += 1;
        format!("'v{}", self.labels)
    }

    /// A loop's Rust label: its Fortis label, or a fresh one.
    fn loop_label(&mut self, label: &Option<String>) -> String {
        match label {
            Some(l) => format!("'{}", ident(l)),
            None => {
                self.labels += 1;
                format!("'l{}", self.labels)
            }
        }
    }

    fn ty(&self, t: &Type) -> String {
        rust_ty(t, self.prog)
    }

    fn local_ty(&self, l: LocalId) -> &Type {
        &self.f.locals[l as usize].ty
    }

    // ---- places -----------------------------------------------------------------------------------------------

    /// The type of a member of a value of type `t`.
    fn member_ty(&self, t: &Type, f: &str) -> Type {
        let s = strip(t);
        if let Type::Tuple(ts) = s {
            return f.parse::<usize>().ok().and_then(|i| ts.get(i).cloned()).unwrap_or_else(|| fortis_types::types::unknown(""));
        }
        if let Type::Record(fs) = s {
            return fs.iter().find(|(n, _)| n == f).map(|(_, ty)| ty.clone()).unwrap_or_else(|| fortis_types::types::unknown(""));
        }
        let n = name_of(s).unwrap_or("");
        if n == "Shared" {
            return args_of(s).first().map(|inner| self.member_ty(inner, f)).unwrap_or_else(|| fortis_types::types::unknown(""));
        }
        match self.prog.type_def(n).map(|d| &d.kind) {
            Some(TypeKind::Class { fields }) => fields.iter().find(|x| x.name == f).map(|x| x.ty.clone()).unwrap_or_else(|| fortis_types::types::unknown("")),
            _ => fortis_types::types::unknown(""),
        }
    }

    /// The Rust path of a place (auto-deref reaches through references; a Shared value is read through).
    pub fn place(&self, p: &Place) -> String {
        self.place_rw(p, false)
    }

    fn place_rw(&self, p: &Place, write: bool) -> String {
        let mut s = if self.manual.contains(&p.local) {
            let n = &self.names[p.local as usize];
            if write { format!("{n}.as_mut().unwrap()") } else { format!("{n}.as_ref().unwrap()") }
        } else {
            self.names[p.local as usize].clone()
        };
        let mut t = self.f.locals[p.local as usize].ty.clone();
        for pr in &p.proj {
            match pr {
                Proj::Field(f) => {
                    if name_of(&t) == Some("Shared") {
                        s = format!("{s}.{}()", if write { "write" } else { "read" });
                    }
                    // a field of an optional: only reachable where the checker narrowed it to present
                    if let Type::Optional(inner) = strip(&t).clone() {
                        s = format!("{s}.{}().unwrap()", if write { "as_mut" } else { "as_ref" });
                        t = *inner;
                    }
                    s.push('.');
                    s.push_str(&ident(f));
                    t = self.member_ty(&t, f);
                }
                Proj::Tuple(i) => {
                    s.push_str(&format!(".{i}"));
                    t = self.member_ty(&t, &i.to_string());
                }
                Proj::Index(i) => s.push_str(&format!("[{i}]")),
            }
        }
        s
    }

    /// An assignable place: a whole exclusive reference is written through.
    fn lvalue(&self, p: &Place) -> String {
        if p.proj.is_empty() && self.forms[p.local as usize] == Form::RefMut {
            return format!("*{}", self.names[p.local as usize]);
        }
        self.place_rw(p, true)
    }

    /// The declared type of a place (its local's type through its projections). A field of an optional is
    /// reached only where that optional was narrowed, so the projection sees the inner type, as `place` does.
    fn place_ty(&self, p: &Place) -> Type {
        let mut t = self.f.locals[p.local as usize].ty.clone();
        for pr in &p.proj {
            if let Proj::Field(_) = pr {
                if let Type::Optional(inner) = strip(&t).clone() {
                    t = *inner;
                }
            }
            t = match pr {
                Proj::Field(f) => self.member_ty(&t, f),
                Proj::Tuple(i) | Proj::Index(i) => self.member_ty(&t, &i.to_string()),
            };
        }
        t
    }

    /// A lent `??` of a non-trivial value as the value the statement holds (D-246): a `Cow<str>` for text, a
    /// `MaybeOwned` otherwise, holding the present value as the left side gives it (a loan, or the copy `m.get(k)`
    /// makes) or the fallback (a place lent, a new value owned). The fallback runs only in the `None` arm (5.7). A
    /// fallback that is itself a lent `??` is that `??`'s own `Cow` or `MaybeOwned`, held in the arm as it is: a
    /// borrow of it there would end with the arm (rustc E0716; D-339).
    /// A lent `??` whose sides are both borrowed, a loan on the left (`xs.get(0)`, an `Option<&T>`) and a place or
    /// another such `??` on the right, is a reference into what they borrow, with no value of the statement's own: it
    /// can outlive the statement, as the value an `if` expression gives (`give xs.get(0) ?? alias d`; D-345). None
    /// when either side may make a value.
    fn lent_coalesce_ref(&mut self, a: &Expr, b: &Expr, ty: &Type) -> Option<String> {
        if !matches!(strip(&a.ty), Type::Optional(i) if i.is_loan()) {
            return None;
        }
        let text = matches!(ty, Type::Loan { inner, .. } if is_string(inner, self.prog));
        let fb = match &b.kind {
            _ if name_of(&b.ty) == Some("Never") => self.expr(b, Want::Value),
            ExprKind::Const(Const::Str(s)) if text => format!("{s:?}"),
            ExprKind::Place(_, Use::Read | Use::Alias) if text => format!("AsRef::<str>::as_ref({})", self.expr(b, Want::Str)),
            ExprKind::Place(_, Use::Read | Use::Alias) => self.expr(b, Want::Ref),
            ExprKind::Coalesce(ba, bb) if lent_coalesce_ty(&b.ty, self.prog) => {
                // a nested `??` qualifies only when it is itself wholly borrowed; nothing is lowered before that is known
                if !matches!(strip(&ba.ty), Type::Optional(i) if i.is_loan()) || !wholly_borrowed(bb, self.prog) {
                    return None;
                }
                self.lent_coalesce_ref(ba, bb, &b.ty)?
            }
            _ => return None,
        };
        let l = self.expr(a, Want::Value);
        Some(if text {
            format!("(match {l} {{ Some(__v) => AsRef::<str>::as_ref(__v), None => {fb} }})")
        } else {
            format!("(match {l} {{ Some(__v) => __v, None => {fb} }})")
        })
    }

    /// Whether a lent `??` (of a value that is not text) can give a value of its own: its fallback is moved or made,
    /// so its result is a `MaybeOwned` that owns the fallback when it is taken. Not when both sides are borrowed (a
    /// reference into them) or the left side is an optional read parameter (lowered on its own).
    fn holds_fallback(&self, a: &Expr, b: &Expr, ty: &Type) -> bool {
        lent_coalesce_ty(ty, self.prog)
            && !matches!(ty, Type::Loan { inner, .. } if is_string(inner, self.prog))
            && !matches!(&a.kind, ExprKind::Place(p, _) if p.proj.is_empty() && self.forms[p.local as usize] == Form::OptRef)
            && !(matches!(strip(&a.ty), Type::Optional(i) if i.is_loan()) && wholly_borrowed(b, self.prog))
    }

    /// A part of a tuple literal. A lent `??` that can give a value of its own is held in the tuple as its
    /// `MaybeOwned`, and a `let` bound to one moves its holder in, so the tuple owns that value and destroys it with
    /// its other parts, in element order, as the interpreter and the VM do; a loan stays a reference (7.9; D-367).
    fn tuple_part(&mut self, x: &Expr) -> String {
        match &x.kind {
            ExprKind::Coalesce(a, b) if self.holds_fallback(a, b, &x.ty) => self.lent_coalesce(a, b, &x.ty),
            ExprKind::Place(p, Use::Move) if p.proj.is_empty() && self.held.contains_key(&p.local) => {
                // the holder goes with the binding: its block no longer destroys it (D-380)
                self.moved.insert(p.local);
                self.held[&p.local].clone()
            }
            // `present(…)` of either: the optional holds it
            ExprKind::Present(i) if matches!(&i.kind, ExprKind::Coalesce(a, b) if self.holds_fallback(a, b, &i.ty))
                || matches!(&i.kind, ExprKind::Place(p, Use::Move) if p.proj.is_empty() && self.held.contains_key(&p.local)) =>
            {
                format!("Some({})", self.tuple_part(i))
            }
            _ => self.expr(x, Want::Value),
        }
    }

    fn lent_coalesce(&mut self, a: &Expr, b: &Expr, ty: &Type) -> String {
        let l = self.expr(a, Want::Value);
        let place = matches!(b.kind, ExprKind::Place(_, Use::Read | Use::Alias));
        let nested = match &b.kind {
            ExprKind::Coalesce(ba, bb) if lent_coalesce_ty(&b.ty, self.prog) => Some(self.lent_coalesce(ba, bb, &b.ty)),
            _ => None,
        };
        if matches!(ty, Type::Loan { inner, .. } if is_string(inner, self.prog)) {
            let fb = match (&b.kind, nested) {
                (_, Some(n)) => n,
                (ExprKind::Const(Const::Str(s)), None) => format!("std::borrow::Cow::Borrowed({s:?})"),
                _ if place => format!("std::borrow::Cow::Borrowed(AsRef::<str>::as_ref({}))", self.expr(b, Want::Str)),
                _ => format!("std::borrow::Cow::<str>::from({})", self.expr(b, Want::Value)),
            };
            return format!("(match {l} {{ Some(__v) => std::borrow::Cow::<str>::from(__v), None => {fb} }})");
        }
        let fb = match nested {
            Some(n) => n,
            None if place => format!("rt::MaybeOwned::Borrowed({})", self.expr(b, Want::Ref)),
            None => format!("rt::MaybeOwned::Owned({})", self.expr(b, Want::Value)),
        };
        format!("(match {l} {{ Some(__v) => rt::IntoMaybeOwned::into_maybe_owned(__v), None => {fb} }})")
    }

    /// Keep ownership at each recovery join, including nested replacements. No branch borrows
    /// a holder created inside that branch: the holder itself reaches the enclosing owner.
    fn holder_value(&mut self, e: &Expr, inner: &Type) -> String {
        if recovery_holder_inner(e, self.prog).is_some() {
            return self.recovery_holder(e);
        }
        if let ExprKind::Coalesce(a, b) = &e.kind {
            if lent_coalesce_ty(&e.ty, self.prog) { return self.lent_coalesce(a, b, &e.ty); }
        }
        if let ExprKind::Place(p, Use::Move) = &e.kind {
            if p.proj.is_empty() && self.held.contains_key(&p.local) { return self.tuple_part(e); }
        }
        let borrowed = matches!(e.kind, ExprKind::Place(_, Use::Read | Use::Alias)) || e.ty.is_loan();
        let value = self.expr(e, if borrowed { Want::Ref } else { Want::Value });
        let holder = if is_string(inner, self.prog) { "std::borrow::Cow" } else { "rt::MaybeOwned" };
        let variant = if borrowed { "Borrowed" } else { "Owned" };
        format!("{holder}::{variant}({value})")
    }

    fn recovery_holder(&mut self, e: &Expr) -> String {
        let inner = recovery_holder_inner(e, self.prog).unwrap();
        let holder = if is_string(&inner, self.prog) { "std::borrow::Cow" } else { "rt::MaybeOwned" };
        match &e.kind {
            ExprKind::OrRecover(a, b) => {
                let value = self.expr(a, Want::Value);
                let fallback = self.holder_value(b, &inner);
                format!("(match {value} {{ Ok(__v) => {holder}::Borrowed(__v), Err(_) => {fallback} }})")
            }
            ExprKind::Catch { value, bind, handler } => {
                let value = self.expr(value, Want::Value);
                let label = self.label();
                self.recover.push(label.clone());
                self.recover_ty.push(e.ty.clone());
                self.recover_held.push(true);
                self.recover_depth.push(self.defers.len());
                let body = self.block_stmts(handler);
                self.recover_depth.pop();
                self.recover_held.pop();
                self.recover_ty.pop();
                self.recover.pop();
                format!("(match {value} {{ Ok(__v) => {holder}::Borrowed(__v), Err({}) => {label}: {{\n{}\n}} }})", self.binding(*bind), indent(&body, 1))
            }
            _ => unreachable!(),
        }
    }

    fn place_expr(&mut self, p: &Place, u: Use, ty: &Type, want: Want, line: u32) -> String {
        // a read of an optional the checker narrowed to present (`if v != null`): its value
        let declared = self.place_ty(p);
        if p.proj.is_empty() && self.forms[p.local as usize] == Form::OptRef {
            return self.opt_ref_read(p, u, ty, want, line);
        }
        // `copy v` of a local holding an optional loan (`let v = xs.get(0)`, an `Option<&T>`) copies the value it
        // lends, narrowed to present or not, as for an optional read parameter (7.5; D-341)
        if p.proj.is_empty() && u == Use::Copy && want == Want::Value && self.forms[p.local as usize] == Form::Owned {
            if let Type::Optional(i) = strip(&declared) {
                if matches!(&**i, Type::Loan { inner, .. } if !trivial(inner, self.prog)) {
                    return self.opt_ref_read(p, u, ty, want, line);
                }
            }
        }
        // a branch-moved cleanup is `Option<T>`: a move takes the inner value, and a read borrows it.
        // `Want::Any` must not move it except on `Use::Move` (a launch records no move of its own).
        if p.proj.is_empty() && self.manual.contains(&p.local) {
            return self.manual_use(p.local, u, ty, want, line);
        }
        // A field or tuple element of a statement temporary is an `Option` slot filled by the discard that
        // precedes this use (7.9). A read or a copy borrows the parent, so cleanup still sees that part. A move
        // stashes every part and the statement drops what remains, after the consumer. Moving `.0` out of the
        // slot's shared borrow is rejected by rustc (E0507); the stash takes the owned tuple once.
        if self.temp_inits.contains_key(&p.local) {
            if p.proj.len() == 1 {
                if let Proj::Field(f) = &p.proj[0] {
                    return self.temp_field_use(p.local, f, u, ty, want, line);
                }
            }
            if !p.proj.is_empty() && p.proj.iter().all(|pr| matches!(pr, Proj::Tuple(_))) {
                return self.temp_tuple_use(p, u, ty, want, line);
            }
        }
        if matches!(strip(&declared), Type::Optional(_)) && fortis_types::types::known(ty) && !matches!(strip(ty), Type::Optional(_)) && name_of(ty) != Some("Never") {
            let code = self.place(p);
            let form = self.forms[p.local as usize];
            let base = if p.proj.is_empty() && form != Form::Owned { format!("(*{code})") } else { code };
            return match want {
                Want::Ref | Want::Str | Want::RefMut => format!("{base}.as_ref().unwrap()"),
                _ if trivial(ty, self.prog) => format!("{base}.unwrap()"),
                _ => format!("{base}.as_ref().unwrap()"),
            };
        }
        let form = self.forms[p.local as usize];
        let whole = p.proj.is_empty();
        let code = self.place(p);
        let triv = trivial(ty, self.prog);
        // a tuple's element whose type is a loan is a reference, or the `MaybeOwned` of a lent `??` the tuple holds
        // (D-367): read through `&*`, it is the reference either way, and an optional one through `as_deref()`
        let element_loan = |t: &Type| matches!(t, Type::Loan { inner, .. } if !is_string(inner, self.prog) && !trivial(inner, self.prog)) && !loan_is_value(t, self.prog);
        let element = !whole && matches!(want, Want::Value | Want::Any) && matches!(p.proj.last(), Some(Proj::Tuple(_)));
        let code = if element && matches!(u, Use::Read | Use::Copy) && element_loan(strip(&declared)) {
            format!("(&*{code})")
        } else if element && u == Use::Read && matches!(strip(&declared), Type::Optional(i) if element_loan(i)) {
            format!("{code}.as_deref()")
        } else {
            code
        };
        match want {
            Want::Value => match u {
                Use::Copy => {
                    if triv {
                        return if whole && form != Form::Owned { format!("(*{code})") } else { code };
                    }
                    self.audit.clone_at(&self.f.name, line, "copy");
                    if is_string(ty, self.prog) {
                        format!("{code}.to_string() /* copy */")
                    } else {
                        format!("{code}.clone() /* copy */")
                    }
                }
                Use::Alias => {
                    if whole && form != Form::Owned {
                        code
                    } else if matches!(want, Want::Value) && (trivial(ty, self.prog) || loan_is_value(ty, self.prog)) {
                        // a loan of a value-like place (a capability handle, a trivial value) is the value itself
                        code
                    } else {
                        format!("&{code}")
                    }
                }
                Use::Editable => {
                    if whole && form == Form::RefMut {
                        code
                    } else {
                        format!("&mut {code}")
                    }
                }
                Use::Move | Use::Read => {
                    // a by-value read of an owned function (`owned fn`, `fn once`, `fn mut`) moves it in Rust.
                    // A read-mode plain `fn` is borrowed instead (`fn_read_borrowed`), so that pass is not recorded
                    // and the caller's block still destroys the closure (7.9, D-181). A manual local is taken by
                    // `manual_use` instead.
                    let fn_by_value = u == Use::Read && whole && form == Form::Owned && matches!(strip(ty), Type::Fn { .. });
                    if (u == Use::Move || fn_by_value) && whole && !self.manual.contains(&p.local) {
                        self.moved.insert(p.local);
                    }
                    // a field move (a move-spread's untaken field, or a field the target keeps) leaves the parent
                    // partially moved. The block exit must not drop the parent after that (E0382).
                    if u == Use::Move && !whole {
                        self.partial.insert(p.local);
                    }
                    if triv && whole && form != Form::Owned {
                        format!("(*{code})")
                    } else if u == Use::Read && (!whole || form != Form::Owned) && matches!(strip(ty), Type::TypeParam(_)) && !self.f.has("noalloc") {
                        // a generic value read through a loan where a value is wanted (`return b` for `b: T`, which
                        // the checker admits since `T` may be any type): its clone, the value a read loan shows (D-82)
                        self.audit.clone_at(&self.f.name, line, "generic value");
                        format!("{code}.clone()")
                    } else {
                        code
                    }
                }
            },
            Want::Ref | Want::Str => {
                if whole && form != Form::Owned {
                    if form == Form::RefMut {
                        format!("&*{code}")
                    } else {
                        code
                    }
                } else {
                    format!("&{code}")
                }
            }
            Want::RefMut => {
                if whole && form == Form::RefMut {
                    format!("&mut *{code}")
                } else {
                    format!("&mut {code}")
                }
            }
            Want::Any => {
                if triv && whole && form != Form::Owned {
                    format!("(*{code})")
                } else {
                    code
                }
            }
        }
    }

    /// The async function whose future a local now holds: a direct call of it, or the future of another local moved
    /// or read into it (D-27).
    fn note_future(&mut self, local: LocalId, init: &Expr) {
        let maker = match &init.kind {
            ExprKind::Call { callee: Callee::Func(g), .. } if self.prog.function(g).map_or(false, |fd| fd.has("async")) => Some(g.clone()),
            ExprKind::Place(p, _) if p.proj.is_empty() => self.future_fns.get(&p.local).cloned(),
            _ => None,
        };
        match maker {
            Some(g) => {
                self.future_fns.insert(local, g);
            }
            None => {
                self.future_fns.remove(&local);
            }
        }
    }

    /// Read or move a local stored as `Option<T>` (see `manual`).
    fn manual_use(&mut self, local: LocalId, u: Use, ty: &Type, want: Want, line: u32) -> String {
        let name = self.names[local as usize].clone();
        let take = format!("{name}.take().unwrap()");
        match (want, u) {
            (_, Use::Move) => take,
            (Want::Value, Use::Copy) => {
                self.audit.clone_at(&self.f.name, line, "copy");
                if is_string(ty, self.prog) {
                    format!("{name}.as_ref().unwrap().to_string() /* copy */")
                } else {
                    format!("{name}.as_ref().unwrap().clone() /* copy */")
                }
            }
            (Want::Value, Use::Read) if !trivial(ty, self.prog) => take,
            (Want::Value, Use::Read) => format!("*{name}.as_ref().unwrap()"),
            (Want::Value, Use::Editable) | (Want::RefMut, _) => format!("{name}.as_mut().unwrap()"),
            (Want::Value, Use::Alias) | (Want::Ref, _) | (Want::Str, _) | (Want::Any, _) => format!("{name}.as_ref().unwrap()"),
        }
    }

    /// A read of an optional read parameter, which is `Option<&T>` (D-2). Narrowed to present (`if v != null`), it is
    /// the `&T`; otherwise the `Option<&T>` itself, whatever form is wanted (it is `Copy`, and matching, testing for
    /// null and lending it on take it as it is); a `copy` builds an owned `T` or `T?`.
    fn opt_ref_read(&mut self, p: &Place, u: Use, ty: &Type, want: Want, line: u32) -> String {
        let code = self.place(p);
        let narrowed = fortis_types::types::known(ty) && !matches!(strip(ty), Type::Optional(_)) && name_of(ty) != Some("Never");
        let copy = u == Use::Copy && want == Want::Value;
        if copy {
            self.audit.clone_at(&self.f.name, line, "copy");
        }
        match (narrowed, copy) {
            (true, true) if is_string(ty, self.prog) => format!("{code}.unwrap().to_string() /* copy */"),
            (true, true) => format!("{code}.unwrap().clone() /* copy */"),
            (true, false) => format!("{code}.unwrap()"),
            (false, true) if matches!(strip(ty), Type::Optional(i) if is_string(i, self.prog)) => format!("{code}.map(|v| v.to_string()) /* copy */"),
            (false, true) => format!("{code}.cloned() /* copy */"),
            (false, false) => code,
        }
    }

    /// An argument for an optional read parameter, which is `Option<&T>` (D-2): a present value is lent (`Some(&x)`),
    /// `null` is `None`, such a parameter passes itself on, and any other optional is lent with `as_ref`.
    fn opt_read_arg(&mut self, e: &Expr, inner: &Type) -> String {
        let text = is_string(inner, self.prog);
        match &e.kind {
            ExprKind::Present(x) => {
                if text {
                    let v = self.expr(x, Want::Str);
                    format!("Some(AsRef::<str>::as_ref({v}))")
                } else {
                    format!("Some({})", self.expr(x, Want::Ref))
                }
            }
            ExprKind::Const(Const::Null) => "None".into(),
            ExprKind::Place(p, _) if p.proj.is_empty() && self.forms[p.local as usize] == Form::OptRef => self.place(p),
            // an optional loan of a value that is not trivial (`xs.get(0)`, `m.get(k)`, a binding of one) is already
            // `Option<&T>`: it passes on as it is (D-338)
            _ if !text && matches!(&e.ty, Type::Optional(i) if matches!(&**i, Type::Loan { inner: li, .. } if !trivial(li, self.prog) && !is_string(li, self.prog))) => {
                self.expr(e, Want::Any)
            }
            // an optional loan of text lent to a parameter whose present type is a type parameter (`Optional.map`,
            // X-479) is `Option<&String>` already
            _ if !text && crate::types::opt_loan_text(&e.ty, self.prog) => self.expr(e, Want::Any),
            // a present value the call did not wrap (a closure call's arguments are not coerced)
            _ if fortis_types::types::known(&e.ty) && !matches!(strip(&e.ty), Type::Optional(_)) => {
                if text {
                    let v = self.expr(e, Want::Str);
                    format!("Some(AsRef::<str>::as_ref({v}))")
                } else {
                    format!("Some({})", self.expr(e, Want::Ref))
                }
            }
            _ => {
                let v = self.expr(e, Want::Any);
                if text {
                    format!("({v}).as_deref()")
                } else {
                    format!("({v}).as_ref()")
                }
            }
        }
    }

    // ---- expressions ------------------------------------------------------------------------------------------

    /// An expression where a value of type `ty` is expected: a variant-keyed record literal is a `VariantMap`.
    pub fn expr_as(&mut self, e: &Expr, ty: &Type) -> String {
        if let Type::Optional(inner) = strip(ty) {
            if !matches!(strip(&e.ty), Type::Optional(_)) && !matches!(e.kind, ExprKind::Const(Const::Null)) && name_of(&e.ty) != Some("Never") {
                return format!("Some({})", self.expr_as(e, inner));
            }
        }
        if let (ExprKind::Record(fs), Type::Record(shape)) = (&e.kind, strip(ty)) {
            self.audit.records.push(shape.clone());
            let parts: Vec<_> = fs.iter().map(|(n, v)| {
                let ty = shape.iter().find(|(f, _)| f == n).map(|(_, t)| t).unwrap_or(&v.ty);
                format!("{}: {}", ident(n), self.expr_as(v, ty))
            }).collect();
            return format!("({} {{ {} }})", record_name(shape), parts.join(", "));
        }
        if let (ExprKind::Record(fs), Some("VariantMap")) = (&e.kind, name_of(ty)) {
            let k = args_of(ty).first().map(|k| self.ty(k)).unwrap_or_default();
            let items: Vec<String> = fs.iter().map(|(n, v)| format!("({k}::{}, {})", ident(n), self.expr(v, Want::Value))).collect();
            return format!("vec![{}]", items.join(", "));
        }
        self.expr(e, Want::Value)
    }

    /// `present(v)` / `success(v)`. A statement temporary is forwarded by reference: the statement still
    /// owns it and drops it (7.9). An owned operand is wrapped by value.
    fn wrap_owned(&mut self, ctor: &str, a: &Expr) -> String {
        if matches!(a.kind, ExprKind::Temp { .. }) {
            format!("{ctor}({})", self.expr(a, Want::Ref))
        } else {
            format!("{ctor}({})", self.expr(a, Want::Value))
        }
    }

    pub fn expr(&mut self, e: &Expr, want: Want) -> String {
        let code = self.expr_raw(e, want);
        // a temporary where a reference is wanted is borrowed. A statement temporary and a field of one
        // already have the form `want` asked for: adding `&` would make the block's expected type the
        // owned value (`see(&{ &handle })` wants the block to be `Handle`).
        let is_place = matches!(e.kind, ExprKind::Place(..));
        let formed = matches!(e.kind, ExprKind::Temp { .. });
        let field_ref = matches!(&e.kind, ExprKind::Field(a, _) if matches!(a.kind, ExprKind::Temp { .. }) && !trivial(&e.ty, self.prog));
        match want {
            Want::Ref | Want::Str if !is_place && !formed && !field_ref => match &e.kind {
                ExprKind::Const(Const::Str(_)) => code,
                // a loan-typed code is the reference already, except a field read: that is the place of the
                // field behind its receiver's reference, and a reference wanted of it is the borrow of that
                // place (D-449). A field of a statement temporary is borrowed by the field arm itself
                // (`field_ref` above), and a text field read for an interpolation keeps its place form
                _ if matches!(e.ty, Type::Loan { .. }) && !loan_is_value(&e.ty, self.prog)
                    && !(matches!(want, Want::Ref) && matches!(&e.kind, ExprKind::Field(..))) => code,
                _ => format!("&{code}"),
            },
            Want::RefMut if !is_place && !formed => {
                // a call whose result is an exclusive loan is the `&mut` itself: reborrow it (`&mut *r`), as the
                // alias arm above passes the reference on, instead of wrapping a second reference around it (D-185)
                if matches!(&e.ty, Type::Loan { mode, .. } if mode == "editable") && !loan_is_value(&e.ty, self.prog) {
                    format!("&mut *{code}")
                } else {
                    format!("&mut {code}")
                }
            }
            _ => code,
        }
    }

    fn expr_raw(&mut self, e: &Expr, want: Want) -> String {
        let line = e.line;
        match &e.kind {
            ExprKind::Const(c) => self.konst(c, &e.ty, want),
            ExprKind::Place(p, u) => self.place_expr(p, *u, &e.ty, want, line),
            ExprKind::Global(g) => {
                let (m, n) = g.split_once("::").unwrap_or(("", g));
                format!("{}()", global_fn(m, n))
            }
            ExprKind::FuncRef(name) => match self.fn_names.get(name) {
                Some(n) => n.clone(),
                None => self.err(line, format!("function value `{name}`")),
            },
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b),
            ExprKind::Unary(UnOp::Not, a) => format!("(!{})", self.expr(a, Want::Value)),
            ExprKind::Unary(UnOp::Neg, a) => {
                if let ExprKind::Const(Const::Int(n)) = &a.kind {
                    return format!("({}{})", -n, int_suffix(&e.ty));
                }
                if name_of(&e.ty) == Some("Decimal") {
                    return format!("{}.negate()", self.expr(a, Want::Value));
                }
                format!("(-{})", self.expr(a, Want::Value))
            }
            ExprKind::And(a, b) => format!("({} && {})", self.expr(a, Want::Value), self.expr(b, Want::Value)),
            ExprKind::Or(a, b) => format!("({} || {})", self.expr(a, Want::Value), self.expr(b, Want::Value)),
            ExprKind::Call { callee, args } => self.call(callee, args, &e.ty, line),
            ExprKind::Construct { ty, fields } => {
                let mut fs: Vec<String> = fields.iter().map(|(n, v)| {
                    let ft = self.member_ty(&e.ty, n);
                    format!("{}: {}", ident(n), self.expr_as(v, &ft))
                }).collect();
                if self.prog.type_def(ty).map_or(false, |d| crate::phantom_ty(self.prog, d).is_some()) {
                    fs.push("__phantom: std::marker::PhantomData".into());
                }
                if crate::recursive_class(self.prog, ty) {
                    return format!("{}(::std::boxed::Box::new({}_ {{ {} }}))", ident(ty), ident(ty), fs.join(", "));
                }
                // A struct literal is not an operand of `match` or `if`. The parentheses keep a field
                // projection of a constructed temporary legal inside `or recover` (Rust grammar).
                format!("({} {{ {} }})", ident(ty), fs.join(", "))
            }
            ExprKind::Variant { ty, variant, fields } => {
                if name_of(&e.ty).map_or(false, |n| n != ty) && self.prog.type_def(ty).is_none() {
                    return self.err(line, format!("variant of an unknown enum `{ty}`"));
                }
                // a variant of a generic enum names its type arguments (`Level::<i64>::Low`)
                let targs = args_of(&e.ty);
                let path = if !targs.is_empty() && self.prog.type_def(ty).map_or(false, |d| !d.type_params.is_empty()) {
                    format!("{}::<{}>", ident(ty), targs.iter().map(|a| self.ty(a)).collect::<Vec<_>>().join(", "))
                } else {
                    ident(ty)
                };
                if fields.is_empty() {
                    format!("{path}::{}", ident(variant))
                } else {
                    // a field of a recursive enum that reaches it is boxed (see `boxes_field`)
                    let fs: Vec<String> = fields
                        .iter()
                        .map(|(n, v)| {
                            let ft = self.variant_field(ty, variant, n, &v.ty);
                            let code = self.expr(v, Want::Value);
                            if crate::boxes_field(self.prog, ty, &ft) {
                                format!("{}: Box::new({})", ident(n), code)
                            } else {
                                format!("{}: {}", ident(n), code)
                            }
                        })
                        .collect();
                    format!("({path}::{} {{ {} }})", ident(variant), fs.join(", "))
                }
            }
            ExprKind::MakeFailure { ty, fields } => {
                let (code, message) = match self.prog.type_def(ty).map(|d| &d.kind) {
                    Some(TypeKind::Error { code, message, .. }) => (code.clone(), message.clone()),
                    _ => (ty.clone(), String::new()),
                };
                let mut s = format!("rt::Failure::new({code:?}, {:?})", source_name(self.f));
                if !message.is_empty() {
                    s.push_str(&format!(".message({message:?})"));
                }
                for (n, v) in fields {
                    let prim = matches!(name_of(&v.ty), Some("Int" | "Int64" | "Int32" | "Int16" | "Int8" | "UInt" | "UInt64" | "UInt32" | "UInt16" | "UInt8" | "Byte" | "Bool" | "Float" | "Decimal" | "String" | "StringView"));
                    if prim {
                        let vc = self.expr(v, Want::Value);
                        s.push_str(&format!(".with({:?}, rt::IntoDyn::into_dyn({vc}))", n));
                    } else if let Some((en, vs)) = payload_free_enum(self.prog, &v.ty) {
                        // the variant is stored beside the display text: a custom `display` is not the discriminant (D-55)
                        let vc = self.expr(v, Want::Value);
                        let arms: Vec<String> = vs.iter().map(|vn| format!("{}::{} => {:?}", ident(&en), ident(vn), vn)).collect();
                        s.push_str(&format!(
                            ".with({:?}, {{ let __v = {vc}; rt::Dyn::Tag {{ variant: match &__v {{ {} }}, shown: rt::Show::show(&__v) }} }})",
                            n,
                            arms.join(", ")
                        ));
                    } else {
                        let vc = self.expr(v, Want::Any);
                        s.push_str(&format!(".with({:?}, rt::Dyn::Other(rt::Show::show(&{vc})))", n));
                    }
                }
                s
            }
            ExprKind::Tuple(es) => {
                let types = match strip(&e.ty) { Type::Tuple(ts) => ts.clone(), _ => vec![] };
                let parts: Vec<String> = es.iter().enumerate().map(|(i, x)| {
                    let code = self.tuple_part(x);
                    if types.get(i).map_or(false, |t| matches!(strip(t), Type::Optional(_))) && !matches!(strip(&x.ty), Type::Optional(_)) && !matches!(x.kind, ExprKind::Const(Const::Null)) {
                        format!("Some({code})")
                    } else { code }
                }).collect();
                format!("({}{})", parts.join(", "), if parts.len() == 1 { "," } else { "" })
            }
            ExprKind::Record(fs) => {
                let shape: Vec<(String, Type)> = match strip(&e.ty) {
                    Type::Record(fields) => fields.clone(),
                    _ => fs.iter().map(|(n, v)| (n.clone(), v.ty.clone())).collect(),
                };
                self.audit.records.push(shape.clone());
                let parts: Vec<String> = fs.iter().map(|(n, v)| {
                    let ty = shape.iter().find(|(field, _)| field == n).map(|(_, t)| t).unwrap_or(&v.ty);
                    format!("{}: {}", ident(n), self.expr_as(v, ty))
                }).collect();
                // Same parentheses as a class constructor: `{ ... }.field` can sit in a `match` scrutinee.
                format!("({} {{ {} }})", record_name(&shape), parts.join(", "))
            }
            ExprKind::Closure { func, captures } => self.closure(func, captures, line),
            ExprKind::Interpolate(parts) => {
                let ps: Vec<String> = parts
                    .iter()
                    .map(|p| match p {
                        Part::Lit(s) => format!("{s:?}"),
                        Part::Show(x) => self.expr(x, Want::Any),
                    })
                    .collect();
                format!("rt::interp!({})", ps.join(", "))
            }
            // a field of a value behind a `Shared` handle is read through it (7.8), as a place's is
            ExprKind::Field(a, n) if name_of(&a.ty) == Some("Shared") => format!("{}.read().{}", self.expr(a, Want::Any), ident(n)),
            ExprKind::Field(a, n) => {
                if let ExprKind::Temp { .. } = &a.kind {
                    // the slot keeps the parent until the statement drops it. The field is read through that
                    // borrow, so the reference outlives this expression (a `let` inside a block would not).
                    // A copyable field is copied out. An owned use of the field is a `Place` move, not this projection.
                    let base = self.expr(a, Want::RefMut);
                    if trivial(&e.ty, self.prog) {
                        format!("({base}).{}", ident(n))
                    } else {
                        format!("&({base}).{}", ident(n))
                    }
                } else {
                    format!("{}.{}", self.expr(a, Want::Any), ident(n))
                }
            }
            ExprKind::TupleField(a, i) => {
                if let ExprKind::Temp { .. } = &a.kind {
                    // the slot keeps the tuple until the statement drops it. A copyable element is copied out.
                    // An owned element is borrowed here; a consuming use is a `Place` move, not this projection,
                    // because moving out of the slot's shared borrow is rejected by rustc (E0507).
                    let base = self.expr(a, Want::RefMut);
                    if trivial(&e.ty, self.prog) {
                        format!("({base}).{i}")
                    } else {
                        format!("&({base}).{i}")
                    }
                } else {
                    format!("{}.{i}", self.expr(a, Want::Any))
                }
            }
            ExprKind::Index(a, i) => format!("rt::collections::index(&{}, {})", self.expr(a, Want::Any), self.expr(i, Want::Value)),
            ExprKind::OptField(a, n) => {
                let inner = match strip(&a.ty) {
                    Type::Optional(i) => (**i).clone(),
                    other => other.clone(),
                };
                let ft = self.member_ty(&inner, n);
                let src = self.expr(a, Want::Any);
                if matches!(strip(&e.ty), Type::Optional(i) if i.is_loan() && !trivial(i, self.prog)) {
                    // The projection retains its source loan (7.9). Consuming an Option<&T> keeps the
                    // referent's lifetime; borrowing that option would lend a temporary wrapper instead.
                    // An owned option is lent in place. Neither case clones the projected field.
                    let src = if matches!(strip(&a.ty), Type::Optional(i) if i.is_loan()) { src } else { format!("{src}.as_ref()") };
                    return if matches!(strip(&ft), Type::Optional(_)) {
                        format!("{src}.and_then(|v| v.{}.as_ref())", ident(n))
                    } else {
                        format!("{src}.map(|v| &v.{})", ident(n))
                    };
                }
                let read = if trivial(&ft, self.prog) { format!("v.{}", ident(n)) } else { format!("v.{}.clone()", ident(n)) };
                if matches!(strip(&ft), Type::Optional(_)) {
                    format!("{src}.as_ref().and_then(|v| {read})")
                } else {
                    format!("{src}.as_ref().map(|v| {read})")
                }
            }
            ExprKind::Present(a) => {
                // a literal wrapped into an optional borrowed text is the static text itself (D-390): the
                // option is `Option<&str>` (D-353)
                if opt_loan_text(&e.ty, self.prog) {
                    if let ExprKind::Const(Const::Str(s)) = &a.kind {
                        format!("Some({s:?})")
                    } else {
                        self.wrap_owned("Some", a)
                    }
                } else {
                    self.wrap_owned("Some", a)
                }
            }
            ExprKind::Success(a) => self.wrap_owned("rt::Outcome::Ok", a),
            // a borrowed element with a default (`xs.get(i) ?? ""`): the result is a loan, so the default is borrowed too
            // the canonical codec (6.10): generated `rt::codec::Codec` impls; refinements travel as descriptors
            ExprKind::Codec { op, target, arg } => {
                let bytes = matches!(name_of(&arg.ty), Some("Bytes") | Some("BytesView"));
                match op {
                    CodecOp::ToText | CodecOp::ToBytes => {
                        let v = self.expr(arg, Want::Any);
                        let f = match (op, bytes) {
                            (CodecOp::ToText, true) => "bytes_to_text",
                            (CodecOp::ToText, false) => "to_text",
                            (_, true) => "bytes_to_bytes",
                            (_, false) => "to_bytes",
                        };
                        format!("rt::codec::{f}(&{v})")
                    }
                    CodecOp::FromText | CodecOp::FromBytes => {
                        if !crate::codec::codec_able(target, self.prog, &mut vec![]) {
                            return self.err(e.line, format!("`{}` is not Decodable natively", target.show()));
                        }
                        let v = self.expr(arg, Want::Any);
                        let f = if matches!(op, CodecOp::FromText) { "from_text" } else { "from_bytes" };
                        format!("rt::codec::{f}::<{}>(&{v}, &{}, {:?})", self.ty(target), crate::codec::refine(target, self.prog), source_name(self.f))
                    }
                }
            }
            // an optional read parameter is `Option<&T>` (D-2): a borrowed result borrows the fallback too; an owned
            // result copies the present value, as for an optional loan below
            ExprKind::Coalesce(a, b) if matches!(&a.kind, ExprKind::Place(p, _) if p.proj.is_empty() && self.forms[p.local as usize] == Form::OptRef) => {
                let l = match &a.kind {
                    ExprKind::Place(p, _) => self.place(p),
                    _ => unreachable!(),
                };
                let text = matches!(strip(&a.ty), Type::Optional(i) if is_string(i, self.prog));
                if e.ty.is_loan() && name_of(&b.ty) == Some("Never") {
                    let fallback = self.expr(b, Want::Value);
                    return format!("(match {l} {{ Some(__v) => __v, None => {fallback} }})");
                }
                // a lent result is a reference to the place the arm reads (`&*…`); an owned one reads the place (D-341)
                let deref = if matches!(&e.ty, Type::Loan { .. }) { "&*" } else { "*" };
                if matches!(b.kind, ExprKind::Place(_, Use::Read | Use::Alias)) {
                    // a fallback that is a place: where the result is owned, either side is copied (text, a copyable
                    // value); otherwise it is lent, as the present value is, and the result is a place read or lent
                    let owned_result = !matches!(&e.ty, Type::Loan { .. });
                    let copyable_inner = match strip(&a.ty) {
                        Type::Optional(i) => trivial(i, self.prog) || name_of(i).and_then(|n| self.prog.type_def(n)).map_or(false, |d| d.copyable),
                        _ => false,
                    };
                    if owned_result && text {
                        let fb = format!("AsRef::<str>::as_ref({}).to_string()", self.expr(b, Want::Str));
                        return or_else(format!("{l}.map(|v| v.to_string())"), fb, leaves(b));
                    }
                    if owned_result && copyable_inner {
                        let fb = format!("({}).clone()", self.expr(b, Want::Ref));
                        return or_else(format!("{l}.cloned()"), fb, leaves(b));
                    }
                    let fb = if text { format!("AsRef::<str>::as_ref({})", self.expr(b, Want::Str)) } else { self.expr(b, Want::Ref) };
                    return format!("({deref}(match {l} {{ Some(__v) => __v, None => {fb} }}))");
                }
                // a value that cannot be copied is borrowed, fallback and all, for the statement (as `&T`)
                let copyable = match strip(&a.ty) {
                    Type::Optional(i) => is_string(i, self.prog) || trivial(i, self.prog) || name_of(i).and_then(|n| self.prog.type_def(n)).map_or(false, |d| d.copyable),
                    _ => false,
                };
                if matches!(&e.ty, Type::Loan { .. }) || !copyable {
                    // the fallback is evaluated only when the parameter is null, and lives as a temporary of the
                    // statement either way; the result is a place (`*…`) that is lent or read, never moved. A
                    // fallback that is itself a lent `??` is held as it is, a `Cow` or a `MaybeOwned` (D-339)
                    if matches!(b.kind, ExprKind::Coalesce(..)) && lent_coalesce_ty(&b.ty, self.prog) {
                        let ExprKind::Coalesce(ba, bb) = &b.kind else { unreachable!() };
                        let inner = self.lent_coalesce(ba, bb, &b.ty);
                        return if text {
                            format!("({deref}(match {l} {{ Some(__v) => std::borrow::Cow::Borrowed(__v), None => {inner} }}))")
                        } else {
                            format!("({deref}(match {l} {{ Some(__v) => rt::MaybeOwned::Borrowed(__v), None => {inner} }}))")
                        };
                    }
                    match (&b.kind, text) {
                        (ExprKind::Const(Const::Str(s)), true) => format!("{l}.unwrap_or({s:?})"),
                        (_, true) => format!("({deref}(match {l} {{ Some(__v) => std::borrow::Cow::Borrowed(__v), None => std::borrow::Cow::Owned({}) }}))", self.expr(b, Want::Value)),
                        // a fallback that is a place is lent; a new value is owned
                        _ if matches!(b.kind, ExprKind::Place(_, Use::Read | Use::Alias)) => format!("({deref}(match {l} {{ Some(__v) => __v, None => {} }}))", self.expr(b, Want::Ref)),
                        _ => format!("({deref}(match {l} {{ Some(__v) => rt::MaybeOwned::Borrowed(__v), None => rt::MaybeOwned::Owned({}) }}))", self.expr(b, Want::Value)),
                    }
                } else if text {
                    // a fallback that is itself a lent `??` is made owned in place (D-339)
                    let fb = if matches!(b.kind, ExprKind::Coalesce(..)) && lent_coalesce_ty(&b.ty, self.prog) {
                        let ExprKind::Coalesce(ba, bb) = &b.kind else { unreachable!() };
                        format!("{}.into_owned()", self.lent_coalesce(ba, bb, &b.ty))
                    } else {
                        self.expr(b, Want::Value)
                    };
                    or_else(format!("{l}.map(|v| v.to_string())"), fb, leaves(b))
                } else {
                    let fb = self.expr(b, Want::Value);
                    or_else(format!("{l}.cloned()"), fb, leaves(b))
                }
            }
            // a borrowed result of a non-trivial value (`show(xs.get(0) ?? fallback())`): the fallback runs only when the
            // left side is null (5.7), so it sits in the `None` arm (D-246). The result borrows a temporary of the
            // statement that holds the present value as the left side gives it (a loan, or the copy `m.get(k)` makes)
            // or the fallback: a place lent, a new value owned. Text is read as `&str` (a `Cow`), the rest through a
            // `MaybeOwned`
            ExprKind::Coalesce(a, b) if lent_coalesce_ty(&e.ty, self.prog) => {
                if let Some(lent) = self.lent_coalesce_ref(a, b, &e.ty) {
                    return lent;
                }
                let held = self.lent_coalesce(a, b, &e.ty);
                format!("(&*{held})")
            }
            // an optional loan of a non-trivial value with an owned fallback: the result is owned, so the loan is cloned
            // (a generic body records no types, so there the loan is known from the getter that produced it)
            ExprKind::Coalesce(a, b)
                if (matches!(strip(&a.ty), Type::Optional(i) if matches!(&**i, Type::Loan { inner, .. } if !trivial(inner, self.prog)))
                    && !matches!(&e.ty, Type::Loan { .. }))
                    || (matches!(&a.ty, Type::Unknown(_))
                        && matches!(&a.kind, ExprKind::Call { callee: Callee::Intrinsic(n), .. } if matches!(n.as_str(), "List.get" | "ListView.get" | "Map.get"))
                        && !matches!(&b.kind, ExprKind::Place(_, u) if *u != Use::Copy)) =>
            {
                let l = self.expr(a, Want::Value);
                let fb = self.expr(b, Want::Value);
                or_else(format!("{l}.cloned()"), fb, leaves(b))
            }
            ExprKind::Coalesce(a, b) => {
                if matches!(a.kind, ExprKind::Temp { .. }) {
                    // the operand stays in its slot until the statement drops it (7.9). `??` only reads it,
                    // so a discarded result is not moved out and dropped without its trace.
                    let left = self.expr(a, Want::RefMut);
                    let right = self.expr(b, Want::RefMut);
                    format!("(match {left} {{ Some(ref mut __v) => __v, None => {right} }})")
                } else if matches!(&a.kind, ExprKind::Place(_, Use::Read))
                    && matches!(strip(&a.ty), Type::Optional(i) if !trivial(i, self.prog) && !i.is_loan())
                    && (matches!(want, Want::Ref | Want::Str) || matches!(strip(&a.ty), Type::Optional(i) if is_string(i, self.prog)))
                {
                    // lent for a call or an interpolation, a bare owned place on the left is a read loan of it
                    // (D-357, 7.5): the result borrows the value in the place or the fallback, a temporary of the
                    // statement, and the place keeps its value for the uses that follow (D-354). Text borrows
                    // through a `Cow`, as for an optional read parameter above; the rest is a reference into the
                    // place or into the fallback's statement slot, which holds a made or moved value for the
                    // statement's own traced drop (7.9). An owned result of a bare left place is rejected before
                    // the lowering (D-357), so a text one reaches here only as an interpolation part, which reads
                    // the `Cow` as its `&str`
                    let l = self.expr(a, Want::Any);
                    let text = matches!(strip(&a.ty), Type::Optional(i) if is_string(i, self.prog));
                    let lc = l.clone();
                    let cow = move |fb: String| format!("(match {lc}.as_deref() {{ Some(__v) => std::borrow::Cow::Borrowed(__v), None => std::borrow::Cow::Owned({fb}) }})");
                    match (&b.kind, text) {
                        (ExprKind::Const(Const::Str(s)), true) => format!("({l}.as_deref().unwrap_or({s:?}))"),
                        (_, true) if !matches!(want, Want::Ref | Want::Str) => format!("(&*{})", cow(self.expr(b, Want::Value))),
                        (_, true) => cow(self.expr(b, Want::Value)),
                        // every fallback lends as the present value does: a place lends itself, and a value the
                        // fallback makes or moves stays in its statement's slot, which the statement's drop
                        // destroys and announces; taking it into a holder of the statement would drop it
                        // unannounced (D-354)
                        _ => format!("(&*(match {l}.as_ref() {{ Some(__v) => __v, None => {} }}))", self.expr(b, Want::Ref)),
                    }
                } else {
                    let l = self.expr(a, Want::Value);
                    let fb = self.expr(b, Want::Value);
                    or_else(l, fb, leaves(b))
                }
            }
            ExprKind::OrFail(a) => {
                // a discarded or borrowed success stays in its temporary slot, so the statement drop announces
                // it. Taking it here would drop the handle with no `drop` event (7.9). An owned use still takes.
                if let ExprKind::Temp { local, .. } = &a.kind {
                    if !matches!(want, Want::Value) {
                        let local = *local;
                        let ab = self.abandon_live();
                        let probe = self.expr(a, Want::RefMut);
                        let name = self.names[local as usize].clone();
                        let fail = if ab.is_empty() && !self.is_async {
                            self.fail_ret("e.into()")
                        } else {
                            format!("{ab} {}", self.fail_ret("e.into()"))
                        };
                        let reborrow = match want {
                            Want::RefMut => format!("{name}.as_mut().unwrap().as_mut().unwrap()"),
                            _ => format!("{name}.as_ref().unwrap().as_ref().unwrap()"),
                        };
                        // `unwrap_err` needs `Debug` on the success type. A class with `cleanup` has none, so match the error out.
                        return format!("{{ let __err = {probe}.is_err(); if __err {{ if let Some(Err(e)) = {name}.take() {{ {fail}; }} }} {reborrow} }}");
                    }
                }
                let v = self.expr(a, Want::Value);
                let ab = self.abandon_live();
                if ab.is_empty() && !self.is_async {
                    format!("{v}?")
                } else {
                    // The outcome is destroyed inside `outcome_err`. An inlined match keeps that destructor in
                    // this function, and rustc then rejects a drop of a local the operand borrows (a mutex still
                    // borrowed by its lock, E0505). The exit runs after that borrow has ended, in the same order.
                    let id = self.next_id();
                    let tmp = format!("__of{id}");
                    let err = format!("__oe{id}");
                    format!("{{ let {tmp} = {v}; if {tmp}.is_err() {{ let {err} = rt::outcome_err({tmp}); {ab} {}; }} match {tmp} {{ Ok(v) => v, Err(_) => unreachable!() }} }}", self.fail_ret(&format!("{err}.into()")))
                }
            }
            ExprKind::OrRecover(a, b) => {
                if recovery_holder_inner(e, self.prog).is_some() {
                    return format!("(&*{})", self.recovery_holder(e));
                }
                // a literal recovered where the result is a borrowed text is the static text itself (D-390),
                // as the `recover` statement's is; where the result is an optional borrowed text it is
                // wrapped (`Option<&str>`, D-353)
                let fb = match &b.kind {
                    ExprKind::Const(Const::Str(s)) if matches!(&e.ty, Type::Loan { inner, origin, .. } if is_string(inner, self.prog) && !origin.is_empty() && origin != "static") => format!("{s:?}"),
                    ExprKind::Const(Const::Str(s)) if opt_loan_text(&e.ty, self.prog) => format!("Some({s:?})"),
                    _ => self.expr(b, Want::Value),
                };
                format!("(match {} {{ Ok(v) => v, Err(_) => {fb} }})", self.expr(a, Want::Value))
            }
            ExprKind::Catch { value, bind, handler } => {
                if recovery_holder_inner(e, self.prog).is_some() {
                    return format!("(&*{})", self.recovery_holder(e));
                }
                let v = self.expr(value, Want::Value);
                let l = self.label();
                self.recover_held.push(false);
                self.recover.push(l.clone());
                self.recover_ty.push(e.ty.clone());
                self.recover_depth.push(self.defers.len());
                let h = self.block_stmts(handler);
                self.recover_depth.pop();
                self.recover_ty.pop();
                self.recover_held.pop();
                self.recover.pop();
                let inner_ty = match strip(&value.ty) {
                    Type::Named(n) if n.name == "Outcome" => n.args.first().cloned(),
                    _ => None,
                };
                let present = matches!(strip(&e.ty), Type::Optional(_)) && inner_ty.map_or(true, |t| !matches!(strip(&t), Type::Optional(_)));
                let ok = if present { "Some(v)" } else { "v" };
                format!("(match {v} {{ Ok(v) => {ok}, Err({}) => {l}: {{\n{}\n}} }})", self.binding(*bind), indent(&h, 1))
            }
            ExprKind::If { cond, then, els } => {
                let c = self.expr(cond, Want::Value);
                let a = self.value_block_typed(then, &e.ty);
                let b = self.value_block_typed(els, &e.ty);
                format!("(if {c} {{ {a} }} else {{ {b} }})")
            }
            ExprKind::Match { scrutinee, arms } => self.match_typed(scrutinee, arms, true, &e.ty),
            ExprKind::Block(b) => self.value_block(b),
            ExprKind::Fail(a) if fortis_types::types::known(&a.ty) && name_of(&a.ty) != Some("Failure") && !self.prog.type_def(name_of(&a.ty).unwrap_or("")).map_or(false, |d| matches!(d.kind, TypeKind::Error { .. })) => {
                format!("{{ let _ = {}; rt::dynamic(\"fail of a non-failure value\") }}", self.expr(a, Want::Any))
            }
            ExprKind::Fail(a) => {
                let v = self.expr(a, Want::Value);
                // `fail` leaves the function: the statement's temporaries drop before the blocks do (7.9, 10.8)
                let drops = self.pull_drops();
                let ab = self.exit_code(0, true);
                if ab.is_empty() && drops.is_empty() {
                    self.fail_ret(&v)
                } else {
                    format!("{{ let __f = {v}; {drops} {ab} {} }}", self.fail_ret("__f.into()"))
                }
            }
            ExprKind::Validate { value, lo, hi, length } => {
                let v = self.expr(value, Want::Value);
                let n = if *length && is_string(&value.ty, self.prog) {
                    "rt::text::scalar_count(&__v) as i128".to_string()
                } else if *length {
                    "__v.len() as i128".to_string()
                } else {
                    "__v as i128".to_string()
                };
                let b = |x: &Option<i128>| x.map_or("None".to_string(), |v| format!("Some({v})"));
                format!("{{ let __v = {v}; rt::numeric::validate({n}, {}, {}, {:?}).map(|_| __v) }}", b(lo), b(hi), source_name(self.f))
            }
            ExprKind::Wrap { value, .. } => self.expr(value, Want::Value),
            ExprKind::ToDynamic { iface, value, borrowed } => {
                let tr = match strip(&e.ty) {
                    Type::Dynamic(n) => dyn_name(n, self.prog),
                    Type::Loan { inner, .. } => match &**inner {
                        Type::Dynamic(n) => dyn_name(n, self.prog),
                        _ => ident(iface),
                    },
                    _ => ident(iface),
                };
                if *borrowed {
                    format!("({} as &dyn {tr})", self.expr(value, Want::Ref))
                } else {
                    format!("(::std::boxed::Box::new({}) as ::std::boxed::Box<dyn {tr}>)", self.expr(value, Want::Value))
                }
            }
            ExprKind::Await(x) => {
                // `await e` (12.3): a cancelled task leaves here, unwinding the open blocks.
                // Temporaries the future borrows are bound around the poll, not inside the future expression.
                self.await_depth += 1;
                let fut = self.awaitable(x);
                self.await_depth -= 1;
                let ab = self.exit_code(0, true);
                let body = format!("(match {fut}.await {{ Ok(__v) => __v, Err(__c) => {{ {ab} return Err(__c); }} }})");
                if self.await_lets.is_empty() {
                    body
                } else {
                    let lets = std::mem::take(&mut self.await_lets);
                    format!("{{ {} {body} }}", lets.join(" "))
                }
            }
            // a statement temporary lives in a local until the statement drops it, so an interpolation hole is not
            // dropped inside `rt::interp!` before the enclosing call returns (5.7, 7.9)
            ExprKind::Temp { local, value } => {
                // the slot is hoisted as `None` and filled where this operand is evaluated, so a skipped
                // `&&` / `||` / `??` operand never creates the value (5.7). The `is_none` guard initialises
                // once, including when an intrinsic lowers the expression twice. A read reborrows; an owned
                // use moves the value out, and the statement drop's `take` then finds `None`.
                let n = self.names[*local as usize].clone();
                let holder = if self.temp_inits.contains_key(local) { None } else { recovery_holder_inner(value, self.prog) };
                let v = if let Some(code) = self.temp_inits.get(local).cloned() {
                    code
                } else if let Some(inner) = holder {
                    let code = self.recovery_holder(value);
                    let ann = if is_string(&inner, self.prog) {
                        "std::borrow::Cow<str>".to_string()
                    } else {
                        self.mo_temps.insert(*local);
                        format!("rt::MaybeOwned<{}>", self.ty(&inner))
                    };
                    self.emitted_temps.push(*local);
                    self.hoist.push(format!("let mut {n}: Option<{ann}> = None;"));
                    self.hoist_owners.push(*local);
                    self.forms[*local as usize] = Form::Owned;
                    self.temp_inits.insert(*local, code.clone());
                    code
                } else {
                    self.emitted_temps.push(*local);
                    let ann = if annotatable(&e.ty, self.prog) { format!(": Option<{}>", self.ty(&e.ty)) } else { String::new() };
                    self.hoist.push(format!("let mut {n}{ann} = None;"));
                    self.hoist_owners.push(*local);
                    self.forms[*local as usize] = Form::Owned;
                    let code = self.expr(value, Want::Value);
                    self.temp_inits.insert(*local, code.clone());
                    code
                };
                let init = format!("if {n}.is_none() {{ {n} = Some({v}); }}");
                match want {
                    Want::Value if trivial(&e.ty, self.prog) => format!("{{ {init} *{n}.as_ref().unwrap() }}"),
                    Want::Value => format!("{{ {init} {n}.take().unwrap() }}"),
                    Want::RefMut => format!("{{ {init} {n}.as_mut().unwrap() }}"),
                    _ => format!("{{ {init} &*{n}.as_mut().unwrap() }}"),
                }
            }
        }
    }

    /// The class name a dropped value announces, when that value itself has `cleanup` (a field's cleanup is
    /// announced by destroying the field, as the interpreter announces only the instance it drops).
    fn cleanup_trace(&self, ty: &Type) -> Option<String> {
        let n = name_of(ty)?;
        self.prog.type_def(n).filter(|d| d.cleanup.is_some()).map(|_| n.to_string())
    }

    /// Statement temporaries, reverse creation order (7.9). Each slot is an `Option`: a skipped operand left
    /// it `None`, and `take` drops only a value that was created. `skip` holds a `for` source this statement
    /// moved into the iterator. An absent optional announces nothing. An `Outcome` announces its success value
    /// only when the success is present. A field or tuple element moved out of the temporary stashes the other
    /// parts; those slots are dropped here, in declaration or index order, after the consumer. No second
    /// `drop Name` (the parent's own cleanup is the only trace, and a moved field of a cleanup parent is not split).
    fn temp_drops(&self, s: &Stmt, skip: &[LocalId]) -> String {
        let moved_fields = field_moved_temps(s);
        let mut out = Vec::new();
        for &t in s.temps.iter().rev() {
            if skip.contains(&t) {
                continue;
            }
            let ty = self.f.locals[t as usize].ty.clone();
            let name = self.names[t as usize].clone();
            out.push(if self.mo_temps.contains(&t) { self.temp_holder_drop_slot(&name, &ty) } else { self.temp_drop_slot(&name, &ty) });
            if moved_fields.contains(&t) {
                if let Some(fields) = self.splittable_fields(&ty) {
                    for (fname, _) in fields {
                        let slot = self.hold_slot(t, &fname);
                        out.push(format!("if let Some(__r) = {slot}.take() {{ drop(__r); }}"));
                    }
                }
            }
            let paths = tuple_paths_in_fn(self.f, t);
            if !paths.is_empty() {
                for (slot, _) in self.tuple_hold_plan(t, &ty, &paths) {
                    out.push(format!("if let Some(__r) = {slot}.take() {{ drop(__r); }}"));
                }
            }
        }
        out.join(" ")
    }

    /// A field of a statement temporary (7.4, 7.9). The parent slot stays occupied for a read or a copy.
    fn temp_field_use(&mut self, local: LocalId, field: &str, u: Use, ty: &Type, want: Want, line: u32) -> String {
        if u == Use::Move {
            return self.move_temp_field(local, field, line);
        }
        let n = self.names[local as usize].clone();
        let f = ident(field);
        let borrow = if matches!(u, Use::Editable) || matches!(want, Want::RefMut) {
            format!("{n}.as_mut().unwrap().{f}")
        } else {
            format!("{n}.as_ref().unwrap().{f}")
        };
        match (want, u) {
            (Want::Value, Use::Copy) if !trivial(ty, self.prog) => {
                self.audit.clone_at(&self.f.name, line, "copy");
                if is_string(ty, self.prog) {
                    format!("{borrow}.to_string() /* copy */")
                } else {
                    format!("{borrow}.clone() /* copy */")
                }
            }
            (Want::Ref | Want::Str, _) => format!("&{borrow}"),
            (Want::RefMut, _) => format!("&mut {borrow}"),
            _ => borrow,
        }
    }

    /// Move one non-copyable field out of a statement temporary. Every field is stashed, the requested one is
    /// taken now, and `temp_drops` drops whatever is still there at the end of the statement (7.9). A parent
    /// that declares `cleanup` cannot be taken apart in safe Rust (its `Drop` forbids the move); that shape
    /// is a loud refusal.
    fn move_temp_field(&mut self, local: LocalId, field: &str, line: u32) -> String {
        let n = self.names[local as usize].clone();
        let ty = self.local_ty(local).clone();
        let f = ident(field);
        let Some(fields) = self.splittable_fields(&ty) else {
            if crate::declares_cleanup(self.prog, &ty) {
                return self.err(line, format!("moving field `{field}` out of a temporary that declares cleanup"));
            }
            return format!("{{ let mut __mv = {n}.take().unwrap(); __mv.{f} }}");
        };
        self.ensure_hold_slots(local, &fields);
        let split = self.split_stores(&n, &ty, local, &fields);
        let slot = self.hold_slot(local, field);
        format!("{{ {split} {slot}.take().unwrap() }}")
    }

    fn hold_slot(&self, local: LocalId, field: &str) -> String {
        format!("__hold{local}_{}", ident(field))
    }

    /// Fields of a class with no cleanup of its own, or of an anonymous record, in declaration order.
    /// `None` for a cleanup class (it is not taken apart) and for anything else.
    fn splittable_fields(&self, ty: &Type) -> Option<Vec<(String, Type)>> {
        match strip(ty) {
            Type::Record(fs) => Some(fs.clone()),
            Type::Named(n) => {
                let d = self.prog.type_def(&n.name)?;
                if d.cleanup.is_some() {
                    return None;
                }
                let TypeKind::Class { fields } = &d.kind else { return None };
                let map: Vec<(String, Type)> = d.type_params.iter().cloned().zip(n.args.iter().cloned()).collect();
                Some(fields.iter().map(|f| (f.name.clone(), crate::subst_type(&f.ty, &map))).collect())
            }
            _ => None,
        }
    }

    fn ensure_hold_slots(&mut self, local: LocalId, fields: &[(String, Type)]) {
        if !self.split_hoisted.insert(local) {
            return;
        }
        for (name, ft) in fields {
            let slot = self.hold_slot(local, name);
            self.hoist.push(format!("let mut {slot}: Option<{}> = None;", self.ty(ft)));
            self.hoist_owners.push(local);
        }
    }

    /// Take the parent once and stash every field. A second move in the same statement finds the slot empty.
    fn split_stores(&self, n: &str, ty: &Type, local: LocalId, fields: &[(String, Type)]) -> String {
        let body = match strip(ty) {
            Type::Named(nm) if crate::recursive_class(self.prog, &nm.name) => {
                let name = ident(&nm.name);
                let pat = fields.iter().map(|(f, _)| ident(f)).collect::<Vec<_>>().join(", ");
                let stores = fields.iter().map(|(f, _)| format!("{} = Some({});", self.hold_slot(local, f), ident(f))).collect::<Vec<_>>().join(" ");
                format!("let {name}(__box) = __mv; let {name}_ {{ {pat}, .. }} = *__box; {stores}")
            }
            _ => fields.iter().map(|(f, _)| format!("{} = Some(__mv.{});", self.hold_slot(local, f), ident(f))).collect::<Vec<_>>().join(" "),
        };
        format!("if {n}.is_some() {{ let __mv = {n}.take().unwrap(); {body} }}")
    }

    /// A tuple element of a statement temporary (7.9). A read or a copy borrows through the path. A move stashes
    /// every element the path does not take, including elements of a tuple the path passes through (`.0.1`).
    fn temp_tuple_use(&mut self, p: &Place, u: Use, ty: &Type, want: Want, line: u32) -> String {
        if u == Use::Move {
            return self.move_temp_tuple(p, line);
        }
        let n = self.names[p.local as usize].clone();
        let path = tuple_proj_path(&p.proj);
        let borrow = if matches!(u, Use::Editable) || matches!(want, Want::RefMut) {
            format!("{n}.as_mut().unwrap(){path}")
        } else {
            format!("{n}.as_ref().unwrap(){path}")
        };
        match (want, u) {
            (Want::Value, Use::Copy) if !trivial(ty, self.prog) => {
                self.audit.clone_at(&self.f.name, line, "copy");
                if is_string(ty, self.prog) {
                    format!("{borrow}.to_string() /* copy */")
                } else {
                    format!("{borrow}.clone() /* copy */")
                }
            }
            (Want::Ref | Want::Str, _) => format!("&{borrow}"),
            (Want::RefMut, _) => format!("&mut {borrow}"),
            _ => borrow,
        }
    }

    /// Move one element out of a statement temporary tuple. The parent is taken once and every element is
    /// stashed; `temp_drops` drops whatever is still there at the end of the statement, after the consumer
    /// (7.9, D-312). The slot stays an `Option`, so the move is not a move out of its shared borrow (E0507).
    fn move_temp_tuple(&mut self, p: &Place, line: u32) -> String {
        let n = self.names[p.local as usize].clone();
        let ty = self.local_ty(p.local).clone();
        let indices: Vec<u32> = p.proj.iter().map(|pr| match pr {
            Proj::Tuple(i) => *i,
            _ => 0,
        }).collect();
        let paths = tuple_paths_in_fn(self.f, p.local);
        let slots = self.tuple_hold_plan(p.local, &ty, &paths);
        let slot = tuple_hold_name(p.local, &indices);
        if !slots.iter().any(|(name, _)| name == &slot) {
            return self.err(line, "moving a tuple element out of a temporary whose shape is not a tuple".to_string());
        }
        self.ensure_tuple_holds(p.local, &slots);
        let split = self.tuple_split_stores(&n, &ty, p.local, &paths);
        format!("{{ {split} {slot}.take().unwrap() }}")
    }

    fn ensure_tuple_holds(&mut self, local: LocalId, slots: &[(String, Type)]) {
        if !self.split_hoisted.insert(local) {
            return;
        }
        for (slot, ft) in slots {
            self.hoist.push(format!("let mut {slot}: Option<{}> = None;", self.ty(ft)));
            self.hoist_owners.push(local);
        }
    }

    /// Element slots for every tuple-element move of `local`, left to right. A path that continues into an
    /// element splits that element; the element a path stops on is one slot.
    fn tuple_hold_plan(&self, local: LocalId, ty: &Type, paths: &[Vec<u32>]) -> Vec<(String, Type)> {
        let mut out = Vec::new();
        self.plan_tuple_holds(local, ty, &[], paths, &mut out);
        out
    }

    fn plan_tuple_holds(&self, local: LocalId, ty: &Type, prefix: &[u32], paths: &[Vec<u32>], out: &mut Vec<(String, Type)>) {
        let Type::Tuple(elems) = strip(ty) else { return };
        for (i, et) in elems.iter().enumerate() {
            let mut path = prefix.to_vec();
            path.push(i as u32);
            let descends = matches!(strip(et), Type::Tuple(_)) && paths.iter().any(|p| p.len() > path.len() && p.starts_with(&path));
            if descends {
                self.plan_tuple_holds(local, et, &path, paths, out);
            } else {
                out.push((tuple_hold_name(local, &path), et.clone()));
            }
        }
    }

    /// Take the parent once and stash every element. A second move in the same statement finds the slot empty.
    fn tuple_split_stores(&self, n: &str, ty: &Type, local: LocalId, paths: &[Vec<u32>]) -> String {
        let body = self.tuple_stash_body("__mv", ty, local, &[], paths);
        format!("if {n}.is_some() {{ let __mv = {n}.take().unwrap(); {body} }}")
    }

    fn tuple_stash_body(&self, src: &str, ty: &Type, local: LocalId, prefix: &[u32], paths: &[Vec<u32>]) -> String {
        let Type::Tuple(elems) = strip(ty) else { return String::new() };
        let mut parts = Vec::new();
        for (i, et) in elems.iter().enumerate() {
            let mut path = prefix.to_vec();
            path.push(i as u32);
            let descends = matches!(strip(et), Type::Tuple(_)) && paths.iter().any(|p| p.len() > path.len() && p.starts_with(&path));
            if descends {
                let binding = format!("__te{}", path.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("_"));
                let inner = self.tuple_stash_body(&binding, et, local, &path, paths);
                parts.push(format!("{{ let {binding} = {src}.{i}; {inner} }}"));
            } else {
                let slot = tuple_hold_name(local, &path);
                parts.push(format!("{slot} = Some({src}.{i});"));
            }
        }
        parts.join(" ")
    }

    fn temp_drop_slot(&self, name: &str, ty: &Type) -> String {
        if let Type::Named(n) = strip(ty) {
            if n.name == "Outcome" {
                let success = n.args.first().cloned();
                let success = success.unwrap_or_else(|| ty.clone());
                let ok = self.drop_owned("__p", &success);
                return format!("if let Some(__slot) = {name}.take() {{ match __slot {{ Ok(__p) => {ok} Err(__e) => {{ drop(__e); }} }} }}");
            }
        }
        let body = self.drop_owned("__slot", ty);
        format!("if let Some(__slot) = {name}.take() {{ {body} }}")
    }

    /// The drop of a slot that holds an `or recover`'s `MaybeOwned` (D-446): the replacement is announced and
    /// destroyed only when it was made; a borrowed success belongs to what the try expression borrows, and its
    /// end announces nothing (7.9).
    fn temp_holder_drop_slot(&self, name: &str, ty: &Type) -> String {
        let trace = self.cleanup_trace(ty).map(|n| format!("if matches!(__slot, rt::MaybeOwned::Owned(_)) {{ rt::trace::event(\"drop {n}\"); }} ")).unwrap_or_default();
        format!("if let Some(__slot) = {name}.take() {{ {trace}drop(__slot); }}")
    }

    /// Drop an owned value. Optional layers are peeled: only a present payload is announced, and an absent
    /// optional announces nothing (7.9).
    fn drop_owned(&self, expr: &str, ty: &Type) -> String {
        let mut layers = 0usize;
        let mut cur = ty;
        loop {
            match strip(cur) {
                Type::Optional(inner) => {
                    layers += 1;
                    cur = inner.as_ref();
                }
                _ => break,
            }
        }
        let trace = self.cleanup_trace(cur).map(|n| format!("rt::trace::event(\"drop {n}\"); ")).unwrap_or_default();
        if layers == 0 {
            format!("{{ {trace}drop({expr}); }}")
        } else {
            let pat = format!("{}__d{}", "Some(".repeat(layers), ")".repeat(layers));
            format!("{{ if let {pat} = {expr} {{ {trace}drop(__d); }} }}")
        }
    }

    fn pull_drops(&mut self) -> String {
        self.in_transfer = true;
        self.transfer_drops.last().cloned().unwrap_or_default()
    }

    fn binding(&self, l: LocalId) -> String {
        let n = &self.names[l as usize];
        if self.f.locals[l as usize].mutable {
            format!("mut {n}")
        } else {
            n.clone()
        }
    }

    fn konst(&mut self, c: &Const, ty: &Type, want: Want) -> String {
        match c {
            Const::Void => "()".into(),
            Const::Bool(b) => b.to_string(),
            Const::Int(n) => format!("{n}{}", int_suffix(ty)),
            Const::Float(b) => {
                let x = f64::from_bits(*b);
                if x.is_finite() {
                    format!("{x:?}f64")
                } else {
                    format!("f64::from_bits({b:#x})")
                }
            }
            Const::Float32(b) => format!("f32::from_bits({b:#x})"),
            Const::Decimal(d) => format!("rt::Decimal::parse({d:?}).unwrap()"),
            Const::Duration(ns) => format!("rt::Duration({ns})"),
            Const::Str(s) => match want {
                Want::Ref | Want::Str | Want::Any => format!("{s:?}"),
                _ => format!("String::from({s:?})"),
            },
            Const::Bytes(b) => format!("vec!{b:?}"),
            Const::Null => "None".into(),
        }
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr) -> String {
        let sym = op.symbol();
        if op == BinOp::Eq || op == BinOp::Ne {
            if matches!(b.kind, ExprKind::Const(Const::Null)) {
                let x = self.expr(a, Want::Any);
                return if op == BinOp::Eq { format!("{x}.is_none()") } else { format!("{x}.is_some()") };
            }
            if matches!(a.kind, ExprKind::Const(Const::Null)) {
                let x = self.expr(b, Want::Any);
                return if op == BinOp::Eq { format!("{x}.is_none()") } else { format!("{x}.is_some()") };
            }
        }
        if op == BinOp::Eq || op == BinOp::Ne {
            let (ao, bo) = (matches!(strip(&a.ty), Type::Optional(_)), matches!(strip(&b.ty), Type::Optional(_)));
            if ao != bo && fortis_types::types::known(&a.ty) && fortis_types::types::known(&b.ty) {
                // a present `T` compared with a `T?` (present injection, 6.8)
                let (o, p) = if ao { (a, b) } else { (b, a) };
                let x = self.expr(o, Want::Any);
                let y = self.expr(p, Want::Value);
                let cmp = if is_string(&p.ty, self.prog) { format!("{x}.as_ref().map(|v| rt::show::s(v)) {sym} Some(rt::show::s(&{y}))") } else { format!("{x}.clone() {sym} Some({y})") };
                return format!("({cmp})");
            }
        }
        if op.is_comparison() {
            if fortis_types::types::int_bounds(&a.ty).is_some() && fortis_types::types::int_bounds(&b.ty).is_some() && int_suffix(&a.ty) != int_suffix(&b.ty) {
                // All Fortis integer values fit i128. Compare accepted operands of different
                // machine widths without changing either value (a Byte and an Int literal, D-560).
                return format!("(({} as i128) {sym} ({} as i128))", self.expr(a, Want::Value), self.expr(b, Want::Value));
            }
            if is_string(&a.ty, self.prog) || is_string(&b.ty, self.prog) {
                let x = self.expr(a, Want::Any);
                let y = self.expr(b, Want::Any);
                return format!("(rt::show::s(&{x}) {sym} rt::show::s(&{y}))");
            }
            if trivial(&a.ty, self.prog) && trivial(&b.ty, self.prog) {
                return format!("({} {sym} {})", self.expr(a, Want::Value), self.expr(b, Want::Value));
            }
            return format!("({} {sym} {})", self.expr(a, Want::Ref), self.expr(b, Want::Ref));
        }
        // a signed remainder the checker proved is the language's: `Int.MIN % -1` is 0, as `Int.rem`, the interpreter
        // and the VM give it, where Rust's `%` panics; every other remainder is the same (D-240). A proved `/` never
        // divides the minimum by -1: the checker leaves that division unproved (6.4)
        if op == BinOp::Rem && matches!(rust_ty(&a.ty, self.prog).as_str(), "i64" | "i32" | "i16" | "i8") {
            return format!("({}).wrapping_rem({})", self.expr(a, Want::Value), self.expr(b, Want::Value));
        }
        format!("({} {sym} {})", self.expr(a, Want::Value), self.expr(b, Want::Value))
    }

    /// An argument converted to its parameter's mode.
    /// An optional place where a non-optional value is wanted: the checker narrowed it (`if v != null`), so its
    /// value is unwrapped, moved, copied or lent as the place is used. None when it is not such a place.
    pub fn narrowed(&mut self, e: &Expr, want: &Type) -> Option<String> {
        let ExprKind::Place(p, u) = &e.kind else { return None };
        let declared = self.place_ty(p);
        let Type::Optional(inner) = strip(&declared) else { return None };
        if matches!(strip(want), Type::Optional(_) | Type::TypeParam(_)) || !fortis_types::types::known(want) {
            return None;
        }
        if p.proj.is_empty() && self.forms[p.local as usize] == Form::OptRef {
            // an optional read parameter is `Option<&T>` (D-2): narrowed, it is the `&T`
            let code = self.place(p);
            return Some(match u {
                Use::Copy if is_string(inner, self.prog) => format!("{code}.unwrap().to_string()"),
                Use::Copy => format!("{code}.unwrap().clone()"),
                _ => format!("{code}.unwrap()"),
            });
        }
        let code = self.place(p);
        let base = if p.proj.is_empty() && self.forms[p.local as usize] != Form::Owned { format!("(*{code})") } else { code };
        Some(match u {
            Use::Move => format!("{base}.unwrap()"),
            // an editable loan of a narrowed optional is `&mut T` (the receiver of `editable p.m()`), not a shared borrow
            Use::Editable => format!("{base}.as_mut().unwrap()"),
            _ if trivial(inner, self.prog) => format!("{base}.unwrap()"),
            Use::Copy => {
                self.audit.clone_at(&self.f.name, e.line, "copy");
                format!("{base}.clone().unwrap()")
            }
            _ => format!("{base}.as_ref().unwrap()"),
        })
    }

    pub fn arg(&mut self, a: &Arg, pty: Option<&Type>) -> String {
        if let Some(code) = pty.and_then(|pt| self.narrowed(&a.value, pt)) {
            return code;
        }
        let t = pty.unwrap_or(&a.value.ty).clone();
        let t = &t;
        if let Type::Fn { params, param_modes, .. } = strip(t) {
            // a closure value (not a literal) passed where `fn(T) U` takes generic parameters by reference: adapt it
            if !matches!(a.value.kind, ExprKind::Closure { .. }) {
                if let Type::Fn { params: have, .. } = strip(&a.value.ty) {
                    let adapt: Vec<bool> = params.iter().zip(have.iter()).enumerate().map(|(i, (want, h))| fn_param_mode(param_modes, i) == Mode::Read && !trivial(want, self.prog) && trivial(h, self.prog)).collect();
                    if adapt.iter().any(|x| *x) {
                        let f = self.expr(&a.value, Want::Any);
                        let ps: Vec<String> = adapt.iter().enumerate().map(|(i, by_ref)| if *by_ref { format!("__a{i}: &{}", rust_ty(&have[i], self.prog)) } else { format!("__a{i}") }).collect();
                        let cs: Vec<String> = adapt.iter().enumerate().map(|(i, by_ref)| if *by_ref { format!("*__a{i}") } else { format!("__a{i}") }).collect();
                        let body = format!("|{}| ({f})({})", ps.join(", "), cs.join(", "));
                        // the parameter is `&impl Fn`. The adapter is a temporary, so the call borrows it; a place
                        // of the original closure stays a borrow inside the adapter and is not moved (7.9, D-181).
                        return if a.mode == Mode::Read && fn_read_borrowed(t) { format!("&({body})") } else { body };
                    }
                }
            }
            // a closure passed for `fn(T) U` takes each non-trivial (or generic) parameter by reference
            self.closure_refs = Some(params.iter().enumerate().map(|(i, p)| fn_param_mode(param_modes, i) == Mode::Read && !trivial(p, self.prog)).collect());
            self.closure_generic = Some(params.iter().enumerate().map(|(i, p)| fn_param_mode(param_modes, i) == Mode::Read && matches!(strip(p), Type::TypeParam(_))).collect());
            // a read-mode plain `fn` borrows (`&impl Fn`). `fn mut`, `fn once` and `owned` take the value.
            let want = if a.mode == Mode::Editable {
                Want::RefMut
            } else if a.mode == Mode::Read && fn_read_borrowed(t) {
                Want::Ref
            } else {
                Want::Value
            };
            let r = self.expr(&a.value, want);
            self.closure_refs = None;
            self.closure_generic = None;
            return r;
        }
        // an optional read parameter is `Option<&T>` (D-2)
        if a.mode == Mode::Read {
            if let Some(inner) = opt_read_inner(t, self.prog) {
                return self.opt_read_arg(&a.value, &inner);
            }
        }
        // a `Shared[T]` value where a `T` is read (a method's receiver, a plain parameter): read through the handle (7.8)
        if a.mode == Mode::Read && pty.map_or(false, |pt| name_of(pt) != Some("Shared")) && name_of(&a.value.ty) == Some("Shared") {
            let v = self.expr(&a.value, Want::Any);
            return if trivial(t, self.prog) { format!("*{v}.read()") } else { format!("&*{v}.read()") };
        }
        match a.mode {
            Mode::Owned => self.expr(&a.value, Want::Value),
            Mode::Editable => self.expr(&a.value, Want::RefMut),
            // a generic parameter is a reference in the generated signature (`x: &T`), whatever the instantiation
            Mode::Read if matches!(strip(t), Type::TypeParam(_)) && pty.is_some() => {
                // a read `String` parameter is a `&str`, but a `T` instantiated with `String` is read as `&String`:
                // the text is copied (outside `noalloc`, which keeps the plain loan)
                if let ExprKind::Place(p, _) = &a.value.kind {
                    if is_string(&a.value.ty, self.prog) && p.proj.is_empty() && self.forms[p.local as usize] != Form::Owned && !self.f.has("noalloc") {
                        return format!("&{}.to_string()", self.expr(&a.value, Want::Str));
                    }
                }
                // A function item has no `PartialEq` or `Show`, so it cannot instantiate `T`. Coerce it to a
                // function pointer, which has both and is still called as `T` (8.4).
                if let Some(fp) = fn_pointer_ty(strip(&a.value.ty), self.prog) {
                    let v = self.expr(&a.value, Want::Any);
                    return format!("&({v} as {fp})");
                }
                if matches!(&a.value.kind, ExprKind::Place(p, _) if p.proj.is_empty() && self.forms[p.local as usize] == Form::OptRef) {
                    // the `Option<&T>` itself, lent (D-2)
                    format!("&{}", self.expr(&a.value, Want::Any))
                } else if matches!(a.value.kind, ExprKind::Place(..)) {
                    self.expr(&a.value, Want::Ref)
                } else {
                    // a temporary (a literal, a call): its owned value, lent for the call
                    format!("&({})", self.expr(&a.value, Want::Value))
                }
            }
            // a value converted to `dynamic I` for a read parameter is lent as `&dyn I`: boxing it would pass a
            // `&Box<dyn I>`, which Rust unsizes (needing `Box<dyn I>: I`) instead of dereferencing (D-148)
            Mode::Read if matches!(strip(t), Type::Dynamic(_)) && matches!(a.value.kind, ExprKind::ToDynamic { .. }) => {
                let ExprKind::ToDynamic { value, .. } = &a.value.kind else { unreachable!() };
                let Type::Dynamic(n) = strip(t) else { unreachable!() };
                format!("({} as &dyn {})", self.expr(value, Want::Ref), dyn_name(n, self.prog))
            }
            // a `dynamic I` value (a box, a lent `&dyn I`, an element of a list of them) lent where `&dyn I` is read:
            // borrowed as the trait object, whichever it is (D-148)
            Mode::Read if matches!(strip(t), Type::Dynamic(_)) && matches!(strip(&a.value.ty), Type::Dynamic(_)) => {
                let Type::Dynamic(n) = strip(t) else { unreachable!() };
                let Type::Dynamic(have) = strip(&a.value.ty).clone() else { unreachable!() };
                let r = self.expr(&a.value, Want::Ref);
                if have.name == n.name {
                    format!("::std::borrow::Borrow::<dyn {}>::borrow({r})", dyn_name(n, self.prog))
                } else {
                    // a `dynamic B` read as the `dynamic A` that `B` requires: borrowed as `dyn B`, then upcast
                    format!("(::std::borrow::Borrow::<dyn {}>::borrow({r}) as &dyn {})", dyn_name(&have, self.prog), dyn_name(n, self.prog))
                }
            }
            Mode::Read => {
                // the parameter's type decides the passing form (the argument's may be a refinement or a literal's)
                let decide = if fortis_types::types::known(t) && !matches!(strip(t), Type::TypeParam(_)) { t.clone() } else { a.value.ty.clone() };
                if trivial(&decide, self.prog) || loan_is_value(&decide, self.prog) {
                    self.expr(&a.value, Want::Value)
                } else if is_string(&decide, self.prog) {
                    self.expr(&a.value, Want::Str)
                } else {
                    self.expr(&a.value, Want::Ref)
                }
            }
        }
    }

    fn call(&mut self, callee: &Callee, args: &[Arg], ty: &Type, line: u32) -> String {
        match callee {
            Callee::Intrinsic(name) => intrinsics::call(self, name, args, ty, line),
            Callee::Func(name) => {
                let g = match self.prog.function(name) {
                    Some(g) => g,
                    None => return self.err(line, format!("call of `{name}`, which the program lacks")),
                };
                let rust = match self.fn_names.get(name) {
                    Some(n) => n.clone(),
                    None => return self.err(line, format!("no Rust name for `{name}`")),
                };
                // arguments in parameter order; when one is an exclusive loan, the others are evaluated first into
                // temporaries (the reservation of 5.7: taking a loan has no effect, so source order is kept)
                let ordered = self.ordered_args(g, args, line);
                // a generic result lent from the receiver (`(alias T from self)?`) of a trivial instantiation is read by value
                let generic_loan = |t: &Type| matches!(t, Type::Loan { inner, .. } if matches!(**inner, Type::TypeParam(_)));
                let adapt = match &g.ret {
                    Type::Optional(i) if generic_loan(i) && trivial(ty, self.prog) => ".copied()",
                    t if generic_loan(t) && trivial(ty, self.prog) => "__deref",
                    _ => "",
                };
                if adapt == ".copied()" {
                    return format!("{rust}({}).copied()", ordered.join(", "));
                }
                if adapt == "__deref" {
                    return format!("(*{rust}({}))", ordered.join(", "));
                }
                let editable = g.params.iter().any(|p| p.mode == Mode::Editable);
                if editable && ordered.len() > 1 {
                    let mut pre = Vec::new();
                    let mut final_args = Vec::new();
                    for (i, (a, p)) in ordered.iter().zip(g.params.iter()).enumerate() {
                        if p.mode == Mode::Editable || a.parse::<f64>().is_ok() || a.ends_with("i64") && a.trim_end_matches("i64").parse::<i64>().is_ok() {
                            final_args.push(a.clone());
                        } else if p.mode == Mode::Read && fn_read_borrowed(&g.locals[p.local as usize].ty) {
                            if let Some(owned) = borrowed_temporary(a) {
                                // `let __a = &{closure}` drops the closure at the semicolon (E0716). Own it here
                                // and lend it at the call, so the caller's block still destroys it (7.9, D-181).
                                pre.push(format!("let __owned{i} = {owned};"));
                                final_args.push(format!("&__owned{i}"));
                            } else {
                                pre.push(format!("let __a{i} = {a};"));
                                final_args.push(format!("__a{i}"));
                            }
                        } else {
                            pre.push(format!("let __a{i} = {a};"));
                            final_args.push(format!("__a{i}"));
                        }
                    }
                    if pre.is_empty() {
                        return format!("{rust}({})", final_args.join(", "));
                    }
                    let call = format!("{rust}({})", final_args.join(", "));
                    // the future borrows these temporaries. Returning them from this block would drop them
                    // before the await (E0597); the await expression holds the lets for the poll.
                    if self.await_depth > 0 && g.has("async") {
                        self.await_lets.extend(pre);
                        return call;
                    }
                    return format!("{{ {} {call} }}", pre.join(" "));
                }
                format!("{rust}({})", ordered.join(", "))
            }
            Callee::Virtual { method } => {
                // through a type parameter: the bound's trait, named, so neither another bound's method of the same
                // name nor a Rust trait's can capture the call (D-82)
                if let Some(Type::TypeParam(p)) = args.first().map(|r| strip(&r.value.ty).clone()) {
                    if let Some(code) = self.bound_call(&p, method, args, ty) {
                        return code;
                    }
                }
                let recv = match args.first() {
                    // a method of the value behind a `Shared` handle is dispatched on that value (7.8)
                    Some(r) if name_of(&r.value.ty) == Some("Shared") => format!("(*{}.read())", self.expr(&r.value, Want::Any)),
                    Some(r) => self.expr(&r.value, Want::Any),
                    None => return self.err(line, "a virtual call without a receiver"),
                };
                // the trait takes each argument as the interface declares its parameter, in its order; an async
                // method of a `dyn` value is its boxed-future twin
                let declared = match args.first().map(|r| strip(&r.value.ty)) {
                    Some(Type::Dynamic(n)) => self.declared_sig(&n.name, method),
                    _ => None,
                };
                let rest = match &declared {
                    Some(ms) => self.declared_args(ms, &args[1..]),
                    None => args[1..].iter().map(|a| self.arg(a, None)).collect(),
                };
                let name = match &declared {
                    Some(ms) if ms.modifiers.iter().any(|m| m == "async") => format!("{}__dyn", ident(method)),
                    _ => ident(method),
                };
                format!("{recv}.{name}({})", rest.join(", "))
            }
            Callee::Value(f) => {
                // `move f(…)` of an async closure: the future owns the closure and its temporary arguments (X-250, X-251)
                let consuming_async = matches!(f.kind, ExprKind::Place(_, Use::Move)) && matches!(strip(&f.ty), Type::Fn { modifiers, .. } if modifiers.iter().any(|m| m == "async"));
                if consuming_async {
                    // `owning_launch` moves the closure into the future. Record that before it returns, so a
                    // block-exit drop does not move the same local again (E0382). `expr` below uses `Want::Any`,
                    // which does not record a move.
                    if let ExprKind::Place(p, Use::Move) = &f.kind {
                        if p.proj.is_empty() && !self.manual.contains(&p.local) {
                            self.moved.insert(p.local);
                        }
                    }
                    if let Some(code) = crate::intrinsics::owning_launch(self, callee, args, false, line) {
                        return code;
                    }
                }
                let fc = self.expr(f, Want::Any);
                // a closure call's arguments are not coerced to its parameters: the function type gives their forms
                let ptys: Vec<Type> = match strip(&f.ty) {
                    Type::Fn { params, .. } if params.len() == args.len() => params.clone(),
                    _ => vec![],
                };
                let rest: Vec<String> = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| match ptys.get(i) {
                        Some(pt) if a.mode == Mode::Read && opt_read_inner(pt, self.prog).is_some() => self.arg(a, Some(pt)),
                        _ => self.arg(a, None),
                    })
                    .collect();
                if matches!(f.kind, ExprKind::Place(_, Use::Move)) {
                    // `move f(…)` consumes the closure (8.2): it is destroyed when the call returns, after the call's
                    // owned parameters, even when Rust infers `Fn` or `FnMut` for it and would only borrow it (7.9,
                    // X-250; D-30). An async closure's call hands it to its future, which owns it and its arguments
                    // until the body has run. The `let __f = f` below moves the local; record that so a block-exit
                    // drop of a closure that owns a cleanup does not move it again (E0382).
                    if let ExprKind::Place(p, _) = &f.kind {
                        if p.proj.is_empty() && !self.manual.contains(&p.local) {
                            self.moved.insert(p.local);
                        }
                    }
                    let is_async = matches!(strip(&f.ty), Type::Fn { modifiers, .. } if modifiers.iter().any(|m| m == "async"));
                    if is_async {
                        let binds: Vec<String> = rest.iter().enumerate().map(|(i, a)| format!("let __a{i} = {a};")).collect();
                        let pass: Vec<String> = (0..rest.len()).map(|i| format!("__a{i}")).collect();
                        return format!("{{ #[allow(unused_mut)] let mut __f = {fc}; {} async move {{ __f({}).await }} }}", binds.join(" "), pass.join(", "));
                    }
                    return format!("{{ #[allow(unused_mut)] let mut __f = {fc}; __f({}) }}", rest.join(", "));
                }
                format!("({fc})({})", rest.join(", "))
            }
        }
    }

    /// The declared signature of an interface method (the program's interfaces, D-82): `Self` and the interface's
    /// parameters stay type parameters, so an argument is passed as the trait takes it.
    fn declared_sig(&self, iface: &str, method: &str) -> Option<MethodSig> {
        for x in iface_closure(self.prog, iface) {
            if let Some(d) = self.prog.type_def(&x) {
                if is_user_iface(self.prog, &x) {
                    if let Some(ms) = d.sigs.iter().find(|s| s.name == method) {
                        return Some(ms.clone());
                    }
                }
            }
        }
        None
    }

    /// The arguments of a call through a trait, in the declared parameters' order (a named argument may come in
    /// any order), each passed as its declared type is.
    fn declared_args(&mut self, ms: &MethodSig, args: &[Arg]) -> Vec<String> {
        let by_name = ms.params.iter().all(|(n, _, _)| args.iter().filter(|a| a.param == *n).count() <= 1) && args.iter().all(|a| ms.params.iter().any(|(n, _, _)| *n == a.param));
        if by_name {
            ms.params.iter().filter_map(|(n, _, t)| args.iter().find(|a| a.param == *n).map(|a| (a.clone(), t.clone()))).collect::<Vec<_>>().into_iter().map(|(a, t)| self.arg(&a, Some(&t))).collect()
        } else {
            args.iter().enumerate().map(|(k, a)| { let t = ms.params.get(k).map(|(_, _, t)| t.clone()); self.arg(a, t.as_ref()) }).collect()
        }
    }

    /// A call through a type parameter `p`: the trait of the bound interface that declares the method, by path; or
    /// a core interface's comparison through `PartialEq`/`PartialOrd`, which every implementation honours (D-82).
    fn bound_call(&mut self, p: &str, method: &str, args: &[Arg], ty: &Type) -> Option<String> {
        let bounds = self.f.bounds.clone();
        let recv = |g: &mut Self| match args[0].mode {
            Mode::Read => g.expr(&args[0].value, Want::Ref),
            Mode::Editable => g.expr(&args[0].value, Want::RefMut),
            Mode::Owned => g.expr(&args[0].value, Want::Value),
        };
        for (_, i, _) in bounds.iter().filter(|(bp, _, _)| bp == p) {
            for x in iface_closure(self.prog, i) {
                if !is_user_iface(self.prog, &x) {
                    continue;
                }
                let ms = match self.prog.type_def(&x).and_then(|d| d.sigs.iter().find(|s| s.name == method && s.receiver.is_some()).cloned()) {
                    Some(ms) => ms,
                    None => continue,
                };
                let r = recv(self);
                let mut all = vec![r];
                all.extend(self.declared_args(&ms, &args[1..]));
                return Some(format!("{}::{}({})", ident(&x), ident(method), all.join(", ")));
            }
        }
        let tp = Type::TypeParam(p.to_string());
        match (method, args.len()) {
            ("equals", 2) => {
                let r = recv(self);
                let o = self.arg(&args[1], Some(&tp));
                Some(format!("::std::cmp::PartialEq::eq({r}, {o})"))
            }
            ("compare", 2) => {
                let r = recv(self);
                let o = self.arg(&args[1], Some(&tp));
                let ord = rust_ty(ty, self.prog);
                let (less, equal, greater) = (ident("Less"), ident("Equal"), ident("Greater"));
                Some(format!(
                    "(match ::std::cmp::PartialOrd::partial_cmp({r}, {o}) {{ Some(::std::cmp::Ordering::Less) => {ord}::{less}, Some(::std::cmp::Ordering::Greater) => {ord}::{greater}, _ => {ord}::{equal} }})"
                ))
            }
            _ => None,
        }
    }

    fn ordered_args(&mut self, g: &Function, args: &[Arg], line: u32) -> Vec<String> {
        let mut out = Vec::new();
        for p in &g.params {
            let pty = g.locals[p.local as usize].ty.clone();
            match args.iter().find(|a| a.param == p.name) {
                Some(a) => {
                    let mut a2 = a.clone();
                    a2.mode = p.mode;
                    out.push(self.arg(&a2, Some(&pty)));
                }
                None => match &p.default {
                    Some(d) => {
                        let a2 = Arg { param: p.name.clone(), mode: p.mode, value: d.clone() };
                        out.push(self.arg(&a2, Some(&pty)));
                    }
                    None => out.push(self.err(line, format!("missing argument `{}`", p.name))),
                },
            }
        }
        out
    }

    fn closure(&mut self, func: &str, captures: &[Expr], line: u32) -> String {
        let lam = match self.prog.function(func) {
            Some(l) => l,
            None => return self.err(line, format!("lambda `{func}` missing")),
        };
        let cdef = lam.closure.clone().unwrap_or(ClosureDef { kind: "fn".into(), captures: vec![] });
        let mut pre = Vec::new();
        let explicit = cdef.captures.iter().any(|c| c.mode != CaptureMode::Implicit);
        // each capture's Rust name and form inside the closure: an implicit one is the enclosing binding itself
        // (Rust captures it by reference); a loan is bound before a `move` closure; the captures the closure owns
        // (`copy`, `move`, bare) are the fields of one tuple, in capture-list order. The body names the whole tuple
        // first, so the closure takes all of it (Rust 2021 would take only the fields the body uses and drop the rest
        // where the closure is made), and destroying the closure drops the fields in order (7.9, X-250; D-30).
        self.labels += 1;
        let env = format!("__env{}", self.labels);
        let mut owned: Vec<String> = Vec::new();
        let mut plan: Vec<(LocalId, String, Form)> = Vec::new();
        // `move t` of a binding that owns its `??` fallback (`let t = empty.get(0) ?? move d`): the closure takes the
        // binding's holder as a field of the env tuple, so it owns that value and destroys it when the consuming
        // call ends, or in index order inside a tuple the body moved it into (7.9; D-378). Inside the body the
        // capture is the holder's reference, as the binding was outside (D-367); a real loan stays a reference.
        let mut held_caps: Vec<(LocalId, usize)> = Vec::new();
        for (cd, v) in cdef.captures.iter().zip(captures) {
            match cd.mode {
                CaptureMode::Implicit => {
                    if let ExprKind::Place(p, _) = &v.kind {
                        plan.push((cd.local, self.names[p.local as usize].clone(), self.forms[p.local as usize]));
                    }
                }
                CaptureMode::Alias | CaptureMode::Editable => {
                    let n = format!("cap_{}", ident(&cd.name));
                    // a loan of an optional read parameter is its `Option<&T>` (D-2)
                    let opt_ref = matches!(&v.kind, ExprKind::Place(p, _) if p.proj.is_empty() && self.forms[p.local as usize] == Form::OptRef);
                    let (init, form) = if opt_ref {
                        (self.expr(v, Want::Any), Form::OptRef)
                    } else if cd.mode == CaptureMode::Alias {
                        (self.expr(v, Want::Ref), Form::Ref)
                    } else {
                        (self.expr(v, Want::RefMut), Form::RefMut)
                    };
                    pre.push(format!("let {n} = {init};"));
                    plan.push((cd.local, n, form));
                }
                CaptureMode::Move if matches!(&v.kind, ExprKind::Place(p, Use::Move) if p.proj.is_empty() && self.held.contains_key(&p.local)) => {
                    let ExprKind::Place(p, _) = &v.kind else { unreachable!() };
                    self.moved.insert(p.local);
                    held_caps.push((cd.local, owned.len()));
                    owned.push(self.held[&p.local].clone());
                }
                _ => {
                    plan.push((cd.local, format!("{env}.{}", owned.len()), Form::Owned));
                    owned.push(self.expr(v, Want::Value));
                }
            }
        }
        if !owned.is_empty() {
            pre.push(format!("let mut {env} = ({},);", owned.join(", ")));
        }
        let start_label = self.labels + 1000;
        let as_future = std::mem::take(&mut self.closure_as_future);
        let async_lambda = lam.has("async");
        let mut sub = FnGen::new(self.prog, lam, self.audit, self.fn_names);
        sub.labels = start_label;
        sub.box_result = matches!(strip(&lam.ret), Type::Fn { .. });
        if as_future || async_lambda {
            sub.is_async = true;
        }
        for (l, n, form) in plan {
            sub.names[l as usize] = n;
            sub.forms[l as usize] = form;
        }
        // a held capture keeps its own name, bound to the holder's reference at the top of the body (D-378)
        let mut held_binds: Vec<String> = Vec::new();
        for (l, i) in &held_caps {
            let holder = format!("{env}.{i}");
            sub.forms[*l as usize] = Form::Ref;
            sub.held.insert(*l, holder.clone());
            held_binds.push(format!("let {} = &*{holder};", sub.names[*l as usize]));
        }
        let refs = self.closure_refs.take();
        let generic = self.closure_generic.take();
        let mut params: Vec<String> = Vec::new();
        for (i, p) in lam.params.iter().enumerate() {
            let t = lam.locals[p.local as usize].ty.clone();
            let by_ref = refs.as_ref().and_then(|r| r.get(i).copied()).unwrap_or(false);
            let is_generic = generic.as_ref().and_then(|r| r.get(i).copied()).unwrap_or(false);
            let pt = match p.mode {
                Mode::Owned => rust_ty(&t, self.prog),
                Mode::Editable => format!("&mut {}", rust_ty(&t, self.prog)),
                Mode::Read if is_generic && !trivial(&t, self.prog) => {
                    sub.forms[p.local as usize] = Form::Ref;
                    format!("&{}", rust_ty(&t, self.prog))
                }
                Mode::Read if by_ref && trivial(&t, self.prog) => {
                    sub.forms[p.local as usize] = Form::Ref;
                    format!("&{}", rust_ty(&t, self.prog))
                }
                Mode::Read => {
                    if opt_read_inner(&t, self.prog).is_some() {
                        sub.forms[p.local as usize] = Form::OptRef;
                    } else if !trivial(&t, self.prog) {
                        sub.forms[p.local as usize] = Form::Ref;
                    }
                    param_ty_in_fn(&t, Mode::Read, self.prog)
                }
            };
            params.push(format!("{}: {pt}", sub.names[p.local as usize]));
        }
        let ret = if lam.is_try() { format!("rt::Outcome<{}>", rust_ty(&lam.ret, self.prog)) } else { rust_ty(&lam.ret, self.prog) };
        // a closure declares no lifetimes: a view `from` a capture is returned with its lifetime elided
        let ret = elide_lifetimes(&ret);
        let body = match &lam.body {
            Some(b) => sub.fn_body(b),
            None => "todo!()".into(),
        };
        let body = if owned.is_empty() { body } else { format!("let _ = &{env};\n{}{body}", held_binds.iter().map(|b| format!("{b}\n")).collect::<String>()) };
        let (sub_labels, sub_errors) = (sub.labels, std::mem::take(&mut sub.errors));
        drop(sub);
        self.labels = sub_labels;
        self.errors.extend(sub_errors);
        // implicit captures are by reference; a `fn` closure over trivial values only takes copies (it may escape)
        let all_trivial = cdef.captures.iter().all(|c| trivial(&lam.locals[c.local as usize].ty, self.prog));
        let mv = if explicit || (cdef.kind == "fn" && all_trivial) { "move " } else { "" };
        let closure = if as_future {
            // blocking work as a future: the closure's body runs when the scheduler reaches the task
            format!("async move {{\n{}\n}}", indent(&body, 1))
        } else if async_lambda {
            format!("async {mv}|{}| -> rt::task::Async<{ret}> {{\n{}\n}}", params.join(", "), indent(&body, 1))
        } else {
            format!("{mv}|{}| -> {ret} {{\n{}\n}}", params.join(", "), indent(&body, 1))
        };
        if pre.is_empty() {
            closure
        } else {
            format!("{{\n{}\n{}\n}}", indent(&pre.join("\n"), 1), indent(&closure, 1))
        }
    }

    // ---- blocks and statements --------------------------------------------------------------------------------

    /// A function body: its statements, then `Ok(())` when a `try` function can fall off the end.
    pub fn fn_body(&mut self, b: &Block) -> String {
        // a branch-moved parameter is shadowed for the whole body: the caller's value moves into the wrapper once
        let mut heads = Vec::new();
        for p in &self.f.params {
            if self.manual.contains(&p.local) && self.forms[p.local as usize] == Form::Owned {
                let n = self.names[p.local as usize].clone();
                heads.push(format!("let mut {n} = Some({n});"));
            }
        }
        let mut s = self.block_stmts(b);
        if !heads.is_empty() {
            s = format!("{}\n{s}", heads.join("\n"));
        }
        let falls = !crate::terminates(b);
        if falls {
            // the function's own exit: its live owned parameters with a cleanup are destroyed last (7.9)
            let tail = self.exit_code(0, false);
            if !tail.is_empty() {
                s.push_str(&format!("\n{tail}"));
            }
        }
        if falls && (self.is_try || self.is_async) {
            s.push_str(&format!("\n#[allow(unreachable_code)]\n{}", self.wrap_ok("()")));
        }
        s
    }

    /// A block used as a value (`give`): a labelled block. `ty` is the block's value type (a give into an
    /// optional is wrapped).
    fn value_block_typed(&mut self, b: &Block, ty: &Type) -> String {
        let l = self.label();
        self.give.push(l.clone());
        self.give_ty.push(ty.clone());
        self.give_depth.push(self.defers.len());
        let s = self.block_stmts(b);
        self.give_depth.pop();
        self.give_ty.pop();
        self.give.pop();
        format!("{l}: {{\n{}\n}}", indent(&s, 1))
    }

    fn value_block(&mut self, b: &Block) -> String {
        self.value_block_typed(b, &fortis_types::types::unknown(""))
    }

    pub fn block_stmts(&mut self, b: &Block) -> String {
        self.abandonable.push(vec![]);
        self.cleanups.push(vec![]);
        self.discards.push(vec![]);
        // a deferred block runs at every exit of its block once its `defer` was reached, inlined there (C.1)
        let flags: Vec<String> = b.defers.iter().map(|_| format!("__defer{}", self.next_id())).collect();
        self.defers.push(flags.iter().map(|f| (f.clone(), None)).collect());
        let scope = self.pending_scope.take();
        self.scopes.push(scope.clone());
        self.quiet.push(false);
        let mut out: Vec<String> = flags.iter().map(|f| format!("let mut {f} = false;")).collect();
        if let Some(sv) = &scope {
            out.insert(0, format!("let {sv} = rt::task::Scope::new();"));
            // the block's own locals that do not depend on the task set are declared before it, in their order, so
            // that Rust drops them after it (a task may borrow them; the scope joins its tasks before the block's
            // locals go, 12.5, 7.9) (D-10). A local whose initialiser reads the set, or a local that does, stays after
            // it: a task handle, a closure or a loan of the set.
            // A local stays after the set only when its value may hold a loan of the set: its initialiser reads the
            // set (or such a local) and its type can hold a loan (a task, a future, a closure, a loan or a view). An
            // owned result (`let b = await t`) holds none (X-137), so it is declared early like the rest, and the
            // block's owners keep their reverse initialisation order (7.9).
            // `depends`: the locals whose values come from the set (transitively, through an alias declared early);
            // `bound`: those of them declared after it
            let mut depends: Vec<LocalId> = Vec::new();
            let mut bound: Vec<LocalId> = Vec::new();
            for s in &b.stmts {
                let (binds, init) = match &s.kind {
                    StmtKind::Let { local, init } => (vec![*local], init),
                    StmtKind::LetPattern { pattern, init } => {
                        let mut v = Vec::new();
                        collect_binds(pattern, &mut v);
                        (v, init)
                    }
                    _ => continue,
                };
                let mut read = Vec::new();
                crate::locals_read(init, &mut read);
                let reads_set = read.iter().any(|l| self.names[*l as usize] == *sv || depends.contains(l));
                if reads_set {
                    depends.extend(binds.iter().copied());
                }
                if reads_set && binds.iter().any(|l| may_hold_loan(&self.local_ty(*l).clone())) {
                    bound.extend(binds);
                } else if matches!(s.kind, StmtKind::Let { .. }) {
                    self.deferred_lets.extend(binds);
                } else {
                    self.deferred_binds.extend(binds);
                }
            }
        }
        let decls_at = self.deferred_decls.len();
        for s in &b.stmts {
            if let StmtKind::Defer(i) = &s.kind {
                let body = self.block(&b.defers[*i]);
                let depth = self.defers.len() - 1;
                self.defers[depth][*i].1 = Some(body);
                out.push(format!("{} = true;", flags[*i]));
                continue;
            }
            out.push(self.stmt(s));
            match &s.kind {
                StmtKind::Let { local, init } => {
                    let local = *local;
                    let t = self.local_ty(local).clone();
                    // a user class implementing Abandonable (a task is cancelled and joined by its scope instead, 12.4),
                    // held by its owner: a local that holds a loan of one (`let r = alias tx`) abandons nothing (D-338)
                    let ab = self.forms[local as usize] == Form::Owned
                        && name_of(&t).filter(|n| *n != "Task").and_then(|n| self.prog.type_def(n)).map_or(false, |d| d.implements.iter().any(|i| i == "Abandonable"));
                    if ab {
                        self.abandonable.last_mut().unwrap().push(local);
                    }
                    self.note_cleanup_local(local);
                    // a closure's type is a function, so it does not own a cleanup itself. An owned capture does.
                    // Dropping the closure in its initialisation slot destroys that capture before an earlier local
                    // (7.9); leaving it to Rust's scope end runs it after those explicit drops.
                    self.note_closure_cleanup(local, init);
                    // a binding that holds its `??` fallback is destroyed with its block too, when it owns it (D-380)
                    if self.held.contains_key(&local) {
                        let ls = self.cleanups.last_mut().unwrap();
                        if !ls.contains(&local) {
                            ls.push(local);
                        }
                    }
                }
                StmtKind::LetPattern { pattern, .. } => {
                    // once: the same binding used to be registered again below, so a cleanup local was dropped
                    // twice (E0382) and traced twice
                    for l in pattern_locals(pattern) {
                        self.note_cleanup_local(l);
                    }
                }
                _ => {}
            }
        }
        // the block's normal end runs its own registered deferred blocks (unless control never reaches it)
        let depth = self.defers.len() - 1;
        if !b.terminates() {
            let tail = self.exit_code(depth, false);
            if !tail.is_empty() {
                out.push(tail);
            }
        }
        let decls: Vec<String> = self.deferred_decls.drain(decls_at..).collect();
        for (i, d) in decls.into_iter().enumerate() {
            out.insert(i, d);
        }
        self.defers.pop();
        self.scopes.pop();
        self.quiet.pop();
        self.abandonable.pop();
        self.cleanups.pop();
        self.discards.pop();
        out.join("\n")
    }

    /// The future `await x` polls; its output is `rt::task::Async<T>`.
    fn awaitable(&mut self, x: &Expr) -> String {
        let fut = self.awaited_future(x);
        if name_of(&x.ty) == Some("Task") {
            return fut;
        }
        // inside open scopes, the root's idle pass runs their children while this await waits (12.3); the
        // enclosing scope wraps outermost, so its children run first
        let open: Vec<String> = self.scopes.iter().flatten().cloned().collect();
        open.iter().rev().fold(fut, |f, sv| format!("{sv}.waiting({f})"))
    }

    fn awaited_future(&mut self, x: &Expr) -> String {
        if let ExprKind::Call { callee: Callee::Intrinsic(n), args } = &x.kind {
            match n.as_str() {
                "join" => {
                    // each future is traced under its function's name, as a launched task is (12.5)
                    let a: Vec<String> = args
                        .iter()
                        .map(|a| {
                            let fname = match &a.value.kind {
                                ExprKind::Call { callee: Callee::Func(f), .. } => self.prog.function(f).map_or("work".to_string(), source_name),
                                _ => "work".to_string(),
                            };
                            format!("{fname:?}, {}", self.expr(&a.value, Want::Value))
                        })
                        .collect();
                    return format!("rt::task::join2({})", a.join(", "));
                }
                "Clock.sleep" => {
                    let d = args.get(1).map(|a| self.expr(&a.value, Want::Value)).unwrap_or_default();
                    return format!("rt::task::sleep(({d}).0)");
                }
                "Mutex.lock" => {
                    let m = args.first().map(|a| self.expr(&a.value, Want::Any)).unwrap_or_default();
                    return format!("std::future::ready(Ok::<_, rt::task::Cancelled>({m}.lock_blocking()))");
                }
                _ => {}
            }
        }
        if name_of(&x.ty) == Some("Task") {
            return format!("{}.join()", self.expr(x, Want::Value));
        }
        format!("rt::task::awaited({})", self.expr(x, Want::Value))
    }

    /// A success value as the function returns it: `Ok` for `try`, and again `Ok` for `async` (whose `Err` is the
    /// cancellation exit).
    pub fn wrap_ok(&self, v: &str) -> String {
        let v = if self.is_try { format!("Ok({v})") } else { v.to_string() };
        if self.is_async {
            format!("Ok({v})")
        } else {
            v
        }
    }

    /// A failure leaving the function: `return Err(f)`, inside `Ok` for an async function.
    pub fn fail_ret(&self, f: &str) -> String {
        if self.is_async {
            format!("return Ok(Err({f}))")
        } else {
            format!("return Err({f})")
        }
    }

    fn next_id(&mut self) -> usize {
        self.labels += 1;
        self.labels
    }

    /// What leaving the open blocks from `from` inward runs, innermost first: each block's registered deferred
    /// blocks in reverse order, then, when unwinding a failure or cancellation, `abandon` on its live Abandonable
    /// locals in reverse initialisation order (5.1, 12.4). A deferred block runs as cleanup (12.1).
    pub fn exit_code(&mut self, from: usize, unwinding: bool) -> String {
        let mut out = Vec::new();
        for depth in (from..self.defers.len()).rev() {
            // a scope's exit: its unjoined tasks are cancelled and joined, before the block's deferred blocks (12.5)
            let scope = self.scopes.get(depth).cloned().flatten();
            let registered: Vec<&String> = self.defers[depth].iter().filter(|(_, b)| b.is_some()).map(|(f, _)| f).collect();
            let mut noisy: Vec<String> = registered.iter().map(|f| f.to_string()).collect();
            if let Some(sv) = &scope {
                noisy.push(format!("{sv}.has_tasks()"));
            }
            // a live Abandonable (when unwinding) or a live local whose own class has a cleanup also ends loans
            // observably. A wrapper that only holds such a value is dropped with the block but does not itself
            // end loans (the interpreter traces `loans-end` for the class that declares cleanup). Depth 0 is the
            // function itself, whose owned parameters are destroyed after its body block (7.9).
            // a match arm's bindings are quiet: they are traced when still live, and they are not a block, so
            // they do not end loans (7.9, 9.8)
            let quiet = self.quiet.get(depth).copied().unwrap_or(false);
            if !quiet {
                let live = |ls: Option<&Vec<LocalId>>, moved: &HashSet<LocalId>| ls.map_or(false, |v| v.iter().any(|l| !moved.contains(l)));
                // a manual local's flag, not the flow-insensitive `moved` set, says whether it is still there
                let declares = |l: &LocalId| crate::declares_cleanup(self.prog, &self.f.locals[*l as usize].ty);
                let own_live = self.cleanups.get(depth).map_or(false, |v| v.iter().any(|l| !self.manual.contains(l) && !self.held.contains_key(l) && self.forms[*l as usize] == Form::Owned && !self.moved.contains(l) && declares(l)));
                let manual_live: Vec<String> = self
                    .cleanups
                    .get(depth)
                    .map(|v| v.iter().filter(|l| self.manual.contains(l) && declares(l)).map(|l| format!("{}.is_some()", self.names[*l as usize])).collect())
                    .unwrap_or_default();
                // a binding that holds its `??` fallback owns a value only when the fallback was taken (D-380)
                let held_live: Vec<String> = self
                    .cleanups
                    .get(depth)
                    .map(|v| {
                        v.iter()
                            .filter(|l| self.held.contains_key(l) && !self.moved.contains(l) && crate::declares_cleanup(self.prog, loan_inner(&self.f.locals[**l as usize].ty)))
                            .map(|l| format!("matches!({}, rt::MaybeOwned::Owned(_))", self.held[l]))
                            .collect()
                    })
                    .unwrap_or_default();
                // an owned optional place ends loans observably only while it holds a value (7.9): its null is
                // silent, as the interpreter's cell of an absent value is
                let opt_live: Vec<String> = self
                    .cleanups
                    .get(depth)
                    .map(|v| {
                        v.iter()
                            .filter(|l| {
                                !self.manual.contains(l) && !self.held.contains_key(l) && !self.moved.contains(l) && !self.partial.contains(l)
                                    && self.forms[**l as usize] == Form::Owned
                                    && matches!(strip(&self.f.locals[**l as usize].ty), Type::Optional(i) if !i.is_loan() && crate::declares_cleanup(self.prog, i))
                            })
                            .map(|l| format!("{}.is_some()", self.names[*l as usize]))
                            .collect()
                    })
                    .unwrap_or_default();
                if depth > 0 && ((unwinding && live(self.abandonable.get(depth), &self.moved)) || own_live) {
                    noisy = vec!["true".into()];
                }
                // depth 0 is the function: an owned parameter is destroyed after the body, and that drop is not a
                // loans-end of the function itself. A branch-moved parameter is the same (7.9, D-181).
                if depth > 0 {
                    noisy.extend(manual_live);
                    noisy.extend(held_live);
                    noisy.extend(opt_live);
                }
                if !noisy.is_empty() {
                    out.push(format!("if {} {{ rt::trace::event(\"loans-end\"); }}", noisy.join(" || ")));
                }
            }
            if let Some(sv) = &scope {
                out.push(format!("let _ = {sv}.finish().await;"));
            }
            for (flag, body) in self.defers[depth].iter().rev() {
                if let Some(b) = body {
                    out.push(format!("if {flag} {{ rt::trace::event(\"defer\"); rt::records::enter_cleanup(); {b} rt::records::leave_cleanup(); }}"));
                }
            }
            if unwinding && depth < self.abandonable.len() {
                for l in self.abandonable[depth].iter().rev() {
                    let t = self.f.locals[*l as usize].ty.clone();
                    let Some(n) = name_of(&t) else { continue };
                    if self.manual.contains(l) {
                        let name = self.names[*l as usize].clone();
                        out.push(format!("if let Some(__held) = {name}.take() {{ rt::trace::event(\"abandon {n}\"); {}::abandon(__held); }}", ident(n)));
                        continue;
                    }
                    if self.moved.contains(l) || self.partial.contains(l) {
                        continue;
                    }
                    out.push(format!("rt::trace::event(\"abandon {n}\"); {}::abandon({});", ident(n), self.names[*l as usize]));
                }
            }
            if let Some(sv) = &scope {
                // the scope ends here: its futures may borrow locals of its body
                out.push(format!("drop({sv});"));
            }
            // the block's live owned locals that own a cleanup (their class, or a field or payload of it) are
            // destroyed next, in reverse initialisation order (7.9). The trace names a class that itself declares
            // cleanup, as the interpreter's does; a field's cleanup is the value's own Drop, not a second event.
            // Explicit, not Rust's scope end: an early exit from a nested block must destroy the inner block's
            // locals before the outer block's deferred blocks run (D-181). The move also keeps Rust from dropping
            // the value a second time when the scope ends.
            let ls = self.cleanups.get(depth).cloned().unwrap_or_default();
            for l in ls.iter().rev() {
                // a binding that holds its `??` fallback destroys that value here, in its turn and traced as the
                // interpreter does, when it owns it; a loan it holds ends and destroys nothing (D-367, D-380)
                if let Some(holder) = self.held.get(l).cloned() {
                    if self.moved.contains(l) || self.partial.contains(l) {
                        continue;
                    }
                    let inner = loan_inner(&self.f.locals[*l as usize].ty).clone();
                    let trace = crate::cleanup_name(self.prog, &inner).map(|n| format!("rt::trace::event(\"drop {n}\"); ")).unwrap_or_default();
                    out.push(format!("if let rt::MaybeOwned::Owned(__v) = {holder} {{ {trace}drop(__v); }}"));
                    continue;
                }
                if self.forms[*l as usize] != Form::Owned {
                    continue;
                }
                let abandoned = unwinding && self.abandonable.get(depth).map_or(false, |a| a.contains(l));
                if self.manual.contains(l) {
                    if abandoned {
                        continue;
                    }
                    let name = self.names[*l as usize].clone();
                    let mut body = String::new();
                    if let Some(n) = crate::cleanup_name(self.prog, &self.f.locals[*l as usize].ty) {
                        body.push_str(&format!("rt::trace::event(\"drop {n}\"); "));
                    }
                    body.push_str("drop(__held);");
                    out.push(format!("if let Some(__held) = {name}.take() {{ {body} }}"));
                    continue;
                }
                if self.moved.contains(l) || self.partial.contains(l) || abandoned {
                    continue;
                }
                // an owned optional place is destroyed by taking the value it holds: the drop is announced for
                // that value alone, as the interpreter announces the cell's instance alone, and its null drops
                // in silence (7.9)
                if let Type::Optional(inner) = strip(&self.f.locals[*l as usize].ty) {
                    if !inner.is_loan() {
                        let name = self.names[*l as usize].clone();
                        let mut body = String::new();
                        if let Some(n) = crate::cleanup_name(self.prog, inner) {
                            body.push_str(&format!("rt::trace::event(\"drop {n}\"); "));
                        }
                        body.push_str("drop(__held);");
                        out.push(format!("if let Some(__held) = {name}.take() {{ {body} }}"));
                        continue;
                    }
                }
                if let Some(n) = crate::cleanup_name(self.prog, &self.f.locals[*l as usize].ty) {
                    out.push(format!("rt::trace::event(\"drop {n}\");"));
                }
                out.push(format!("drop({});", self.names[*l as usize]));
            }
            // fields matched by `_` follow the named bindings, in declaration order, with no `drop` trace:
            // destroying a field through the consumed value is not a second drop of a local (7.9, 9.8)
            if let Some(ds) = self.discards.get(depth).cloned() {
                out.extend(ds);
            }
        }
        out.join(" ")
    }

    /// `abandon` for each live Abandonable local of the open blocks, innermost and latest first (12.4).
    fn abandon_live(&mut self) -> String {
        self.exit_code(0, true)
    }

    fn block(&mut self, b: &Block) -> String {
        format!("{{\n{}\n}}", indent(&self.block_stmts(b), 1))
    }

    /// A slice returned as a view of a text parameter (`StringView from text`): borrowed, not copied (7.6).
    fn text_view(&mut self, e: &Expr) -> Option<String> {
        let inner = match &e.kind {
            ExprKind::OrFail(inner) => inner,
            _ => return None,
        };
        match &inner.kind {
            ExprKind::Call { callee: Callee::Intrinsic(n), args } if (n == "StringView.slice" || n == "String.slice") && args.len() == 3 => {
                let a: Vec<String> = args.iter().map(|x| self.arg(x, None)).collect();
                Some(format!("rt::text::slice_view({}, {}, {})?", a[0], a[1], a[2]))
            }
            _ => None,
        }
    }

    /// A returned expression that is already `Option<&str>`: a call of a function, method or closure whose own
    /// result is an optional loan of text (its signature is concrete), or a read of an optional read parameter
    /// (D-2's `Option<&str>`); anything else (an intrinsic, a generic prelude call, a binding of one, `null`, an
    /// `if`, `match` or value block of them) is `Option<&String>` and bridges in `ret` (D-353).
    fn answers_opt_text(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Call { callee: Callee::Func(name), .. } => self.prog.function(name).map_or(false, |g| opt_loan_text(&g.ret, self.prog)),
            ExprKind::Call { callee: Callee::Value(head), .. } => matches!(&head.ty, Type::Fn { ret, .. } if opt_loan_text(ret, self.prog)),
            ExprKind::Place(p, _) if p.proj.is_empty() && self.forms[p.local as usize] == Form::OptRef => true,
            _ => false,
        }
    }

    fn ret(&mut self, v: Option<&Expr>) -> String {
        let by_ref = matches!(&self.f.ret, Type::Loan { .. }) && !loan_is_value(&self.f.ret, self.prog);
        // a returned exclusive loan is the `&mut` itself, reborrowed from the editable parameter or receiver it
        // comes from (`&mut *c`): the signature is `&mut T`, which a read reference does not match (D-185)
        let by_mut = by_ref && matches!(&self.f.ret, Type::Loan { mode, .. } if mode == "editable");
        let text_loan = matches!(&self.f.ret, Type::Loan { inner, .. } if is_string(inner, self.prog));
        // a byte string returned as a bytes view is the static slice itself (D-390): the signature is `&[u8]`,
        // which the literal's `&[..]` is, where the owned `vec!` would be a reference to a temporary
        let bytes_view = matches!(&self.f.ret, Type::Loan { inner, .. } if name_of(inner) == Some("BytesView"));
        let view = match v {
            Some(e) if by_ref && text_loan => self.text_view(e),
            _ => None,
        };
        let mut val = match (view, v) {
            (Some(code), _) => code,
            (None, Some(e)) if bytes_view && matches!(&e.kind, ExprKind::Const(Const::Bytes(_))) => {
                let ExprKind::Const(Const::Bytes(b)) = &e.kind else { unreachable!() };
                format!("&{b:?}")
            }
            (None, Some(e)) => self.expr(e, if by_mut { Want::RefMut } else if by_ref { Want::Ref } else { Want::Value }),
            (None, None) => "()".into(),
        };
        // an optional loan of text is `Option<&str>` in the signature; a value that is not already one bridges
        // here (D-353). A `null` needs no bridge: its `None` is the option the signature says, and a bare `None`
        // gives the bridge's generic referent nothing to infer (D-390)
        if let Some(e) = v {
            if opt_loan_text(&self.f.ret, self.prog) && !self.answers_opt_text(e) && !matches!(e.kind, ExprKind::Const(Const::Null)) {
                val = format!("rt::text::opt_view({val})");
            }
        }
        if self.box_result && v.is_some() {
            // any callable boxes to the closure's `Box<dyn Fn…>` result (an already boxed one, as a `Box<Box<…>>`)
            val = format!("Box::new({val})");
            self.audit.boxed_results += 1;
        }
        // an outcome returned from a function that is not `try` (a woven handler chain): a failure there is
        // outside the checked language (the interpreter's dynamic error)
        if let Some(e) = v {
            if !self.is_try && name_of(&e.ty) == Some("Outcome") && name_of(&self.f.ret) != Some("Outcome") {
                val = format!("{val}.unwrap_or_else(|f| rt::dynamic(&format!(\"`fail` in a function without `try` ({{}})\", f.code)))");
            }
        }
        let drops = self.pull_drops();
        let pre = self.exit_code(0, false);
        let wrapped = self.wrap_ok("__ret");
        if !pre.is_empty() || !drops.is_empty() {
            return format!("{{ let __ret = {val}; {drops} {pre} return {wrapped}; }}");
        }
        format!("return {};", self.wrap_ok(&val))
    }

    fn stmt(&mut self, s: &Stmt) -> String {
        let mark = self.hoist.len();
        let drops = self.temp_drops(s, &rust_moved_temps(s));
        self.transfer_drops.push(drops.clone());
        let saved = self.in_transfer;
        self.in_transfer = false;
        let body = self.stmt_body(s);
        let transferred = self.in_transfer;
        self.in_transfer = saved;
        self.transfer_drops.pop();
        // the drop text is built before the body lowers, and an `or recover`'s holder slot is known only once
        // the body has lowered its temporary (D-446): rebuilt here, the drop of that slot announces a made
        // replacement only (7.9)
        let drops = if s.temps.iter().any(|t| self.mo_temps.contains(t)) { self.temp_drops(s, &rust_moved_temps(s)) } else { drops };
        let inits_all = self.hoist.split_off(mark);
        let owners = self.hoist_owners.split_off(mark);
        let mut inits = Vec::new();
        let mut bubble_l = Vec::new();
        let mut bubble_o = Vec::new();
        for (owner, line) in owners.into_iter().zip(inits_all) {
            if s.temps.contains(&owner) {
                inits.push(line);
            } else {
                bubble_o.push(owner);
                bubble_l.push(line);
            }
        }
        self.hoist_owners.extend(bubble_o);
        self.hoist.extend(bubble_l);
        if inits.is_empty() && (transferred || drops.is_empty()) {
            return body;
        }
        let mut parts = inits;
        if !body.is_empty() {
            parts.push(body);
        }
        if !transferred && !drops.is_empty() {
            parts.push(drops);
        }
        parts.join("\n")
    }

    fn stmt_body(&mut self, s: &Stmt) -> String {
        let line = s.line;
        match &s.kind {
            StmtKind::Let { local, init } => {
                self.launch_detachable = self.detachable.contains(local);
                let t = self.local_ty(*local).clone();
                let editable = matches!(&t, Type::Loan { mode, .. } if mode == "editable") || matches!(&init.kind, ExprKind::Place(_, Use::Editable));
                let value_loan = !editable && (trivial(&t, self.prog) || loan_is_value(&t, self.prog));
                let form = match &init.kind {
                    _ if value_loan => Form::Owned,
                    // a loan of an optional read parameter is its `Option<&T>` (D-2)
                    ExprKind::Place(p, Use::Alias) if p.proj.is_empty() && self.forms[p.local as usize] == Form::OptRef => Form::OptRef,
                    ExprKind::Place(_, Use::Alias) => Form::Ref,
                    ExprKind::Place(_, Use::Editable) => Form::RefMut,
                    _ if matches!(&t, Type::Loan { mode, .. } if mode == "editable") => Form::RefMut,
                    _ if matches!(&t, Type::Loan { .. }) => Form::Ref,
                    _ => Form::Owned,
                };
                self.forms[*local as usize] = form;
                self.held.remove(local);
                // a binding of a lent `??` that can give a value of its own names that value's holder beside it, so a
                // move of the binding into a tuple moves what it owns with it (7.9; D-367). The binding is the
                // holder's reference, as the statement's temporary was.
                let holds = match &init.kind {
                    ExprKind::Coalesce(a, b) => self.holds_fallback(a, b, &init.ty),
                    _ => recovery_holder_inner(init, self.prog).is_some_and(|t| !is_string(&t, self.prog)),
                };
                if form == Form::Ref && !self.f.locals[*local as usize].mutable && !self.deferred_lets.contains(local) && holds {
                    let held = match &init.kind {
                        ExprKind::Coalesce(a, b) => self.lent_coalesce(a, b, &init.ty),
                        _ => self.recovery_holder(init),
                    };
                    let holder = format!("__held{local}");
                    let name = self.names[*local as usize].clone();
                    self.held.insert(*local, holder.clone());
                    self.note_future(*local, init);
                    return format!("let {holder} = {held}; let {name} = &*{holder};");
                }
                // a binding moved from such a binding (`let q = move t`) takes the holder with it, so the value is
                // destroyed with the new binding's block, in its order (7.9; D-380)
                if let ExprKind::Place(p, Use::Move) = &init.kind {
                    if p.proj.is_empty() && form == Form::Ref && !self.f.locals[*local as usize].mutable && !self.deferred_lets.contains(local) {
                        if let Some(src) = self.held.get(&p.local).cloned() {
                            self.moved.insert(p.local);
                            let holder = format!("__held{local}");
                            let name = self.names[*local as usize].clone();
                            self.held.insert(*local, holder.clone());
                            self.note_future(*local, init);
                            return format!("let {holder} = {src}; let {name} = &*{holder};");
                        }
                    }
                }
                let v = match form {
                    Form::Owned if matches!(&init.kind, ExprKind::Const(Const::Int(_))) || matches!(&init.kind, ExprKind::Unary(UnOp::Neg, x) if matches!(x.kind, ExprKind::Const(Const::Int(_)))) => {
                        let n = match &init.kind {
                            ExprKind::Const(Const::Int(n)) => *n,
                            ExprKind::Unary(_, x) => match &x.kind {
                                ExprKind::Const(Const::Int(n)) => -n,
                                _ => 0,
                            },
                            _ => 0,
                        };
                        format!("({n}{})", int_suffix(&t))
                    }
                    Form::Owned => self.expr_as(init, &t),
                    Form::Ref => self.expr(init, Want::Ref),
                    Form::RefMut => self.expr(init, Want::RefMut),
                    Form::OptRef => self.expr(init, Want::Any),
                };
                // an owned binding is declared `mut`: a method that edits its receiver needs it (lint allowed)
                let mutable = form == Form::Owned;
                let manual = form == Form::Owned && self.manual.contains(local);
                let stored = if manual { format!("Some({v})") } else { v.clone() };
                // an owned binding carries its type (it settles numeric literals and empty collections). A manual
                // local's type is `Option<T>`, which the annotation would not name. A generic whose use lost its
                // arguments is annotated from the initialiser, or left to inference: `: Chain` is not a type (D-55).
                let ann_src = annotation_type(&t, &init.ty, self.prog);
                let ann = if !manual && form == Form::Owned && annotatable(&ann_src, self.prog) && !matches!(init.kind, ExprKind::Closure { .. }) && spelled(self.prog, &ann_src) {
                    format!(": {}", self.ty(&ann_src))
                } else {
                    String::new()
                };
                self.note_future(*local, init);
                let name = self.names[*local as usize].clone();
                if let Some(i) = self.deferred_lets.iter().position(|l| l == local) {
                    // declared before its scope's task set (D-10): here it is initialised. The flag starts false so
                    // an exit generated before this statement, which does not yet list the local, is the only exit
                    // that could see it uninitialised; this statement's own exits are later.
                    self.deferred_lets.remove(i);
                    if manual {
                        self.deferred_decls.push(format!("let mut {name} = None;"));
                        return format!("{name} = {stored};");
                    }
                    self.deferred_decls.push(format!("let {}{name}{ann};", if mutable { "mut " } else { "" }));
                    return format!("{name} = {stored};");
                }
                format!("let {}{name}{ann} = {stored};", if mutable || manual { "mut " } else { "" })
            }
            StmtKind::LetPattern { pattern, init } => {
                let v = self.expr(init, Want::Value);
                let (at, guards_at) = (self.arm_prelude.len(), self.arm_guards.len());
                // bindings declared before their scope's task set (D-10) are bound here under fresh names, then moved in
                let mut binds = Vec::new();
                collect_binds(pattern, &mut binds);
                let early: Vec<(LocalId, String)> = binds
                    .iter()
                    .filter(|l| self.deferred_binds.contains(l))
                    .map(|l| (*l, std::mem::replace(&mut self.names[*l as usize], format!("__dp{l}"))))
                    .collect();
                self.deferred_binds.retain(|l| !binds.contains(l));
                self.guard_prelude.clear();
                self.pat_guards.clear();
                self.pat_ref = false;
                self.arm_discards.clear();
                self.discard_ord.clear();
                // a partial pattern drops a discarded owning part at the end of the scope; name it and drop it
                // here, which is when the destructuring releases it (7.9, D-305). The drop is not traced: the
                // part is not a block local. A fallback that still matches `_` records the same drop.
                let mut discards = Vec::new();
                let p = self.let_pat(pattern, &init.ty, &mut discards);
                let mut extra = std::mem::take(&mut self.arm_discards);
                extra.sort_by(|a, b| a.0.cmp(&b.0));
                let extra_drops: Vec<String> = extra.into_iter().map(|(_, s)| s).collect();
                let mut moves = Vec::new();
                for (l, name) in early {
                    let tmp = std::mem::replace(&mut self.names[l as usize], name.clone());
                    self.deferred_decls.push(format!("let mut {name};"));
                    moves.push(format!("{name} = {tmp};"));
                }
                if !moves.is_empty() {
                    self.arm_prelude.push(moves.join(" "));
                }
                // a boxed class's contents are destructured after the binding (D-3); a `let` pattern is irrefutable.
                // a boxed enum field's extractions follow the binding (D-55). A let has no guard, so the selection
                // fragments are dropped and only the extractions run.
                let mut prelude: Vec<String> = self.arm_prelude.drain(at..).collect();
                self.arm_guards.truncate(guards_at);
                self.boxed_locals.clear();
                self.last_box = None;
                self.guard_prelude.clear();
                self.pat_guards.clear();
                self.guard_as_ref.clear();
                self.in_guard = false;
                for l in pattern_locals(pattern) {
                    if self.manual.contains(&l) {
                        let n = self.names[l as usize].clone();
                        prelude.push(format!("let mut {n} = Some({n});"));
                    }
                }
                let mut lines = vec![format!("let {p} = {v};")];
                lines.extend(discards);
                lines.extend(prelude);
                // the `Some` wrap of a fallback `_` is in the prelude, so its take runs after that wrap
                lines.extend(extra_drops);
                lines.join("\n")
            }
            StmtKind::Assign { place, value, drop_old } => {
                if place.proj.is_empty() {
                    self.note_future(place.local, value);
                }
                if place.proj.is_empty() && self.manual.contains(&place.local) && self.forms[place.local as usize] == Form::Owned {
                    let v = self.expr(value, Want::Value);
                    let name = self.names[place.local as usize].clone();
                    // the replacement is built while the old value is still held: the right-hand side may read
                    // this place (7.9). The old value is destroyed after that, then the new one is stored.
                    // Assignment does not emit `drop Name`; the interpreter traces that for a block exit or an
                    // owned parameter, and the cleanup method still runs.
                    let drop_prev = if *drop_old { format!("if let Some(__held) = {name}.take() {{ drop(__held); }} ") } else { String::new() };
                    return format!("{{ let __new = {v}; {drop_prev}{name} = Some(__new); }}");
                }
                let v = self.expr(value, Want::Value);
                format!("{} = {v};", self.lvalue(place))
            }
            StmtKind::Compound { place, op, value } => {
                // `x op= e` is `x = x op e` (LR 1.1), evaluated left to right (5.7): `x` is read into a temporary
                // before `e` runs (Rust's `x op= e` on primitives evaluates `e` first), and the result is stored
                // into the place `x` names after `e` has run (D-60). The operands are numbers, so the read copies.
                let code = self.place(place);
                let read = if place.proj.is_empty() && self.forms[place.local as usize] != Form::Owned { format!("(*{code})") } else { code };
                let lv = self.lvalue(place);
                let v = self.expr(value, Want::Value);
                // a signed `%=` is the language's remainder, as `%` is (D-240)
                if *op == BinOp::Rem && matches!(rust_ty(&value.ty, self.prog).as_str(), "i64" | "i32" | "i16" | "i8") {
                    return format!("{{ let __lhs = {read}; {lv} = __lhs.wrapping_rem({v}); }}");
                }
                format!("{{ let __lhs = {read}; {lv} = __lhs {} {v}; }}", op.symbol())
            }
            StmtKind::Expr(e) => {
                // a statement temporary's value is created by evaluating this expression (the hoist is only
                // `let mut slot = None`), so the expression has to be emitted (5.7, 7.9)
                let c = self.expr(e, Want::Any);
                if name_of(&e.ty) == Some("Never") && matches!(e.kind, ExprKind::Call { .. }) {
                    format!("{c};\nunreachable!(\"a call of type Never returned\");")
                } else {
                    format!("{c};")
                }
            }
            StmtKind::Discard(e) => {
                // a discarded statement temporary fills its hoist and is not moved out; the statement drops it (7.9)
                if matches!(e.kind, ExprKind::Temp { .. }) {
                    format!("let _ = {};", self.expr(e, Want::Any))
                } else if let ExprKind::Place(p, u) = &e.kind {
                    // `discard place` reads the place (7.2): the binding still owns the value and drops it at block
                    // exit. Emitting the owned name would move it, so a later block-exit drop is a second move
                    // (E0382) and the value would die at this statement, before a later use of the block (D-181).
                    // `discard move place` does consume it now, as a temporary of this statement (7.9).
                    if p.proj.is_empty() && self.forms[p.local as usize] == Form::Owned && !trivial(&e.ty, self.prog) {
                        let name = self.names[p.local as usize].clone();
                        let manual = self.manual.contains(&p.local);
                        if *u == Use::Move {
                            let mut trace = String::new();
                            if let Some(n) = crate::cleanup_name(self.prog, &self.f.locals[p.local as usize].ty) {
                                trace = format!("rt::trace::event(\"drop {n}\"); ");
                            }
                            if manual {
                                return format!("if let Some(__held) = {name}.take() {{ {trace}drop(__held); }}");
                            }
                            self.moved.insert(p.local);
                            return format!("{{ {trace}drop({name}); }}");
                        }
                        if matches!(u, Use::Read | Use::Alias) {
                            if manual {
                                return format!("let _ = {name}.as_ref();");
                            }
                            return format!("let _ = &{name};");
                        }
                    }
                    format!("let _ = {};", self.expr(e, Want::Any))
                } else {
                    format!("let _ = {};", self.expr(e, Want::Any))
                }
            }
            StmtKind::Drop(p) => {
                // A manual local is `Option`: `.take()` drops it only when a value is present, and the trace
                // sits in that arm so an empty slot announces nothing (7.9, D-181). A place already in `moved`
                // was destroyed by that move, so it is not dropped again. One trace per destroyed value.
                let ty = self.place_ty(p);
                let trace = self.cleanup_trace(&ty).map(|n| format!("rt::trace::event(\"drop {n}\"); ")).unwrap_or_default();
                if p.proj.is_empty() && self.manual.contains(&p.local) {
                    let name = self.names[p.local as usize].clone();
                    format!("if let Some(__held) = {name}.take() {{ {trace}drop(__held); }}")
                } else if self.moved.contains(&p.local) {
                    String::new()
                } else {
                    let code = self.place_expr(p, Use::Move, &ty, Want::Value, line);
                    format!("{{ {trace}drop({code}); }}")
                }
            }
            StmtKind::If { cond, then, els } => {
                let c = self.expr(cond, Want::Value);
                let a = self.block(then);
                match els {
                    Some(b) => {
                        let b = self.block(b);
                        format!("if {c} {a} else {b}")
                    }
                    None => format!("if {c} {a}"),
                }
            }
            StmtKind::Loop { label, body } => {
                let rl = self.loop_label(label);
                let l = format!("{rl}: ");
                self.loops.push((label.clone(), self.defers.len(), rl));
                let b = self.block(body);
                self.loops.pop();
                format!("{l}loop {b}")
            }
            StmtKind::For { label, binding, source, body } => {
                let rl = self.loop_label(label);
                let l = format!("{rl}: ");
                let var = binding.map_or("_".to_string(), |b| self.names[b as usize].clone());
                let src = match source {
                    ForSource::Range { lo, hi, inclusive } => {
                        if let Some(b) = binding {
                            self.forms[*b as usize] = Form::Owned;
                        }
                        format!("{}{}{}", self.expr(lo, Want::Value), if *inclusive { "..=" } else { ".." }, self.expr(hi, Want::Value))
                    }
                    ForSource::Items(e) => {
                        let kind = name_of(&e.ty).unwrap_or("");
                        // a cursor is what `iterate()` of a class that implements `Iterable` gave the loop (5.3, D-545):
                        // it lends its items, so the loop owns none of them
                        let moving = kind != "ListCursor" && (matches!(e.kind, ExprKind::Place(_, Use::Move)) || !matches!(e.kind, ExprKind::Place(..)));
                        if let Some(b) = binding {
                            let et = self.local_ty(*b).clone();
                            self.forms[*b as usize] = if moving || trivial(&et, self.prog) { Form::Owned } else { Form::Ref };
                        }
                        // a statement temporary moves into the iterator (`Want::Value`). The statement does not
                        // drop it again (`rust_moved_temps`)
                        let src = self.expr(e, if moving || matches!(e.kind, ExprKind::Temp { .. }) { Want::Value } else { Want::Any });
                        let triv = binding.map_or(true, |b| trivial(&self.local_ty(b).clone(), self.prog));
                        match (kind, moving) {
                            ("ListCursor", _) if triv => format!("{src}.copied()"),
                            ("ListCursor", _) => src,
                            ("Map", true) => format!("{src}.into_entries()"),
                            ("Set", true) => format!("{src}.into_items()"),
                            ("Map", false) | ("Set", false) if triv => format!("{src}.iter().cloned()"),
                            (_, true) => src,
                            (_, false) if triv => format!("{src}.iter().copied()"),
                            _ => format!("{src}.iter()"),
                        }
                    }
                };
                self.loops.push((label.clone(), self.defers.len(), rl));
                let b = self.block(body);
                self.loops.pop();
                // a binding moved on one path is `Option` (D-181). The iterator yields `T`, so the body wraps it
                // before any `.take()`. The wrapper encloses the body's drops.
                let b = if let Some(id) = binding {
                    if self.manual.contains(id) && self.forms[*id as usize] == Form::Owned {
                        format!("{{\nlet mut {var} = Some({var});\n{b}\n}}")
                    } else {
                        b
                    }
                } else {
                    b
                };
                format!("{l}for {var} in {src} {b}")
            }
            StmtKind::Break(l) | StmtKind::Continue(l) => {
                let word = if matches!(s.kind, StmtKind::Break(_)) { "break" } else { "continue" };
                let target = match l {
                    Some(name) => self.loops.iter().rev().find(|(x, _, _)| x.as_deref() == Some(name.as_str())).map(|(_, d, r)| (*d, r.clone())),
                    None => self.loops.last().map(|(_, d, r)| (*d, r.clone())),
                };
                let drops = self.pull_drops();
                let pre = target.as_ref().map(|(d, _)| self.exit_code(*d, false)).unwrap_or_default();
                let jump = match &target {
                    Some((_, r)) => format!("{word} {r};"),
                    None => format!("{word};"),
                };
                if pre.is_empty() && drops.is_empty() {
                    jump
                } else {
                    format!("{{ {drops} {pre} {jump} }}")
                }
            }
            StmtKind::Return(v) => self.ret(v.as_ref()),
            StmtKind::Give(e) => {
                // a literal given into a borrowed text value is the static text itself (D-390), as a literal
                // recovered into one is; an owned text (`StringView` with no origin, or `static`) keeps the
                // owned `String::from`. A `present` into an optional borrowed text (`Option<&str>`, D-353) is
                // wrapped as the option the signature says, its value read as a `str`
                let give_opt_text = self.give_ty.last().map_or(false, |t| opt_loan_text(t, self.prog));
                let borrowed_text = give_opt_text || self.give_ty.last().map_or(false, |t| matches!(t, Type::Loan { inner, origin, .. } if is_string(inner, self.prog) && !origin.is_empty() && origin != "static"));
                let mut v = match &e.kind {
                    ExprKind::Const(Const::Str(s)) if borrowed_text => format!("{s:?}"),
                    ExprKind::Present(a) if give_opt_text => match &a.kind {
                        ExprKind::Const(Const::Str(s)) => format!("Some({s:?})"),
                        _ => format!("Some(AsRef::<str>::as_ref({}))", self.expr(a, Want::Str)),
                    },
                    _ => self.expr(e, Want::Value),
                };
                if let Some(t) = self.give_ty.last() {
                    if matches!(strip(t), Type::Optional(_)) && !matches!(strip(&e.ty), Type::Optional(_)) && !matches!(e.kind, ExprKind::Const(Const::Null)) && fortis_types::types::known(&e.ty) && name_of(&e.ty) != Some("Never") {
                        v = format!("Some({v})");
                    }
                }
                let drops = self.pull_drops();
                let pre = self.give_depth.last().copied().map(|d| self.exit_code(d, false)).unwrap_or_default();
                match self.give.last() {
                    Some(l) if pre.is_empty() && drops.is_empty() => format!("break {l} {v};"),
                    Some(l) => format!("{{ let __gv = {v}; {drops} {pre} break {l} __gv; }}"),
                    None => self.err(line, "`give` outside a value block"),
                }
            }
            StmtKind::Recover(e) => {
                // a literal recovered into a borrowed text value is the static text itself
                let borrowed_text = self.recover_ty.last().map_or(false, |t| matches!(t, Type::Loan { inner, .. } if is_string(inner, self.prog)) || name_of(t) == Some("StringView"));
                let v = if self.recover_held.last() == Some(&true) {
                    let ty = self.recover_ty.last().unwrap().clone();
                    self.holder_value(e, loan_inner(&ty))
                } else {
                    match &e.kind {
                        ExprKind::Const(Const::Str(s)) if borrowed_text => format!("{s:?}"),
                        _ => self.expr(e, Want::Value),
                    }
                };
                let drops = self.pull_drops();
                let pre = self.recover_depth.last().copied().map(|d| self.exit_code(d, false)).unwrap_or_default();
                match self.recover.last() {
                    Some(l) if pre.is_empty() && drops.is_empty() => format!("break {l} {v};"),
                    Some(l) => format!("{{ let __rv = {v}; {drops} {pre} break {l} __rv; }}"),
                    None => self.err(line, "`recover` outside a catch"),
                }
            }
            StmtKind::Fail(e) if fortis_types::types::known(&e.ty) && name_of(&e.ty) != Some("Failure") && !self.prog.type_def(name_of(&e.ty).unwrap_or("")).map_or(false, |d| matches!(d.kind, TypeKind::Error { .. })) => {
                let v = self.expr(e, Want::Any);
                format!("{{ let _ = {v}; rt::dynamic(\"fail of a non-failure value\") }}")
            }
            StmtKind::Fail(e) => {
                let v = self.expr(e, Want::Value);
                let drops = self.pull_drops();
                let ab = self.abandon_live();
                format!("{{ let __f = {v}; {drops} {ab} {}; }}", self.fail_ret("__f.into()"))
            }
            StmtKind::Block(b) => self.block(b),
            StmtKind::Match { scrutinee, arms } => format!("{};", self.match_(scrutinee, arms, false)),
            StmtKind::Capture { field, value } => {
                let v = self.expr(value, Want::Any);
                format!("__frame.retain(|(k, _)| *k != {field:?}); __frame.push(({field:?}, rt::Show::show(&{v})));")
            }
            StmtKind::Record { schema, fields } => {
                // written straight to the record stream in key order: an integer, a Boolean or text without a heap
                // allocation (11.4, C.3); any other value by its display
                let mut fs: Vec<(String, String)> = fields
                    .iter()
                    .map(|(n, v)| {
                        let base = name_of(&v.ty).unwrap_or("");
                        let val = if matches!(base, "Int" | "Bool") {
                            format!("&({}) as &dyn rt::records::Value", self.expr(v, Want::Value))
                        } else if is_string(&v.ty, self.prog) {
                            format!("&({}) as &dyn rt::records::Value", self.expr(v, Want::Str))
                        } else {
                            format!("&rt::records::Shown(&{}) as &dyn rt::records::Value", self.expr(v, Want::Any))
                        };
                        (n.clone(), val)
                    })
                    .collect();
                fs.sort_by(|a, b| a.0.cmp(&b.0));
                let items: Vec<String> = fs.iter().map(|(n, v)| format!("({n:?}, {v})")).collect();
                format!("rt::records::event_with({schema:?}, {}, &[{}]);", crate::schema_version(self.prog, schema), items.join(", "))
            }
            StmtKind::Report { value, bind } => {
                let v = self.expr(value, Want::Value);
                let mut s = format!("let __reported = {v}; rt::records::report(&__reported, \"report\");");
                if let Some(b) = bind {
                    s.push_str(&format!(" let {} = rt::Report {{ code: __reported.code() }};", self.names[*b as usize]));
                }
                s
            }
            // `defer` and `scope` lower with the asynchronous rows (DEL-39)
            StmtKind::Defer(_) => self.err(line, "a `defer` outside its block's statement list"),
            StmtKind::Scope { local, body } => {
                if !self.is_async {
                    return self.err(line, "`scope` outside an async function");
                }
                let name = self.names[*local as usize].clone();
                self.forms[*local as usize] = Form::Owned;
                self.pending_scope = Some(name);
                self.block(body)
            }
            StmtKind::Expect { cond, evidence: Some((_, sym, _)) } if matches!(cond.kind, ExprKind::Binary(..)) => {
                // a comparison's operands are evaluated once and compared through references; the evidence shows
                // those values (14; X-121)
                let (op, a, b) = match &cond.kind {
                    ExprKind::Binary(op, a, b) => (*op, &**a, &**b),
                    _ => unreachable!(),
                };
                let bind = |g: &mut Self, x: &Expr, name: &str| -> (String, String) {
                    if trivial(&x.ty, g.prog) {
                        (format!("let {name} = &({});", g.expr(x, Want::Value)), name.to_string())
                    } else {
                        (format!("let {name} = &({});", g.expr(x, Want::Ref)), format!("*{name}"))
                    }
                };
                let a_null = matches!(a.kind, ExprKind::Const(Const::Null));
                let b_null = matches!(b.kind, ExprKind::Const(Const::Null));
                let (la, ra) = if a_null { (String::new(), String::new()) } else { bind(self, a, "__ea") };
                let (lb, rb) = if b_null { (String::new(), String::new()) } else { bind(self, b, "__eb") };
                let (ao, bo) = (matches!(strip(&a.ty), Type::Optional(_)), matches!(strip(&b.ty), Type::Optional(_)));
                let cmp = if a_null || b_null {
                    let o = if a_null { &rb } else { &ra };
                    if op == BinOp::Eq { format!("{o}.is_none()") } else { format!("{o}.is_some()") }
                } else if ao != bo {
                    let (o, p) = if ao { (&ra, &rb) } else { (&rb, &ra) };
                    let pe = if ao { b } else { a };
                    if is_string(&pe.ty, self.prog) {
                        format!("({o}.as_ref().map(|v| rt::show::s(v)) {} Some(rt::show::s({p})))", op.symbol())
                    } else {
                        format!("({o}.as_ref() {} Some({p}))", op.symbol())
                    }
                } else if is_string(&a.ty, self.prog) || is_string(&b.ty, self.prog) {
                    format!("(rt::show::s({ra}) {} rt::show::s({rb}))", op.symbol())
                } else {
                    format!("(*{ra} {} *{rb})", op.symbol())
                };
                let show = |r: &str, null: bool| if null { "\"null\".to_string()".to_string() } else { format!("rt::Show::show({r})") };
                let ev = format!("format!(\"actual={{}} expected={sym} {{}}\", {}, {})", show(&ra, a_null), show(&rb, b_null));
                let fail = if self.f.closure.is_some() { format!("rt::expect_failed({ev});") } else { format!("return Err(TestFailure::Expect({ev}));") };
                format!("{{\n    {la}\n    {lb}\n    if !{cmp} {{\n        {fail}\n    }}\n}}")
            }
            StmtKind::Expect { cond, evidence } => {
                let c = self.expr(cond, Want::Value);
                let ev = match evidence {
                    Some((a, op, b)) => {
                        let x = self.expr(a, Want::Any);
                        let y = self.expr(b, Want::Any);
                        format!("format!(\"actual={{}} expected={op} {{}}\", rt::Show::show(&{x}), rt::Show::show(&{y}))")
                    }
                    None => "String::from(\"actual=false expected=true\")".into(),
                };
                // the evidence re-evaluates the operands on failure only, as the interpreter does (14); inside a closure
                // the failure unwinds to the test runner (test builds unwind), as the interpreter's exception does
                if self.f.closure.is_some() {
                    format!("if !({c}) {{\n    rt::expect_failed({ev});\n}}")
                } else {
                    format!("if !({c}) {{\n    return Err(TestFailure::Expect({ev}));\n}}")
                }
            }
        }
    }

    fn match_(&mut self, scrutinee: &Expr, arms: &[Arm], value: bool) -> String {
        self.match_typed(scrutinee, arms, value, &fortis_types::types::unknown(""))
    }

    fn match_typed(&mut self, scrutinee: &Expr, arms: &[Arm], value: bool, ty: &Type) -> String {
        let want_opt = matches!(strip(ty), Type::Optional(_));
        // a statement temporary is an `Option`. An arm that binds by value takes the value out (the statement's
        // `take` then finds `None`). A pattern that only discriminates (`One { h: _, .. }`) does not move in
        // Rust, so the value stays in the slot and the statement drops it (7.9).
        let (s, by_ref) = if let ExprKind::Temp { .. } = &scrutinee.kind {
            // an arm that binds by value moves the temporary out (`Want::Value`). A pattern that only
            // discriminates borrows the slot, and the statement drops it (7.9).
            if arms_bind_owned(arms) {
                (self.expr(scrutinee, Want::Value), false)
            } else {
                (self.expr(scrutinee, Want::RefMut), true)
            }
        } else {
            let place_move = matches!(scrutinee.kind, ExprKind::Place(_, Use::Move));
            let moving = place_move || !matches!(scrutinee.kind, ExprKind::Place(..));
            // `match move place` whose arms neither bind nor open the value is not a Rust move: a whole-value
            // `_` leaves the local in place, and the block drops and traces it (D-300). A pattern that opens
            // the value moves it, so a field matched by `_` drops with the arm (D-315). An arm that binds
            // moves the scrutinee, and a wildcard arm drops it there.
            let keep = place_move && !arms_bind_owned(arms) && !arms.iter().any(|a| pattern_opens(&a.pattern));
            let triv = trivial(&scrutinee.ty, self.prog);
            let strings = arms.iter().any(|a| matches!(a.pattern, Pattern::Literal(Const::Str(_))));
            let take = !keep && (moving || triv);
            let s = if strings {
                format!("rt::show::s(&{})", self.expr(scrutinee, Want::Any))
            } else if take {
                self.expr(scrutinee, Want::Value)
            } else {
                self.expr(scrutinee, Want::Ref)
            };
            let by_ref = !take || strings;
            (s, by_ref)
        };
        let mut out = Vec::new();
        // a match on an optional loan of a type parameter (`match items.get(i) { present(x) => … }`, `Option<&T>`) binds
        // `x` as the `&T` itself, so a comparison or a call reads it as the reference it is, not as `&&T` (X-577)
        let loan_of_param = !by_ref && matches!(&scrutinee.ty, Type::Optional(i) if matches!(&**i, Type::Loan { inner, .. } if matches!(&**inner, Type::TypeParam(_))));
        for a in arms {
            for l in &a.locals {
                let lt = self.local_ty(*l).clone();
                self.forms[*l as usize] = if by_ref && !trivial(&lt, self.prog) {
                    Form::Ref
                } else if by_ref || (loan_of_param && matches!(strip(&lt), Type::TypeParam(_))) {
                    Form::Ref
                } else {
                    Form::Owned
                };
            }
            self.arm_prelude.clear();
            self.arm_guards.clear();
            self.boxed_locals.clear();
            self.last_box = None;
            self.guard_prelude.clear();
            self.pat_guards.clear();
            self.guard_as_ref.clear();
            self.in_guard = false;
            self.pat_ref = by_ref;
            self.arm_discards.clear();
            self.discard_ord.clear();
            let mut p = self.pattern(&a.pattern, &scrutinee.ty);
            let mut discs = std::mem::take(&mut self.arm_discards);
            discs.sort_by(|a, b| a.0.cmp(&b.0));
            let disc_stmts: Vec<String> = discs.into_iter().map(|(_, s)| s).collect();
            for l in &a.locals {
                if self.manual.contains(l) && self.forms[*l as usize] == Form::Owned {
                    let n = self.names[*l as usize].clone();
                    // after the box prelude's `let name = *name`, so the wrapper holds `T` and the guard, which
                    // ran on the raw binding, is already done
                    self.arm_prelude.push(format!("let mut {n} = Some({n});"));
                }
            }
            let mut boxed_guards = std::mem::take(&mut self.arm_guards);
            let boxed_locals = std::mem::take(&mut self.boxed_locals);
            let last_box = self.last_box.take();
            if matches!(a.pattern, Pattern::Is { .. }) && name_of(&scrutinee.ty) == Some("Outcome") {
                // `is E(...)` over an outcome matches its failure
                let (bind, guard) = p.split_once(" if ").map(|(b, g)| (b.to_string(), g.to_string())).unwrap_or((p.clone(), String::new()));
                p = format!("Err({bind}) if {guard}");
            }
            let prelude = std::mem::take(&mut self.arm_prelude).join(" ");
            // a boxed enum binding is borrowed for the guard by the same dereference the arm's body uses (D-55)
            let gp = std::mem::take(&mut self.guard_prelude).join(" ");
            // a guard runs before the prelude: one that reads a boxed class's bindings runs inside the outermost box's
            // test, where they are bound by reference (D-3). A boxed enum payload, and any binding a guard
            // fragment introduced, is read as `&T` for that guard and restored before the arm body (D-55).
            let pgs = std::mem::take(&mut self.pat_guards);
            let noted = std::mem::take(&mut self.guard_as_ref);
            let reads_boxed = a.guard.as_ref().map_or(false, |g| {
                let mut used = Vec::new();
                crate::locals_read(g, &mut used);
                used.iter().any(|l| boxed_locals.contains(l))
            });
            // does not capture `self`: the guard expression borrows it on the next line
            fn mark_ref(forms: &mut [Form], ids: &[LocalId]) -> Vec<(LocalId, Form)> {
                let mut saved = Vec::new();
                for l in ids {
                    if !saved.iter().any(|(id, _)| id == l) {
                        saved.push((*l, forms[*l as usize]));
                        forms[*l as usize] = Form::Ref;
                    }
                }
                saved
            }
            let g = match (&a.guard, reads_boxed, &last_box) {
                (Some(g), true, Some((b, inner, nested, pushed))) if p == *b => {
                    let mut ids = boxed_locals.clone();
                    ids.extend(noted.iter().copied());
                    let saved = mark_ref(&mut self.forms, &ids);
                    let ug = self.expr(g, Want::Value);
                    for (l, f) in saved {
                        self.forms[l as usize] = f;
                    }
                    let ug = if gp.is_empty() { ug } else { format!("{{ {gp} ({ug}) }}") };
                    // `if let` rather than `match`: a record pattern is irrefutable, and its wildcard arm would be
                    // unreachable. An irrefutable `if let` is allowed in the generated crate.
                    let ug = nest_pat_guards(&ug, &pgs);
                    if *pushed {
                        boxed_guards.pop();
                    }
                    let mut conds: Vec<String> = nested.iter().map(|c| format!("({c})")).collect();
                    conds.push(format!("({ug})"));
                    boxed_guards.push(format!("matches!(&*{b}.0, {inner} if {})", conds.join(" && ")));
                    String::new()
                }
                (Some(g), true, _) => {
                    self.err(g.line, "an arm guard that reads a binding of a class pattern nested inside another pattern");
                    String::new()
                }
                (Some(g), false, _) => {
                    let saved = mark_ref(&mut self.forms, &noted);
                    let ug = self.expr(g, Want::Value);
                    for (l, f) in saved {
                        self.forms[l as usize] = f;
                    }
                    let ug = if gp.is_empty() { ug } else { format!("{{ {gp} ({ug}) }}") };
                    let cond = nest_pat_guards(&ug, &pgs);
                    format!(" if ({cond})")
                }
                (None, _, _) => {
                    if pgs.is_empty() {
                        String::new()
                    } else {
                        let cond = nest_pat_guards("true", &pgs);
                        format!(" if ({cond})")
                    }
                }
            };
            let g = if boxed_guards.is_empty() {
                if !g.is_empty() && p.contains(" if ") { g.replacen(" if ", " && ", 1) } else { g }
            } else {
                // the pattern's own condition (an `is` pattern), then the boxed contents', then the arm's guard
                let mut conds: Vec<String> = Vec::new();
                if let Some((pp, pg)) = p.clone().split_once(" if ") {
                    p = pp.to_string();
                    conds.push(format!("({pg})"));
                }
                conds.extend(boxed_guards.iter().map(|c| format!("({c})")));
                if let Some(ug) = g.strip_prefix(" if ") {
                    conds.push(format!("({ug})"));
                }
                format!(" if {}", conds.join(" && "))
            };
            // a by-value arm owns its bindings. They are a quiet frame: `return`, `break` and `fail` trace the
            // ones still live, and the fall-through does too. A binding the arm gives back is moved while the
            // body is lowered, and `exit_code` skips it. The frame is not a block, so it does not emit
            // `loans-end` (7.9, 9.8). A function value, and a value that only owns a cleanup through a field,
            // is dropped here too, before an outer `defer` (D-181).
            let mut arm_clean = Vec::new();
            if !by_ref {
                let mut binds = Vec::new();
                collect_binds(&a.pattern, &mut binds);
                for l in binds {
                    if self.forms[l as usize] == Form::Owned && self.explicit_drop_ty(self.local_ty(l)) {
                        arm_clean.push(l);
                    }
                }
            }
            // `_` fields are recorded on the same quiet frame and run after the named bindings (7.9, 9.8).
            // The frame is pushed before the body, so a `return` inside the arm destroys them first.
            let arm_depth = if by_ref || (arm_clean.is_empty() && disc_stmts.is_empty()) {
                None
            } else {
                let depth = self.defers.len();
                self.abandonable.push(vec![]);
                self.cleanups.push(arm_clean);
                self.discards.push(disc_stmts);
                self.defers.push(vec![]);
                self.scopes.push(None);
                self.quiet.push(true);
                Some(depth)
            };
            let body = match &a.body {
                ArmBody::Expr(e) => {
                    if value {
                        // a literal arm of a value match into a borrowed text is the static text itself
                        // (D-390), as a given one is; a `present` into an optional borrowed text is wrapped
                        // as the option the signature says, its value read as a `str`
                        let opt_text = opt_loan_text(ty, self.prog);
                        let borrowed_text = opt_text || matches!(ty, Type::Loan { inner, origin, .. } if is_string(inner, self.prog) && !origin.is_empty() && origin != "static");
                        let v = match &e.kind {
                            ExprKind::Const(Const::Str(s)) if borrowed_text => format!("{s:?}"),
                            ExprKind::Present(a) if opt_text => match &a.kind {
                                ExprKind::Const(Const::Str(s)) => format!("Some({s:?})"),
                                _ => format!("Some(AsRef::<str>::as_ref({}))", self.expr(a, Want::Str)),
                            },
                            _ => self.expr(e, Want::Value),
                        };
                        if want_opt && !matches!(strip(&e.ty), Type::Optional(_)) && !matches!(e.kind, ExprKind::Const(Const::Null)) && fortis_types::types::known(&e.ty) && name_of(&e.ty) != Some("Never") {
                            format!("Some({v})")
                        } else {
                            v
                        }
                    } else {
                        format!("{{ {}; }}", self.expr(e, Want::Any))
                    }
                }
                ArmBody::Block(b) => {
                    if value {
                        self.value_block_typed(b, ty)
                    } else {
                        self.block(b)
                    }
                }
            };
            // the fall-through drop is inside the prelude. A manual binding's `let mut name = Some(name)`
            // has to be in scope for that `.take()`, not only for the arm body (D-181). `exit_code` is generated
            // after the body, so a binding the arm moved is already in `moved` and is not dropped here.
            // A value arm keeps that result: the drops run after it is bound, and the prelude's `Option` wrap
            // stays outside this block so the `_` take can see it.
            let body = if let Some(depth) = arm_depth {
                let terminates = match &a.body {
                    ArmBody::Block(b) => b.terminates(),
                    ArmBody::Expr(e) => matches!(e.kind, ExprKind::Fail(_)) || matches!(&e.ty, Type::Named(n) if n.name == "Never"),
                };
                let tail = if terminates { String::new() } else { self.exit_code(depth, false) };
                self.quiet.pop();
                self.scopes.pop();
                self.defers.pop();
                self.discards.pop();
                self.cleanups.pop();
                self.abandonable.pop();
                if tail.is_empty() {
                    body
                } else if value {
                    format!("{{ let __ret = {{ {body} }}; {tail} __ret }}")
                } else {
                    format!("{{ {body} {tail} }}")
                }
            } else {
                body
            };
            let body = if prelude.is_empty() { body } else { format!("{{ {prelude} {body} }}") };
            out.push(format!("{p}{g} => {body},"));
        }
        // a match the checker proved exhaustive by its arms may still need a catch-all for Rust
        if !arms.iter().any(|a| matches!(a.pattern, Pattern::Wildcard | Pattern::Otherwise(_) | Pattern::Bind(_))) {
            out.push("#[allow(unreachable_patterns)]\n_ => unreachable!(\"match proved exhaustive\"),".into());
        }
        format!("match {s} {{\n{}\n}}", indent(&out.join("\n"), 1))
    }

    fn owns(&self, t: &Type) -> bool {
        crate::owns_cleanup(self.prog, t, &mut Vec::new())
    }

    /// `field` as it is inside `owner`: a generic class's field is declared as the parameter, and the value's
    /// type carries the argument (`Box[N]`'s `item: T` is `N`).
    fn instantiate(&self, owner: &Type, field: &Type) -> Type {
        let params = name_of(owner).and_then(|n| self.prog.type_def(n)).map(|d| d.type_params.clone()).unwrap_or_default();
        if params.is_empty() {
            return field.clone();
        }
        let map: Vec<(String, Type)> = params.into_iter().zip(args_of(owner)).collect();
        crate::subst_type(field, &map)
    }

    fn class_fields_inst(&self, ty_name: &str, owner: &Type) -> Vec<(String, Type)> {
        match self.prog.type_def(ty_name).map(|d| &d.kind) {
            Some(TypeKind::Class { fields }) => fields.iter().map(|f| (f.name.clone(), self.instantiate(owner, &f.ty))).collect(),
            _ => vec![],
        }
    }

    fn variant_fields_inst(&self, enum_ty: &str, variant: &str, owner: &Type) -> Vec<(String, Type)> {
        match self.prog.type_def(enum_ty).map(|d| &d.kind) {
            Some(TypeKind::Enum { variants }) => variants.iter().find(|v| v.name == variant).map(|v| v.fields.iter().map(|(n, ft)| (n.clone(), self.instantiate(owner, ft))).collect()).unwrap_or_default(),
            _ => vec![],
        }
    }

    fn discard_temp(&mut self, drops: &mut Vec<String>) -> String {
        let n = format!("__discard{}", self.next_id());
        drops.push(format!("std::mem::drop({n});"));
        n
    }

    fn with_parts(path: &str, parts: &[String]) -> String {
        if parts.is_empty() { format!("{path} {{ .. }}") } else { format!("{path} {{ {}, .. }}", parts.join(", ")) }
    }

    /// A `let` pattern. A discarded subtree that owns a cleanup is bound and dropped at this statement: Rust
    /// drops a wildcard of a partial pattern only at the end of the scope (7.9, D-305). A match pattern drops
    /// its `_` fields with the arm instead (`bind_wildcard`).
    fn let_pat(&mut self, p: &Pattern, t: &Type, drops: &mut Vec<String>) -> String {
        if pure_discard(p) && self.owns(t) {
            return self.discard_temp(drops);
        }
        match p {
            Pattern::Tuple(ps) => {
                let items = match strip(t) {
                    Type::Tuple(ts) => ts.clone(),
                    _ => vec![],
                };
                if items.len() != ps.len() {
                    return self.pattern(p, t);
                }
                let parts: Vec<String> = ps.iter().zip(items.iter()).map(|(sp, ft)| self.let_pat(sp, ft, drops)).collect();
                format!("({})", parts.join(", "))
            }
            Pattern::Class { ty, fields } => {
                if crate::recursive_class(self.prog, ty) {
                    return self.pattern(p, t);
                }
                let cfs = self.class_fields_inst(ty, t);
                if cfs.is_empty() || fields.iter().any(|(n, _)| !cfs.iter().any(|(x, _)| x == n)) {
                    return self.pattern(p, t);
                }
                let mut parts = Vec::new();
                for (n, ft) in &cfs {
                    if let Some((_, sp)) = fields.iter().find(|(x, _)| x == n) {
                        parts.push(format!("{}: {}", ident(n), self.let_pat(sp, ft, drops)));
                    } else if self.owns(ft) {
                        let d = self.discard_temp(drops);
                        parts.push(format!("{}: {d}", ident(n)));
                    }
                }
                Self::with_parts(&class_path(ty, self.prog), &parts)
            }
            Pattern::Variant { ty, variant, fields } => {
                let vt = self.variant_fields_inst(ty, variant, t);
                let boxed = fields.iter().any(|(n, _)| vt.iter().find(|(x, ft)| x == n).map_or(false, |(_, ft)| crate::boxes_field(self.prog, ty, ft)));
                if fields.is_empty() || vt.is_empty() || boxed || fields.iter().any(|(n, _)| !vt.iter().any(|(x, _)| x == n)) {
                    return self.pattern(p, t);
                }
                let mut parts = Vec::new();
                for (n, ft) in &vt {
                    if let Some((_, sp)) = fields.iter().find(|(x, _)| x == n) {
                        parts.push(format!("{}: {}", ident(n), self.let_pat(sp, ft, drops)));
                    } else if self.owns(ft) {
                        let d = self.discard_temp(drops);
                        parts.push(format!("{}: {d}", ident(n)));
                    }
                }
                Self::with_parts(&format!("{}::{}", ident(ty), ident(variant)), &parts)
            }
            Pattern::Record(fs) => {
                let shape = match strip(t) {
                    Type::Record(rf) => rf.clone(),
                    _ => vec![],
                };
                if shape.is_empty() || fs.iter().any(|(n, _)| !shape.iter().any(|(x, _)| x == n)) {
                    return self.pattern(p, t);
                }
                let mut parts = Vec::new();
                for (n, ft) in &shape {
                    if let Some((_, sp)) = fs.iter().find(|(x, _)| x == n) {
                        parts.push(format!("{}: {}", ident(n), self.let_pat(sp, ft, drops)));
                    } else if self.owns(ft) {
                        let d = self.discard_temp(drops);
                        parts.push(format!("{}: {d}", ident(n)));
                    }
                }
                Self::with_parts(&record_name(&shape), &parts)
            }
            Pattern::Present(i) => {
                let inner = match strip(t) {
                    Type::Optional(x) => (**x).clone(),
                    other => other.clone(),
                };
                format!("Some({})", self.let_pat(i, &inner, drops))
            }
            Pattern::Success(i) => {
                let inner = args_of(t).first().cloned().unwrap_or_else(|| t.clone());
                format!("Ok({})", self.let_pat(i, &inner, drops))
            }
            Pattern::Failure(i) => format!("Err({})", self.let_pat(i, &fortis_types::types::named("Failure"), drops)),
            _ => self.pattern(p, t),
        }
    }

    /// The declared type of a variant's field (the expression's own type when it is not found).
    fn variant_field(&self, ty: &str, variant: &str, field: &str, fallback: &Type) -> Type {
        match self.prog.type_def(ty).map(|d| &d.kind) {
            Some(TypeKind::Enum { variants }) => variants.iter().find(|v| &v.name == variant).and_then(|v| v.fields.iter().find(|(x, _)| x == field)).map(|(_, ft)| ft.clone()),
            _ => None,
        }
        .unwrap_or_else(|| fallback.clone())
    }

    /// A consuming `_` of a non-trivial value. The name is wrapped in `Option` by the arm prelude and taken
    /// when the arm exits, so the arm scope's own drop sees `None` and does not destroy it twice. A reference
    /// pattern, a guard fragment and a trivial value stay `_` (7.9, 9.8; D-150).
    fn bind_wildcard(&mut self, ft: &Type) -> String {
        if self.pat_ref || self.in_guard || trivial(ft, self.prog) {
            return "_".into();
        }
        let name = format!("__disc{}", self.next_id());
        self.arm_prelude.push(format!("let mut {name} = Some({name});"));
        self.arm_discards.push((self.discard_ord.clone(), format!("if let Some(__held) = {name}.take() {{ drop(__held); }}")));
        name
    }

    /// Push `idx` (the field's place in its type's declaration) for the pattern of that field. The saved path
    /// is restored by the caller. Sorting the paths lexicographically drops a nested `_` before a later sibling.
    fn set_discard_ord(&mut self, idx: u32) -> Vec<u32> {
        let saved = self.discard_ord.clone();
        self.discard_ord.push(idx);
        saved
    }

    pub fn pattern(&mut self, p: &Pattern, t: &Type) -> String {
        match p {
            Pattern::Wildcard => self.bind_wildcard(t),
            Pattern::Bind(l) => {
                // a binding inside a guard fragment matches `&T` (match ergonomics). The guard reads it as a
                // reference; an owned arm binding is extracted separately for the body.
                if self.in_guard && !trivial(&self.local_ty(*l).clone(), self.prog) {
                    self.guard_as_ref.push(*l);
                }
                self.names[*l as usize].clone()
            },
            Pattern::Literal(c) => match c {
                Const::Int(n) => n.to_string(),
                Const::Str(s) => format!("{s:?}"),
                Const::Bool(b) => b.to_string(),
                other => self.konst(other, t, Want::Value),
            },
            // a range pattern's closed interval (9.8, X-517); the build clipped its open ends to the base's range
            Pattern::Range { lo, hi } => format!("{}..={}", lo.unwrap_or(i128::MIN), hi.unwrap_or(i128::MAX)),
            Pattern::Null => "None".into(),
            Pattern::Present(i) => {
                let inner = match strip(t) {
                    Type::Optional(x) => (**x).clone(),
                    other => other.clone(),
                };
                let saved = self.set_discard_ord(0);
                let s = format!("Some({})", self.pattern(i, &inner));
                self.discard_ord = saved;
                s
            }
            Pattern::Success(i) => {
                let inner = args_of(t).first().cloned().unwrap_or_else(|| t.clone());
                let saved = self.set_discard_ord(0);
                let s = format!("Ok({})", self.pattern(i, &inner));
                self.discard_ord = saved;
                s
            }
            Pattern::Failure(i) => {
                let saved = self.set_discard_ord(0);
                let s = format!("Err({})", self.pattern(i, &fortis_types::types::named("Failure")));
                self.discard_ord = saved;
                s
            }
            Pattern::Variant { ty, variant, fields } => {
                if fields.is_empty() {
                    let payload = match self.prog.type_def(ty).map(|d| &d.kind) {
                        Some(TypeKind::Enum { variants }) => variants.iter().find(|v| &v.name == variant).map_or(false, |v| !v.fields.is_empty()),
                        _ => false,
                    };
                    return if payload { format!("{}::{} {{ .. }}", ident(ty), ident(variant)) } else { format!("{}::{}", ident(ty), ident(variant)) };
                }
                let vt: Vec<(String, Type)> = match self.prog.type_def(ty).map(|d| &d.kind) {
                    Some(TypeKind::Enum { variants }) => variants.iter().find(|v| &v.name == variant).map(|v| v.fields.clone()).unwrap_or_default(),
                    _ => vec![],
                };
                let mut fs = Vec::new();
                for (n, sp) in fields {
                    let (idx, ft) = vt
                        .iter()
                        .enumerate()
                        .find(|(_, (x, _))| x == n)
                        .map(|(i, (_, ft))| (i as u32, ft.clone()))
                        .unwrap_or_else(|| (0, t.clone()));
                    let saved = self.set_discard_ord(idx);
                    fs.push(self.variant_field_pat(ty, n, sp, &ft));
                    self.discard_ord = saved;
                }
                format!("{}::{} {{ {}, .. }}", ident(ty), ident(variant), fs.join(", "))
            }
            Pattern::Class { ty, fields } if crate::recursive_class(self.prog, ty) => {
                // a class that contains itself is a boxed newtype (`Node(Box<Node_>)`), and Rust cannot match through
                // a box: the pattern binds the value, the arm's prelude moves the box's contents out and destructures
                // them, and refutable contents become a guard that tests them by reference first (D-3)
                self.labels += 1;
                let b = format!("__box{}", self.labels);
                let class_fields: Vec<(String, Type)> = match self.prog.type_def(ty).map(|d| &d.kind) {
                    Some(TypeKind::Class { fields }) => fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect(),
                    _ => vec![],
                };
                let (at, guards_at) = (self.arm_prelude.len(), self.arm_guards.len());
                let mut fs = Vec::new();
                for (n, sp) in fields {
                    let (idx, ft) = class_fields
                        .iter()
                        .enumerate()
                        .find(|(_, (x, _))| x == n)
                        .map(|(i, (_, ft))| (i as u32, ft.clone()))
                        .unwrap_or_else(|| (0, t.clone()));
                    let saved = self.set_discard_ord(idx);
                    fs.push(format!("{}: {}", ident(n), self.pattern(sp, &ft)));
                    self.discard_ord = saved;
                }
                let inner = format!("{}_ {{ {}, .. }}", ident(ty), fs.join(", "));
                // the guards of nested boxes test bindings of this pattern: they go inside its test
                let nested: Vec<String> = self.arm_guards.drain(guards_at..).collect();
                let pushed = fields.iter().any(|(_, sp)| refutable(sp)) || !nested.is_empty();
                if pushed {
                    let cond = if nested.is_empty() { String::new() } else { format!(" if {}", nested.join(" && ")) };
                    self.arm_guards.push(format!("matches!(&*{b}.0, {inner}{cond})"));
                }
                let nested_kept = nested;
                // every binding of the contents is made by the prelude
                for (_, sp) in fields {
                    collect_binds(sp, &mut self.boxed_locals);
                }
                self.last_box = Some((b.clone(), inner.clone(), nested_kept, pushed));
                self.arm_prelude.insert(at, format!("let {inner} = *{b}.0 else {{ unreachable!(\"the arm tested it\") }};"));
                b
            }
            Pattern::Class { ty, fields } => {
                let class_fields = self.class_fields_inst(ty, t);
                let mut fs = Vec::new();
                for (n, sp) in fields {
                    // the field's own type, so a trivial field stays `_` and a nested pattern sees that type
                    let (idx, ft) = class_fields
                        .iter()
                        .enumerate()
                        .find(|(_, (x, _))| x == n)
                        .map(|(i, (_, ft))| (i as u32, ft.clone()))
                        .unwrap_or_else(|| (0, t.clone()));
                    let saved = self.set_discard_ord(idx);
                    fs.push(format!("{}: {}", ident(n), self.pattern(sp, &ft)));
                    self.discard_ord = saved;
                }
                // a runtime type (`Capabilities`) is named by its runtime path
                let path = rust_ty(&fortis_types::types::named(ty), self.prog);
                let path = if path.contains('<') || !path.contains("::") { ident(ty) } else { path };
                format!("{path} {{ {}, .. }}", fs.join(", "))
            }
            Pattern::Is { ty, bind, fields } => {
                let code = match self.prog.type_def(ty).map(|d| &d.kind) {
                    Some(TypeKind::Error { code, .. }) => code.clone(),
                    _ => ty.clone(),
                };
                let n = bind.map_or("__f".to_string(), |b| self.names[b as usize].clone());
                // payload fields bound by name: primitives typed, others by their display form
                for (field, sp) in fields {
                    if let Pattern::Bind(l) = sp {
                        let lt = self.local_ty(*l).clone();
                        self.forms[*l as usize] = Form::Owned;
                        // a payload-free enum carries its variant beside its display text (`Dyn::Tag`): matching the
                        // display would panic when `display` is not the variant's name (D-55)
                        let enum_variants = match name_of(&lt) {
                            Some(en) => match self.prog.type_def(en).map(|d| &d.kind) {
                                Some(TypeKind::Enum { variants }) if variants.iter().all(|v| v.fields.is_empty()) => {
                                    Some((en.to_string(), variants.iter().map(|v| v.name.clone()).collect::<Vec<_>>()))
                                }
                                _ => None,
                            },
                            None => None,
                        };
                        let acc = match name_of(&lt) {
                            Some("Int" | "Int64") => format!("{n}.int({field:?})"),
                            Some("String" | "StringView") => format!("{n}.text({field:?})"),
                            Some("Bool") => format!("{n}.boolean({field:?})"),
                            Some("Decimal") => format!("{n}.decimal({field:?})"),
                            _ => match enum_variants {
                                Some((en, vs)) => {
                                    let arms: Vec<String> = vs.iter().map(|v| format!("{v:?} => {}::{}", ident(&en), ident(v))).collect();
                                    format!("match {n}.tag({field:?}).as_str() {{ {}, _ => unreachable!(\"a payload field the checker typed\") }}", arms.join(", "))
                                }
                                None => format!("{n}.field({field:?})"),
                            },
                        };
                        self.arm_prelude.push(format!("let {} = {acc};", self.names[*l as usize]));
                    }
                }
                format!("{n} if {n}.code == {code:?}")
            }
            Pattern::Tuple(ps) => {
                let items = match strip(t) {
                    Type::Tuple(ts) => ts.clone(),
                    _ => vec![],
                };
                let mut parts = Vec::new();
                for (i, sp) in ps.iter().enumerate() {
                    let ft = items.get(i).cloned().unwrap_or_else(|| t.clone());
                    let saved = self.set_discard_ord(i as u32);
                    parts.push(self.pattern(sp, &ft));
                    self.discard_ord = saved;
                }
                format!("({})", parts.join(", "))
            }
            Pattern::Record(fs) => {
                let shape = match strip(t) {
                    Type::Record(rf) => rf.clone(),
                    _ => vec![],
                };
                let mut parts = Vec::new();
                for (n, sp) in fs {
                    let (idx, ft) = shape
                        .iter()
                        .enumerate()
                        .find(|(_, (x, _))| x == n)
                        .map(|(i, (_, ft))| (i as u32, ft.clone()))
                        .unwrap_or_else(|| (0, t.clone()));
                    let saved = self.set_discard_ord(idx);
                    parts.push(format!("{}: {}", ident(n), self.pattern(sp, &ft)));
                    self.discard_ord = saved;
                }
                format!("{} {{ {}, .. }}", record_name(&shape), parts.join(", "))
            }
            Pattern::Otherwise(b) => match b {
                Some(b) => self.names[*b as usize].clone(),
                None => self.bind_wildcard(t),
            },
        }
    }

    /// A closure that owns a cleanup through a `move`, `copy` or bare capture. Implicit and loan captures do not.
    fn note_closure_cleanup(&mut self, local: LocalId, init: &Expr) {
        if self.forms[local as usize] != Form::Owned {
            return;
        }
        let ExprKind::Closure { func, captures } = &init.kind else { return };
        let Some(lam) = self.prog.function(func) else { return };
        let Some(cdef) = &lam.closure else { return };
        let owns = cdef.captures.iter().zip(captures.iter()).any(|(cd, v)| {
            matches!(cd.mode, CaptureMode::Value | CaptureMode::Copy | CaptureMode::Move) && crate::owns_cleanup(self.prog, &v.ty, &mut Vec::new())
        });
        if !owns {
            return;
        }
        let ls = self.cleanups.last_mut().unwrap();
        if !ls.contains(&local) {
            ls.push(local);
        }
    }

    /// An owned local that owns a cleanup, or any owned function value, is destroyed with its block, not at
    /// Rust's scope end (7.9, D-181). A function value's captures are destroyed with it, including a closure
    /// that was moved or returned into the local: its type is `Fn`, which does not itself own a cleanup.
    fn note_cleanup_local(&mut self, local: LocalId) {
        if self.forms[local as usize] != Form::Owned {
            return;
        }
        let t = self.local_ty(local).clone();
        if self.explicit_drop_ty(&t) {
            let ls = self.cleanups.last_mut().unwrap();
            if !ls.contains(&local) {
                ls.push(local);
            }
        }
    }

    /// A value whose block-end drop must be explicit: it owns a `cleanup`, or it is a function value whose
    /// captures might (7.9).
    fn explicit_drop_ty(&self, t: &Type) -> bool {
        matches!(strip(t), Type::Fn { .. }) || crate::owns_cleanup(self.prog, t, &mut Vec::new())
    }

    /// One field of a variant pattern. A field that lowers as `Box<T>` is not matched by looking inside the box
    /// (that needs the unstable `box` pattern). A name binds the box and the arm reborrows or moves it; any other
    /// pattern selects through `box.as_ref()` in a guard fragment, and the arm body extracts the bindings.
    fn variant_field_pat(&mut self, enum_ty: &str, field: &str, sp: &Pattern, ft: &Type) -> String {
        if !crate::boxes_field(self.prog, enum_ty, ft) {
            return format!("{}: {}", ident(field), self.pattern(sp, ft));
        }
        let bind = match sp {
            Pattern::Bind(l) | Pattern::Otherwise(Some(l)) => Some(*l),
            _ => None,
        };
        if let Some(l) = bind {
            let nm = self.names[l as usize].clone();
            if self.in_guard {
                self.guard_as_ref.push(l);
                self.guard_prelude.push(format!("let {nm} = {nm}.as_ref();"));
            } else if self.forms[l as usize] == Form::Owned && !self.pat_ref {
                self.guard_as_ref.push(l);
                self.guard_prelude.push(format!("let {nm} = {nm}.as_ref();"));
                self.arm_prelude.push(format!("let {nm} = *{nm};"));
            } else {
                let d = format!("let {nm} = {nm}.as_ref();");
                self.guard_prelude.push(d.clone());
                self.arm_prelude.push(d);
                self.guard_as_ref.push(l);
            }
            return format!("{}: {nm}", ident(field));
        }
        if matches!(sp, Pattern::Wildcard | Pattern::Otherwise(None)) {
            // the box is the field. Dropping it drops what it holds; `_` would leave that until the outer
            // value drops (7.9, 9.8).
            return format!("{}: {}", ident(field), self.bind_wildcard(ft));
        }
        let tmp = format!("__bx{}", self.next_id());
        let idx = self.pat_guards.len();
        self.pat_guards.push((format!("{tmp}.as_ref()"), String::new()));
        let saved = self.in_guard;
        self.in_guard = true;
        let inner = self.pattern(sp, ft);
        self.in_guard = saved;
        self.pat_guards[idx].1 = inner;
        if !saved {
            let src = if self.pat_ref { format!("{tmp}.as_ref()") } else { format!("*{tmp}") };
            self.extract(sp, ft, &src, self.pat_ref);
        }
        format!("{}: {tmp}", ident(field))
    }

    /// Bindings a nested pattern needs in the arm body, taken out of an expression the guard already proved
    /// matches. `by_ref` means `expr` is `&T` (a ref match, or `box.as_ref()`); otherwise it is an owned `T`.
    fn extract(&mut self, p: &Pattern, t: &Type, expr: &str, by_ref: bool) {
        if !pattern_binds(p) {
            return;
        }
        if !self.contains_box(p, t) {
            match p {
                Pattern::Bind(l) | Pattern::Otherwise(Some(l)) => {
                    let nm = self.names[*l as usize].clone();
                    self.arm_prelude.push(format!("let {nm} = {expr};"));
                }
                Pattern::Wildcard | Pattern::Otherwise(None) => {}
                _ => {
                    let plain = self.plain_pat(p, t);
                    self.emit_let_pat(&plain, expr, self.pat_refutable(p));
                }
            }
            return;
        }
        match p {
            Pattern::Variant { ty, variant, fields } => {
                let mut parts = Vec::new();
                let mut nested: Vec<(String, Pattern, Type, bool)> = Vec::new();
                for (n, sp) in fields {
                    let ft = self.variant_field(ty, variant, n, t);
                    let boxed = crate::boxes_field(self.prog, ty, &ft);
                    if boxed || self.contains_box(sp, &ft) {
                        let tmp = format!("__ex{}", self.next_id());
                        parts.push(format!("{}: {tmp}", ident(n)));
                        nested.push((tmp, sp.clone(), ft, boxed));
                    } else if pattern_binds(sp) {
                        parts.push(format!("{}: {}", ident(n), self.plain_pat(sp, &ft)));
                    } else {
                        parts.push(format!("{}: _", ident(n)));
                    }
                }
                let pat = if parts.is_empty() {
                    format!("{}::{}", ident(ty), ident(variant))
                } else {
                    format!("{}::{} {{ {}, .. }}", ident(ty), ident(variant), parts.join(", "))
                };
                let refutable = self.enum_refutable(ty);
                self.emit_let_pat(&pat, expr, refutable);
                for (tmp, sp, ft, boxed) in nested {
                    let src = if boxed {
                        if by_ref { format!("{tmp}.as_ref()") } else { format!("*{tmp}") }
                    } else {
                        tmp
                    };
                    self.extract(&sp, &ft, &src, by_ref);
                }
            }
            Pattern::Present(inner) => {
                let inner_ty = match strip(t) {
                    Type::Optional(x) => (**x).clone(),
                    _ => t.clone(),
                };
                self.extract_wrapped("Some", inner, &inner_ty, expr, by_ref);
            }
            Pattern::Success(inner) => {
                let inner_ty = args_of(t).first().cloned().unwrap_or_else(|| t.clone());
                self.extract_wrapped("Ok", inner, &inner_ty, expr, by_ref);
            }
            Pattern::Failure(inner) => {
                self.extract_wrapped("Err", inner, &fortis_types::types::named("Failure"), expr, by_ref);
            }
            Pattern::Tuple(ps) => {
                let items = match strip(t) {
                    Type::Tuple(ts) => ts.clone(),
                    _ => vec![],
                };
                let mut parts = Vec::new();
                let mut nested: Vec<(String, Pattern, Type)> = Vec::new();
                for (i, sp) in ps.iter().enumerate() {
                    let ft = items.get(i).cloned().unwrap_or_else(|| t.clone());
                    if self.contains_box(sp, &ft) {
                        let tmp = format!("__ex{}", self.next_id());
                        parts.push(tmp.clone());
                        nested.push((tmp, sp.clone(), ft));
                    } else if pattern_binds(sp) {
                        parts.push(self.plain_pat(sp, &ft));
                    } else {
                        parts.push("_".into());
                    }
                }
                let refutable = self.pat_refutable(p);
                self.emit_let_pat(&format!("({})", parts.join(", ")), expr, refutable);
                for (tmp, sp, ft) in nested {
                    self.extract(&sp, &ft, &tmp, by_ref);
                }
            }
            Pattern::Record(fs) => {
                let shape = match strip(t) {
                    Type::Record(rf) => rf.clone(),
                    _ => vec![],
                };
                let refutable = self.pat_refutable(p);
                let name = record_name(&shape);
                let fallback = t.clone();
                self.extract_named(&name, fs, &|n| shape.iter().find(|(x, _)| x == n).map(|x| x.1.clone()).unwrap_or_else(|| fallback.clone()), expr, by_ref, refutable);
            }
            Pattern::Class { ty, fields } => {
                let path = class_path(ty, self.prog);
                let refutable = self.pat_refutable(p);
                let prog = self.prog;
                let ty = ty.clone();
                self.extract_named(&path, fields, &|n| class_field_ty(prog, &ty, n), expr, by_ref, refutable);
            }
            _ => {}
        }
    }

    /// `let Some(tmp) = expr` (or `Ok` / `Err`) and continue into a pattern that still contains a box.
    fn extract_wrapped(&mut self, ctor: &str, inner: &Pattern, inner_ty: &Type, expr: &str, by_ref: bool) {
        if self.contains_box(inner, inner_ty) {
            let tmp = format!("__ex{}", self.next_id());
            self.emit_let_pat(&format!("{ctor}({tmp})"), expr, true);
            self.extract(inner, inner_ty, &tmp, by_ref);
        } else {
            let plain = format!("{ctor}({})", self.plain_pat(inner, inner_ty));
            self.emit_let_pat(&plain, expr, true);
        }
    }

    fn extract_named(&mut self, path: &str, fields: &[(String, Pattern)], field_ty: &dyn Fn(&str) -> Type, expr: &str, by_ref: bool, refutable: bool) {
        let mut parts = Vec::new();
        let mut nested: Vec<(String, Pattern, Type)> = Vec::new();
        for (n, sp) in fields {
            let ft = field_ty(n);
            if self.contains_box(sp, &ft) {
                let tmp = format!("__ex{}", self.next_id());
                parts.push(format!("{}: {tmp}", ident(n)));
                nested.push((tmp, sp.clone(), ft));
            } else if pattern_binds(sp) {
                parts.push(format!("{}: {}", ident(n), self.plain_pat(sp, &ft)));
            } else {
                parts.push(format!("{}: _", ident(n)));
            }
        }
        self.emit_let_pat(&format!("{path} {{ {}, .. }}", parts.join(", ")), expr, refutable);
        for (tmp, sp, ft) in nested {
            self.extract(&sp, &ft, &tmp, by_ref);
        }
    }

    fn emit_let_pat(&mut self, pat: &str, expr: &str, refutable: bool) {
        if refutable {
            self.arm_prelude.push(format!("let {pat} = {expr} else {{ unreachable!(\"a pattern the checker proved\") }};"));
        } else {
            self.arm_prelude.push(format!("let {pat} = {expr};"));
        }
    }

    fn enum_refutable(&self, ty: &str) -> bool {
        match self.prog.type_def(ty).map(|d| &d.kind) {
            Some(TypeKind::Enum { variants }) => variants.len() != 1,
            _ => true,
        }
    }

    /// A pattern Rust can reject (a specific variant, `Some`, `Ok`). A struct or tuple pattern is refutable when
    /// one of its fields is.
    fn pat_refutable(&self, p: &Pattern) -> bool {
        match p {
            Pattern::Variant { ty, .. } => self.enum_refutable(ty),
            Pattern::Present(_) | Pattern::Null | Pattern::Success(_) | Pattern::Failure(_) => true,
            Pattern::Tuple(ps) => ps.iter().any(|sp| self.pat_refutable(sp)),
            Pattern::Record(fs) | Pattern::Class { fields: fs, .. } => fs.iter().any(|(_, sp)| self.pat_refutable(sp)),
            _ => false,
        }
    }

    /// Whether the pattern matches through a boxed field of a recursive enum.
    fn contains_box(&self, p: &Pattern, t: &Type) -> bool {
        match p {
            Pattern::Variant { ty, variant, fields } => fields.iter().any(|(n, sp)| {
                let ft = self.variant_field(ty, variant, n, t);
                crate::boxes_field(self.prog, ty, &ft) || self.contains_box(sp, &ft)
            }),
            Pattern::Present(i) => {
                let inner = match strip(t) {
                    Type::Optional(x) => (**x).clone(),
                    _ => t.clone(),
                };
                self.contains_box(i, &inner)
            }
            Pattern::Success(i) => {
                let inner = args_of(t).first().cloned().unwrap_or_else(|| t.clone());
                self.contains_box(i, &inner)
            }
            Pattern::Failure(i) => self.contains_box(i, &fortis_types::types::named("Failure")),
            Pattern::Tuple(ps) => {
                let items = match strip(t) {
                    Type::Tuple(ts) => ts.clone(),
                    _ => vec![],
                };
                ps.iter().enumerate().any(|(i, sp)| self.contains_box(sp, items.get(i).unwrap_or(t)))
            }
            Pattern::Record(fs) => {
                let shape = match strip(t) {
                    Type::Record(rf) => rf.clone(),
                    _ => vec![],
                };
                fs.iter().any(|(n, sp)| {
                    let ft = shape.iter().find(|(x, _)| x == n).map(|x| x.1.clone()).unwrap_or_else(|| t.clone());
                    self.contains_box(sp, &ft)
                })
            }
            Pattern::Class { ty, fields } => fields.iter().any(|(n, sp)| self.contains_box(sp, &class_field_ty(self.prog, ty, n))),
            _ => false,
        }
    }

    /// A pattern string with no box peeling. The caller has already checked the pattern contains no boxed field.
    fn plain_pat(&self, p: &Pattern, t: &Type) -> String {
        match p {
            Pattern::Wildcard => "_".into(),
            Pattern::Bind(l) => self.names[*l as usize].clone(),
            Pattern::Otherwise(b) => b.map_or("_".to_string(), |b| self.names[b as usize].clone()),
            Pattern::Literal(c) => match c {
                Const::Int(n) => n.to_string(),
                Const::Str(s) => format!("{s:?}"),
                Const::Bool(b) => b.to_string(),
                _ => "_".into(),
            },
            // a range pattern's closed interval (9.8, X-517); the build clipped its open ends to the base's range
            Pattern::Range { lo, hi } => format!("{}..={}", lo.unwrap_or(i128::MIN), hi.unwrap_or(i128::MAX)),
            Pattern::Null => "None".into(),
            Pattern::Present(i) => {
                let inner = match strip(t) {
                    Type::Optional(x) => (**x).clone(),
                    _ => t.clone(),
                };
                format!("Some({})", self.plain_pat(i, &inner))
            }
            Pattern::Success(i) => {
                let inner = args_of(t).first().cloned().unwrap_or_else(|| t.clone());
                format!("Ok({})", self.plain_pat(i, &inner))
            }
            Pattern::Failure(i) => format!("Err({})", self.plain_pat(i, &fortis_types::types::named("Failure"))),
            Pattern::Variant { ty, variant, fields } => {
                if fields.is_empty() {
                    return format!("{}::{}", ident(ty), ident(variant));
                }
                let vt: Vec<(String, Type)> = match self.prog.type_def(ty).map(|d| &d.kind) {
                    Some(TypeKind::Enum { variants }) => variants.iter().find(|v| &v.name == variant).map(|v| v.fields.clone()).unwrap_or_default(),
                    _ => vec![],
                };
                let fs: Vec<String> = fields
                    .iter()
                    .map(|(n, sp)| {
                        let ft = vt.iter().find(|(x, _)| x == n).map(|x| x.1.clone()).unwrap_or_else(|| t.clone());
                        format!("{}: {}", ident(n), self.plain_pat(sp, &ft))
                    })
                    .collect();
                format!("{}::{} {{ {}, .. }}", ident(ty), ident(variant), fs.join(", "))
            }
            Pattern::Class { ty, fields } => {
                let fs: Vec<String> = fields.iter().map(|(n, sp)| format!("{}: {}", ident(n), self.plain_pat(sp, &class_field_ty(self.prog, ty, n)))).collect();
                format!("{} {{ {}, .. }}", class_path(ty, self.prog), fs.join(", "))
            }
            Pattern::Tuple(ps) => {
                let items = match strip(t) {
                    Type::Tuple(ts) => ts.clone(),
                    _ => vec![],
                };
                let parts: Vec<String> = ps.iter().enumerate().map(|(i, sp)| self.plain_pat(sp, items.get(i).unwrap_or(t))).collect();
                format!("({})", parts.join(", "))
            }
            Pattern::Record(fs) => {
                let shape = match strip(t) {
                    Type::Record(rf) => rf.clone(),
                    _ => vec![],
                };
                let parts: Vec<String> = fs
                    .iter()
                    .map(|(n, sp)| {
                        let ft = shape.iter().find(|(x, _)| x == n).map(|x| x.1.clone()).unwrap_or_else(|| t.clone());
                        format!("{}: {}", ident(n), self.plain_pat(sp, &ft))
                    })
                    .collect();
                format!("{} {{ {}, .. }}", record_name(&shape), parts.join(", "))
            }
            Pattern::Is { .. } => "_".into(),
        }
    }
}

/// Whether dropping a value of type `t` may use a loan it holds: a task handle, a future or a guard (their drop runs
/// code over what they borrow), or a type not known here. A plain loan, a view or a closure holds its loans without
/// using them when dropped, and an owned value holds none (X-137), so Rust lets those outlive the task set (D-10).
fn may_hold_loan(t: &Type) -> bool {
    match strip(t) {
        Type::Unknown(_) | Type::TypeParam(_) | Type::Dynamic(_) => true,
        Type::Optional(i) => may_hold_loan(i),
        Type::Tuple(ts) => ts.iter().any(may_hold_loan),
        Type::Named(n) => matches!(n.name.as_str(), "Task" | "Future" | "Scope" | "MutexGuard") || n.args.iter().any(may_hold_loan),
        _ => false,
    }
}

/// A type whose Rust spelling is complete without inference: no loans, functions, unknowns or lifetimes.
fn annotatable(t: &Type, prog: &Program) -> bool {
    match t {
        Type::Loan { .. } | Type::Fn { .. } | Type::Unknown(_) | Type::Dynamic(_) => false,
        // a future's, a task's and a scope's Rust types are unnameable or carry lifetimes: inferred
        Type::Named(n) if matches!(n.name.as_str(), "Future" | "Task" | "Scope") => false,
        Type::Named(n) => {
            let declared = rust_ty(t, prog) != ident(&n.name) || prog.type_def(&n.name).map_or(false, |d| matches!(d.kind, TypeKind::Class { .. } | TypeKind::Enum { .. }));
            declared && n.name != "ListCursor" && prog.type_def(&n.name).map_or(true, |d| d.lifetimes.is_empty()) && n.args.iter().all(|a| annotatable(a, prog))
        }
        Type::Refined { .. } | Type::TypeParam(_) => true,
        Type::Optional(i) => annotatable(i, prog),
        Type::Tuple(ts) => ts.iter().all(|x| annotatable(x, prog)),
        Type::Record(fs) => fs.iter().all(|(_, x)| annotatable(x, prog)),
    }
}

/// A generic use whose arguments were dropped (`Chain` for `Chain[T]`) is annotated from the initialiser when
/// that still names them. `: Chain` is not a Rust type (D-55).
fn annotation_type(local: &Type, init: &Type, prog: &Program) -> Type {
    let generic = |t: &Type| name_of(t).and_then(|n| prog.type_def(n)).map_or(false, |d| !d.type_params.is_empty());
    if generic(local) && args_of(local).is_empty() && !args_of(init).is_empty() {
        init.clone()
    } else {
        local.clone()
    }
}

/// Whether `t` can be written as a Rust type. A generic definition with no arguments cannot (`Chain`).
fn spelled(prog: &Program, t: &Type) -> bool {
    match name_of(t).and_then(|n| prog.type_def(n)) {
        Some(d) if !d.type_params.is_empty() && args_of(t).is_empty() => false,
        _ => true,
    }
}

/// Whether a `??` fallback borrows all it gives: a place read or lent, or a lent `??` whose left side is a loan and
/// whose own fallback is wholly borrowed (D-345).
fn wholly_borrowed(b: &Expr, prog: &Program) -> bool {
    match &b.kind {
        _ if name_of(&b.ty) == Some("Never") => true,
        ExprKind::Const(Const::Str(_)) => true,
        ExprKind::Place(_, Use::Read | Use::Alias) => true,
        ExprKind::Coalesce(ba, bb) => lent_coalesce_ty(&b.ty, prog) && matches!(strip(&ba.ty), Type::Optional(i) if i.is_loan()) && wholly_borrowed(bb, prog),
        ExprKind::OrRecover(_, fallback) => b.ty.is_loan() && wholly_borrowed(fallback, prog),
        ExprKind::Catch { handler, .. } => b.ty.is_loan() && !recover_owns(handler, prog),
        ExprKind::Call { .. } | ExprKind::OrFail(_) => b.ty.is_loan(),
        _ => false,
    }
}

/// The inner type of a recovery's successful loan when a replacement may own a value
/// (D-446, R23): the statement temporary of it holds a `MaybeOwned` (a `Cow` for text), so a borrowed success and an
/// owned replacement sit in one slot, and the statement's drop destroys and announces a made replacement as the
/// interpreter does (7.9). A wholly borrowed replacement needs no value of the statement's own.
fn recovery_holder_inner(e: &Expr, prog: &Program) -> Option<Type> {
    let (value, owns) = match &e.kind {
        ExprKind::OrRecover(a, b) => (&**a, !wholly_borrowed(b, prog)),
        ExprKind::Catch { value, handler, .. } => (&**value, recover_owns(handler, prog)),
        _ => return None,
    };
    let Type::Named(n) = strip(&value.ty) else { return None };
    if n.name != "Outcome" || !owns { return None; }
    let Type::Loan { inner, mode, .. } = n.args.first()? else { return None };
    if mode == "editable" || trivial(inner, prog) { return None; }
    Some((**inner).clone())
}

/// Inspect only this handler's recover statements, never those of a nested catch expression.
fn recover_owns(b: &Block, prog: &Program) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Recover(e) => !wholly_borrowed(e, prog),
        StmtKind::Block(b) | StmtKind::Loop { body: b, .. } | StmtKind::For { body: b, .. } | StmtKind::Scope { body: b, .. } => recover_owns(b, prog),
        StmtKind::If { then, els, .. } => recover_owns(then, prog) || els.as_ref().is_some_and(|b| recover_owns(b, prog)),
        StmtKind::Match { arms, .. } => arms.iter().any(|a| matches!(&a.body, ArmBody::Block(b) if recover_owns(b, prog))),
        _ => false,
    })
}

/// Whether a `??` of this type is lent: its result borrows a non-trivial value (D-246).
fn lent_coalesce_ty(t: &Type, prog: &Program) -> bool {
    matches!(t, Type::Loan { inner, .. } if !trivial(inner, prog))
}

/// `opt ?? fallback` of an `Option` and its fallback's code: inline, as a `match`, when the fallback may leave the
/// function (an `or fail`'s `?`, a `fail`), which a closure cannot; otherwise `unwrap_or_else` evaluates it lazily
/// all the same (5.7).
fn or_else(opt: String, fallback: String, inline: bool) -> String {
    if inline {
        format!("(match {opt} {{ Some(__v) => __v, None => {fallback} }})")
    } else {
        format!("{opt}.unwrap_or_else(|| {fallback})")
    }
}

/// Whether evaluating `e` may leave the enclosing function: an `or fail`, a `fail`, or a block of statements (a
/// `catch` handler, an `if` or `match` with blocks) that may `return`. A lambda's body is its own function.
fn leaves(e: &Expr) -> bool {
    let mut found = false;
    crate::visit_expr(e, &mut |x| {
        found |= matches!(x.kind, ExprKind::OrFail(_) | ExprKind::Fail(_) | ExprKind::Catch { .. } | ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Block(_));
    });
    found
}

pub fn int_suffix(t: &Type) -> &'static str {
    match name_of(t) {
        Some("Int32") => "i32",
        Some("Int16") => "i16",
        Some("Int8") => "i8",
        Some("UInt") | Some("UInt64") => "u64",
        Some("UInt32") => "u32",
        Some("UInt16") => "u16",
        Some("UInt8") | Some("Byte") => "u8",
        Some("Float") => "f64",
        _ => "i64",
    }
}

/// The interpreter's name for a function in failure origins: `main`, `parse`, `method`.
pub fn source_name(f: &Function) -> String {
    let l = &f.label;
    l.strip_prefix("function ").or_else(|| l.strip_prefix("method ").map(|m| m.rsplit('.').next().unwrap_or(m))).unwrap_or(l).split('#').next().unwrap_or("").to_string()
}

pub fn global_fn(module: &str, name: &str) -> String {
    format!("const_{}_{}", ident(&module.replace('.', "_")), ident(name))
}

/// The task locals a function detaches (`scope.detach(t, supervisor)`): their launches own what they hold.
fn detached_tasks(f: &Function) -> std::collections::HashSet<LocalId> {
    let mut out = std::collections::HashSet::new();
    fn walk_block(b: &Block, out: &mut std::collections::HashSet<LocalId>) {
        for s in &b.stmts {
            walk_stmt(s, out);
        }
        for d in &b.defers {
            walk_block(d, out);
        }
    }
    fn walk_stmt(s: &Stmt, out: &mut std::collections::HashSet<LocalId>) {
        match &s.kind {
            StmtKind::Expr(e) | StmtKind::Discard(e) => walk_expr(e, out),
            StmtKind::If { then, els, .. } => {
                walk_block(then, out);
                if let Some(b) = els {
                    walk_block(b, out);
                }
            }
            StmtKind::Loop { body, .. } | StmtKind::For { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => walk_block(body, out),
            _ => {}
        }
    }
    fn walk_expr(e: &Expr, out: &mut std::collections::HashSet<LocalId>) {
        match &e.kind {
            ExprKind::Temp { value, .. } => walk_expr(value, out),
            ExprKind::Call { callee: Callee::Intrinsic(n), args } if n == "Scope.detach" => {
                if let Some(ExprKind::Place(p, _)) = args.get(1).map(|a| &a.value.kind) {
                    out.insert(p.local);
                }
            }
            ExprKind::Call { args, .. } => args.iter().for_each(|a| walk_expr(&a.value, out)),
            ExprKind::Field(a, _) | ExprKind::TupleField(a, _) | ExprKind::Unary(_, a) | ExprKind::Await(a) => walk_expr(a, out),
            _ => {}
        }
    }
    if let Some(b) = &f.body {
        walk_block(b, &mut out);
    }
    out
}

/// `&'name T` as `&T` (and `&'name mut T` as `&mut T`), keeping `'static`.
fn elide_lifetimes(t: &str) -> String {
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find("&'") {
        out.push_str(&rest[..i + 1]);
        let after = &rest[i + 2..];
        let end = after.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(after.len());
        let name = &after[..end];
        if name == "static" {
            out.push_str(&format!("'{name}"));
            rest = &after[end..];
        } else {
            rest = after[end..].strip_prefix(' ').unwrap_or(&after[end..]);
        }
    }
    out.push_str(rest);
    out
}

fn tuple_proj_path(proj: &[Proj]) -> String {
    proj.iter().map(|pr| match pr { Proj::Tuple(i) => format!(".{i}"), _ => String::new() }).collect()
}

fn tuple_hold_name(local: LocalId, path: &[u32]) -> String {
    let tail = path.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("_");
    format!("__tup{local}_{tail}")
}

/// Every tuple-element move of `local` in this function (`.0`, `.0.1`). The stash and the statement drop
/// use the same paths, so a part the move does not take is still dropped at the end of the statement (7.9).
fn tuple_paths_in_fn(f: &Function, local: LocalId) -> Vec<Vec<u32>> {
    let mut all: HashMap<LocalId, Vec<Vec<u32>>> = HashMap::new();
    if let Some(body) = &f.body {
        note_tuple_block(body, &mut all);
    }
    all.remove(&local).unwrap_or_default()
}

fn note_tuple_block(b: &Block, out: &mut HashMap<LocalId, Vec<Vec<u32>>>) {
    for s in &b.stmts {
        note_tuple_stmt(s, out);
    }
    for d in &b.defers {
        note_tuple_block(d, out);
    }
}

fn push_tuple_place(p: &Place, out: &mut HashMap<LocalId, Vec<Vec<u32>>>) {
    if p.proj.is_empty() || !p.proj.iter().all(|pr| matches!(pr, Proj::Tuple(_))) {
        return;
    }
    let path = p.proj.iter().map(|pr| match pr { Proj::Tuple(i) => *i, _ => 0 }).collect();
    out.entry(p.local).or_default().push(path);
}

fn note_tuple_stmt(s: &Stmt, out: &mut HashMap<LocalId, Vec<Vec<u32>>>) {
    match &s.kind {
        StmtKind::Let { init, .. } | StmtKind::LetPattern { init, .. } | StmtKind::Assign { value: init, .. } | StmtKind::Compound { value: init, .. } | StmtKind::Expr(init) | StmtKind::Discard(init) | StmtKind::Return(Some(init)) | StmtKind::Give(init) | StmtKind::Recover(init) | StmtKind::Fail(init) | StmtKind::Capture { value: init, .. } | StmtKind::Report { value: init, .. } | StmtKind::Expect { cond: init, .. } => note_tuple_expr(init, out),
        StmtKind::Drop(p) => push_tuple_place(p, out),
        StmtKind::Return(None) | StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Defer(_) => {}
        StmtKind::If { cond, then, els } => {
            note_tuple_expr(cond, out);
            note_tuple_block(then, out);
            if let Some(b) = els {
                note_tuple_block(b, out);
            }
        }
        StmtKind::Loop { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => note_tuple_block(body, out),
        StmtKind::For { source, body, .. } => {
            match source {
                ForSource::Items(e) => note_tuple_expr(e, out),
                ForSource::Range { lo, hi, .. } => {
                    note_tuple_expr(lo, out);
                    note_tuple_expr(hi, out);
                }
            }
            note_tuple_block(body, out);
        }
        StmtKind::Match { scrutinee, arms } => {
            note_tuple_expr(scrutinee, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    note_tuple_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(e) => note_tuple_expr(e, out),
                    ArmBody::Block(b) => note_tuple_block(b, out),
                }
            }
        }
        StmtKind::Record { fields, .. } => {
            for (_, e) in fields {
                note_tuple_expr(e, out);
            }
        }
    }
}

fn note_tuple_expr(e: &Expr, out: &mut HashMap<LocalId, Vec<Vec<u32>>>) {
    match &e.kind {
        ExprKind::Place(p, Use::Move) => push_tuple_place(p, out),
        ExprKind::Const(_) | ExprKind::Place(_, _) | ExprKind::Global(_) | ExprKind::FuncRef(_) => {}
        ExprKind::Codec { arg, .. } | ExprKind::Unary(_, arg) | ExprKind::Field(arg, _) | ExprKind::TupleField(arg, _) | ExprKind::OptField(arg, _) | ExprKind::Present(arg) | ExprKind::Success(arg) | ExprKind::OrFail(arg) | ExprKind::Fail(arg) | ExprKind::Validate { value: arg, .. } | ExprKind::Wrap { value: arg, .. } | ExprKind::Await(arg) | ExprKind::ToDynamic { value: arg, .. } | ExprKind::Temp { value: arg, .. } => note_tuple_expr(arg, out),
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Index(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) => {
            note_tuple_expr(a, out);
            note_tuple_expr(b, out);
        }
        ExprKind::Call { callee, args } => {
            if let Callee::Value(v) = callee {
                note_tuple_expr(v, out);
            }
            for a in args {
                note_tuple_expr(&a.value, out);
            }
        }
        ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => {
            for (_, v) in fields {
                note_tuple_expr(v, out);
            }
        }
        ExprKind::Tuple(xs) => {
            for x in xs {
                note_tuple_expr(x, out);
            }
        }
        ExprKind::Closure { captures, .. } => {
            for c in captures {
                note_tuple_expr(c, out);
            }
        }
        ExprKind::Interpolate(parts) => {
            for p in parts {
                if let Part::Show(x) = p {
                    note_tuple_expr(x, out);
                }
            }
        }
        ExprKind::Catch { value, handler, .. } => {
            note_tuple_expr(value, out);
            note_tuple_block(handler, out);
        }
        ExprKind::If { cond, then, els } => {
            note_tuple_expr(cond, out);
            note_tuple_block(then, out);
            note_tuple_block(els, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            note_tuple_expr(scrutinee, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    note_tuple_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(x) => note_tuple_expr(x, out),
                    ArmBody::Block(b) => note_tuple_block(b, out),
                }
            }
        }
        ExprKind::Block(b) => note_tuple_block(b, out),
    }
}

/// Temporaries this statement moves a field out of (`Use::Move` of one field). The statement drops the
/// stashed siblings; a nested statement's own temporary is not in this statement's `temps` (7.9).
fn field_moved_temps(s: &Stmt) -> HashSet<LocalId> {
    let mut out = HashSet::new();
    note_field_move_stmt(s, &mut out);
    out
}

fn note_field_move_stmt(s: &Stmt, out: &mut HashSet<LocalId>) {
    match &s.kind {
        StmtKind::Let { init, .. } | StmtKind::LetPattern { init, .. } | StmtKind::Assign { value: init, .. } | StmtKind::Compound { value: init, .. } | StmtKind::Expr(init) | StmtKind::Discard(init) | StmtKind::Return(Some(init)) | StmtKind::Give(init) | StmtKind::Recover(init) | StmtKind::Fail(init) | StmtKind::Capture { value: init, .. } | StmtKind::Report { value: init, .. } | StmtKind::Expect { cond: init, .. } => note_field_move_expr(init, out),
        StmtKind::Return(None) | StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Defer(_) | StmtKind::Drop(_) => {}
        StmtKind::If { cond, then, els } => {
            note_field_move_expr(cond, out);
            note_field_move_block(then, out);
            if let Some(b) = els {
                note_field_move_block(b, out);
            }
        }
        StmtKind::Loop { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => note_field_move_block(body, out),
        StmtKind::For { source, body, .. } => {
            match source {
                ForSource::Items(e) => note_field_move_expr(e, out),
                ForSource::Range { lo, hi, .. } => {
                    note_field_move_expr(lo, out);
                    note_field_move_expr(hi, out);
                }
            }
            note_field_move_block(body, out);
        }
        StmtKind::Match { scrutinee, arms } => {
            note_field_move_expr(scrutinee, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    note_field_move_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(e) => note_field_move_expr(e, out),
                    ArmBody::Block(b) => note_field_move_block(b, out),
                }
            }
        }
        StmtKind::Record { fields, .. } => {
            for (_, e) in fields {
                note_field_move_expr(e, out);
            }
        }
    }
}

fn note_field_move_block(b: &Block, out: &mut HashSet<LocalId>) {
    for s in &b.stmts {
        note_field_move_stmt(s, out);
    }
    for d in &b.defers {
        note_field_move_block(d, out);
    }
}

fn note_field_move_expr(e: &Expr, out: &mut HashSet<LocalId>) {
    match &e.kind {
        ExprKind::Place(p, Use::Move) => {
            if p.proj.len() == 1 && matches!(p.proj[0], Proj::Field(_)) {
                out.insert(p.local);
            }
        }
        ExprKind::Const(_) | ExprKind::Place(_, _) | ExprKind::Global(_) | ExprKind::FuncRef(_) => {}
        ExprKind::Codec { arg, .. } | ExprKind::Unary(_, arg) | ExprKind::Field(arg, _) | ExprKind::TupleField(arg, _) | ExprKind::OptField(arg, _) | ExprKind::Present(arg) | ExprKind::Success(arg) | ExprKind::OrFail(arg) | ExprKind::Fail(arg) | ExprKind::Validate { value: arg, .. } | ExprKind::Wrap { value: arg, .. } | ExprKind::Await(arg) | ExprKind::ToDynamic { value: arg, .. } | ExprKind::Temp { value: arg, .. } => note_field_move_expr(arg, out),
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Index(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) => {
            note_field_move_expr(a, out);
            note_field_move_expr(b, out);
        }
        ExprKind::Call { args, .. } => {
            for a in args {
                note_field_move_expr(&a.value, out);
            }
        }
        ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => {
            for (_, v) in fields {
                note_field_move_expr(v, out);
            }
        }
        ExprKind::Tuple(xs) => {
            for x in xs {
                note_field_move_expr(x, out);
            }
        }
        ExprKind::Closure { captures, .. } => {
            for c in captures {
                note_field_move_expr(c, out);
            }
        }
        ExprKind::Interpolate(parts) => {
            for p in parts {
                if let Part::Show(x) = p {
                    note_field_move_expr(x, out);
                }
            }
        }
        ExprKind::Catch { value, handler, .. } => {
            note_field_move_expr(value, out);
            note_field_move_block(handler, out);
        }
        ExprKind::If { cond, then, els } => {
            note_field_move_expr(cond, out);
            note_field_move_block(then, out);
            note_field_move_block(els, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            note_field_move_expr(scrutinee, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    note_field_move_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(x) => note_field_move_expr(x, out),
                    ArmBody::Block(b) => note_field_move_block(b, out),
                }
            }
        }
        ExprKind::Block(b) => note_field_move_block(b, out),
    }
}

/// Temporaries this statement moves into Rust, which therefore drops them. A `for` source always moves. A
/// match does not: its scrutinee stays in the `Option` unless an arm binds by value, and that path `take`s
/// the slot itself, so the statement's `take` finds `None` (7.9).
fn rust_moved_temps(s: &Stmt) -> Vec<LocalId> {
    let mut out = Vec::new();
    note_moved_stmt(s, &mut out);
    out
}

fn note_moved_stmt(s: &Stmt, out: &mut Vec<LocalId>) {
    match &s.kind {
        StmtKind::Let { init, .. } | StmtKind::LetPattern { init, .. } | StmtKind::Assign { value: init, .. } | StmtKind::Compound { value: init, .. } | StmtKind::Expr(init) | StmtKind::Discard(init) | StmtKind::Return(Some(init)) | StmtKind::Give(init) | StmtKind::Recover(init) | StmtKind::Fail(init) | StmtKind::Capture { value: init, .. } | StmtKind::Report { value: init, .. } | StmtKind::Expect { cond: init, .. } => note_moved_expr(init, out),
        StmtKind::Return(None) | StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Defer(_) | StmtKind::Drop(_) => {}
        StmtKind::If { cond, then, els } => {
            note_moved_expr(cond, out);
            note_moved_block(then, out);
            if let Some(b) = els {
                note_moved_block(b, out);
            }
        }
        StmtKind::Loop { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => note_moved_block(body, out),
        StmtKind::For { source, body, .. } => {
            match source {
                ForSource::Items(e) => {
                    if let ExprKind::Temp { local, .. } = &e.kind {
                        out.push(*local);
                    }
                    note_moved_expr(e, out);
                }
                ForSource::Range { lo, hi, .. } => {
                    note_moved_expr(lo, out);
                    note_moved_expr(hi, out);
                }
            }
            note_moved_block(body, out);
        }
        StmtKind::Match { scrutinee, arms } => {
            note_moved_match(scrutinee, arms, out);
        }
        StmtKind::Record { fields, .. } => {
            for (_, e) in fields {
                note_moved_expr(e, out);
            }
        }
    }
}

fn note_moved_block(b: &Block, out: &mut Vec<LocalId>) {
    for s in &b.stmts {
        note_moved_stmt(s, out);
    }
    for d in &b.defers {
        note_moved_block(d, out);
    }
}

fn note_moved_match(scrutinee: &Expr, arms: &[Arm], out: &mut Vec<LocalId>) {
    // the scrutinee temporary is not moved into the match. A binding arm `take`s the `Option`; every other
    // pattern leaves it there, and the statement drops it (7.9).
    let _ = arms;
    note_moved_expr(scrutinee, out);
    for a in arms {
        if let Some(g) = &a.guard {
            note_moved_expr(g, out);
        }
        match &a.body {
            ArmBody::Expr(e) => note_moved_expr(e, out),
            ArmBody::Block(b) => note_moved_block(b, out),
        }
    }
}

fn note_moved_expr(e: &Expr, out: &mut Vec<LocalId>) {
    match &e.kind {
        ExprKind::Const(_) | ExprKind::Place(_, _) | ExprKind::Global(_) | ExprKind::FuncRef(_) => {}
        ExprKind::Codec { arg, .. } | ExprKind::Unary(_, arg) | ExprKind::Field(arg, _) | ExprKind::TupleField(arg, _) | ExprKind::OptField(arg, _) | ExprKind::Present(arg) | ExprKind::Success(arg) | ExprKind::OrFail(arg) | ExprKind::Fail(arg) | ExprKind::Validate { value: arg, .. } | ExprKind::Wrap { value: arg, .. } | ExprKind::Await(arg) | ExprKind::ToDynamic { value: arg, .. } | ExprKind::Temp { value: arg, .. } => note_moved_expr(arg, out),
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Index(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) => {
            note_moved_expr(a, out);
            note_moved_expr(b, out);
        }
        ExprKind::Call { args, .. } => {
            for a in args {
                note_moved_expr(&a.value, out);
            }
        }
        ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => {
            for (_, v) in fields {
                note_moved_expr(v, out);
            }
        }
        ExprKind::Tuple(xs) => {
            for x in xs {
                note_moved_expr(x, out);
            }
        }
        ExprKind::Closure { captures, .. } => {
            for c in captures {
                note_moved_expr(c, out);
            }
        }
        ExprKind::Interpolate(parts) => {
            for p in parts {
                if let Part::Show(x) = p {
                    note_moved_expr(x, out);
                }
            }
        }
        ExprKind::Catch { value, handler, .. } => {
            note_moved_expr(value, out);
            note_moved_block(handler, out);
        }
        ExprKind::If { cond, then, els } => {
            note_moved_expr(cond, out);
            note_moved_block(then, out);
            note_moved_block(els, out);
        }
        ExprKind::Match { scrutinee, arms } => note_moved_match(scrutinee, arms, out),
        ExprKind::Block(b) => note_moved_block(b, out),
    }
}

/// The owned parameters of `f` that own a cleanup, directly or through a field: the function's oldest values,
/// destroyed at its exit after its body block unless moved (7.9). The trace names only a class that itself
/// declares cleanup.
fn owned_cleanup_params(prog: &Program, f: &Function) -> Vec<LocalId> {
    f.params
        .iter()
        .filter(|p| p.mode == Mode::Owned)
        .filter(|p| {
            let t = &f.locals[p.local as usize].ty;
            matches!(strip(t), Type::Fn { .. }) || crate::owns_cleanup(prog, t, &mut Vec::new())
        })
        .map(|p| p.local)
        .collect()
}

/// Guard fragments are stored outermost first. Nesting them in that order tests an inner binding before the
/// outer `if let` introduces it. Reversing makes the first fragment the innermost test (D-55).
fn nest_pat_guards(inner: &str, pgs: &[(String, String)]) -> String {
    pgs.iter().rev().fold(inner.to_string(), |acc, (sc, pat)| format!("if let {pat} = {sc} {{ {acc} }} else {{ false }}"))
}

/// A payload-free enum's variants, when `t` is one. Its display text is not a discriminant (D-55).
fn payload_free_enum(prog: &Program, t: &Type) -> Option<(String, Vec<String>)> {
    let en = name_of(t)?;
    let d = prog.type_def(en)?;
    let TypeKind::Enum { variants } = &d.kind else { return None };
    if !variants.is_empty() && variants.iter().all(|v| v.fields.is_empty()) {
        Some((en.to_string(), variants.iter().map(|v| v.name.clone()).collect()))
    } else {
        None
    }
}

/// Locals moved where the move might not run: a branch, a loop, a deferred block, a handler, or the right
/// operand of `&&`, `||` or `??`. Only those that own a cleanup or are function values are stored as `Option`;
/// a straight-line move is already recorded in `moved` and dropped by that move (D-181).
fn manual_locals(prog: &Program, f: &Function) -> HashSet<LocalId> {
    let mut found = HashSet::new();
    if let Some(b) = &f.body {
        walk_block_moves(b, 0, &mut found);
    }
    found.retain(|l| {
        let t = &f.locals[*l as usize].ty;
        matches!(strip(t), Type::Fn { .. }) || crate::owns_cleanup(prog, t, &mut Vec::new())
    });
    found
}

fn walk_block_moves(b: &Block, cond: u32, out: &mut HashSet<LocalId>) {
    for s in &b.stmts {
        walk_stmt_moves(s, cond, out);
    }
    // a defer runs only when its registration was reached
    for d in &b.defers {
        walk_block_moves(d, cond + 1, out);
    }
}

fn walk_stmt_moves(s: &Stmt, cond: u32, out: &mut HashSet<LocalId>) {
    match &s.kind {
        StmtKind::Let { init, .. } | StmtKind::LetPattern { init, .. } => walk_expr_moves(init, cond, out),
        StmtKind::Assign { value, .. } | StmtKind::Discard(value) | StmtKind::Expr(value) | StmtKind::Give(value) | StmtKind::Recover(value) | StmtKind::Fail(value) | StmtKind::Capture { value, .. } | StmtKind::Report { value, .. } => {
            walk_expr_moves(value, cond, out)
        }
        StmtKind::Return(v) => {
            if let Some(e) = v {
                walk_expr_moves(e, cond, out);
            }
        }
        StmtKind::Compound { value, .. } => walk_expr_moves(value, cond, out),
        StmtKind::Record { fields, .. } => {
            for (_, e) in fields {
                walk_expr_moves(e, cond, out);
            }
        }
        StmtKind::Expect { cond: c, evidence } => {
            walk_expr_moves(c, cond, out);
            if let Some((a, _, b)) = evidence {
                walk_expr_moves(a, cond, out);
                walk_expr_moves(b, cond, out);
            }
        }
        StmtKind::If { cond: c, then, els } => {
            walk_expr_moves(c, cond, out);
            walk_block_moves(then, cond + 1, out);
            if let Some(e) = els {
                walk_block_moves(e, cond + 1, out);
            }
        }
        StmtKind::Loop { body, .. } => walk_block_moves(body, cond + 1, out),
        StmtKind::For { source, body, .. } => {
            match source {
                ForSource::Range { lo, hi, .. } => {
                    walk_expr_moves(lo, cond, out);
                    walk_expr_moves(hi, cond, out);
                }
                ForSource::Items(e) => walk_expr_moves(e, cond, out),
            }
            walk_block_moves(body, cond + 1, out);
        }
        // a nested block runs whenever its parent does: it is not itself a branch
        StmtKind::Block(b) | StmtKind::Scope { body: b, .. } => walk_block_moves(b, cond, out),
        StmtKind::Match { scrutinee, arms } => {
            walk_expr_moves(scrutinee, cond, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    walk_expr_moves(g, cond + 1, out);
                }
                match &a.body {
                    ArmBody::Expr(e) => walk_expr_moves(e, cond + 1, out),
                    ArmBody::Block(b) => walk_block_moves(b, cond + 1, out),
                }
            }
        }
        StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Defer(_) | StmtKind::Drop(_) => {}
    }
}

fn walk_expr_moves(e: &Expr, cond: u32, out: &mut HashSet<LocalId>) {
    if cond > 0 {
        if let ExprKind::Place(p, Use::Move) = &e.kind {
            if p.proj.is_empty() {
                out.insert(p.local);
            }
        }
    }
    match &e.kind {
        ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) => {
            walk_expr_moves(a, cond, out);
            walk_expr_moves(b, cond + 1, out);
        }
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
            walk_expr_moves(a, cond, out);
            walk_expr_moves(b, cond, out);
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
        | ExprKind::ToDynamic { value: a, .. } => walk_expr_moves(a, cond, out),
        ExprKind::Call { callee, args } => {
            if let Callee::Value(v) = callee {
                walk_expr_moves(v, cond, out);
            }
            for a in args {
                walk_expr_moves(&a.value, cond, out);
            }
        }
        ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => {
            for (_, x) in fields {
                walk_expr_moves(x, cond, out);
            }
        }
        ExprKind::Tuple(es) | ExprKind::Closure { captures: es, .. } => {
            for x in es {
                walk_expr_moves(x, cond, out);
            }
        }
        ExprKind::Interpolate(parts) => {
            for p in parts {
                if let Part::Show(x) = p {
                    walk_expr_moves(x, cond, out);
                }
            }
        }
        ExprKind::Catch { value, handler, .. } => {
            walk_expr_moves(value, cond, out);
            walk_block_moves(handler, cond + 1, out);
        }
        ExprKind::If { cond: c, then, els } => {
            walk_expr_moves(c, cond, out);
            walk_block_moves(then, cond + 1, out);
            walk_block_moves(els, cond + 1, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            walk_expr_moves(scrutinee, cond, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    walk_expr_moves(g, cond + 1, out);
                }
                match &a.body {
                    ArmBody::Expr(x) => walk_expr_moves(x, cond + 1, out),
                    ArmBody::Block(b) => walk_block_moves(b, cond + 1, out),
                }
            }
        }
        ExprKind::Block(b) => walk_block_moves(b, cond, out),
        ExprKind::Codec { arg, .. } => walk_expr_moves(arg, cond, out),
        ExprKind::Temp { value, .. } => walk_expr_moves(value, cond, out),
        ExprKind::Const(_) | ExprKind::Place(_, _) | ExprKind::Global(_) | ExprKind::FuncRef(_) => {}
    }
}

fn class_path(ty: &str, prog: &Program) -> String {
    let path = rust_ty(&fortis_types::types::named(ty), prog);
    if path.contains('<') || !path.contains("::") {
        ident(ty)
    } else {
        path
    }
}

fn class_field_ty(prog: &Program, ty: &str, field: &str) -> Type {
    match prog.type_def(ty).map(|d| &d.kind) {
        Some(TypeKind::Class { fields }) => fields.iter().find(|f| f.name == field).map(|f| f.ty.clone()).unwrap_or_else(|| fortis_types::types::unknown("")),
        _ => fortis_types::types::unknown(""),
    }
}

fn pattern_binds(p: &Pattern) -> bool {
    match p {
        Pattern::Bind(_) | Pattern::Otherwise(Some(_)) => true,
        Pattern::Present(i) | Pattern::Success(i) | Pattern::Failure(i) => pattern_binds(i),
        Pattern::Variant { fields, .. } | Pattern::Class { fields, .. } | Pattern::Record(fields) => fields.iter().any(|(_, sp)| pattern_binds(sp)),
        Pattern::Tuple(ps) => ps.iter().any(pattern_binds),
        Pattern::Is { bind, fields, .. } => bind.is_some() || fields.iter().any(|(_, sp)| pattern_binds(sp)),
        _ => false,
    }
}

fn pattern_locals(p: &Pattern) -> Vec<LocalId> {
    let mut out = Vec::new();
    fn walk(p: &Pattern, out: &mut Vec<LocalId>) {
        match p {
            Pattern::Bind(l) | Pattern::Otherwise(Some(l)) => out.push(*l),
            Pattern::Present(i) | Pattern::Success(i) | Pattern::Failure(i) => walk(i, out),
            Pattern::Variant { fields, .. } | Pattern::Class { fields, .. } | Pattern::Record(fields) => {
                for (_, sp) in fields {
                    walk(sp, out);
                }
            }
            Pattern::Tuple(ps) => {
                for sp in ps {
                    walk(sp, out);
                }
            }
            Pattern::Is { bind, fields, .. } => {
                if let Some(l) = bind {
                    out.push(*l);
                }
                for (_, sp) in fields {
                    walk(sp, out);
                }
            }
            _ => {}
        }
    }
    walk(p, &mut out);
    out
}
