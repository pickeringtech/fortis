//! Blocks and statements.

use fortis_syntax::lexer::Kind;
use fortis_syntax::view::{C, N};
use fortis_types::types::*;

use super::{err, line_of, tok_line, BResult, Builder};
use crate::model::*;

/// The binding takes the constructor's disambiguated result (`Currency@fortis.money`) when the annotation names
/// the same enum by its bare name (D-142).
fn adopt_shadow(declared: Type, init: &Expr) -> Type {
    match (&declared, &init.ty) {
        (Type::Fn { capture_origins, param_modes, ret: want, params, kind, modifiers, origin, dynamic, identity }, Type::Fn { ret: have, .. }) if nominal_extends(want, have) => Type::Fn {
            capture_origins: capture_origins.clone(),
            param_modes: param_modes.clone(),
            params: params.clone(),
            ret: have.clone(),
            kind: kind.clone(),
            modifiers: modifiers.clone(),
            origin: origin.clone(),
            dynamic: *dynamic,
            identity: identity.clone(),
        },
        (Type::Named(w), Type::Named(h)) if extends(&w.name, &h.name) => init.ty.clone(),
        (Type::Refined { base, .. }, Type::Named(h)) if extends(&base.name, &h.name) => init.ty.clone(),
        _ => declared,
    }
}

fn nominal_extends(want: &Type, have: &Type) -> bool {
    match (strip(want), strip(have)) {
        (Type::Named(w), Type::Named(h)) => extends(&w.name, &h.name),
        _ => false,
    }
}

fn extends(bare: &str, ir: &str) -> bool {
    ir.split_once('@').is_some_and(|(n, _)| n == bare)
}

impl<'t, 'a> Builder<'t, 'a> {
    pub fn fb(&mut self) -> &mut super::Fb {
        self.fbs.last_mut().unwrap()
    }

    /// A lexical block: a scope whose declared locals are destroyed at its exit.
    pub fn block(&mut self, blk: N<'a>) -> BResult<Block> {
        self.fb().push_scope();
        let mut out = Block { line: line_of(blk), ..Default::default() };
        let r = self.block_body(blk, &mut out);
        let locals = self.fb().pop_scope();
        r?;
        out.locals = locals;
        Ok(out)
    }

    fn block_body(&mut self, blk: N<'a>, out: &mut Block) -> BResult<()> {
        for st in blk.child_nodes("statement") {
            if let Some(d) = st.child_node("deferStmt") {
                let b = self.block(d.find("block").unwrap())?;
                out.stmts.push(Stmt { line: line_of(st), temps: vec![], kind: StmtKind::Defer(out.defers.len()) });
                out.defers.push(b);
                continue;
            }
            let s = self.statement(st)?;
            out.stmts.extend(s);
        }
        Ok(())
    }

    fn statement(&mut self, st: N<'a>) -> BResult<Vec<Stmt>> {
        self.fb().stmt_temps.push(vec![]);
        let result = self.statement_built(st);
        let temps = self.fb().stmt_temps.pop().unwrap_or_default();
        result.map(|mut ss| {
            if let Some(s) = ss.last_mut() {
                s.temps.extend(temps);
            }
            ss
        })
    }

