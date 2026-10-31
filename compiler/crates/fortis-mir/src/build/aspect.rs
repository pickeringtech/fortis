//! Aspect weaving (Language Reference 11.8, as interpreter/eval.py `call_with_aspects` elaborates it): before hooks
//! in application order, the body, after hooks in reverse, and on a failure the handlers innermost first, where a
//! handler that returns a value completes the call and one that fails passes its failure on.
//!
//! Each hook block becomes a `try` function over the aspect's parameters, its `needs` bindings and the hook's own
//! binding (the result or the failure); the original body becomes `<label>#body`; the handlers are chained in
//! `<label>#handlers`; the woven function sequences them with explicit failure tests.

use fortis_syntax::lexer::Kind;
use fortis_syntax::view::{C, N};
use fortis_types::decls::{DeclRef, Sig};
use fortis_types::types::*;

use super::fb::Fb;
use super::{err, line_of, BResult, BuildError, Builder};
use crate::model::*;

/// One applied aspect: its declaration and the values of its parameters (expressions over constants).
struct Applied<'a> {
    decl: DeclRef<'a>,
    args: Vec<(String, Type, Expr)>,
    /// (needs name, target parameter local)
    needs: Vec<(String, Type, LocalId)>,
}

impl<'t, 'a> Builder<'t, 'a> {
    fn applied(&mut self, uses: &[N<'a>], target: &Fb) -> BResult<Vec<Applied<'a>>> {
        let mut out = Vec::new();
        for u in uses {
            let ts = u.direct_tokens();
            let aname = ts[1].text.clone();
            if self.core_aspect(&aname) {
                continue;
            }
            let line = line_of(*u);
            // evaluate the use's arguments, then follow specialisations to the base aspect
            let mut given: Vec<(Option<String>, Expr)> = Vec::new();
            for a in u.find("argList").map(|al| al.child_nodes("arg")).unwrap_or_default() {
                let at = a.direct_tokens();
                let lbl = if at.len() > 1 && at[0].kind == Kind::Lower && at[1].text == ":" { Some(at[0].text.clone()) } else { None };
                let e = a.child_node("expr").ok_or_else(|| BuildError { line, message: "an aspect argument without a value".into() })?;
                given.push((lbl, self.const_expr(e)?));
            }
            let mut d = self.ty.dc.table.get(&aname).cloned().ok_or_else(|| BuildError { line, message: format!("`@{aname}` is not a declared aspect") })?;
            loop {
                let info = d.aspect.as_ref().ok_or_else(|| BuildError { line, message: format!("`{aname}` is not an aspect") })?;
                let mut bound: Vec<(String, Type, Expr)> = Vec::new();
                for (i, (lbl, v)) in given.iter().enumerate() {
                    let name = match lbl {
                        Some(l) if info.params.iter().any(|p| &p.0 == l) => l.clone(),
                        None if i == 0 && !info.params.is_empty() => info.params[0].0.clone(),
                        _ => continue,
                    };
                    let pt = info.params.iter().find(|p| p.0 == name).map(|p| p.1.clone()).unwrap_or_else(|| unknown(""));
                    bound.push((name, pt, v.clone()));
                }
                for (pn, pt, _) in &info.params {
                    if bound.iter().any(|b| &b.0 == pn) {
                        continue;
                    }
                    if let Some(ce) = self.aspect_param_default(&d, pn) {
                        let v = self.const_expr(ce)?;
                        bound.push((pn.clone(), pt.clone(), v));
                    }
                }
                match &info.base {
                    None => {
                        let mut needs = Vec::new();
                        for (nn, nt, _) in &info.needs {
                            let want = base_name(strip(nt)).map(|s| s.to_string());
                            let p = target.params.iter().find(|p| base_name(strip(&target.locals[p.local as usize].ty)).map(|s| s.to_string()) == want);
                            if let Some(p) = p {
                                needs.push((nn.clone(), nt.clone(), p.local));
                            }
                        }
                        out.push(Applied { decl: d.clone(), args: bound, needs });
                        break;
                    }
                    Some((bname, bal, _)) => {
                        // the base's arguments are written over this aspect's parameters
                        let mut next: Vec<(Option<String>, Expr)> = Vec::new();
                        for a in bal.map(|al| al.child_nodes("arg")).unwrap_or_default() {
                            let at = a.direct_tokens();
                            if at.first().map_or(false, |t| t.text == "...") {
                                for (lbl, v) in &given {
                                    if lbl.as_ref().map_or(false, |l| !info.params.iter().any(|p| &p.0 == l)) {
                                        next.push((lbl.clone(), v.clone()));
                                    }
                                }
                                continue;
                            }
                            let lbl = if at.len() > 1 && at[0].kind == Kind::Lower && at[1].text == ":" { Some(at[0].text.clone()) } else { None };
                            let e = a.child_node("expr").unwrap();
                            let toks = e.tokens();
                            let v = if toks.len() == 1 && toks[0].kind == Kind::Lower && bound.iter().any(|b| b.0 == toks[0].text) {
                                bound.iter().find(|b| b.0 == toks[0].text).unwrap().2.clone()
                            } else {
                                self.const_expr(e)?
                            };
                            next.push((lbl, v));
                        }
                        given = next;
                        d = self.ty.dc.table.get(bname).cloned().ok_or_else(|| BuildError { line, message: format!("base aspect `{bname}` is not declared") })?;
                    }
                }
            }
        }
        Ok(out)
    }

    fn aspect_param_default(&self, d: &DeclRef<'a>, pn: &str) -> Option<N<'a>> {
        let ap = d.node?.find("aspectParams")?;
        for p in ap.child_nodes("param") {
            if p.direct_tokens().first().map_or(false, |t| t.text == pn) {
                return p.child_node("constExpr");
            }
        }
        None
    }

    /// A hook block as a `try` function over the aspect's parameters, its needs and the hook's own binding.
    fn hook_fn(&mut self, a: &Applied<'a>, kind: &str, blk: N<'a>, extra: Option<(String, Type)>, ret: Type, target: &str) -> BResult<String> {
        let label = format!("aspect {} {kind}@{}", a.decl.name, line_of(blk));
        let name = self.qualified(&a.decl.module, &label);
        if self.prog.functions.iter().any(|f| f.name == name) {
            return Ok(name);
        }
        let mut fb = Fb::new(&label, &a.decl.module, ret, true);
        for (pn, pt, _) in &a.args {
            let l = fb.add_local(pn, pt.clone(), false, LocalKind::Param);
            fb.bind(pn, l);
            fb.params.push(Param { local: l, name: pn.clone(), mode: Mode::Read, default: None });
        }
        for (nn, nt, _) in &a.needs {
            let l = fb.add_local(nn, nt.clone(), false, LocalKind::Param);
            fb.bind(nn, l);
            fb.params.push(Param { local: l, name: nn.clone(), mode: Mode::Read, default: None });
        }
        if let Some((en, et)) = &extra {
            let l = fb.add_local(en, et.clone(), false, LocalKind::Param);
            fb.bind(en, l);
            fb.params.push(Param { local: l, name: en.clone(), mode: Mode::Owned, default: None });
        }
        self.fbs.push(fb);
        let body = self.block(blk);
        let fb = self.fbs.pop().unwrap();
        let body = body?;
        // a hook's effects are part of every target's summary (11.8): it carries the target's
        let effects = self.effects_of(target);
        self.prog.functions.push(fb.finish(line_of(blk), Some(body), vec!["try".into()], effects, None, None));
        Ok(name)
    }

    /// The arguments of a hook call made from a function whose locals hold the aspect's values and needs.
    fn hook_args(a: &Applied<'a>, values: &[(String, LocalId, Type)], needs: &[(String, LocalId, Type)], extra: Option<(String, Expr)>, line: u32) -> Vec<Arg> {
        let mut out = Vec::new();
        for (pn, l, t) in values {
            out.push(Arg { param: pn.clone(), mode: Mode::Read, value: Expr::new(ExprKind::Place(Place::local(*l), Use::Read), t.clone(), line) });
        }
        for (nn, l, t) in needs {
            out.push(Arg { param: nn.clone(), mode: Mode::Read, value: Expr::new(ExprKind::Place(Place::local(*l), Use::Read), t.clone(), line) });
        }
        if let Some((en, e)) = extra {
            out.push(Arg { param: en, mode: Mode::Owned, value: e });
        }
        let _ = a;
        out
    }

    /// A function with applied aspects: its body, hooks and handlers as functions, and the woven function.
    pub fn woven_function(&mut self, sig: &Sig<'a>, label: &str, self_t: Option<Type>, uses: Vec<N<'a>>) -> BResult<()> {
        let first = self.prog.functions.len();
        let bounds = self.bounds_of(sig, self_t.as_ref());
        let r = self.woven_function_inner(sig, label, self_t, uses);
        // every function the weave makes (the wrapper, its hooks and handlers) keeps the target's `where` bounds, so
        // a backend that writes them generically can call the bounded body (D-82)
        for f in self.prog.functions[first..].iter_mut() {
            if f.bounds.is_empty() {
                f.bounds = bounds.clone();
            }
        }
        r
    }

    fn woven_function_inner(&mut self, sig: &Sig<'a>, label: &str, self_t: Option<Type>, uses: Vec<N<'a>>) -> BResult<()> {
        let body_label = format!("{label}#body");
        self.function_plain(sig, &body_label, self_t.clone(), uses.clone())?;
        let line = sig.node.map(line_of).unwrap_or(0);
        let is_try = sig.has_mod("try");
        let target = self.target_fb(sig, label, self_t);
        let chain = self.applied(&uses, &target)?;
        let succ = sig.ret.clone();
        let outcome = |t: &Type| named_args("Outcome", vec![t.clone()]);
        // hook functions
        let mut befores: Vec<Vec<String>> = Vec::new();
        let mut afters: Vec<Option<(String, String)>> = Vec::new();
        let mut handlers: Vec<Option<(String, String)>> = Vec::new();
        for a in &chain {
            let info = a.decl.aspect.as_ref().unwrap();
            let mut bs = Vec::new();
            for blk in info.before.clone() {
                bs.push(self.hook_fn(a, "before", blk, None, void(), label)?);
            }
            befores.push(bs);
            afters.push(match info.after.clone() {
                Some((rn, rt, Some(blk))) => {
                    let rt = if known(&rt) { rt } else { succ.clone() };
                    Some((rn.clone(), self.hook_fn(a, "after", blk, Some((rn, rt)), void(), label)?))
                }
                _ => None,
            });
            handlers.push(match info.on_failure.clone() {
                Some((fname, ht, Some(blk), _)) => Some((fname.clone(), self.hook_fn(a, "failure", blk, Some((fname, named("Failure"))), ht, label)?)),
                _ => None,
            });
        }
        // the woven function
        let mut fb = target;
        fb.push_scope();
        let mut stmts: Vec<Stmt> = Vec::new();
        let mut vals: Vec<Vec<(String, LocalId, Type)>> = Vec::new();
        let mut needs: Vec<Vec<(String, LocalId, Type)>> = Vec::new();
        for a in &chain {
            let mut vs = Vec::new();
            for (pn, pt, v) in &a.args {
                let l = fb.declare(&format!("{}.{pn}", a.decl.name), pt.clone(), false, LocalKind::Temp);
                stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Let { local: l, init: v.clone() } });
                vs.push((pn.clone(), l, pt.clone()));
            }
            vals.push(vs);
            needs.push(a.needs.iter().map(|(nn, nt, l)| (nn.clone(), *l, nt.clone())).collect());
        }
        // the handler chain: <label>#handlers(failure, every aspect's values and needs)
        let handlers_name = self.handlers_fn(label, &chain, &vals, &needs, &handlers, &succ, line)?;
        let handle_call = |fb: &mut Fb, failure: Expr| -> Stmt {
            let mut args = vec![Arg { param: "failure".into(), mode: Mode::Owned, value: failure }];
            for (i, vs) in vals.iter().enumerate() {
                for (pn, l, t) in vs {
                    args.push(Arg { param: format!("{i}.{pn}"), mode: Mode::Read, value: Expr::new(ExprKind::Place(Place::local(*l), Use::Read), t.clone(), line) });
                }
                for (nn, l, t) in &needs[i] {
                    args.push(Arg { param: format!("{i}.{nn}"), mode: Mode::Read, value: Expr::new(ExprKind::Place(Place::local(*l), Use::Read), t.clone(), line) });
                }
            }
            let _ = fb;
            let call = Expr::new(ExprKind::Call { callee: Callee::Func(handlers_name.clone()), args }, outcome(&succ), line);
            let v = if is_try { Expr::new(ExprKind::OrFail(Box::new(call)), succ.clone(), line) } else { call };
            Stmt { line, temps: vec![], kind: StmtKind::Return(Some(v)) }
        };
        let guard = |fb: &mut Fb, call: Expr, ty: Type, stmts_handler: &dyn Fn(&mut Fb, Expr) -> Stmt| -> Expr {
            fb.push_scope();
            let f = fb.declare("failure", named("Failure"), false, LocalKind::Binding);
            let ret = stmts_handler(fb, Expr::new(ExprKind::Place(Place::local(f), Use::Move), named("Failure"), line));
            fb.pop_scope();
            Expr::new(ExprKind::Catch { value: Box::new(call), bind: f, handler: Block { stmts: vec![ret], line, ..Default::default() } }, ty, line)
        };
        for (i, a) in chain.iter().enumerate() {
            for b in &befores[i] {
                let call = Expr::new(ExprKind::Call { callee: Callee::Func(b.clone()), args: Self::hook_args(a, &vals[i], &needs[i], None, line) }, outcome(&void()), line);
                let g = guard(&mut fb, call, void(), &handle_call);
                stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Expr(g) });
            }
        }
        let body_args: Vec<Arg> = fb.params.iter().map(|p| Arg { param: p.name.clone(), mode: p.mode, value: Expr::new(ExprKind::Place(Place::local(p.local), if p.mode == Mode::Owned { Use::Move } else { Use::Read }), fb.locals[p.local as usize].ty.clone(), line) }).collect();
        let body_call = Expr::new(ExprKind::Call { callee: Callee::Func(self.qualified(&self.ir_module, &body_label)), args: body_args }, if is_try { outcome(&succ) } else { succ.clone() }, line);
        let result = fb.declare("result", succ.clone(), false, LocalKind::Temp);
        let init = if is_try { guard(&mut fb, body_call, succ.clone(), &handle_call) } else { body_call };
        stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Let { local: result, init } });
        for (i, a) in chain.iter().enumerate().rev() {
            if let Some((rn, f)) = &afters[i] {
                let rv = Expr::new(ExprKind::Place(Place::local(result), Use::Read), succ.clone(), line);
                let call = Expr::new(ExprKind::Call { callee: Callee::Func(f.clone()), args: Self::hook_args(a, &vals[i], &needs[i], Some((rn.clone(), rv)), line) }, outcome(&void()), line);
                let g = guard(&mut fb, call, void(), &handle_call);
                stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Expr(g) });
            }
        }
        stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Return(Some(Expr::new(ExprKind::Place(Place::local(result), Use::Move), succ.clone(), line))) });
        let locals = fb.pop_scope();
        let body = Block { stmts, locals, line, defers: vec![] };
        let effects = self.effects_of(label);
        self.prog.functions.push(fb.finish(line, Some(body), sig.modifiers.clone(), effects, None, None));
        Ok(())
    }

    /// `<label>#handlers(failure, ...)`: each handler, innermost first; one that returns completes the call.
    fn handlers_fn(
        &mut self,
        label: &str,
        chain: &[Applied<'a>],
        vals: &[Vec<(String, LocalId, Type)>],
        needs: &[Vec<(String, LocalId, Type)>],
        handlers: &[Option<(String, String)>],
        succ: &Type,
        line: u32,
    ) -> BResult<String> {
        let hl = format!("{label}#handlers");
        let mut fb = Fb::new(&hl, &self.ir_module, succ.clone(), true);
        let cur = fb.add_local("failure", named("Failure"), true, LocalKind::Param);
        fb.params.push(Param { local: cur, name: "failure".into(), mode: Mode::Owned, default: None });
        let mut mine: Vec<Vec<(String, LocalId, Type)>> = Vec::new();
        let mut my_needs: Vec<Vec<(String, LocalId, Type)>> = Vec::new();
        for (i, vs) in vals.iter().enumerate() {
            let mut m = Vec::new();
            for (pn, _, t) in vs {
                let l = fb.add_local(&format!("{i}.{pn}"), t.clone(), false, LocalKind::Param);
                fb.params.push(Param { local: l, name: format!("{i}.{pn}"), mode: Mode::Read, default: None });
                m.push((pn.clone(), l, t.clone()));
            }
            mine.push(m);
            let mut n = Vec::new();
            for (nn, _, t) in &needs[i] {
                let l = fb.add_local(&format!("{i}.{nn}"), t.clone(), false, LocalKind::Param);
                fb.params.push(Param { local: l, name: format!("{i}.{nn}"), mode: Mode::Read, default: None });
                n.push((nn.clone(), l, t.clone()));
            }
            my_needs.push(n);
        }
        let mut stmts = Vec::new();
        let outcome = named_args("Outcome", vec![succ.clone()]);
        for (i, a) in chain.iter().enumerate().rev() {
            let (fname, f) = match &handlers[i] {
                Some(h) => h.clone(),
                None => continue,
            };
            let fv = Expr::new(ExprKind::Place(Place::local(cur), Use::Move), named("Failure"), line);
            let call = Expr::new(ExprKind::Call { callee: Callee::Func(f), args: Self::hook_args(a, &mine[i], &my_needs[i], Some((fname, fv)), line) }, outcome.clone(), line);
            let r = fb.add_local("handled", outcome.clone(), false, LocalKind::Temp);
            let f2 = fb.add_local("next", named("Failure"), false, LocalKind::Binding);
            let v = fb.add_local("value", succ.clone(), false, LocalKind::Binding);
            stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Let { local: r, init: call } });
            stmts.push(Stmt {
                line,
                temps: vec![],
                kind: StmtKind::Match {
                    scrutinee: Expr::new(ExprKind::Place(Place::local(r), Use::Move), outcome.clone(), line),
                    arms: vec![
                        Arm {
                            pattern: Pattern::Failure(Box::new(Pattern::Bind(f2))),
                            guard: None,
                            locals: vec![f2],
                            body: ArmBody::Block(Block {
                                stmts: vec![Stmt { line, temps: vec![], kind: StmtKind::Assign { place: Place::local(cur), value: Expr::new(ExprKind::Place(Place::local(f2), Use::Move), named("Failure"), line), drop_old: false } }],
                                line,
                                ..Default::default()
                            }),
                        },
                        Arm {
                            pattern: Pattern::Success(Box::new(Pattern::Bind(v))),
                            guard: None,
                            locals: vec![v],
                            body: ArmBody::Block(Block { stmts: vec![Stmt { line, temps: vec![], kind: StmtKind::Return(Some(Expr::new(ExprKind::Place(Place::local(v), Use::Move), succ.clone(), line))) }], line, ..Default::default() }),
                        },
                    ],
                },
            });
        }
        stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Fail(Expr::new(ExprKind::Place(Place::local(cur), Use::Move), named("Failure"), line)) });
        let name = fb.name.clone();
        let effects = self.effects_of(label);
        self.prog.functions.push(fb.finish(line, Some(Block { stmts, line, ..Default::default() }), vec!["try".into()], effects, None, None));
        Ok(name)
    }

    /// The function builder of a target function's signature (its parameters as locals), without a body.
    fn target_fb(&mut self, sig: &Sig<'a>, label: &str, self_t: Option<Type>) -> Fb {
        let mut fb = Fb::new(label, &self.ir_module, sig.ret.clone(), sig.has_mod("try"));
        if let (Some(st), Some(recv)) = (&self_t, &sig.receiver) {
            let mode = match recv.as_str() {
                "editable self" => Mode::Editable,
                "owned self" => Mode::Owned,
                _ => Mode::Read,
            };
            let sl = fb.add_local("self", st.clone(), mode != Mode::Read, LocalKind::SelfParam);
            fb.self_local = Some(sl);
            fb.params.push(Param { local: sl, name: "self".into(), mode, default: None });
        }
        for (pn, pt, pm) in &sig.params {
            let mode = match pm.trim_start_matches("...") {
                "editable" => Mode::Editable,
                "owned" => Mode::Owned,
                _ => Mode::Read,
            };
            let l = fb.add_local(pn, pt.clone(), mode == Mode::Editable, LocalKind::Param);
            fb.bind(pn, l);
            fb.params.push(Param { local: l, name: pn.clone(), mode, default: None });
        }
        fb
    }
}

#[allow(dead_code)]
fn _unused(_: C, _: BResult<()>) -> BResult<()> {
    err(0, "")
}
