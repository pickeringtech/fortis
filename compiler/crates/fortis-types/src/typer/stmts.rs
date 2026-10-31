//! Statements, function exits and the module walk (Language Reference 5, 9.4, 12.1, 14): the statement half of
//! checker/typer.py.

use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet};

use fortis_diag::Json;
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{declarations, imports, C, N};

use super::*;
use crate::decls::{named_of_decl, tok_of, Sig};
use crate::effects::Summary;
use crate::intervals::{self as iv, Facts};
use crate::literals::{concrete, interp, is_interp};
use crate::owners::{root_of, Owners};
use crate::types::*;

impl<'a> Typer<'a> {
    /// A `give` or `recover` value: typed as before; when the branch's target type is known (an expected type, or
    /// the success type a catch recovers to), a generic call in it takes that type, so it is no inference failure.
    pub fn branch_infer(&mut self, e: Option<N<'a>>, env: &Env) -> Type {
        let target = self.branch_expected.last().cloned().flatten();
        let was = self.branch_target;
        self.branch_target = target.as_ref().map_or(false, |t| known(&strip(t)));
        let t = self.infer(e, env, target.as_ref());
        self.branch_target = was;
        t
    }

    pub fn walk_block(&mut self, block: N<'a>, env: &Env) {
        let e = env_new(Some(env));
        self.depth += 1;
        let n_deferred = self.deferred.len();
        let n_bodies = self.defer_bodies.len();
        let stmts = block.child_nodes("statement");
        for (i, st) in stmts.iter().enumerate() {
            let st = *st;
            self.block_rest = stmts[i + 1..].to_vec(); // what runs after a `defer` registered here, before it (D-75)
            crate::journal::at(st.tokens().first().map_or(0, |t| t.line));
            self.statement(st, &e);
            self.temporaries_escape(st);
            self.own.end_statement();
            self.pending = None;
            self.carried.clear();
        }
        // D-75: the block's deferred bodies run at its exit, last registered first
        let bodies: Vec<(i64, N<'a>)> = self.defer_bodies[n_bodies..].iter().rev().cloned().collect();
        for (_, blk) in bodies {
            let mut f = std::mem::take(&mut self.facts);
            self.deferred_effects(&mut f, blk);
            self.facts = f;
        }
        self.defer_bodies.truncate(n_bodies);
        if !self.terminates(block) {
            let d = self.depth;
            self.report_open(d, "at the end of the block");
        } else {
            let d = self.depth;
            for (p, (_, od)) in self.own.obligations.items.clone() {
                if od >= d {
                    self.own.close_obligation(&p, 0, "block exit");
                }
            }
        }
        let d = self.depth;
        self.own.end_block(d);
        if self.deferred.len() > n_deferred {
            let runs: Vec<usize> = self.deferred[n_deferred..].iter().rev().map(|x| x.2.line).collect();
            let end_line = block.tokens().last().map_or(0, |t| t.line);
            crate::journal::cleanup("block end", "", end_line, runs);
        }
        self.deferred.truncate(n_deferred);
        self.guards.retain(|g| g.1 < d);
        self.depth -= 1;
    }

    /// A future that keeps a loan of a temporary argument may not outlive it (12.3): the temporary ends with its
    /// statement (5.7), so a statement that binds, assigns, returns or gives such a future is rejected (D-31).
    pub fn temporaries_escape(&mut self, st: N<'a>) {
        // A nested statement ends only its own temporaries, not a surrounding match scrutinee's.
        let ts = st.tokens();
        if let (Some(first), Some(last)) = (ts.first(), ts.last()) {
            self.temporary_borrows.retain(|t| !((first.line, first.col) <= (t.line, t.col) && (t.line, t.col) <= (last.line, last.col)));
        }
        let temps = std::mem::take(&mut self.carried_temps);
        if temps.is_empty() {
            return;
        }
        let escapes = match st.kid(0) {
            Some(C::N(n)) => n.name() == "letStmt" || n.name() == "assignStmt",
            _ => st.direct_tokens().first().map_or(false, |t| t.text == "return" || t.text == "give"),
        };
        if escapes {
            self.temporaries_kept(temps);
        }
    }

    /// E-OWN-051 at each temporary a future that outlives its statement borrows (12.3; D-31).
    pub fn temporaries_kept(&mut self, temps: Vec<(Token, usize)>) {
        for (tok, _) in temps {
            self.own_err(
                "E-OWN-051",
                "this argument is a temporary that ends with the statement, but the future that borrows it outlives the statement: bind it first, or take the parameter `owned` (12.3, 5.7)".to_string(),
                &tok,
                &tok.text,
                "loan of a temporary kept by a future",
                "LANG-07-04",
                "",
            );
        }
    }

    pub fn statement(&mut self, st: N<'a>, env: &Env) {
        let inner = match st.kid(0) {
            Some(C::N(n)) => n,
            _ => {
                self.bare_statement(st, env);
                return;
            }
        };
        match inner.name() {
            "letStmt" => self.let_(inner, env),
            "assignStmt" => self.assign(inner, env),
            "ifStmt" => self.if_statement(inner, env),
            "whileStmt" => {
                let blk = inner.find("block").unwrap();
                self.widen(blk);
                let cond = inner.find("expr").unwrap();
                // the condition is evaluated on every iteration (11.6 counts it as the loop)
                self.ctx.push("whilecond".into());
                let (yes, no, _) = self.condition_states(cond, env);
                self.ctx.pop();
                self.facts = yes;
                let breaks = self.loop_body(inner, blk, env, &[], true, Some(cond));
                // D-72: the loop ends when the condition is false or at a `break` that targets it, with the state there
                let mut exits = vec![no];
                exits.extend(breaks);
                self.facts = Facts::join_all(&exits);
            }
            "loopStmt" => {
                let blk = inner.find("block").unwrap();
                self.widen(blk);
                self.loop_body(inner, blk, env, &[], false, None);
            }
            "forStmt" => self.for_stmt(inner, env),
            "deferStmt" => self.defer_stmt(inner, env),
            "scopeStmt" => {
                let e = env_new(Some(env));
                let sname = inner.direct_tokens()[1].text.clone();
                env_set(&e, &sname, named("Scope"), true, "mut");
                self.scope_depths.push((sname, self.depth + 1));
                self.walk_block(inner.find("block").unwrap(), &e);
                self.scope_depths.pop();
            }
            "captureStmt" => self.capture_stmt(inner, env),
            "recordStmt" => self.record_stmt(inner, env),
            "expectStmt" => {
                if !self.in_test_code {
                    // `expect` is test code (14; D-414): outside a `test` or `property` body it has no meaning
                    let actual = if self.current_label.is_empty() { "a function body".to_string() } else { self.current_label.clone() };
                    self.typ("E-TYP-120", "`expect` outside a `test` or `property` body: it is test code, with no meaning in production code (14)".to_string(), &inner.tokens()[0], "a test or property body", &actual, "expect", "LANG-14-04");
                }
                self.condition(inner.find("expr").unwrap(), env)
            }
            "exprStmt" => self.expr_statement(inner, env),
            "block" => self.walk_block(inner, env),
            _ => {
                for c in inner.kids() {
                    if c.is("expr") {
                        self.infer_c(c, env, None);
                    }
                }
            }
        }
    }

    pub fn widen(&mut self, block: N<'a>) {
        for p in self.assigned_places(block) {
            self.inval_log.push(p.clone());
            self.facts.invalidate(&p);
        }
    }

    pub fn if_statement(&mut self, inner: N<'a>, env: &Env) {
        let conds = inner.child_nodes("expr");
        let blocks = inner.child_nodes("block");
        let has_else = blocks.len() > conds.len();
        let entry = self.facts.clone();
        let mut ends: Vec<Facts> = Vec::new();
        let mut own_ends: Vec<Owners> = Vec::new();
        let own_entry = self.own.clone();
        let mut fall = entry.clone();
        let mut own_fall = own_entry;
        for (cond, blk) in conds.iter().zip(blocks.iter()) {
            self.facts = fall.clone();
            self.own = own_fall.copy();
            let pre = self.facts.clone();
            self.condition(*cond, env);
            let own_after_cond = self.own.copy();
            self.facts = pre;
            let (yes, no, _) = self.narrow(*cond, env); // the condition's effects, applied once, are in both states
            self.facts = yes;
            self.walk_block(*blk, env);
            if !self.terminates(*blk) {
                ends.push(self.facts.clone());
                own_ends.push(self.own.clone());
            }
            fall = no; // D-73: the false path keeps the condition's effects
            own_fall = own_after_cond;
        }
        if has_else {
            self.facts = fall.clone();
            self.own = own_fall.copy();
            self.walk_block(*blocks.last().unwrap(), env);
            if !self.terminates(*blocks.last().unwrap()) {
                ends.push(self.facts.clone());
                own_ends.push(self.own.clone());
            }
        } else {
            ends.push(fall.clone());
            own_ends.push(own_fall.clone());
        }
        if ends.is_empty() {
            self.facts = entry;
            self.own = own_fall;
            return;
        }
        let mut joined = ends[0].clone();
        for f in &ends[1..] {
            joined = joined.join(f);
        }
        self.facts = joined;
        let mut oj = own_ends[0].clone();
        for o in &own_ends[1..] {
            oj = oj.join(o);
        }
        self.own = oj;
        self.report_half();
    }

    pub fn expr_statement(&mut self, inner: N<'a>, env: &Env) {
        let e = inner.kid(0).unwrap().node().unwrap();
        let m = e.find("matchExpr");
        if let Some(m) = m {
            if e.kid(0).map_or(false, |k| k.is("outcomeExpr")) && e.first_token().map_or(false, |t| t.text == "match") {
                self.match_(m, env, None, true);
                return;
            }
        }
        if self.log_statement(inner, env) {
            return;
        }
        let t = self.infer(Some(e), env, None);
        let s = strip(&t).clone();
        if self.is_linear(&t) && self.path_of_expr(Some(e)).is_none() {
            let t0 = ftok(e);
            let last = e.tokens().into_iter().filter(|x| x.kind != Kind::Nl).last().unwrap().clone();
            let corr = if self.in_try && self.defer_depth == 0 && s.is_named("Outcome") && !self.ctx.iter().any(|c| c == "lambda") && last.line == t0.line {
                let f = self.dc.file.clone();
                let end = crate::decls::tok_end(&last);
                let mut edits = vec![crate::decls::edit(&f, last.line, end, end, " or fail")];
                let success_void = matches!(&s, Type::Named(n) if n.args.first().map_or(true, |a| *a == void()));
                if !success_void {
                    edits.push(crate::decls::edit(&f, t0.line, t0.col, t0.col, "discard "));
                }
                Some(crate::decls::correction("propagate the failure", edits))
            } else {
                None
            };
            self.rec_c(
                "E-OBL-001",
                format!("the {} produced here is dropped at the end of the statement; consume it (or fail, or recover, catch, match, bind it)", s.show()),
                "LANG-10-01",
                t0,
                Json::obj([("obligation_origin", Json::str(format!("{}:{}", t0.line, t0.col))), ("unconsumed_path", Json::str("statement result"))]),
                corr,
            );
        } else if known(&s) && s != void() && s != never() && !self.is_linear(&t) && e.find("matchExpr").is_none() && !self.ignorable_call(e) {
            let c = self.insert_before("discard the value", ftok(e), "discard ");
            self.rec_c(
                "E-TYP-140",
                format!("the value of this statement ({}) is unused: write `discard` to drop it (5.5)", s.show()),
                "LANG-05-08",
                ftok(e),
                DeclChecker::typ_fields("Void or `discard`", &s.show(), "expression statement"),
                Some(c),
            );
        }
    }

    pub fn bare_statement(&mut self, st: N<'a>, env: &Env) {
        let ts = st.direct_tokens();
        let word = ts[0].text.clone();
        let w0 = ts[0].clone();
        let e = st.child_node("expr");
        if self.defer_depth > 0 && ["return", "give", "break", "continue"].contains(&word.as_str()) {
            let last = st.tokens().into_iter().filter(|x| x.kind != Kind::Nl).last().unwrap().clone();
            let corr = if last.line == w0.line {
                let f = self.dc.file.clone();
                Some(crate::decls::correction("remove the statement from the deferred block", vec![crate::decls::edit(&f, w0.line, w0.col, crate::decls::tok_end(&last), "")]))
            } else {
                None
            };
            self.rec_c(
                "E-DEF-001",
                format!("`{word}` inside a deferred block: a defer cannot alter the enclosing function's outcome or leave the block (12.1)"),
                "LANG-12-01",
                &w0,
                Json::obj([("offending_statement", Json::str(&word)), ("rule_reference", Json::str("12.1"))]),
                corr,
            );
            if e.is_some() {
                self.infer(e, env, None);
            }
            return;
        }
        if word == "return" {
            if !self.ctx.iter().any(|c| c == "lambda") {
                self.obs_exit("success", &w0, &format!("the return at line {}", w0.line));
            }
            if let Some(e) = e {
                if !self.scope_depths.is_empty() && self.task_typed(e, env) {
                    let sname = self.scope_depths.last().unwrap().0.clone();
                    self.rec(
                        "E-ASY-001",
                        format!("a Task leaves scope `{sname}` through this return: a task is awaited inside its scope or detached (12.5)"),
                        "LANG-12-04",
                        &w0,
                        Json::obj([("offending_statement", Json::str("return")), ("rule_reference", Json::str("12.5"))]),
                    );
                }
                self.pending = None;
                let rt_ = self.ret_type.clone();
                let local = self.returned_local(e, env);
                if local.is_some() {
                    self.in_marker = true; // its read is the move below, as in `return move x`
                }
                let recording = self.lends_result(rt_.as_ref()); // the parts of a `from` result are judged by their types (D-381)
                let t = self.infer(Some(e), env, rt_.as_ref());
                if let Some(local) = &local {
                    self.in_marker = false;
                    if self.is_bearing(&t) {
                        // X-345: `return x` of a local `let` binding is `return move x` (7.4, 7.5)
                        self.use_place(Some(local), ftok(e), "move", env);
                        self.pending = Some((local.clone(), "move".into(), ftok(e).clone()));
                        self.implicit_moves.insert(e.id());
                    } else {
                        self.use_place(Some(local), ftok(e), "read", env);
                    }
                } else if let Some((ppath, mk, mtok)) = self.pending.clone() {
                    // `return move x` of a local whose move the bare form already shows: the formatter drops `move`
                    let bare = e.find("markerExpr").map_or(false, |m| m.kids().len() == 2 && m.find("place").map_or(false, |p| p.tokens().len() == 1));
                    let local_let = env_lookup(env, &ppath).map_or(false, |b| (b.kind == "let" || b.kind == "mut") && b.depth > 0);
                    if mk == "move" && bare && local_let && self.is_bearing(&t) {
                        self.redundant_markers.insert((mtok.line, mtok.col));
                    }
                }
                let pe = self.path_of_expr(Some(e));
                self.consume(pe, ftok(e).line, "returned");
                if let Some(rt) = &rt_ {
                    self.expect_return(&t, rt, ftok(e), "returned value");
                    self.loan_into_owned(&t, Some(rt), ftok(e), "the returned value");
                    self.owned_into_lent(&t, Some(rt), e, env);
                }
                if recording {
                    self.node_types = None;
                }
                self.check_returned_loans(e, &t, env, "return");
            } else if let Some(rt) = self.ret_type.clone() {
                if known(&rt) && rt != void() {
                    let displays = diagnostic_types(&[&rt, &named("Void")]);
                    self.typ("E-TYP-040", format!("bare `return` in a function returning {}", displays[0]), &w0, &displays[0], &displays[1], "return", "LANG-05-05");
                }
            }
            self.journal_exit("return", w0.line);
            self.linear_loop_exit(0, &w0, "return", false);
            self.report_open(0, "at this return");
            return;
        }
        if word == "give" {
            self.pending = None;
            let t = self.branch_infer(e, env);
            let handed = self.handed_on(e, &t); // the loans the value carries out of the branch (7.3; D-355)
            let owned = self.owns_value(e, &t, env) || self.has_owned_part(e);
            self.branch_value(e, &t, "give");
            if !self.ctx.iter().rev().find(|c| c.as_str() != "loop").map_or(false, |c| c == "arm" || c == "ifexpr") {
                let extra = if self.ctx.last().map_or(false, |c| c == "catch") { " (a catch block ends with `recover`)" } else { "" };
                let actual = self.ctx.last().cloned().unwrap_or_else(|| "function body".into());
                self.typ("E-TYP-120", format!("`give` outside a match arm or if-expression branch{extra}"), &w0, "a match arm or if-expression branch", &actual, "give", "LANG-05-05");
                return;
            }
            let tk = ftok(e.unwrap()).clone();
            self.arm_types.last_mut().unwrap().push((t, tk));
            if let Some(al) = self.arm_loans.last_mut() {
                al.extend(handed);
            }
            if let Some(ao) = self.arm_owned.last_mut() {
                *ao = *ao || owned;
            }
            if let Some(d) = self.exit_frames.last().map(|f| f.0) {
                self.linear_loop_exit(d, &w0, "give", false);
                self.report_open(d + 1, "at this give");
                let s = self.cleanup_state(d); // D-76: the expression's value leaves from here
                self.exit_frames.last_mut().unwrap().1.push(s);
            }
            return;
        }
        if word == "recover" {
            self.pending = None;
            let t = self.branch_infer(e, env);
            let handed = self.handed_on(e, &t);
            let owned = self.owns_value(e, &t, env) || self.has_owned_part(e);
            self.branch_value(e, &t, "recover");
            if self.ctx.iter().rev().find(|c| c.as_str() != "loop").map(|c| c.as_str()) != Some("catch") {
                let actual = self.ctx.last().cloned().unwrap_or_else(|| "function body".into());
                self.typ("E-TYP-120", "`recover` outside a catch block".into(), &w0, "a catch block", &actual, "recover", "LANG-05-05");
                return;
            }
            if let Some(cb) = self.catch_bound.last() {
                if !self.reported.contains(cb) {
                    self.discharges.push((w0.line, "recover without report".into()));
                    self.eff.add("recovers", "proved", Some(&w0));
                }
            }
            let tk = ftok(e.unwrap()).clone();
            self.arm_types.last_mut().unwrap().push((t, tk));
            if let Some(al) = self.arm_loans.last_mut() {
                al.extend(handed);
            }
            if let Some(ao) = self.arm_owned.last_mut() {
                *ao = *ao || owned;
            }
            if let Some(d) = self.exit_frames.last().map(|f| f.0) {
                self.linear_loop_exit(d, &w0, "recover", false);
                self.consume(self.catch_bound.last().cloned(), w0.line, "discharged");
                self.report_open(d + 1, "at this recover");
                let s = self.cleanup_state(d); // D-76: the catch expression's value leaves from here
                self.exit_frames.last_mut().unwrap().1.push(s);
            }
            return;
        }
        if word == "break" || word == "continue" {
            if !self.ctx.iter().any(|c| c == "loop") {
                let actual = self.ctx.last().cloned().unwrap_or_else(|| "function body".into());
                self.typ("E-TYP-120", format!("`{word}` outside a loop"), &w0, "a loop body", &actual, &word, "LANG-05-05");
            } else if ts.len() > 1 && !self.loop_labels.iter().any(|l| l.as_deref() == Some(ts[1].text.as_str())) {
                let labels: Vec<String> = self.loop_labels.iter().flatten().cloned().collect();
                self.rec("E-NAM-001", format!("no enclosing loop is labelled `{}`", ts[1].text), "LANG-04-05", ts[1], Self::nam(&ts[1].text, labels));
            } else if !self.break_frames.is_empty() {
                let target = if ts.len() > 1 { Some(ts[1].text.clone()) } else { None };
                let idx = (0..self.break_frames.len()).rev().find(|&i| target.is_none() || self.break_frames[i].0 == target);
                if let Some(i) = idx {
                    let d = self.break_frames[i].1;
                    self.linear_loop_exit(d + if word == "continue" { 1 } else { 0 }, &w0, &word, false);
                    self.report_open(d + 1, &format!("at this {word}"));
                    if word == "break" {
                        let d = self.break_frames[i].1;
                        let s = self.cleanup_state(d); // D-72: the loop is left from here
                        self.break_frames[i].2.push(s);
                        let o = self.own.copy(); // with these loans live (D-362)
                        self.break_frames[i].3.push(o);
                    } else {
                        let o = self.own.copy(); // the header is reached from here (D-362)
                        self.break_frames[i].4.push(o);
                    }
                }
            }
            return;
        }
        if word == "report" {
            self.eff.add("io", "proved", Some(&w0));
            let t = strip(&self.infer(e, env, None)).clone();
            let rp = self.path_of_expr(e);
            if let Some(rp) = &rp {
                self.use_place(Some(rp), ftok(e.unwrap()), "move", env);
                self.own.move_(rp);
            }
            self.consume(rp.clone(), ftok(e.unwrap()).line, "report");
            for b in self.catch_bound.clone() {
                if rp.as_deref() == Some(b.as_str()) || self.own.is_moved(&b) {
                    self.reported.insert(b);
                }
            }
            if known(&t) && !["Failure", "Report"].contains(&core_name(&t).unwrap_or("")) {
                let displays = diagnostic_types(&[&t, &named("Failure")]);
                self.typ("E-TYP-040", format!("`report` takes a {expected}, got {}", displays[0], expected = displays[1]), ftok(e.unwrap()), &displays[1], &displays[0], "report", "LANG-06-01");
            }
            if let Some(i) = ts.iter().position(|t| t.text == "as") {
                env_set(env, &ts[i + 1].text, named("Report"), false, "let");
            }
            return;
        }
        if word == "discard" {
            let t = self.infer(e, env, None);
            let s = strip(&t).clone();
            if s.is_named("Future") && self.path_of_expr(e).is_none() {
                let marked = e.map(|e| e.find_all("markerExpr").iter().any(|m| ["editable", "move"].contains(&m.tokens()[0].text.as_str()))).unwrap_or(false);
                if !marked {
                    return;
                }
            }
            if self.is_linear(&t) {
                let path = self.path_of_expr(e).unwrap_or_else(|| "discarded value".into());
                self.obl_err("E-OBL-001", format!("`discard` of a linear value ({}): a linear value is consumed, never discarded (5.5)", s.show()), &w0, &format!("{}:{}", w0.line, w0.col), &path, "LANG-05-08");
            }
            if self.path_of_expr(e).is_none() || e.map_or(false, |e| e.find("markerExpr").is_some()) {
                self.charge_drop(&t, &w0); // a discarded value of the statement's own is destroyed here (7.9, 11.3; D-393)
            }
            return;
        }
        if e.is_some() {
            self.infer(e, env, None);
        }
    }

    /// The loans the returned value `e` (of type `t`) keeps, against the result's declared origin: a `return`, or an
    /// expression body, which is the return of its expression (7.6; D-395). A loan of a local or a temporary is
    /// E-OWN-051, a loan of a second parameter E-OWN-050, a loan-carrying result with no origin E-OWN-053.
    pub fn check_returned_loans(&mut self, e: N<'a>, t: &Type, env: &Env, how: &str) {
        if self.borrows_value(t) { self.check_temporary_borrows(); }
        if *strip(t) == void() {
            self.pending = None;
            self.carried.clear(); // Void cannot carry a loan of a call's argument (D-552)
            return;
        }
        let rt_ = self.ret_type.clone();
        let rp = self.path_of_expr(Some(e));
        let mut rt = rt_.clone().unwrap_or_else(|| unknown(""));
        while let Type::Optional(i) = &rt {
            if matches!(**i, Type::Loan { .. } | Type::Optional(_)) {
                rt = (**i).clone();
            } else {
                break;
            }
        }
        let mut origin: Option<String> = match &rt {
            Type::Loan { origin, .. } => Some(origin.clone()),
            Type::Fn { origin, .. } if !origin.is_empty() => Some(origin.clone()),
            _ => None,
        };
        if origin.is_none() {
            if let Some(r) = &rt_ {
                let mut nested = Vec::new();
                Self::nested_origins(r, true, &mut nested);
                if nested.len() == 1 {
                    origin = Some(nested[0].0.clone()); // `((alias T from xs)?, Int)`
                }
            }
        }
        // a loan held inside an optional, a tuple or a record is returned only when the result lends one; an
        // owned result was reported by loan_into_owned (D-341, D-348)
        let lends = rt_.as_ref().map_or(false, |r| Self::lent(r).is_some());
        let mut origins = Vec::new();
        Self::captured_loans(t, &mut origins);
        let mut loan_roots: Vec<String> = origins.iter().filter(|(o, _)| o != "static").map(|(o, _)| root_of(o)).collect();
        if rp.is_none() && !t.is_loan() {
            self.coalesced_marker(Some(e), t, if how == "return" { "`return`" } else { "`=>`" }); // `return p ?? v` (D-357)
        }
        if let Some((ppath, mk, _)) = self.pending.take() {
            if mk == "move" {
                for l in &self.own.loans {
                    let ld = l.borrow();
                    // a moved closure or lifetime value keeps its loans; a parameter's loan of its own argument is the caller's
                    if ld.holder.as_deref() == Some(root_of(&ppath).as_str()) && (lends || !ld.nested) && ld.holder.as_deref() != Some(root_of(&ld.owner).as_str()) {
                        loan_roots.push(root_of(&ld.owner));
                    }
                }
                self.own.move_(&ppath);
            } else if mk == "alias" || mk == "editable" {
                loan_roots.push(root_of(&self.own.owner_path(&ppath)));
            }
        } else if rp.is_some() && self.is_bearing(t) && e.find("markerExpr").is_none() && !t.is_loan() {
            let rp = rp.clone().unwrap();
            let what = if how == "return" { format!("`return {rp}`") } else { format!("`=> {rp}`") };
            self.own_err(
                "E-OWN-001",
                format!("{what}: an existing ownership-bearing place needs `move {rp}`, `copy {rp}` or a `from` signature with `alias {rp}`"),
                ftok(e),
                &rp,
                "marker required",
                "LANG-07-01",
                "",
            );
        } else if let Some(rp) = &rp {
            if self.own.holders.contains(&root_of(rp)) && self.loan_carrying(t) {
                let owner = self.own.holders.get(&root_of(rp)).unwrap().borrow().owner.clone();
                loan_roots.push(root_of(&owner));
            }
        }
        for l in &self.carried {
            let ld = l.borrow();
            // a loan of a binding that owns a part of its value was reported by owned_into_lent, as the function's own
            if (lends || !ld.nested) && !(lends && self.part_owners.contains(&root_of(&ld.owner))) {
                loan_roots.push(root_of(&ld.owner));
            }
        }
        self.carried.clear();
        let where_ = if how == "return" { "this return" } else { "the expression body" };
        if origin.is_none() && !loan_roots.is_empty() {
            for r in &loan_roots {
                let b = env_lookup(env, r);
                if b.as_ref().map_or(false, |b| ["param", "editable", "owned"].contains(&b.kind.as_str())) || r == "self" {
                    self.own_err("E-OWN-053", format!("the returned value keeps a loan of `{r}` but the result type names no origin: write `… from {r}`"), ftok(e), r, "loan-carrying result without an origin", "LANG-07-06", "");
                } else {
                    self.own_err("E-OWN-051", format!("the returned value keeps a loan of `{r}`, a local or temporary that ends with the function"), ftok(e), r, "loan of a local returned", "LANG-07-06", "");
                }
            }
        }
        if let Some(origin) = &origin {
            if origin == "static" || origin == "self" || env_lookup(env, origin).is_some() {
                for r in &loan_roots {
                    if r == origin {
                        continue;
                    }
                    let b = env_lookup(env, r);
                    if b.as_ref().map_or(false, |b| ["param", "editable", "owned"].contains(&b.kind.as_str())) || r == "self" {
                        self.own_err(
                            "E-OWN-050",
                            format!("the result is declared `from {origin}` but {where_} keeps a loan of `{r}`: a result may name one origin"),
                            ftok(e),
                            r,
                            &format!("second origin {r}"),
                            "LANG-07-06",
                            "",
                        );
                    } else {
                        self.own_err("E-OWN-051", format!("the result keeps a loan of `{r}`, a local or temporary that ends with the function"), ftok(e), r, "loan of a local returned", "LANG-07-06", "");
                    }
                }
            }
        }
    }

    pub fn let_(&mut self, inner: N<'a>, env: &Env) {
        let ts = inner.direct_tokens();
        let words: Vec<&str> = ts.iter().map(|t| t.text.as_str()).collect();
        if let Some(pat) = inner.child_node("pattern") {
            let pl = inner.child_node("place");
            let pt = match pl {
                Some(pl) => strip(&self.place_type(pl, env)).clone(),
                None => unknown(""),
            };
            if let Some(pl) = pl {
                let pp = self.path_of_place(pl);
                self.use_place(Some(&pp), ftok(pl), "move", env);
                if self.use_ok {
                    self.own.move_(&pp);
                }
            }
            self.pattern(pat, &pt, env, true);
            let bad = if known(&pt) { self.let_refutable(pat, &pt) } else { None };
            if let Some(bad) = bad {
                // a `let` destructures a class, tuple or record: its pattern matches every value (5.1; D-243)
                let at = ftok(bad);
                let shown = match bad.kid(0) {
                    Some(C::N(k0)) if k0.name() == "rangePattern" => bad.tokens().iter().map(|t| t.text.as_str()).collect::<String>(),
                    _ => at.text.clone(),
                };
                self.rec(
                    "E-MAT-004",
                    format!("a `let` pattern must match every value of {}, but `{shown}` can fail to match: use `match` to handle the other cases", pt.show()),
                    "LANG-05-01",
                    at,
                    Json::obj([("missing_variants", Json::Arr(vec![])), ("arm_set", Json::str(&pt.show()))]),
                );
            }
            for b in self.names_in(pat) {
                if let Some(bt) = env_get(env, &b) {
                    if self.is_linear(&bt) {
                        self.obligate(&b, ftok(pat));
                    }
                }
            }
            return;
        }
        let ann = inner.child_node("type");
        let e = inner.child_node("expr");
        let mutable = words.contains(&"mut");
        let nm = (*ts.iter().find(|t| t.kind == Kind::Lower).unwrap()).clone();
        self.pending = None;
        let have;
        let t;
        if let Some(ann) = ann {
            let want = self.dc.type_of(Some(ann));
            self.dc.check_keys(&want, ftok(ann), None, false);
            self.check_stored_type(&want, ftok(ann), false);
            for (code, rule, shown, why) in self.dc.dynamic_problems(&want) {
                self.typ(code, format!("the annotation of `{}`: {why}", nm.text), ftok(ann), "a dynamic-eligible interface whose bounds hold", &shown, &nm.text, rule);
            }
            have = self.infer(e, env, Some(&want));
            if is_interp(&have) && *strip(&want) == string_view() {
                self.typ("E-TYP-040", "an interpolated literal is a fresh String; a binding annotated StringView cannot hold it (3.5)".into(), ftok(e.unwrap()), "String", "an interpolated literal", &nm.text, "LANG-03-05");
            } else {
                self.expect(&have, &want, ftok(e.unwrap()), "initialiser");
                self.loan_into_owned(&have, Some(&want), ftok(e.unwrap()), &format!("`{}`", nm.text));
            }
            t = self.with_closure(&want, &have);
            self.set_fact(Some(&nm.text), &have);
        } else {
            let h0 = self.infer(e, env, None);
            have = concrete(&h0);
            let mut tt = have.clone();
            if let Type::Refined { base, kind, .. } = &tt {
                if (kind == "exact" || kind == "range") && self.is_int(&tt) {
                    tt = Type::Named(base.clone());
                }
            }
            if tt == null_t() {
                self.typ("E-TYP-040", format!("`let {} = null` needs a type annotation", nm.text), &nm, "T?", "null", &nm.text, "LANG-06-08");
            }
            t = tt;
            self.set_fact(Some(&nm.text), &have);
        }
        if self.borrows_value(&have) { self.check_temporary_borrows_in(e); }
        self.own.reinit(&nm.text);
        self.own.close_obligation(&nm.text, nm.line, "rebound");
        env_set(env, &nm.text, t.clone(), mutable, if mutable { "mut" } else { "let" });
        let d = self.depth;
        env_update(env, &nm.text, |b| b.depth = d);
        // the binding owns a part of what it holds beside its loans: a `??` fallback, a tuple's own element, a branch's
        // own value (7.9; D-366, D-369)
        self.part_owners.remove(&nm.text);
        if self.has_owned_part(e) && (have.is_loan() || self.holds_loan(&have)) {
            self.part_owners.insert(nm.text.clone());
        }
        let cd = if known(strip(&t)) { self.decl_of(strip(&t)) } else { None };
        if let Some(cd) = cd {
            if cd.has_cleanup {
                crate::journal::cleanup("drop", &format!("{}: {}", nm.text, cd.name), nm.line, vec![]);
            }
        }
        if Self::lent(&have).is_none() || self.part_owners.contains(&nm.text) {
            let charged = if ann.is_some() { t.clone() } else { have.clone() };
            self.charge_drop(&charged, &nm); // dropping what the binding owns runs its cleanup (11.3; D-393, D-400)
        }
        if strip(&t).is_named("MutexGuard") {
            self.guards.push((nm.text.clone(), self.depth, nm.clone()));
        }
        let view = matches!(strip(&have), Type::Named(n) if (n.package.is_empty() && ["StringView", "BytesView", "ListView", "ListCursor"].contains(&n.name.as_str())));
        if e.map_or(true, |e| e.find("markerExpr").is_none()) && !have.is_loan() && !view {
            self.fresh.insert(nm.text.clone());
        } else {
            self.fresh.remove(&nm.text);
        }
        self.bind_initialiser(&nm.text, e, &have, env);
        if self.is_linear(&t) {
            self.obligate(&nm.text, &nm);
        } else if self.is_linear(&have) && self.path_of_expr(e).is_none() {
            self.obligate(&nm.text, &nm);
        }
    }

    pub fn bind_initialiser(&mut self, binding: &str, e: Option<N<'a>>, t: &Type, env: &Env) {
        let path = self.path_of_expr(e);
        if let Some(lc) = self.last_closure.clone() {
            if e.map_or(false, |e| e.find("lambda").is_some()) {
                if lc.linear {
                    let d = self.depth;
                    self.own.open_obligation(binding, ftok(e.unwrap()).clone(), d);
                }
                self.closures.insert(binding.to_string(), lc);
            }
        }
        self.last_closure = None;
        let d = self.depth;
        self.keep_nested(binding, d, t, e); // a loan inside an optional, a tuple or a record (7.3; D-348)
        let inner = strip(t).clone();
        let lifetime_value = self.decl_of(&inner).map_or(false, |d2| d2.kind == "class" && d2.lifetime.is_some());
        for l in self.carried.clone() {
            {
                let mut ld = l.borrow_mut();
                crate::journal::kept(&ld.owner, &ld.mode, ld.line, binding, self.depth);
                ld.holder = Some(binding.to_string());
                ld.depth = self.depth;
            }
            let is_view = matches!(&inner, Type::Named(n) if (n.package.is_empty() && ["StringView", "BytesView", "ListView", "ListCursor"].contains(&n.name.as_str())));
            if !lifetime_value && (t.is_loan() || is_view) {
                self.own.holders.set_default(binding, l.clone());
            }
        }
        self.carried.clear();
        if path.is_none() {
            self.coalesced_marker(e, t, &format!("`let {binding} =`")); // `let x = p ?? v` with `p` an owned place (D-357)
        }
        if let Some((ppath, mk, mtok)) = self.pending.take() {
            if (mk == "alias" || mk == "editable") && self.use_ok {
                let d = self.depth;
                self.own.add_loan(&ppath, &mk, Some(binding), d, mtok.line);
            } else if mk == "move" && self.use_ok {
                self.own.move_(&ppath);
            }
            return;
        }
        if let Some(path) = path {
            let function_name = env_lookup(env, &root_of(&path)).is_none() && self.dc.table.get(&path).map_or(false, |d| d.kind == "func");
            if !function_name && self.is_bearing(t) && e.map_or(true, |e| e.find("markerExpr").is_none()) {
                let t0 = ftok(e.unwrap()).clone();
                let marker = if self.dc.is_copyable(strip(t)) { "copy " } else { "move " };
                let c = self.insert_before("add the marker", &t0, marker);
                self.rec_c(
                    "E-OWN-001",
                    format!("`{path}` is an existing ownership-bearing place: write `copy {path}`, `alias {path}` or `move {path}`"),
                    "LANG-07-01",
                    &t0,
                    Json::obj([("place", Json::str(&path)), ("conflict", Json::str("marker required")), ("spans", Json::str(&path)), ("loan_region", Json::str(""))]),
                    Some(c),
                );
            }
        }
    }

    pub fn assign(&mut self, inner: N<'a>, env: &Env) {
        let pl = inner.find("place").unwrap();
        let e = inner.find("expr");
        let op = inner.direct_tokens().into_iter().find(|t| ["=", "+=", "-=", "*=", "/=", "%="].contains(&t.text.as_str())).unwrap().clone();
        let ts = pl.direct_tokens();
        let root = ts[0].clone();
        let b = if root.text != "self" { env_lookup(env, &root.text) } else { None };
        let target_t = self.place_type_access(pl, env, true);
        let joined: String = ts.iter().map(|t| t.text.clone()).collect();
        if root.text == "self" {
            if self.self_mode != "editable self" && ts.len() > 1 {
                self.rec("E-NAM-002", format!("assignment to `{joined}` through a read `self`: the method needs `editable self`"), "LANG-05-01", &root, Self::nam(&root.text, vec!["editable self".into()]));
            } else if ts.len() == 1 {
                self.rec("E-NAM-002", "`self` cannot be rebound".into(), "LANG-05-01", &root, Self::nam("self", vec![]));
            }
        } else if let Some(b) = &b {
            if ts.len() == 1 {
                if !b.mutable {
                    let extra = if ["param", "editable", "owned"].contains(&b.kind.as_str()) { " (a parameter cannot be reassigned)" } else { "" };
                    self.rec("E-NAM-002", format!("`{}` is immutable{extra}", root.text), "LANG-05-01", &root, Self::nam(&root.text, vec!["let mut".into()]));
                } else if b.kind == "capture-copy" {
                    self.rec("E-NAM-002", format!("`{}` is a `copy` capture: read-only in the closure body (8.2); capture it with `move` into a binding the body may edit", root.text), "LANG-05-01", &root, Self::nam(&root.text, vec!["move".into()]));
                }
            } else if !(b.mutable || b.kind == "editable" || matches!(&b.ty, Type::Loan { mode, .. } if mode == "editable")) {
                self.rec("E-NAM-002", format!("`{}` is not mutable: `{joined}` cannot be assigned", root.text), "LANG-05-01", &root, Self::nam(&root.text, vec!["let mut".into(), "editable".into()]));
            } else if (b.kind == "let" || b.kind == "mut") && self.writes_through_read_loan(pl, b) {
                // a `let mut` binding that holds a read loan may be rebound, never written through: what it lends
                // belongs to the owner, and a read loan is read-only (7.3; D-356)
                let place = self.path_of_place(pl);
                self.rec("E-NAM-002", format!("`{}` holds a read loan: `{place}` cannot be assigned through it (7.3); take an `editable` loan, or change the owner", root.text), "LANG-05-01", &root, Self::nam(&root.text, vec!["editable".into()]));
            }
        }
        let path = self.path_of_place(pl);
        self.pending = None;
        let mut rt = self.infer(e, env, Some(&target_t));
        if self.borrows_value(&rt) { self.check_temporary_borrows_in(e); }
        if ts.len() == 1 && self.has_owned_part(e) && (rt.is_loan() || self.holds_loan(&rt)) {
            self.part_owners.insert(root.text.clone()); // it now owns a part beside its loans, on this path at least (7.9; D-369)
        }
        if op.text != "=" {
            let o = synth(&op.text[..1], op.line, op.col);
            rt = self.arith(&o, &target_t, &rt);
        }
        if let Some(b) = &b {
            if b.kind.starts_with("capture-") && b.kind != "capture-alias" && !(b.kind == "capture-copy" && ts.len() == 1) && self.lambda_kind.last().map(|k| k.as_str()) == Some("fn") {
                let corr = self.lambda_toks.last().map(|(fnt, _)| {
                    let f = self.dc.file.clone();
                    crate::decls::correction("strengthen the closure kind", vec![crate::decls::edit(&f, fnt.line, fnt.col, crate::decls::tok_end(fnt), "fn mut")])
                });
                self.rec_c(
                    "E-FUN-010",
                    format!("this `fn` lambda mutates its capture `{}` ({}): declare it `fn mut`", root.text, &b.kind[8..]),
                    "LANG-08-02",
                    &root,
                    Json::obj([("violation", Json::str("mutation in a fn")), ("capture_span", Json::str(&root.text)), ("use_span", Json::str(&root.text))]),
                    corr,
                );
            }
        }
        self.use_place(Some(&path), &root, "write", env);
        self.note_write(&path, &root, env);
        if !self.scope_depths.is_empty() && !path.contains('.') && strip(&rt).is_named("Task") {
            if let Some(b2) = env_lookup(env, &path) {
                if b2.depth < self.scope_depths.last().unwrap().1 {
                    let sname = self.scope_depths.last().unwrap().0.clone();
                    self.rec(
                        "E-ASY-001",
                        format!("a Task is stored in `{path}`, declared outside scope `{sname}`: a task is awaited inside its scope or detached (12.5)"),
                        "LANG-12-04",
                        &root,
                        Json::obj([("offending_statement", Json::str("assignment")), ("rule_reference", Json::str("12.5"))]),
                    );
                }
            }
        }
        if self.own.obligations.contains(&path) && !self.own.is_moved(&path) {
            let tok0 = self.own.obligations.get(&path).unwrap().0.clone();
            self.obl_err("E-OBL-001", format!("the linear value in `{path}` is overwritten without being consumed"), &root, &format!("{}:{}", tok0.line, tok0.col), &path, "LANG-10-01");
            self.consume(Some(path.clone()), root.line, "overwritten");
        } else if path.contains('.') {
            let dt = strip(&self.declared_type(pl, env)).clone();
            if self.is_linear(&dt) && !self.own.is_moved(&path) {
                self.obl_err("E-OBL-001", format!("the linear value in field `{path}` is overwritten without being consumed"), &root, &format!("{}:{}", root.line, root.col), &path, "LANG-10-01");
            }
        }
        if !path.contains('.') && self.use_ok {
            let rtext = root.text.clone();
            let old: Vec<Loan> = self.own.loans.iter().filter(|l| l.borrow().holder.as_deref() == Some(rtext.as_str())).cloned().collect();
            self.own.loans.retain(|l| l.borrow().holder.as_deref() != Some(rtext.as_str())); // the old value's stored loans end with it (7.9)
            self.assign_nested(&root, b.as_ref(), &rt, e, old, env);
            if Self::cursor_type(&rt) {
                // a `let mut` cursor binding owns its cursor (7.7): the assignment replaces the value, and the new
                // cursor's stored loan is held by the binding until the end of the binding's own block (7.3, 7.9; D-401)
                let depth = match b.as_ref() {
                    Some(b) if b.kind == "let" || b.kind == "mut" => b.depth,
                    _ => self.depth,
                };
                for l in std::mem::take(&mut self.carried) {
                    {
                        let mut ld = l.borrow_mut();
                        crate::journal::kept(&ld.owner, &ld.mode, ld.line, &rtext, depth);
                        ld.holder = Some(rtext.clone());
                        ld.depth = depth;
                    }
                    self.own.holders.insert(&rtext, l);
                }
            }
        }
        if self.path_of_expr(e).is_none() {
            self.coalesced_marker(e, &rt, &format!("`{path} =`")); // `x = p ?? v` with `p` an owned place (D-357)
        }
        if let Some((ppath, mk, mtok)) = self.pending.take() {
            if mk == "move" {
                self.own.move_(&ppath);
            } else if mk == "alias" || mk == "editable" {
                self.own_err("E-OWN-011", format!("a loan (`{mk} {ppath}`) cannot be stored by assignment; bind it with `let`"), &mtok, &ppath, "loan assigned", "LANG-07-05", "");
            }
        } else if let Some(pe) = self.path_of_expr(e) {
            if self.is_bearing(&rt) && e.map_or(true, |e| e.find("markerExpr").is_none()) {
                self.own_err("E-OWN-001", format!("`{pe}` is an existing ownership-bearing place: write `copy`, `alias` or `move`"), ftok(e.unwrap()), &pe, "marker required", "LANG-07-01", "");
            }
        }
        self.own.reinit(&path);
        if self.is_linear(&rt) && !path.contains('.') {
            self.obligate(&path, &root);
        }
        if let Some(e) = e {
            let declared = self.declared_type(pl, env);
            self.check_static_source(&rt, &declared, ftok(e), "assigned value");
        }
        let decl_t = strip(&self.declared_type(pl, env)).clone();
        if let Type::Refined { kind, .. } = &decl_t {
            if kind == "length" && ts.len() > 1 {
                self.ref_("E-REF-003", format!("`{path}` has a length refinement {} and may not be mutated; mutate an unrefined collection and validate the result", decl_t.show()), &root, "violated", &decl_t.show(), "", "");
            }
        }
        if known(&decl_t) {
            if let Some(e) = e {
                self.expect(&rt, &decl_t, ftok(e), "assigned value");
                let declared = self.declared_type(pl, env);
                self.loan_into_owned(&rt, Some(&declared), ftok(e), &format!("`{path}`"));
            }
        }
        // the first lambda stored in a closure binding pins it, including after `= null` (8.4)
        if op.text == "=" && !path.contains('.') && root.text != "self" {
            if let Some(cur) = env_get(env, &root.text) {
                let narrowed = self.with_closure(&cur, &rt);
                env_update(env, &root.text, |binding| binding.ty = narrowed);
            }
        }
        if op.text == "=" && path.contains('.') {
            if let Some(e) = e {
                self.note_field_assign(pl, &rt, ftok(e), env);
            }
        }
        self.set_fact(Some(&path), &rt);
    }

    /// `t = e` where the new value holds a loan inside an optional, a tuple or a record (`t = xs.get(1)`): `t` holds its
    /// loans until the end of the block that declares `t`, and the old ones too when `e` reads `t` (the new value can be
    /// the old one: `t = t ?? xs.get(1)`). An owner declared in a deeper block than `t` is dropped while `t` still lends
    /// it: E-OWN-011 (7.3, 7.9; D-348).
    pub fn assign_nested(&mut self, root: &Token, b: Option<&Binding>, rt: &Type, e: Option<N<'a>>, old: Vec<Loan>, env: &Env) {
        let depth = match b {
            Some(b) if b.kind == "let" || b.kind == "mut" => b.depth,
            _ => self.depth,
        };
        let held = self.keep_nested(&root.text, depth, rt, e);
        if !old.is_empty() && self.holds_loan(rt) && e.map_or(false, |e| e.tokens().iter().any(|tk| tk.text == root.text)) {
            self.own.loans.extend(old);
        }
        for l in held {
            let (owner, mode) = {
                let ld = l.borrow();
                (ld.owner.clone(), ld.mode.clone())
            };
            if let Some(ob) = env_lookup(env, &root_of(&owner)) {
                if (ob.kind == "let" || ob.kind == "mut") && ob.depth > depth {
                    let r = &root.text;
                    self.own_err(
                        "E-OWN-011",
                        format!("`{r}` keeps a loan of `{owner}`, which is dropped at the end of its block while `{r}` lives on"),
                        root,
                        &owner,
                        &format!("{mode} loan held by {r}"),
                        "LANG-07-05",
                        &format!("until the end of {r}'s block"),
                    );
                    break;
                }
            }
        }
    }

    pub fn declared_type(&mut self, pl: N<'a>, env: &Env) -> Type {
        let saved = std::mem::take(&mut self.facts);
        let t = self.place_type(pl, env);
        self.facts = saved;
        t
    }

    /// Walk a loop body; return the states at the `break`s that leave this loop (D-72). `exits_at_header`: the loop
    /// also ends at its header, when a `while` or `for` condition is false, before the body has run or after any
    /// iteration (a `continue` included); a `loop` ends only at a `break`.
    pub fn loop_body(&mut self, inner: N<'a>, block: N<'a>, env: &Env, fresh: &[String], exits_at_header: bool, cond: Option<N<'a>>) -> Vec<Facts> {
        let lab = inner.find("label").map(|l| l.tokens()[0].text.clone());
        self.loop_labels.push(lab);
        let own_lab = inner.child_nodes("label").first().map(|l| l.tokens()[0].text.clone()); // the loop's own label
        self.break_frames.push((own_lab, self.depth, vec![], vec![], vec![]));
        self.ctx.push("loop".into());
        let entry = self.own.copy();
        let header = self.facts.clone();
        let item = self.linear_loops.iter().rev().find(|(d, _)| *d == self.depth).map(|(_, t)| t.clone());
        if let Some(item) = &item {
            self.own.open_obligation(&item.text, item.clone(), self.depth + 1);
        }
        self.walk_block(block, env);
        self.own = entry.join(&self.own);
        for b in fresh {
            self.own.reinit(b);
        }
        self.facts = header.clone();
        if let Some(item) = &item {
            self.own.open_obligation(&item.text, item.clone(), self.depth + 1);
        }
        self.walk_block(block, env);
        self.facts = header;
        self.ctx.pop();
        self.loop_labels.pop();
        let (_, _, breaks, break_owns, continue_owns) = self.break_frames.pop().unwrap();
        if let Some(cond) = cond {
            // a `while` condition is evaluated on every iteration (5.3), and the body returns to it: at its normal
            // end or at a `continue`. A `move` in the condition is a use after the move on the next evaluation
            // unless every path back re-assigns the place (7.4; D-448).
            let hits: Vec<(String, Token)> = self
                .move_markers(cond)
                .into_iter()
                .filter(|(path, _)| continue_owns.iter().any(|o| o.is_moved(path)) || self.own.is_moved(path))
                .collect();
            for (path, mtok) in hits {
                self.own_err(
                    "E-OWN-020",
                    format!("`move {path}` runs on every iteration of a `while` (5.3), and `{path}` is moved where the body returns to the condition: bind the value before the loop, or re-assign `{path}` in the body"),
                    &mtok,
                    &path,
                    "move in a re-entered condition",
                    "LANG-07-04",
                    "",
                );
            }
        }
        // 7.3: a loan is live at the loop's exit when it is live on any path that leaves it: the header with the condition
        // false (the body not run, or run to its end or to a `continue`) and every `break`; a body that ends a loan
        // (`t = null`) ends it on its own path only (D-362). The loans of bindings declared inside the loop are over.
        let mut paths: Vec<Owners> = Vec::new();
        if exits_at_header {
            paths.push(entry);
            paths.extend(continue_owns);
        }
        paths.extend(break_owns);
        for o in &paths {
            for l in &o.loans {
                let ld = l.borrow();
                if ld.holder.is_some() && ld.depth <= self.depth && !self.own.loans.iter().any(|m| {
                    let md = m.borrow();
                    md.owner == ld.owner && md.mode == ld.mode && md.holder == ld.holder
                }) {
                    drop(ld);
                    self.own.loans.push(l.clone());
                }
            }
        }
        breaks
    }

    pub fn for_stmt(&mut self, inner: N<'a>, env: &Env) {
        let ts = inner.direct_tokens();
        let var = ts[ts.iter().position(|t| t.text == "for").unwrap() + 1].clone();
        let moving = ts.iter().any(|t| t.text == "move");
        let src = inner.kids().into_iter().filter_map(|c| c.node()).find(|c| ["expr", "rangeExpr", "place"].contains(&c.name())).unwrap();
        let mut elem = unknown("loop variable");
        let mut st_opt: Option<Type> = None;
        if src.name() == "rangeExpr" {
            self.infer(Some(src), env, None);
            let rk = src.kids();
            let lo_t = self.infer_c(rk[0], env, None);
            let hi_t = self.infer_c(rk[2], env, None);
            let (li, hi_) = (self.interval_of(&lo_t), self.interval_of(&hi_t));
            elem = int();
            if let (Some(li), Some(hi_)) = (li, hi_) {
                let top = hi_.1 - if rk[1].text_is("..<") { 1 } else { 0 };
                if li.0 <= top {
                    elem = refined(nt("Int"), Some(li.0), Some(top), "range");
                }
            }
        } else {
            let t0 = if src.name() != "place" { self.infer(Some(src), env, None) } else { self.place_type(src, env) };
            let mut st = strip(&t0).clone();
            if let Some((nd, rep)) = self.collection_representation(&st) {
                if moving && self.foreign(&nd) {
                    self.rec("E-NAM-004", format!("consuming iteration of {} is visible only in its declaring module {}", nd.name, nd.module), "LANG-04-05", ftok(src), Self::nam("consume", vec![]));
                }
                st = rep;
            }
            if let Type::Refined { base, .. } = &st {
                st = Type::Named(base.clone());
            }
            match &st {
                Type::Named(n) if n.package.is_empty() && ["List", "ListView", "Array", "Set", "FixedList"].contains(&n.name.as_str()) && !n.args.is_empty() => {
                    elem = n.args[0].clone();
                    if moving && n.name == "ListView" {
                        self.typ("E-TYP-040", "a ListView only loans its elements and is not Consumable; iterate it by read loan".into(), &var, "Consumable[T, I]", &st.show(), "for", "LANG-06-01");
                    }
                },
                Type::Named(n) if (n.package.is_empty() && ["Map", "OrderedMap", "FixedMap"].contains(&n.name.as_str())) && n.args.len() >= 2 => elem = Type::Tuple(vec![n.args[0].clone(), n.args[1].clone()]),
                _ => {
                    if known(&st) {
                        let d = self.decl_of(&st);
                        let protocol = if moving { "Consumable" } else { "Iterable" };
                        let it = d.as_ref().and_then(|d| d.implements.iter().find(|i| i.interface_key() == protocol).cloned());
                        match (d, it) {
                            (Some(d), Some(it)) => {
                                elem = if let Some(a0) = it.args.first() {
                                    let rs = self.recv_subst(&st, &d);
                                    self.subst(a0, &rs)
                                } else {
                                    unknown("")
                                };
                                if moving {
                                    if let Some(consume) = d.methods.get("consume") {
                                        let summary = self.callee_summary(Some(&d), consume, &self.method_label(&d, "consume"));
                                        self.import_call(&summary, &var, None, &[], env, false);
                                        let cursor = self.subst(&consume.ret, &self.recv_subst(&st, &d));
                                        if self.is_linear(&cursor) {
                                            self.obl_err("E-OBL-003", format!("the consuming cursor {} is linear, but Consumer.next returning null does not consume the cursor; transfer the contents to a standard collection before looping (5.3)", cursor.show()), &var, &format!("{}:{}", var.line, var.col), "cursor", "LANG-10-02");
                                        }
                                        if let Some(cd) = self.decl_of(&cursor) {
                                            if let Some(advance) = cd.methods.get("next") {
                                                let summary = self.callee_summary(Some(&cd), advance, &self.method_label(&cd, "next"));
                                                self.import_call(&summary, &var, None, &[], env, false);
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {
                                self.typ("E-TYP-040", format!("`for` over {}, which is not {protocol}", st.show()), ftok(src), &format!("{protocol}[T, I]"), &st.show(), "for", "LANG-06-01");
                            }
                        }
                    }
                }
            }
            st_opt = Some(st);
        }
        let e = env_new(Some(env));
        self.widen(inner.find("block").unwrap());
        let src_path = if src.name() == "place" { Some(self.path_of_place(src)) } else { self.path_of_expr(Some(src)) };
        if !moving && src_path.is_none() && st_opt.as_ref().map_or(false, |st| self.is_linear(st)) {
            self.obl_err("E-OBL-001", "reading iteration loans a temporary linear collection which is dropped after the loop; bind it and use consuming iteration".into(), ftok(src), &format!("{}:{}", ftok(src).line, ftok(src).col), "temporary iterable", "LANG-10-01");
        }
        if let Some(sp) = &src_path {
            if src.name() != "rangeExpr" {
                if moving {
                    self.use_place(Some(sp), ftok(src), "move", env);
                    self.own.move_(sp);
                } else if self.is_bearing(&st_opt.clone().unwrap_or_else(|| unknown(""))) {
                    let d = self.depth + 1;
                    let h = format!("for@{}", ftok(src).line);
                    self.own.add_loan(sp, "alias", Some(&h), d, ftok(src).line);
                }
            }
        }
        if var.text != "_" {
            env_set(&e, &var.text, elem.clone(), false, "for");
            self.own.reinit(&var.text);
            self.set_fact(Some(&var.text), &elem);
            if let Some(sp) = &src_path {
                if !moving && src.name() != "rangeExpr" && self.is_bearing(&elem) {
                    let d = self.depth + 1;
                    let l = self.own.add_loan(sp, "alias", Some(&var.text), d, ftok(src).line);
                    self.own.holders.insert(&var.text, l);
                }
            }
            if iv::RANGED_LENGTH_FACT && src.name() == "rangeExpr" && src.kid(1).map_or(false, |k| k.text_is("..<")) {
                if let Some(cp) = self.count_receiver(src.kid(2).unwrap().node().unwrap()) {
                    // D-71: the body must neither assign nor exclusively loan `p`, a prefix of it or a field of it
                    if !self.assigned_places(inner.find("block").unwrap()).iter().any(|q| iv::overlaps(q, &cp)) {
                        self.facts.below.insert(var.text.clone(), cp);
                    }
                }
            }
        }
        let fresh = if var.text != "_" { vec![var.text.clone()] } else { vec![] };
        let linear = moving && self.is_linear(&elem);
        if linear {
            self.linear_loops.push((self.depth, var.clone()));
        }
        self.loop_body(inner, inner.find("block").unwrap(), &e, &fresh, true, None);
        if linear {
            self.linear_loops.pop();
        }
    }

    // ---- function exits (5.4) ------------------------------------------------------------
    pub fn terminates(&self, block: N<'a>) -> bool {
        self.terminates_to(block, true)
    }

    /// Whether the block's end is unreachable. `give` ends only its own match arm or if-expression branch (5.5), so it
    /// ends the block when `gives` (the block is in that arm or branch, not in a match nested in it, D-76).
    pub fn terminates_to(&self, block: N<'a>, gives: bool) -> bool {
        let sts = block.child_nodes("statement");
        let last = match sts.last() {
            Some(l) => *l,
            None => return false,
        };
        let inner = match last.kid(0) {
            Some(C::N(n)) => n,
            _ => {
                let w = &last.direct_tokens()[0].text;
                return ["return", "fail", "recover", "break", "continue"].contains(&w.as_str()) || (w == "give" && gives);
            }
        };
        match inner.name() {
            "ifStmt" => {
                let blocks = inner.child_nodes("block");
                let kids = inner.kids();
                let elses = kids.iter().filter(|c| c.text_is("else")).count() as i64;
                let ifs = kids.iter().filter(|c| c.text_is("if")).count() as i64;
                let has_else = elses > ifs - 1;
                has_else && blocks.iter().all(|b| self.terminates_to(*b, gives))
            }
            "loopStmt" => !inner.tokens().iter().any(|t| t.text == "break"),
            "exprStmt" => {
                let e = inner.kid(0).unwrap().node().unwrap();
                let ts = e.tokens();
                if ts.first().map_or(false, |t| t.text == "fail") {
                    return true;
                }
                if let Some(m) = e.find("matchExpr") {
                    if ts.first().map_or(false, |t| t.text == "match") {
                        let arms = m.child_nodes("matchArm");
                        return !arms.is_empty() && arms.iter().all(|a| self.arm_terminates_to(*a, false));
                    }
                }
                self.never_call(e)
            }
            "block" => self.terminates_to(inner, gives),
            "scopeStmt" => self.terminates_to(inner.find("block").unwrap(), gives),
            _ => false,
        }
    }

    /// Whether the node holds a `give` that ends the arm or branch the node is in, not one nested in it (5.5).
    pub fn gives_to(&self, node: N<'a>) -> bool {
        node.kids().into_iter().any(|c| match c {
            C::T(t) => t.text == "give",
            C::N(n) => !matches!(n.name(), "matchExpr" | "ifExpr" | "lambda") && self.gives_to(n),
        })
    }

    pub fn arm_terminates(&self, arm: N<'a>) -> bool {
        self.arm_terminates_to(arm, true)
    }

    pub fn arm_terminates_to(&self, arm: N<'a>, gives: bool) -> bool {
        let body = arm.kids().into_iter().filter_map(|c| c.node()).filter(|c| c.name() == "expr" || c.name() == "block").last().unwrap();
        if body.name() == "block" {
            // in a nested match, a `give` anywhere in the arm continues after the match (D-76)
            return !(!gives && self.gives_to(body)) && self.terminates_to(body, gives);
        }
        let ts = body.tokens();
        !ts.is_empty() && (ts[0].text == "fail" || self.never_call(body))
    }

    pub fn never_call(&self, e: N<'a>) -> bool {
        let pe = match e.find("postfixExpr") {
            Some(p) => p,
            None => return false,
        };
        let kids = pe.kids();
        let k0 = kids[0].node().unwrap();
        let pk = k0.kids();
        if pk.len() == 1 {
            if let C::T(t) = pk[0] {
                if t.kind == Kind::Lower {
                    return self.dc.table.get(&t.text).map_or(false, |fd| fd.kind == "func" && fd.sig.as_ref().map_or(false, |s| s.ret == never()));
                }
            }
        }
        let tr = k0.find("typeRef");
        let mn = k0.find("memberName");
        if let (Some(tr), Some(mn)) = (tr, mn) {
            let name = tr.direct_tokens().iter().find(|t| t.kind == Kind::Upper).map(|t| t.text.clone()).unwrap_or_default();
            let d = self.dc.table.get(&name).filter(|d| d.kind != "unknown");
            if let Some(d) = d {
                return d.methods.get(&mn.tokens()[0].text).map_or(false, |s| s.ret == never());
            }
        }
        false
    }

    /// An aggregate element has finished. A `move` is consumed now, so the next element cannot drop the marker
    /// (a match arm used to clear it); `copy` is done. A loan stays pending for the binding that receives the value.
    pub fn settle_element(&mut self) {
        let Some((ppath, mk, _)) = self.pending.clone() else { return };
        if mk == "move" {
            self.pending = None;
            if self.use_ok {
                self.own.move_(&ppath);
            }
        } else if mk == "copy" {
            self.pending = None;
        }
    }

    /// `return x` moves `x` when `x` is a local `let` binding of the function: its use visibly ends there (7.5,
    /// X-345). A field, a parameter, a capture or any other expression keeps the markers of 7.5.
    fn returned_local(&self, e: N<'a>, env: &Env) -> Option<String> {
        if e.find("markerExpr").is_some() {
            return None;
        }
        let path = self.path_of_expr(Some(e))?;
        if path.contains('.') || path.contains('[') {
            return None;
        }
        let b = env_lookup(env, &path)?;
        if (b.kind == "let" || b.kind == "mut") && b.depth > 0 {
            Some(path)
        } else {
            None
        }
    }

    /// `give` and `recover` hand their value to the enclosing binding: an existing ownership-bearing place needs a
    /// marker, and `move` moves it on this path (7.1; X-120). An expression match arm is its `give` (9.8; D-159).
    pub fn branch_value(&mut self, e: Option<N<'a>>, t: &Type, word: &str) {
        if word != "=>" && self.borrows_value(t) { self.check_temporary_borrows_in(e); }
        let e = match e {
            Some(e) => e,
            None => return,
        };
        let rp = self.path_of_expr(Some(e));
        if rp.is_none() && !t.is_loan() {
            self.coalesced_marker(Some(e), t, &format!("`{word}`")); // `give p ?? v` hands `p`'s value on (D-357)
        }
        if let Some((ppath, mk, _)) = self.pending.clone() {
            if mk == "move" {
                self.pending = None;
                self.own.move_(&ppath);
            }
        } else if let Some(rp) = rp {
            if self.is_bearing(t) && e.find("markerExpr").is_none() && !t.is_loan() {
                self.own_err(
                    "E-OWN-001",
                    format!("`{word} {rp}`: an existing ownership-bearing place needs `move {rp}` or `copy {rp}`"),
                    ftok(e),
                    &rp,
                    "marker required",
                    "LANG-07-01",
                    "",
                );
            }
        }
    }

    /// Check a function or lambda body; an expression body's type is returned (an inferred lambda result, 8.2).
    pub fn body(&mut self, body: Option<N<'a>>, env: &Env, ret: &Type, head_tok: &Token, label: &str) -> Option<Type> {
        let body = match body {
            Some(b) => b,
            None => return None,
        };
        let b = body.kid(0).and_then(|c| c.node()).filter(|n| n.name() == "block");
        if let Some(b) = b {
            self.walk_block(b, env);
            if !self.terminates(b) {
                // a path reaches the end of the body: what the parameters still hold is dropped there, as at a `return` (6.12)
                self.report_open(0, "at the end of the body");
                if known(ret) && *ret != void() {
                    self.typ("E-TYP-110", format!("{label} returns {} but a path reaches the end of its body without `return`, `fail` or a Never call", ret.show()), head_tok, &ret.show(), "fall-through", label, "LANG-05-05");
                } else {
                    self.obs_exit("success", head_tok, "the end of the body");
                }
            }
            if !self.terminates(b) {
                self.journal_exit("end of body", b.tokens().last().map_or(head_tok.line, |t| t.line));
            }
        } else {
            let e = body.find("expr");
            self.carried_temps.clear();
            let recording = self.lends_result(Some(ret)); // the parts of a `from` result are judged by their types (D-381)
            let t = self.infer(e, env, Some(ret));
            // the body's value is returned: a future in it that borrows a temporary outlives it (12.3; D-31)
            let temps = std::mem::take(&mut self.carried_temps);
            self.temporaries_kept(temps);
            if known(ret) {
                if let Some(e) = e {
                    self.expect_return(&t, ret, ftok(e), &format!("{label} result"));
                    self.loan_into_owned(&t, Some(ret), ftok(e), &format!("the {label} result"));
                    self.owned_into_lent(&t, Some(ret), e, env);
                }
            }
            if recording {
                self.node_types = None;
            }
            // an expression body is the return of its expression: an existing place needs a marker (7.1; X-118), and the
            // loans the value keeps are judged against the declared origin as a `return`'s are (7.6; D-395)
            if let Some(e) = e {
                self.check_returned_loans(e, &t, env, "body");
                self.report_open(0, "at the expression body"); // the expression body is the return: what the parameters still hold is dropped there (6.12)
                self.obs_exit("success", ftok(e), "the expression body");
            }
            return Some(t);
        }
        None
    }

    pub fn params_env(&self, sig: &Sig<'a>, env: &Env) -> Env {
        let e = env_new(Some(env));
        for (pn, pt, mode) in &sig.params {
            let k = match mode.trim_start_matches('.') {
                "editable" => "editable",
                "owned" => "owned",
                _ => "param",
            };
            env_set(&e, pn, pt.clone(), false, k);
        }
        e
    }

    // ---- module ------------------------------------------------------------------------
    pub fn run(&mut self, mtree: N<'a>) {
        let saved = self.nrecords();
        self.final_pass = false;
        self.run_once(mtree);
        self.truncate_records(saved);
        self.final_pass = true;
        if self.record_types {
            self.node_types = Some(HashMap::new());
        }
        self.run_once(mtree);
    }

    /// Note this module's unannotated constants and discard the records. A package check runs this on every
    /// module before typing any of them, so an importer sees the initialiser's type whichever module is first (D-54.4).
    pub fn seed_consts(&mut self, mtree: N<'a>) {
        self.check_imports(mtree);
        let names: Vec<String> = self.dc.table.decls.values().filter(|d| d.kind == "const" && d.module == self.module).map(|d| d.name.clone()).collect();
        for name in names {
            let _ = self.const_value_type(&name);
        }
        self.dc.records.clear();
    }

    fn run_once(&mut self, mtree: N<'a>) {
        self.elaborations.clear();
        self.check_imports(mtree);
        for u in self.units(mtree) {
            self.run_unit(mtree, &u);
        }
    }

    /// The import checks that open a module's pass (E-NAM-004).
    pub fn check_imports(&mut self, mtree: N<'a>) {
        for imp in imports(mtree) {
            let path = imp.path.join(".");
            match &imp.members {
                None => {
                    let a = imp.alias.clone().unwrap_or_else(|| imp.path.last().unwrap().clone());
                    self.aliases.insert(a, path.clone());
                }
                Some(ms) => {
                    for m in ms {
                        if let Some(d) = self.dc.table.get(m).cloned() {
                            if d.module == path && !d.exported && d.kind != "unknown" {
                                let mut cands: Vec<String> = self.dc.table.decls.iter().filter(|(_, v)| v.module == path && v.exported).map(|(k, _)| k.clone()).collect();
                                cands.sort();
                                self.rec("E-NAM-004", format!("{path} does not export `{m}`"), "LANG-04-05", imp.token, Self::nam(m, cands));
                            }
                        }
                    }
                }
            }
        }
    }

    /// The units a module's pass checks, in order: its declarations (in table order), its `implement` blocks, then
    /// its tests and properties. Each has an ID that survives edits elsewhere: a declaration's name, an implement
    /// block's interface and target, a test's name (DEL-46 checks units incrementally).
    pub fn units(&self, mtree: N<'a>) -> Vec<Unit> {
        let mut out: Vec<Unit> = self.dc.table.decls.values().filter(|d| d.module == self.module).map(|d| Unit::Decl(d.name.clone())).collect();
        for (i, (iface, target, module, _)) in self.dc.table.implement_blocks.iter().enumerate() {
            if *module == self.module {
                out.push(Unit::Implement(i, format!("{} for {}", iface.name, target.name)));
            }
        }
        for (i, d) in declarations(mtree).iter().enumerate() {
            if d.kind == "test" || d.kind == "property" {
                out.push(Unit::Test(i, format!("{} {}", d.kind, d.token.map(|t| t.text.clone()).unwrap_or_default())));
            }
        }
        out
    }

    /// Check one unit of the module's pass.
    pub fn run_unit(&mut self, mtree: N<'a>, unit: &Unit) {
        match unit {
            Unit::Decl(name) => {
                let Some(d) = self.dc.table.decls.values().find(|d| d.module == self.module && d.name == *name).cloned() else { return };
        self.check_storage_decl(&d);
        match d.kind.as_str() {
            "func" => {
                if let Some(sig) = d.sig.clone() {
                    let tok = tok_of(&d);
                    if d.exported {
                        self.check_exported_sig(&sig, &d.name, &tok);
                    }
                    self.check_result_origin(&sig, &d.name, &tok);
                    self.check_param_defaults(&sig, &d.name);
                    self.check_ignorable(&sig, &d.name);
                    self.param_default_tparams(&sig, &sig.type_params.clone());
                    if sig.body.is_some() {
                        self.current_aspects = d.aspects.clone();
                        self.run_body(&sig, None, "self", &tok, &format!("function {}", d.name));
                        self.current_aspects.clear();
                    }
                }
            }
            "aspect" => {
                if self.final_pass {
                    self.check_aspect_decl(&d);
                    if let Some(ap) = d.node.and_then(|n| n.child_node("aspectParams")) {
                        let types: Vec<(String, Type)> = d.aspect.as_ref().map(|a| a.params.iter().map(|(n, t, _)| (n.clone(), t.clone())).collect()).unwrap_or_default();
                        self.check_defaults(ap.child_nodes("param"), &types, &format!("aspect {}", d.name));
                    }
                }
            }
            "class" | "enum" => {
                let self_t = named_of_decl(&d, d.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect());
                let tok = tok_of(&d);
                if let Some(lt) = &d.lifetime {
                    self.own_err(
                        "E-OWN-053",
                        format!("{} declares the lifetime parameter `{lt}`: stored loans are not in Language 1.0 (7.7, X-137); own the value, copy it, hold a Shared, or pass it to each call", d.name),
                        &tok,
                        &d.name,
                        "stored loan",
                        "LANG-07-06",
                        "",
                    );
                }
                for (fname, (ft, exp, _)) in d.fields.iter() {
                    if d.exported && *exp {
                        self.check_type_exported(ft, &format!("{}.{fname}", d.name), &tok, false);
                    }

                }
                for m in d.node.unwrap().kids().into_iter().filter_map(|c| c.node()) {
                    let fd = if m.name() == "classMember" || m.name() == "enumMember" { m.find("fieldDecl") } else { None };
                    if let Some(fd) = fd {
                        if let Some(ce) = fd.child_node("constExpr") {
                            // outside every body: a default cannot create the capabilities (D-149)
                            self.decl_context(format!("default of {}.{}", d.name, fd.direct_tokens()[0].text));
                            // a generic class's field default is typed with its type parameters in scope (X-180)
                            let saved_scope = std::mem::replace(&mut self.tparams_in_scope, d.type_params.iter().cloned().collect());
                            let want = self.dc.type_of(fd.find("type"));
                            let t = self.infer(Some(ce), &env_new(None), Some(&want));
                            self.tparams_in_scope = saved_scope;
                            let fname = fd.direct_tokens()[0].text.clone();
                            self.expect(&t, &want, ftok(ce), &format!("default of `{fname}`"));
                            self.const_tparams(Some(ce), &d.type_params.clone());
                        }
                    }
                    let cd = if m.name() == "classMember" || m.name() == "enumMember" { m.child_node("constDecl") } else { None };
                    if let Some((cd, cce)) = cd.and_then(|cd| cd.child_node("constExpr").map(|c| (cd, c))) {
                        if m.name() == "classMember" {
                            // a class constant is a constant like a module's (D-149)
                            let cname = cd.direct_tokens().iter().find(|t| t.kind == Kind::Lower).map(|t| t.text.clone()).unwrap_or_else(|| "?".into());
                            self.decl_context(format!("const {}.{cname}", d.name));
                            let want = cd.child_node("type").map(|ty| self.dc.type_of(Some(ty)));
                            let ct = self.infer(Some(cce), &env_new(None), want.as_ref());
                            if let Some(want) = &want {
                                self.expect(&ct, want, ftok(cce), &format!("const `{cname}`"));
                            }
                            // the constant an interface declares has the interface's type (9.3, 9.5; D-65)
                            let ctype = want.clone().unwrap_or(ct);
                            for i in d.implements.clone() {
                                let Some(idecl) = self.dc.resolve_named(&i, None) else { continue };
                                if idecl.kind != "interface" {
                                    continue;
                                }
                                let Some(declared) = idecl.constants.get(&cname).cloned() else { continue };
                                if !known(&declared) {
                                    continue;
                                }
                                let isub: BTreeMap<String, Type> = idecl.type_params.iter().cloned().zip(i.args.iter().cloned()).collect();
                                let itype = self.dc.subst(&declared, &isub);
                                if crate::literals::assignable(&ctype, &itype).0 == Some(false) {
                                    let displays = diagnostic_types(&[&ctype, &itype]);
                                    let cshown = if crate::literals::is_strlit(&ctype) { "a string literal".to_string() } else { displays[0].clone() };
                                    self.typ("E-TYP-080", format!("{}.{cname} does not satisfy {}.{cname}: {cshown} is not {} (9.5)", d.name, i.name, displays[1]), ftok(cce), &displays[1], &cshown, &cname, "LANG-09-03");
                                }
                            }
                        }
                        // an associated call on a type parameter in a class or enum constant (E-TYP-170, X-180)
                        self.const_tparams(Some(cce), &d.type_params.clone());
                    }
                    let cl = if m.name() == "classMember" { m.find("cleanupDecl") } else { None };
                    if let Some(cl) = cl {
                        self.fresh_activation();
                        self.current_label = format!("cleanup {}", d.name);
                        self.self_type = Some(self_t.clone());
                        self.self_mode = "editable self".into();
                        self.ret_type = Some(void());
                        self.in_try = false;
                        self.ctx = vec!["function".into()];
                        self.loop_labels.clear();
                        self.journal_begin(&format!("cleanup {}", d.name), cl.tokens()[0].line);
                        // a generic class's cleanup: its type parameters and `where` bounds are in force, as in its methods
                        let saved_scope = (std::mem::replace(&mut self.tparams_in_scope, d.type_params.iter().cloned().collect()), std::mem::replace(&mut self.where_in_scope, d.where_.clone()));
                        let mut bounds: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                        for (tp, iface) in d.where_.iter() {
                            bounds.entry(tp.clone()).or_default().insert(iface.interface_key());
                        }
                        let saved_bounds = std::mem::replace(&mut self.dc.tparam_bounds, bounds);
                        self.walk_block(cl.find("block").unwrap(), &env_new(None));
                        self.tparams_in_scope = saved_scope.0;
                        self.where_in_scope = saved_scope.1;
                        self.dc.tparam_bounds = saved_bounds;
                        if self.eff.needs_auth && self.receiver_authorities(&self_t).is_empty() {
                            // cleanup is a method: the authority it uses is one its class holds in a field (11.6, X-270; D-173)
                            let at = self.eff.sites.get("io").cloned().unwrap_or_else(|| cl.tokens()[0].clone());
                            self.eff_err(
                                "E-EFF-010",
                                format!("cleanup {} performs I/O, but {} holds no Authority in a field: a cleanup uses the authority its class holds (11.6)", d.name, d.name),
                                &at,
                                "an Authority field",
                                "io",
                                &format!("cleanup {}", d.name),
                                "LANG-11-07",
                            );
                        }
                        self.summaries.insert(format!("cleanup {}", d.name), self.eff.clone());
                        self.journal_end();
                        self.self_type = None;
                        self.ret_type = None;
                        self.ctx.clear();
                    }
                }
                for (mname, sig) in d.methods.iter() {
                    let stok = sig.node.map(|n| ftok(n).clone()).unwrap_or_else(|| tok.clone());
                    if d.exported && sig.exported {
                        self.check_exported_sig(sig, &format!("{}.{mname}", d.name), &stok);
                    }
                    self.check_result_origin(sig, &format!("{}.{mname}", d.name), &stok);
                    self.check_param_defaults(sig, &format!("{}.{mname}", d.name));
                    self.check_ignorable(sig, &format!("{}.{mname}", d.name));
                    let scope: Vec<String> = d.type_params.iter().chain(sig.type_params.iter()).cloned().collect();
                    self.param_default_tparams(sig, &scope);
                    if sig.body.is_some() {
                        let recv = sig.receiver.clone().unwrap_or_else(|| "self".into());
                        self.run_body(sig, Some(self_t.clone()), &recv, &stok, &format!("method {}.{mname}", d.name));
                    }
                }
                // a `convert` member's body is a method body (6.13, X-551): `convert to T` has the receiver `self`
                // and returns `T`; `convert from S(s)` binds the source `s` and returns the declaring type
                for cv in d.converts.iter() {
                    let sig = &cv.sig;
                    let stok = cv.node.tokens()[0].clone();
                    if d.exported && sig.exported {
                        self.check_exported_sig(sig, &format!("{}.{}", d.name, sig.name), &stok);
                    }
                    self.check_result_origin(sig, &format!("{}.{}", d.name, sig.name), &stok);
                    if sig.body.is_some() {
                        let self_arg = if cv.direction == "to" { Some(self_t.clone()) } else { None };
                        let recv = sig.receiver.clone().unwrap_or_else(|| "self".into());
                        self.run_body(sig, self_arg, &recv, &stok, &format!("method {}.{}", d.name, sig.name));
                    }
                }
                // A mixin's bodies are checked at every application, with the class as self and the mixin's
                // lexical scope (9.6, X-476). A method the class declares itself takes precedence. An
                // application that lacks a required method is already E-TYP-080, and its bodies are not typed
                for mx in d.mixins.clone() {
                    if !self.check_mixin_applications { break; }
                    let Some(md) = self.dc.resolve_named(&mx, None).filter(|md| md.kind == "mixin") else { continue };
                    let iface = self.dc.interface_members(&d);
                    if md.methods.keys().any(|r| r.strip_prefix("requires:").map_or(false, |n| !d.methods.contains(n) && !iface.contains(n))) {
                        continue;
                    }
                    for (mname, msig) in md.methods.iter() {
                        if mname.starts_with("requires:") || d.methods.contains(mname) || msig.body.is_none() {
                            continue;
                        }
                        let mtok = msig.node.map(|n| n.tokens()[0].clone()).unwrap_or_else(|| tok_of(&d));
                        let recv = msig.receiver.clone().unwrap_or_else(|| "self".into());
                        self.run_mixin_body(&d, &md, mname, msig, self_t.clone(), &recv, &mtok);
                    }
                }
            }
            "const" => {
                let tok = tok_of(&d);
                let node = d.node.unwrap();
                let mut export_ty = d.alias.clone();
                if let Some(ce) = node.child_node("constExpr") {
                    // typed whether or not it is annotated, outside every body: a constant cannot create the capabilities
                    // (D-149). an unannotated constant's type is its initialiser's, stored for later uses (D-54.4)
                    self.decl_context(format!("const {}", d.name));
                    let annotated = node.child_node("type").is_some();
                    let t = self.infer(Some(ce), &env_new(None), if annotated { d.alias.as_ref() } else { None });
                    if annotated {
                        let a = d.alias.clone().unwrap();
                        self.expect(&t, &a, ftok(ce), &format!("const `{}`", d.name));
                    } else if known(&t) {
                        self.note_inferred_const(&d.module, &d.name, &t);
                        export_ty = Some(t);
                    }
                }
                if d.exported {
                    if let Some(a) = &export_ty {
                        if known(a) {
                            self.check_type_exported(a, &d.name, &tok, false);
                        }
                    }
                }
            }
            "type" => {
                if d.exported {
                    if let Some(a) = &d.alias {
                        self.check_type_exported(a, &d.name, &tok_of(&d), true);
                    }
                }
            }
            "interface" => {
                for (mname, sig) in d.methods.iter() {
                    if d.exported {
                        self.check_exported_sig(sig, &format!("{}.{mname}", d.name), &tok_of(&d));
                    }
                    self.check_param_defaults(sig, &format!("{}.{mname}", d.name));
                    let scope: Vec<String> = d.type_params.iter().chain(sig.type_params.iter()).cloned().collect();
                    self.param_default_tparams(sig, &scope);
                }
            }
            _ => {}
        }

            }
            Unit::Implement(i, _) => {
                let Some((_, target, module, node)) = self.dc.table.implement_blocks.get(*i).cloned() else { return };
                if module != self.module {
                    return;
                }
            for m in node.child_nodes("implementMember") {
                if let Some(head) = m.find("methodHead") {
                    let sig = self.dc.signature(head, true, m.find("funcBody"));
                    self.check_param_defaults(&sig, &format!("{}.{}", target.name, sig.name));
                    let owner_params: Vec<String> = self.decl_of(&Type::Named(target.clone())).map(|o| o.type_params.clone()).unwrap_or_default();
                    let scope: Vec<String> = owner_params.into_iter().chain(sig.type_params.iter().cloned()).collect();
                    self.param_default_tparams(&sig, &scope);
                    if sig.body.is_some() {
                        let recv = sig.receiver.clone().unwrap_or_else(|| "self".into());
                        self.run_body(&sig, Some(Type::Named(target.clone())), &recv, ftok(head), &format!("method {}.{}", target.name, sig.name));
                    }
                }
            }

            }
            Unit::Test(i, _) => {
                if let Some(d) = declarations(mtree).get(*i) {
                    if d.kind == "test" || d.kind == "property" {
                        self.run_test_body(d.node, d.kind, d.token);
                    }
                }
            }
        }
    }

    fn run_test_body(&mut self, node: N<'a>, kind: &str, tok: Option<&Token>) {
        self.fresh_activation();
        self.current_label = format!("test {}", tok.map(|t| t.text.clone()).unwrap_or_default());
        let env = env_new(None);
        let ts = node.direct_tokens();
        let words: Vec<&str> = ts.iter().map(|t| t.text.as_str()).collect();
        let rows: Vec<N<'a>> = node.child_nodes("caseRow");
        self.row_name = None;
        self.row_uses.clear();
        if kind == "test" {
            if let Some(i) = words.iter().position(|w| *w == "as") {
                self.row_name = Some(words[i + 1].to_string());
                env_set(&env, words[i + 1], unknown("test row"), false, "let");
            }
        }
        if kind == "property" {
            if let Some(pl) = node.find("paramList") {
                for p in pl.child_nodes("param") {
                    let pts = p.direct_tokens();
                    if let Some(pt) = p.find("paramType") {
                        if pts.first().map_or(false, |t| t.kind == Kind::Lower) {
                            let t = self.dc.type_of(pt.find("type"));
                            env_set(&env, &pts[0].text, t, false, "param");
                        }
                    }
                }
            }
        }
        self.self_type = None;
        self.self_mode = "self".into();
        self.ret_type = Some(void());
        self.in_try = true;
        self.ctx = vec!["function".into()];
        self.loop_labels.clear();
        let blk = *node.child_nodes("block").last().unwrap();
        let label = self.current_label.clone();
        self.journal_begin(&label, node.tokens()[0].line);
        self.test_body = true; // `or fail` is implied in the body (14, X-345)
        self.in_test_code = true;
        for init in fortis_names::reading::outcome_lets(blk) {
            self.mark_consumed(Some(init)); // a binding the body consumes as an outcome keeps it (14)
        }
        self.walk_block(blk, &env);
        if !rows.is_empty() {
            self.check_rows(&rows); // every row literal against how the body uses the row's fields (14, D-50.3)
        }
        self.test_body = false;
        self.in_test_code = false;
        self.row_name = None;
        self.row_uses.clear();
        self.summaries.insert(label.clone(), self.eff.clone()); // a test's effects, as a body's (the IR and its validator read it)
        self.journal_end();
        self.self_type = None;
        self.ret_type = None;
        self.ctx.clear();
    }

    /// Every row literal of a `cases` table is checked against how the body uses the row's fields (14): a field passed
    /// where a type is expected is checked as that literal would be at the use (E-REF-001 for a refined parameter), and
    /// a row that lacks a field the body reads is E-TYP-050 (D-50.3).
    fn check_rows(&mut self, rows: &[N<'a>]) {
        let env = env_new(None);
        let uses = self.row_uses.clone();
        for row in rows {
            let label = row.direct_tokens()[0].clone();
            let Some(lit) = row.find("recordLit") else { continue };
            let kids = lit.kids();
            let mut ks: Vec<&Token> = kids.iter().filter(|k| k.is("ident")).map(|k| k.node().unwrap().tokens()[0]).collect();
            if ks.is_empty() {
                ks = kids.iter().filter_map(|k| k.tok()).filter(|t| matches!(t.kind, Kind::Lower | Kind::Upper | Kind::Kw)).collect();
            }
            let es = lit.child_nodes("expr");
            let present: BTreeSet<String> = ks.iter().map(|k| k.text.clone()).collect();
            for (k, e) in ks.iter().zip(es.iter()) {
                let wants = uses.get(&k.text).cloned().unwrap_or_default();
                if wants.is_empty() {
                    self.infer(Some(*e), &env, None);
                    continue;
                }
                let t = self.infer(Some(*e), &env, Some(&wants[0]));
                let first = e.tokens()[0].clone();
                for want in &wants {
                    self.expect(&t, want, &first, &format!("row field `{}` of {}", k.text, label.text));
                }
            }
            let missing: Vec<String> = uses.keys().filter(|f| !present.contains(*f)).cloned().collect();
            if !missing.is_empty() {
                let s = if missing.len() > 1 { "s" } else { "" };
                let list = missing.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
                self.typ("E-TYP-050", format!("row {} lacks field{s} {list} that the body reads", label.text), &label, &missing.join(", "), "absent", &label.text, "LANG-14-01");
            }
        }
    }

    /// Module aliases are lexical names too: an applied body uses the imports beside its declaration.
    pub fn body_aliases(dc: &DeclChecker<'a>) -> BTreeMap<String, String> {
        let Some(lm) = dc.library().modules.get(&dc.module_name) else { return BTreeMap::new() };
        imports(lm.tree()).into_iter().filter(|i| i.members.is_none()).map(|i| {
            let alias = i.alias.clone().unwrap_or_else(|| i.path.last().unwrap().clone());
            (alias, i.path.join("."))
        }).collect()
    }

    fn run_mixin_body(&mut self, owner: &DeclRef<'a>, mixin: &DeclRef<'a>, member: &str, sig: &Sig<'a>, self_t: Type, mode: &str, tok: &Token) {
        let home = self.dc.body_context(mixin);
        let foreign = mixin.module != self.module;
        if foreign {
            let key = (home.lib, mixin.module.clone());
            if !self.mixin_home_summaries.contains_key(&key) {
                // A private helper belongs to this lexical module, so its inferred effects are available as
                // they are to a local application. Seed ordinary bodies twice without checking applications:
                // an import cycle or another class using this mixin must not recurse into this seed pass.
                let lm = home.library().modules.get(&mixin.module).expect("an imported mixin has a library module");
                let mut seed = Typer::new(home.clone());
                seed.final_pass = false;
                seed.check_mixin_applications = false;
                seed.run_once(lm.tree());
                seed.run_once(lm.tree());
                self.mixin_home_summaries.insert(key, seed.summaries);
            }
        }
        let aliases = if foreign { Self::body_aliases(&home) } else { self.aliases.clone() };
        let saved_dc = std::mem::replace(&mut self.dc, home);
        let saved_module = std::mem::replace(&mut self.module, mixin.module.clone());
        let saved_package = std::mem::replace(&mut self.package, mixin.package.clone());
        let saved_aliases = std::mem::replace(&mut self.aliases, aliases);
        let saved_summaries = if foreign {
            let old = std::mem::take(&mut self.summaries);
            // The applying module's summaries remain usable for self's methods, under their foreign labels.
            // Never let an applying module's same-spelled helper stand for the mixin's lexical helper.
            for (label, summary) in &old {
                let qualified = label.split_once(' ').map(|(kind, name)| {
                    if name.contains("::") { label.clone() } else { format!("{kind} {saved_module}::{name}") }
                }).unwrap_or_else(|| label.clone());
                self.summaries.insert(qualified, summary.clone());
            }
            if let Some(home) = self.mixin_home_summaries.get(&(self.dc.lib, mixin.module.clone())) {
                self.summaries.extend(home.clone());
            }
            Some(old)
        } else { None };
        // Node IDs are shared by every application of a mixin; positions are shared by unrelated module files.
        // Keep this body's lowering facts separate from the applying module's and the other applications'.
        let saved_legacy = std::mem::take(&mut self.legacy_conversions);
        let saved_sources = std::mem::take(&mut self.legacy_conversion_sources);
        let saved_types = self.node_types.take();
        if self.record_types && self.final_pass { self.node_types = Some(HashMap::new()); }
        let saved_closures = std::mem::take(&mut self.closure_info);
        let saved_copy_refused = std::mem::take(&mut self.copy_refused);
        let saved_moves = std::mem::take(&mut self.implicit_moves);
        let saved_checked = std::mem::take(&mut self.checked_ops);
        let saved_fails = std::mem::take(&mut self.implied_fails);
        let saved_regions = std::mem::take(&mut self.implied_regions);
        let saved_markers = self.redundant_markers.clone();
        let saved_calls = self.call_sigs.clone();
        let saved_migrations = self.migrations.len();
        self.mixin_view = Some(mixin.name.clone());
        let label = format!("method {}.{member}", owner.name);
        self.run_body(sig, Some(self_t), mode, tok, &label);
        self.mixin_view = None;
        let legacy_conversions = std::mem::replace(&mut self.legacy_conversions, saved_legacy);
        let legacy_conversion_sources = std::mem::replace(&mut self.legacy_conversion_sources, saved_sources);
        if !foreign { self.legacy_conversions.extend(legacy_conversions.clone()); }
        let facts = super::MixinBodyFacts {
            legacy_conversions,
            legacy_conversion_sources,
            node_types: self.node_types.take().unwrap_or_default(),
            closure_info: std::mem::replace(&mut self.closure_info, saved_closures),
            implicit_moves: std::mem::replace(&mut self.implicit_moves, saved_moves),
            checked_ops: std::mem::replace(&mut self.checked_ops, saved_checked),
            implied_fails: std::mem::replace(&mut self.implied_fails, saved_fails),
            implied_regions: std::mem::replace(&mut self.implied_regions, saved_regions),
        };
        self.node_types = saved_types;
        self.copy_refused = saved_copy_refused;
        if self.record_types && self.final_pass {
            self.mixin_body_facts.insert((owner.type_identity(), owner.name.clone(), member.to_string()), facts);
        }
        if foreign {
            // Formatter edits and markers are for the file it is formatting, never a foreign body with the
            // same line and column. The body's diagnostic corrections already name its own file.
            self.redundant_markers = saved_markers;
            self.call_sigs = saved_calls;
            self.migrations.truncate(saved_migrations);
        }
        if let Some(mut old) = saved_summaries {
            if let Some(summary) = self.summaries.get(&label) { old.insert(label, summary.clone()); }
            self.summaries = old;
        }
        let mut records = std::mem::replace(&mut self.dc, saved_dc).records;
        for r in &mut records {
            r.message.push_str(&format!(" (mixin {} applied to {}.{})", mixin.name, owner.module, owner.name));
        }
        self.dc.records.extend(records);
        self.module = saved_module;
        self.package = saved_package;
        self.aliases = saved_aliases;
    }

    pub fn run_body(&mut self, sig: &Sig<'a>, self_t: Option<Type>, mode: &str, head_tok: &Token, label: &str) {
        let saved_bounds = std::mem::take(&mut self.dc.tparam_bounds);
        let mut bounds: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let owner = self_t.as_ref().and_then(|t| self.decl_of(t));
        let mut wh: Vec<(String, NamedT)> = owner.as_ref().map(|o| o.where_.clone()).unwrap_or_default();
        wh.extend(sig.where_.iter().cloned());
        for (tp, iface) in wh {
            bounds.entry(tp).or_default().insert(iface.interface_key());
        }
        self.dc.tparam_bounds = bounds;
        let owner = self_t.as_ref().and_then(|t| self.decl_of(t));
        let mut scope: BTreeSet<String> = owner.as_ref().map(|o| o.type_params.iter().cloned().collect()).unwrap_or_default();
        scope.extend(sig.type_params.iter().cloned());
        let mut wh: Vec<(String, NamedT)> = owner.as_ref().map(|o| o.where_.clone()).unwrap_or_default();
        wh.extend(sig.where_.iter().cloned());
        let saved_tp = std::mem::replace(&mut self.dc.type_params, scope.clone()); // an annotation in the body names them (D-52.7)
        let saved_scope = (std::mem::replace(&mut self.tparams_in_scope, scope), std::mem::replace(&mut self.where_in_scope, wh));
        let mut linear: BTreeSet<String> = owner.as_ref().map(|o| o.linear.iter().cloned().collect()).unwrap_or_default();
        linear.extend(sig.linear.iter().cloned());
        let saved_dc_linear = std::mem::replace(&mut self.dc.linear_params, linear.clone());
        let saved_linear = std::mem::replace(&mut self.linear_in_scope, linear);
        for t in std::iter::once(&sig.ret).chain(sig.params.iter().map(|p| &p.1)) {
            self.dc.check_linear_instantiation(t, &self.linear_in_scope, head_tok);
        }
        self.run_body_inner(sig, self_t, mode, head_tok, label);
        self.dc.tparam_bounds = saved_bounds;
        self.dc.type_params = saved_tp;
        self.tparams_in_scope = saved_scope.0;
        self.where_in_scope = saved_scope.1;
        self.linear_in_scope = saved_linear;
        self.dc.linear_params = saved_dc_linear;
    }

    fn seed_params(&mut self, sig: &Sig<'a>, head_tok: &Token) {
        for (pn, pt, pmode) in &sig.params {
            if (pmode.is_empty() || pmode == "editable") && self.is_bearing(pt) {
                let l = self.own.add_loan(pn, if pmode == "editable" { "editable" } else { "alias" }, Some(pn), 0, head_tok.line);
                self.own.holders.insert(pn, l);
            } else if pmode.trim_start_matches('.') == "owned" && self.is_linear(pt) {
                self.own.open_obligation(pn, head_tok.clone(), 0);
            }
        }
    }

    fn run_body_inner(&mut self, sig: &Sig<'a>, self_t: Option<Type>, mode: &str, head_tok: &Token, label: &str) {
        // the journal covers the woven body: the applied aspects' before, after and handler blocks are its facts
        if self.final_pass {
            crate::journal::begin(label, head_tok.line);
            if let Some(mixin) = &self.mixin_view {
                crate::journal::source(&self.dc.file, &format!("method {mixin}.{}", sig.name));
            }
        }
        self.facts = Facts::default();
        self.own = Owners::default();
        self.depth = 0;
        self.eff = Summary::default();
        self.fresh.clear();
        self.discharges.clear();
        self.reported.clear();
        self.current_label = label.to_string();
        // fault injection for the internal-defect tests (E-INT, DEL-43): `FORTIS_FAULT=typer:LABEL` fails here, as a
        // checker defect would, when the function's label is LABEL
        if let Ok(v) = std::env::var("FORTIS_FAULT") {
            if v.strip_prefix("typer:") == Some(label) {
                panic!("injected fault in the typer at {label}");
            }
        }
        self.head_toks = sig.node.map(|n| n.tokens().into_iter().cloned().collect()).unwrap_or_default();
        self.editable_params = sig.params.iter().filter(|p| p.2.trim_start_matches('.') == "editable").map(|p| p.0.clone()).collect();
        self.seed_params(sig, head_tok);
        self.self_type = self_t;
        self.self_mode = mode.to_string();
        self.ret_type = Some(sig.ret.clone());
        self.in_try = self.expanded_mods(sig).contains("try");
        self.ctx = vec!["function".into()];
        self.loop_labels.clear();
        self.eff.authorities = self.authority_params(sig);
        self.schema = None;
        self.caps_calls = 0;
        self.schema_unknown = false;
        self.apply_aspects(sig, head_tok, label);
        self.facts = Facts::default();
        self.own = Owners::default();
        self.depth = 0;
        self.part_owners.clear();
        self.temporary_values.clear();
        self.temporary_borrows.clear();
        self.owned_parts.clear();
        self.coalesced_places.clear();
        self.seed_params(sig, head_tok);
        for (_, pt, pm) in &sig.params {
            if pm.trim_start_matches('.') == "owned" {
                self.charge_drop(pt, head_tok); // the function owns the argument and may drop it (7.9, 11.3; D-393)
            }
        }
        if mode == "owned self" {
            if let Some(st) = self.self_type.clone() {
                self.charge_drop(&st, head_tok);
                if let Some(owner) = self.decl_of(&st).filter(|o| o.kind == "class") {
                    let sub = self.dc.type_substitution(&owner, &st);
                    for (fname, (ft, _, _)) in owner.fields.iter() {
                        if self.is_linear(&self.dc.subst(ft, &sub)) { self.obligate(&format!("self.{fname}"), head_tok); }
                    }
                } else if self.is_linear(&st) { self.obligate("self", head_tok); }
            }
        }
        let env = self.params_env(sig, &env_new(None));
        let ret = sig.ret.clone();
        self.body(sig.body, &env, &ret, head_tok, label);
        self.summaries.insert(label.to_string(), self.eff.clone());
        if self.final_pass {
            crate::journal::set_effects(&self.eff);
            if let Some(ir) = crate::journal::end() {
                self.ir_out.push(ir);
            }
            self.check_claims(sig, head_tok, label);
        }
        self.self_type = None;
        self.ret_type = None;
        self.ctx.clear();
    }

    /// The journal of a body typed outside run_body (a cleanup, a test, a property), in the final pass only.
    fn journal_begin(&mut self, label: &str, line: usize) {
        if self.final_pass {
            crate::journal::begin(label, line);
        }
    }

    fn journal_end(&mut self) {
        if self.final_pass {
            crate::journal::set_effects(&self.eff);
            if let Some(ir) = crate::journal::end() {
                self.ir_out.push(ir);
            }
        }
    }

    /// An exit of the function: the active deferred blocks run on it, innermost first (12.1).
    pub fn journal_exit(&self, kind: &str, line: usize) {
        let runs: Vec<usize> = self.deferred.iter().rev().map(|d| d.2.line).collect();
        crate::journal::cleanup("exit", kind, line, runs);
    }

    #[allow(dead_code)]
    fn _unused_imports(_a: Type) -> Type {
        let _ = interp();
        unknown("")
    }
}