    fn statement_built(&mut self, st: N<'a>) -> BResult<Vec<Stmt>> {
        let line = line_of(st);
        let one = |kind: StmtKind| Ok(vec![Stmt { line, kind, temps: vec![] }]);
        let first = st.kids()[0];
        let n = match first {
            C::N(n) => n,
            C::T(_) => {
                let words: Vec<String> = st.direct_tokens().iter().map(|t| t.text.clone()).collect();
                let e = st.child_node("expr");
                return match words[0].as_str() {
                    "return" => {
                        let ret = self.fbs.last().unwrap().ret.clone();
                        let v = match e {
                            Some(e) => {
                                let mut v = self.expr_moved(e)?;
                                // `return x` of a local `let` binding moves it, as `return move x` does (X-345)
                                if self.ty.implicit_moves.contains(&e.id()) {
                                    if let ExprKind::Place(p, Use::Read) = v.kind {
                                        v.kind = ExprKind::Place(p, Use::Move);
                                    }
                                }
                                Some(self.coerce(v, &ret))
                            }
                            None => None,
                        };
                        one(StmtKind::Return(v))
                    }
                    "give" => {
                        let v = self.expr_moved(e.unwrap())?;
                        one(StmtKind::Give(v))
                    }
                    "recover" => {
                        let v = self.expr_moved(e.unwrap())?;
                        one(StmtKind::Recover(v))
                    }
                    "report" => {
                        let v = self.expr(e.unwrap())?;
                        let bind = match words.iter().position(|w| w == "as") {
                            Some(i) => Some(self.fb().declare(&words[i + 1], named("Report"), false, LocalKind::Binding)),
                            None => None,
                        };
                        one(StmtKind::Report { value: v, bind })
                    }
                    "discard" => {
                        let v = self.expr(e.unwrap())?;
                        // `discard move p` consumes the place itself (7.9): the VM decides at the place whether the
                        // local owns what it destroys, so the moved place is not held as a temporary of its own
                        let v = match v.kind {
                            ExprKind::Temp { local, value } if matches!(value.kind, ExprKind::Place(_, Use::Move)) => {
                                self.untrack_temp(local);
                                *value
                            }
                            _ => v,
                        };
                        one(StmtKind::Discard(v))
                    }
                    "break" => one(StmtKind::Break(words.get(1).cloned())),
                    "continue" => one(StmtKind::Continue(words.get(1).cloned())),
                    w => err(line, format!("statement `{w}`")),
                };
            }
        };
        match n.name() {
            "letStmt" => self.let_stmt(n),
            "assignStmt" => self.assign_stmt(n),
            "ifStmt" => {
                let s = self.if_stmt(n)?;
                one(s)
            }
            "whileStmt" => {
                let label = n.find("label").map(|l| l.direct_tokens()[0].text.clone());
                let cond = self.seal_cond(n.child_node("expr").unwrap())?;
                let cl = cond.line;
                let exit = Stmt {
                    line: cl,
                    temps: vec![],
                    kind: StmtKind::If {
                        cond: Expr::new(ExprKind::Unary(UnOp::Not, Box::new(cond)), boolean(), cl),
                        then: Block { stmts: vec![Stmt { line: cl, temps: vec![], kind: StmtKind::Break(label.clone()) }], line: cl, ..Default::default() },
                        els: None,
                    },
                };
                let body = self.block(n.find("block").unwrap())?;
                let bl = body.line;
                one(StmtKind::Loop { label, body: Block { stmts: vec![exit, Stmt { line: bl, temps: vec![], kind: StmtKind::Block(body) }], line: bl, ..Default::default() } })
            }
            "loopStmt" => {
                let label = n.find("label").map(|l| l.direct_tokens()[0].text.clone());
                let body = self.block(n.find("block").unwrap())?;
                one(StmtKind::Loop { label, body })
            }
            "forStmt" => self.for_stmt(n),
            "block" => {
                let b = self.block(n)?;
                one(StmtKind::Block(b))
            }
            "exprStmt" => {
                let e = n.kids()[0].node().unwrap();
                if let Some((_, level, al)) = fortis_names::reading::log_call(n) {
                    if self.resolve("log", line_of(n)).is_none() && self.ty.dc.table.get("log").is_none() {
                        // a log statement (13, LANG-13-04): one `log` record on the diagnostic plane, as an intrinsic
                        let mut args = Vec::new();
                        for a in al.map(|al| al.child_nodes("arg")).unwrap_or_default() {
                            if let Some(x) = a.child_node("expr") {
                                args.push(Arg { param: "message".into(), mode: Mode::Read, value: self.expr(x)? });
                            }
                        }
                        let call = Expr::new(ExprKind::Call { callee: Callee::Intrinsic(format!("log.{}", level.text)), args }, void(), line_of(n));
                        return one(StmtKind::Expr(call));
                    }
                }
                if let Some(m) = Self::bare(e, "matchExpr") {
                    let (scrutinee, arms) = self.match_parts(m, false)?;
                    return one(StmtKind::Match { scrutinee, arms });
                }
                let v = self.expr(e)?;
                one(StmtKind::Expr(v))
            }
            "captureStmt" => {
                let ts = n.direct_tokens();
                let v = self.expr(n.child_node("expr").unwrap())?;
                one(StmtKind::Capture { field: ts[1].text.clone(), value: v })
            }
            "recordStmt" => {
                let ts = n.direct_tokens();
                let mut fields = Vec::new();
                if let Some(al) = n.child_node("argList") {
                    for a in al.child_nodes("arg") {
                        let at = a.direct_tokens();
                        let name = if at.len() > 1 && at[1].text == ":" { at[0].text.clone() } else { String::new() };
                        let v = self.expr(a.child_node("expr").unwrap())?;
                        fields.push((name, v));
                    }
                }
                let schema = self.ty.dc.table.get(&ts[1].text).map(|d| self.tname(&d.name, &d.type_identity())).unwrap_or_else(|| ts[1].text.clone());
                one(StmtKind::Record { schema, fields })
            }
            "expectStmt" => {
                let e = n.child_node("expr").unwrap();
                let evidence = match self.comparison(e) {
                    Some((a, op, b)) => Some((self.expr_c(a)?, op, self.expr_c(b)?)),
                    None => None,
                };
                let cond = self.expr(e)?;
                one(StmtKind::Expect { cond, evidence })
            }
            "scopeStmt" => {
                let name = n.direct_tokens().get(1).map(|t| t.text.clone()).unwrap_or_default();
                let blk = match n.find("block") {
                    Some(b) => b,
                    None => return err(line, "`scope` without a body"),
                };
                // the scope's name is a local of its body, so it ends with it
                self.fb().push_scope();
                let l = self.fb().add_local(&name, named("Scope"), false, LocalKind::Let);
                self.fb().bind(&name, l);
                let mut body = Block { line: line_of(blk), ..Default::default() };
                let r = self.block_body(blk, &mut body);
                let locals = self.fb().pop_scope();
                r?;
                body.locals = locals;
                one(StmtKind::Scope { local: l, body })
            }
            other => err(line, format!("statement {other}")),
        }
    }

    /// The node under a chain of single-child wrappers, when it has the given rule.
    pub fn bare(mut e: N<'a>, rule: &str) -> Option<N<'a>> {
        loop {
            if e.name() == rule {
                return Some(e);
            }
            let k = e.kids();
            if k.len() != 1 {
                return None;
            }
            match k[0] {
                C::N(n) => e = n,
                C::T(_) => return None,
            }
        }
    }

    /// `a op b` under single-child wrappers: the evidence of a false `expect` (14).
    fn comparison(&self, e: N<'a>) -> Option<(C<'a>, String, C<'a>)> {
        let mut node = e;
        while ["expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr"].contains(&node.name()) && node.kids().len() == 1 {
            node = node.kids()[0].node()?;
        }
        let k = node.kids();
        if ["equalityExpr", "relationalExpr"].contains(&node.name()) && k.len() == 3 {
            return Some((k[0], k[1].tok()?.text.clone(), k[2]));
        }
        None
    }

    fn let_stmt(&mut self, n: N<'a>) -> BResult<Vec<Stmt>> {
        let line = line_of(n);
        let ts = n.direct_tokens();
        if let Some(pat) = n.child_node("pattern") {
            let pl = n.child_node("place").unwrap();
            let init = self.place_expr(pl, Use::Move)?;
            let pt = init.ty.clone();
            let p = self.pattern(pat, &pt, true)?;
            return Ok(vec![Stmt { line, temps: vec![], kind: StmtKind::LetPattern { pattern: p, init } }]);
        }
        let mutable = ts.iter().any(|t| t.text == "mut");
        let nm = ts.iter().find(|t| t.kind == Kind::Lower).unwrap().text.clone();
        let e = n.child_node("expr").unwrap();
        let declared = n.child_node("type").map(|t| self.ty.dc.type_of(Some(t)));
        let init = self.expr_moved(e)?;
        let (ty, init) = match declared {
            Some(t) => {
                let v = self.coerce(init, &t);
                // `let f: fn() money.Currency = money.Currency.GBP` keeps the standard enum when the program
                // declares `Currency` too: the constructor's type names it `Currency@fortis.money` (D-142).
                (adopt_shadow(t, &v), v)
            }
            None => {
                let t = fortis_types::literals::concrete(&init.ty);
                (t, init)
            }
        };
        let local = self.fb().declare(&nm, ty, mutable, LocalKind::Let);
        Ok(vec![Stmt { line, temps: vec![], kind: StmtKind::Let { local, init } }])
    }

    fn assign_stmt(&mut self, n: N<'a>) -> BResult<Vec<Stmt>> {
        let line = line_of(n);
        let op = n.direct_tokens().iter().map(|t| t.text.clone()).find(|t| ["=", "+=", "-=", "*=", "/=", "%="].contains(&t.as_str())).unwrap();
        let (place, pty) = self.place_of(n.child_node("place").unwrap())?;
        let v = self.expr_moved(n.child_node("expr").unwrap())?;
        if op == "=" {
            let v = self.coerce(v, &pty);
            let drop_old = !self.ty.dc.is_trivial(strip(&pty));
            return Ok(vec![Stmt { line, temps: vec![], kind: StmtKind::Assign { place, value: v, drop_old } }]);
        }
        let bop = BinOp::from_symbol(&op[..1]).unwrap();
        Ok(vec![Stmt { line, temps: vec![], kind: StmtKind::Compound { place, op: bop, value: v } }])
    }

    fn if_stmt(&mut self, n: N<'a>) -> BResult<StmtKind> {
        let kids = n.kids();
        // collect (cond, block) pairs and the final else block
        let mut arms: Vec<(N<'a>, N<'a>)> = Vec::new();
        let mut els: Option<N<'a>> = None;
        let mut i = 0;
        while i < kids.len() {
            if kids[i].text_is("if") {
                arms.push((kids[i + 1].node().unwrap(), kids[i + 2].node().unwrap()));
                i += 3;
            } else if kids[i].text_is("else") && kids.get(i + 1).map_or(false, |k| k.is("block")) {
                els = kids[i + 1].node();
                i += 2;
            } else {
                i += 1;
            }
        }
        let mut tail: Option<Block> = match els {
            Some(b) => Some(self.block(b)?),
            None => None,
        };
        // build inside-out so that conditions are lowered in source order afterwards
        let mut built: Vec<(Expr, Block)> = Vec::new();
        for (c, b) in &arms {
            let cond = self.seal_cond(*c)?;
            let blk = self.block(*b)?;
            built.push((cond, blk));
        }
        let mut result: Option<StmtKind> = None;
        for (cond, then) in built.into_iter().rev() {
            let line = cond.line;
            let kind = StmtKind::If { cond, then, els: tail.take() };
            tail = Some(Block { stmts: vec![Stmt { line, temps: vec![], kind: kind.clone() }], line, ..Default::default() });
            result = Some(kind);
        }
        Ok(result.unwrap())
    }

    fn for_stmt(&mut self, n: N<'a>) -> BResult<Vec<Stmt>> {
        let line = line_of(n);
        let label = n.find("label").map(|l| l.direct_tokens()[0].text.clone());
        let ts = n.direct_tokens();
        let var = ts[ts.iter().position(|t| t.text == "for").unwrap() + 1].text.clone();
        let src = n.kids().into_iter().filter_map(|c| c.node()).find(|c| ["expr", "rangeExpr", "place"].contains(&c.name())).unwrap();
        let (source, elem) = if src.name() == "rangeExpr" {
            let rk = src.kids();
            let lo = self.expr_c(rk[0])?;
            let hi = self.expr_c(rk[2])?;
            (ForSource::Range { lo, hi, inclusive: rk[1].text_is("..") }, int())
        } else {
            let v = if src.name() == "place" { self.place_expr(src, Use::Move)? } else { self.expr(src)? };
            let st = self.ty.collection_representation(&v.ty).map(|(_, rep)| rep).unwrap_or_else(|| strip(&v.ty).clone());
            let st = match st {
                Type::Refined { base, .. } => Type::Named(base),
                t => t,
            };
            let moved = src.name() == "place";
            let (v, elem) = match &st {
                Type::Named(nt) if nt.package.is_empty() && ["List", "ListView", "Array", "Set", "FixedList"].contains(&nt.name.as_str()) && !nt.args.is_empty() => (v, nt.args[0].clone()),
                Type::Named(nt) if nt.package.is_empty() && ["Map", "OrderedMap", "FixedMap"].contains(&nt.name.as_str()) && nt.args.len() >= 2 => (v, Type::Tuple(vec![nt.args[0].clone(), nt.args[1].clone()])),
                // a class that implements `Iterable[T, I]` (5.3, 7.7): the loop walks the cursor `iterate()` gives (D-545)
                _ if !moved => match self.iterate_call(v, &st, line) {
                    Ok((call, elem)) => (call, elem),
                    Err(v) => (v, unknown("loop item")),
                },
                _ => match self.consume_call(v, &st, line) {
                    Ok((cursor, elem, next)) => return self.consuming_for(n, label, &var, cursor, elem, next),
                    Err(v) => (v, unknown("loop item")),
                },
            };
            (ForSource::Items(v), elem)
        };
        self.fb().push_scope();
        let binding = if var == "_" { None } else { Some(self.fb().declare(&var, elem, false, LocalKind::Binding)) };
        let body = self.block(n.find("block").unwrap());
        self.fb().pop_scope();
        let body = body?;
        let _ = tok_line;
        Ok(vec![Stmt { line, temps: vec![], kind: StmtKind::For { label, binding, source, body } }])
    }

    fn consuming_for(&mut self, n: N<'a>, label: Option<String>, var: &str, cursor: Expr, elem: Type, next: Callee) -> BResult<Vec<Stmt>> {
        let line = line_of(n);
        self.fb().push_scope();
        let cursor_ty = cursor.ty.clone();
        let local = self.fb().temp(cursor_ty.clone());
        self.fb().push_scope();
        let item = self.fb().declare(var, elem.clone(), false, LocalKind::Binding);
        let body = self.block(n.find("block").unwrap())?;
        let items = self.fb().pop_scope();
        let next = Expr::new(ExprKind::Call {
            callee: next,
            args: vec![Arg { param: "self".into(), mode: Mode::Editable, value: Expr::new(ExprKind::Place(Place::local(local), Use::Editable), cursor_ty, line) }],
        }, Type::Optional(Box::new(elem)), line);
        let step = Stmt::new(line, StmtKind::Match {
            scrutinee: next,
            arms: vec![
                Arm { pattern: Pattern::Present(Box::new(Pattern::Bind(item))), guard: None, locals: items, body: ArmBody::Block(body) },
                Arm { pattern: Pattern::Null, guard: None, locals: vec![], body: ArmBody::Block(Block { stmts: vec![Stmt::new(line, StmtKind::Break(None))], line, ..Default::default() }) },
            ],
        });
        let locals = self.fb().pop_scope();
        let block = Block { locals, line, stmts: vec![
            Stmt::new(line, StmtKind::Let { local, init: cursor }),
            Stmt::new(line, StmtKind::Loop { label, body: Block { stmts: vec![step], line, ..Default::default() } }),
        ], ..Default::default() };
        Ok(vec![Stmt::new(line, StmtKind::Block(block))])
    }
}
