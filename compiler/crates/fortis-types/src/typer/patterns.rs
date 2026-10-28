//! Match, patterns and if-expressions (Language Reference 5.3, 9.8): the pattern half of checker/typer.py.

use std::collections::{BTreeMap, BTreeSet};

use fortis_diag::Json;
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{C, N};

use super::*;
use crate::literals::{assignable, is_strlit, literal_type};
use crate::owners::Owners;
use crate::types::*;

impl<'a> Typer<'a> {
    /// `break` states recorded during this arm name its bindings. Put back the facts those names had on entry, so a
    /// loop outside the match does not keep this arm's guard facts (the same rule as the arm's residual and exits).
    fn scrub_arm_breaks(&mut self, marks: &[usize], names: &[String], outer: &Facts) {
        let n = marks.len().min(self.break_frames.len());
        for i in 0..n {
            for f in &mut self.break_frames[i].2[marks[i]..] {
                f.restore_names(names, outer);
            }
        }
    }

    pub fn match_(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>, as_statement: bool) -> Type {
        let kids = node.kids();
        let moving = kids.iter().any(|k| k.text_is("move"));
        let scrut = kids.iter().filter_map(|k| k.node()).find(|k| k.name() == "expr" || k.name() == "place");
        let mut st = strip(&self.infer(scrut, env, None)).clone();
        if is_strlit(&st) {
            st = string_view();
        }
        let sp = match scrut {
            Some(s) if s.name() == "place" => Some(self.path_of_place(s)),
            other => self.path_of_expr(other),
        };
        if let (Some(p), true) = (&sp, moving) {
            self.use_place(Some(p), ftok(scrut.unwrap()), "move", env);
            self.own.move_(p);
        }
        let owning = moving || sp.is_none();
        let kw = ctok(kids[0]).clone();
        let arms = node.child_nodes("matchArm");
        let mut covered: BTreeSet<String> = BTreeSet::new();
        let mut catch_all: Option<Token> = None;
        let mut arm_results: Vec<(Type, Token)> = Vec::new();
        let closed = self.closed_variants(&st);
        // D-76: each arm runs from the state left when the arms before it did not match (a failed `when` guard keeps its
        // effects); the state after the match joins the arms that complete
        let entry = self.facts.clone();
        let mut residual = entry.clone();
        let mut ends: Vec<Facts> = vec![];
        // each arm starts from the ownership state of the paths that have not matched yet, and the state after the
        // match joins the arms that continue (a move in one arm is not a move in another; D-159 review)
        let own_entry = Self::snapshot_own(&self.own);
        let mut own_residual = Self::snapshot_own(&own_entry);
        let mut own_ends: Vec<Owners> = vec![];
        for arm in &arms {
            self.facts = residual.clone();
            // the live arm does not carry the residual's half: an inner join would report it early or drop it
            self.own = own_residual.copy();
            let e = env_new(Some(env));
            let pat = arm.find("pattern").unwrap();
            let akids = arm.kids();
            let has_when = akids.iter().any(|k| k.text_is("when"));
            let guard = if has_when {
                akids.iter().enumerate().find(|(i, c)| c.is("expr") && *i < akids.len() - 1).map(|(_, c)| c.node().unwrap())
            } else {
                None
            };
            // A fact about a name this arm binds belongs to this binding only (6.8, 9.8). Drop what an earlier arm
            // recorded under that name before this pattern and its guard establish their own facts. `outer` is what the
            // name meant on entry; residual and exit states put it back, so a later arm or the code after the match
            // does not inherit this binding's guard (a field path of the name included).
            let bound = self.names_in(pat);
            let outer = self.facts.split_off_names(&bound);
            let (cov, is_catch) = self.pattern(pat, &st, &e, moving);
            let mut arm_holders = Vec::new();
            let mut arm_obligations = Vec::new();
            if owning {
                for b in self.names_in(pat) {
                    if let Some(bb) = env_local(&e, &b) {
                        if self.is_linear(&bb.ty) {
                            let d = self.depth + 1;
                            self.own.open_obligation(&b, ftok(pat).clone(), d);
                            arm_obligations.push(b.clone());
                        }
                    }
                }
                let pt = pat.direct_tokens();
                if pt.first().map_or(false, |t| t.text == "is") && !pt.iter().any(|t| t.text == "as") && st.is_named("Failure") {
                    self.discharges.push((pt[0].line, "is-arm without as".into()));
                    self.eff.add("recovers", "proved", Some(pt[0]));
                }
            }
            if let (Some(sp), false) = (&sp, moving) {
                for b in self.names_in(pat) {
                    if env_local(&e, &b).is_some() {
                        env_update(&e, &b, |x| x.kind = "pattern-loan".into());
                        let d = self.depth + 1;
                        let l = self.own.add_loan(sp, "alias", Some(&b), d, ftok(pat).line);
                        self.own.holders.insert(&b, l);
                        arm_holders.push(b.clone());
                    }
                }
            }
            if let Some(g) = guard {
                // the arm sees the guard's true state, as an `if` body sees its condition's (6.8, 9.8); a later arm
                // sees the false state, effects included (D-76)
                let before = self.nrecords();
                let (true_f, false_f, _) = self.condition_states(g, &e);
                for r in &mut self.dc.records[before..] {
                    if r.code == "E-TYP-010" && r.message.starts_with("condition has type") {
                        r.message.replace_range(.."condition".len(), "guard");
                        r.fields.insert("primary_span".into(), Json::str("when"));
                    }
                }
                residual = if is_catch || self.irrefutable(pat) { false_f } else { residual.join(&false_f) };
                // own_outside_arm and the own_residual update read self.own, not self.facts. condition_states has
                // typed the guard, so self.own is the post-guard state master snapshotted before the arm body. Take
                // that before the body, and before self.facts becomes the true state the body sees.
                let guarded = self.own_outside_arm(&arm_holders, &arm_obligations);
                own_residual = if is_catch || self.irrefutable(pat) {
                    let mut kept = guarded;
                    Self::retain_half(&own_residual, &mut kept); // replacing the residual must keep a half already found
                    kept
                } else {
                    self.join_own(&own_residual, &guarded)
                };
                self.facts = true_f;
            } else {
                covered.extend(cov);
                if is_catch && catch_all.is_none() {
                    catch_all = Some(ftok(pat).clone());
                }
            }
            residual.restore_names(&bound, &outer);
            let body_node = akids.iter().filter_map(|k| k.node()).filter(|k| k.name() == "expr" || k.name() == "block").last();
            let body_node = match body_node {
                Some(b) => b,
                None => continue,
            };
            let mut completes = false;
            let break_at: Vec<usize> = self.break_frames.iter().map(|f| f.2.len()).collect();
            if body_node.name() == "block" {
                self.ctx.push("arm".into());
                self.arm_types.push(vec![]);
                self.exit_frames.push((self.depth, vec![]));
                self.give_seen.push(false);
                self.branch_expected.push(expected.cloned());
                self.walk_block(body_node, &e);
                self.branch_expected.pop();
                let gives = self.arm_types.pop().unwrap();
                let gave = self.exit_frames.pop().map(|f| f.1).unwrap_or_default();
                self.ctx.pop();
                self.give_seen.pop();
                let falls = !self.terminates(body_node); // D-76: an early `give` does not stop the arm falling through
                if !as_statement {
                    if falls {
                        self.typ("E-TYP-110", "match arm block in expression position does not end in `give`".into(), ftok(body_node), "give value", "no give", "match arm", "LANG-05-05");
                    }
                    arm_results.extend(gives);
                }
                completes = !gave.is_empty() || falls;
                ends.extend(gave.into_iter().map(|f| f.sealed(&bound, &outer))); // D-76: the state at every `give`, an early one included
                if falls {
                    ends.push(self.facts.sealed(&bound, &outer));
                }
            } else {
                // the arm's own marker is not the enclosing expression's: a `move` already evaluated beside this
                // match stays pending for its consumer (D-159 review)
                let outer_pending = self.pending.clone();
                self.pending = None;
                let t = self.infer(Some(body_node), &e, expected);
                self.branch_value(Some(body_node), &t, "=>"); // an expression arm is its `give` (9.8): the value is handed on (D-159)
                if self.pending.is_none() {
                    self.pending = outer_pending;
                }
                if *strip(&t) != never() {
                    ends.push(self.facts.sealed(&bound, &outer));
                    completes = true;
                }
                arm_results.push((t, ftok(body_node).clone()));
            }
            self.scrub_arm_breaks(&break_at, &bound, &outer);
            for b in &arm_holders {
                self.own.loans.retain(|l| l.borrow().holder.as_deref() != Some(b.as_str()));
                self.own.holders.remove(b);
            }
            for b in &arm_obligations {
                if let Some((tok0, _)) = self.own.forget(b) {
                    self.obl_err("E-OBL-001", format!("the failure `{b}` bound by this arm is not consumed (report, fail, move or match it)"), &tok0, &format!("{}:{}", tok0.line, tok0.col), b, "LANG-10-01");
                }
            }
            if completes {
                own_ends.push(Self::snapshot_own(&self.own));
            }
        }
        let mat = |missing: Vec<String>, arm: &str| Json::obj([("missing_variants", Json::strs(missing)), ("arm_set", Json::str(arm))]);
        let mut exhaustive = true;
        if let Some(closed) = closed {
            if let Some(ca) = catch_all {
                self.rec("E-MAT-002", format!("catch-all arm over the closed type {}: name every case instead", st.show()), "LANG-09-05", &ca, mat(vec![], &st.show()));
            } else {
                let missing: Vec<String> = closed.iter().filter(|v| !self.covers(v, &covered, &st)).cloned().collect();
                exhaustive = missing.is_empty();
                let corr = if as_statement { self.missing_arms(node, &st, &missing, &kw) } else { None };
                for (i, v) in missing.iter().enumerate() {
                    self.rec_c("E-MAT-001", format!("match over {} does not cover `{v}`", st.show()), "LANG-09-05", &kw, mat(missing.clone(), &st.show()), if i == 0 { corr.clone() } else { None });
                }
            }
        } else if known(&st) && catch_all.is_none() && !self.irrefutable_present(&arms) {
            exhaustive = false;
            self.rec("E-MAT-001", format!("match over the open type {} has no catch-all arm", st.show()), "LANG-09-05", &kw, mat(vec!["otherwise".into()], &st.show()));
        } else if !known(&st) && catch_all.is_none() && !self.irrefutable_present(&arms) {
            exhaustive = false;
        }
        if !exhaustive {
            ends.push(residual); // D-79: no arm may run; the state is the residual
            own_ends.push(Self::snapshot_own(&own_residual));
        }
        self.facts = if ends.is_empty() { entry } else { Facts::join_all(&ends) };
        if own_ends.is_empty() {
            self.own = own_entry;
        } else {
            let mut joined = Self::snapshot_own(&own_ends[0]);
            for o in &own_ends[1..] {
                joined = self.join_own(&joined, o);
            }
            self.own = joined;
        }
        Self::retain_half(&own_residual, &mut self.own); // a guard's half lives on the residual, not on the arms that continue
        self.report_half();
        if as_statement {
            return void();
        }
        self.unify_branches(&arm_results, "match arm", expected)
    }

    /// Ownership after a guard, without this arm's pattern loans, for the paths that do not take the arm.
    fn own_outside_arm(&self, holders: &[String], obligations: &[String]) -> Owners {
        let mut o = self.own.copy();
        o.loans.retain(|l| match l.borrow().holder.as_deref() {
            Some(h) => !holders.iter().any(|b| b == h),
            None => true,
        });
        for b in holders {
            o.holders.remove(b);
        }
        for b in obligations {
            o.obligations.remove(b);
        }
        o
    }

    /// `Owners::copy` clears a half-obligation. A match snapshot keeps it until `report_half`.
    fn snapshot_own(o: &Owners) -> Owners {
        let mut s = o.copy();
        s.half = o.half.clone();
        s
    }

    /// A half-obligation already found on `previous` stays on `new`. `Owners::join` keeps only its own pair.
    fn retain_half(previous: &Owners, new: &mut Owners) {
        let mut seen: BTreeSet<String> = new.half.iter().map(|(p, _)| p.clone()).collect();
        for (p, tok) in &previous.half {
            if !seen.contains(p) && !new.obligations.contains(p) {
                seen.insert(p.clone());
                new.half.push((p.clone(), tok.clone()));
            }
        }
    }

    /// Join two arm states. A half-obligation from either side stays one: `Owners::join` keeps only the latest pair.
    fn join_own(&self, left: &Owners, right: &Owners) -> Owners {
        let mut joined = left.join(right);
        Self::retain_half(left, &mut joined);
        Self::retain_half(right, &mut joined);
        joined
    }

    /// Empty arms for missing payload-free cases of a statement match, inserted before its closing brace.
    fn missing_arms(&self, node: N<'a>, st: &Type, missing: &[String], kw: &Token) -> Option<Json> {
        let d = self.decl_of(st);
        for v in missing {
            let payload_free = match &d {
                Some(d) if d.kind == "enum" => d.variants.get(v).map_or(false, |p| p.is_empty()),
                _ => v == "null" || v == "true" || v == "false",
            };
            if !payload_free {
                return None;
            }
        }
        let close = node.tokens().into_iter().filter(|t| t.kind != Kind::Nl).last()?.clone();
        if close.text != "}" || close.line == kw.line {
            return None;
        }
        let ind = " ".repeat(close.col - 1);
        let text: String = missing.iter().map(|v| format!("{ind}  {v} => {{\n{ind}  }}\n")).collect();
        let f = self.dc.file.clone();
        Some(crate::decls::correction("add the missing arms", vec![crate::decls::edit(&f, close.line, 1, 1, &text)]))
    }

    /// Whether the arms cover case `v` of the scrutinee type: an arm covers it whole, or the arms' nested patterns cover
    /// every case of its payload together (D-79).
    pub fn covers(&self, v: &str, covered: &BTreeSet<String>, st: &Type) -> bool {
        if covered.contains(v) {
            return true;
        }
        let inner: Option<Type> = if st.is_optional() && v == "present" {
            st.opt_inner().cloned()
        } else {
            match st {
                Type::Named(n) if n.name == "Outcome" && !n.args.is_empty() && v == "success" => Some(n.args[0].clone()),
                _ => match self.decl_of(st) {
                    Some(d) if d.kind == "enum" => match d.variants.get(v) {
                        Some(p) if p.len() == 1 => {
                            let mm = self.recv_subst(st, &d);
                            p.iter().next().map(|(_, t)| self.subst(t, &mm))
                        }
                        _ => None,
                    },
                    _ => None,
                },
            }
        };
        let Some(inner) = inner else { return false };
        let prefix = format!("{v}/");
        let sub: BTreeSet<String> = covered.iter().filter_map(|c| c.strip_prefix(&prefix).map(|s| s.to_string())).collect();
        if sub.is_empty() {
            return false;
        }
        let inner = strip(&inner).clone();
        match self.closed_variants(&inner) {
            Some(cases) => cases.iter().all(|x| self.covers(x, &sub, &inner)),
            None => false,
        }
    }

    pub fn irrefutable_present(&self, arms: &[N<'a>]) -> bool {
        arms.iter().any(|arm| self.irrefutable(arm.find("pattern").unwrap()) && !arm.kids().iter().any(|k| k.text_is("when")))
    }

    pub fn irrefutable(&self, pat: N<'a>) -> bool {
        let ts = pat.direct_tokens();
        if ts.is_empty() {
            return false;
        }
        let w0 = ts[0].text.as_str();
        if w0 == "_" || w0 == "otherwise" || (ts.len() == 1 && ts[0].kind == Kind::Lower) {
            return true;
        }
        if w0 == "(" || w0 == "{" {
            return pat.child_nodes("pattern").into_iter().all(|p| self.irrefutable(p));
        }
        false
    }

    pub fn closed_variants(&self, st: &Type) -> Option<Vec<String>> {
        if st.is_optional() {
            return Some(vec!["present".into(), "null".into()]);
        }
        let st = match st {
            Type::Refined { base, .. } => Type::Named(base.clone()),
            other => other.clone(),
        };
        if let Type::Named(n) = &st {
            if n.name == "Outcome" {
                return Some(vec!["success".into(), "failure".into()]);
            }
            if n.name == "Bool" {
                return Some(vec!["true".into(), "false".into()]);
            }
            if let Some(d) = self.decl_of(&st) {
                if d.kind == "enum" {
                    return Some(d.variants.keys().cloned().collect());
                }
            }
        }
        None
    }

    /// Bind the pattern's names in env against the scrutinee type; (covered cases, is catch-all).
    pub fn pattern(&mut self, pat: N<'a>, st: &Type, env: &Env, moving: bool) -> (BTreeSet<String>, bool) {
        for b in self.names_in(pat) {
            self.own.reinit(&b);
            self.own.close_obligation(&b, 0, "rebound");
        }
        let mut ts: Vec<&Token> = pat.direct_tokens();
        let words: Vec<String> = ts.iter().map(|t| t.text.clone()).collect();
        let subs = pat.child_nodes("pattern");
        if ts.is_empty() {
            if let Some(C::N(k0)) = pat.kid(0) {
                if k0.name() == "literal" {
                    ts = k0.tokens();
                }
            }
        }
        if ts.is_empty() {
            return (BTreeSet::new(), false);
        }
        let head = ts[0];
        let one = |s: &str| -> BTreeSet<String> { [s.to_string()].into_iter().collect() };
        if head.text == "_" {
            return (BTreeSet::new(), true);
        }
        if head.text == "otherwise" {
            if ts.len() >= 3 {
                env_set(env, &ts[2].text, st.clone(), false, "pattern");
            }
            return (BTreeSet::new(), true);
        }
        if ts.len() == 1 && head.kind == Kind::Lower {
            env_set(env, &head.text, st.clone(), false, "pattern");
            return (BTreeSet::new(), true);
        }
        if head.text == "true" || head.text == "false" {
            if known(st) && base_name(st) != Some("Bool") {
                self.typ("E-TYP-040", format!("literal pattern Bool against {}", st.show()), head, &st.show(), "Bool", "pattern", "LANG-06-01");
            }
            return (one(&head.text), false);
        }
        if pat.kid(0).map_or(false, |k| k.is("literal")) {
            let lt = self.stamp_literal(literal_type(pat.tokens()[0]));
            if known(st) {
                let a = if is_strlit(&lt) { string_view() } else { lt };
                let (ok, _) = assignable(&a, st);
                if ok == Some(false) && *st != string() {
                    self.typ("E-TYP-040", format!("literal pattern {} against {}", a.show(), st.show()), head, &st.show(), &a.show(), "pattern", "LANG-06-01");
                }
            }
            if base_name(st) == Some("Bool") {
                return (one(&head.text), false);
            }
            return (BTreeSet::new(), false);
        }
        if head.text == "null" {
            if known(st) && !st.is_optional() {
                self.typ("E-TYP-040", format!("`null` pattern against {}, which is not optional", st.show()), head, "T?", &st.show(), "null", "LANG-06-08");
            }
            return (one("null"), false);
        }
        if head.text == "present" {
            if known(st) && !st.is_optional() {
                self.typ("E-TYP-040", format!("`present` pattern against {}, which is not optional", st.show()), head, "T?", &st.show(), "present", "LANG-06-08");
            }
            let inner = st.opt_inner().cloned().unwrap_or_else(|| unknown(""));
            let Some(s0) = subs.first() else { return (one("present"), false) };
            // D-79: `present(0)` covers one present value, not every one; an irrefutable inner pattern covers the case, and
            // refutable ones cover it only together (`present(true)` and `present(false)` over Bool?)
            let (icov, icatch) = self.pattern(*s0, &inner, env, moving);
            if icatch || self.irrefutable(*s0) {
                return (one("present"), false);
            }
            return (icov.iter().map(|c| format!("present/{c}")).collect(), false);
        }
        if head.text == "success" || head.text == "failure" {
            let mut inner = unknown("");
            match st {
                Type::Named(n) if n.name == "Outcome" && !n.args.is_empty() => {
                    inner = if head.text == "success" { n.args[0].clone() } else { named("Failure") };
                }
                _ => {
                    if known(st) {
                        self.typ("E-TYP-040", format!("`{}` pattern against {}, which is not an Outcome", head.text, st.show()), head, "Outcome[T]", &st.show(), &head.text, "LANG-06-01");
                    }
                }
            }
            let Some(s0) = subs.first() else { return (one(&head.text), false) };
            let (icov, icatch) = self.pattern(*s0, &inner, env, moving); // D-79, as for `present`
            if icatch || self.irrefutable(*s0) {
                return (one(&head.text), false);
            }
            return (icov.iter().map(|c| format!("{}/{c}", head.text)).collect(), false);
        }
        if head.text == "is" {
            if known(st) && base_name(st) != Some("Failure") {
                self.typ("E-TYP-040", format!("`is` pattern against {}, which is not a Failure", st.show()), head, "Failure", &st.show(), "is", "LANG-06-01");
            }
            let ename = ts[1].text.clone();
            let ed = self.dc.table.get(&ename).cloned();
            let payload: BTreeMap<String, Type> = match &ed {
                Some(ed) if ed.kind == "error" => ed.payload.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                _ => {
                    let mut cands: Vec<String> = self.dc.table.decls.iter().filter(|(_, v)| v.kind == "error").map(|(k, _)| k.clone()).collect();
                    cands.sort();
                    self.rec("E-NAM-001", format!("`{ename}` is not a declared error"), "LANG-04-05", ts[1], Self::nam(&ename, cands));
                    ed.as_ref().map(|ed| ed.payload.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
                }
            };
            let order: Vec<(String, Type)> = match &ed {
                Some(ed) => ed.payload.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                None => vec![],
            };
            let _ = payload;
            self.pattern_fields(pat, &order, env, moving, ts[1], &ename, false);
            if let Some(i) = words.iter().position(|w| w == "as") {
                env_set(env, &ts[i + 1].text, named("Failure"), false, "pattern");
            }
            return (BTreeSet::new(), false);
        }
        if head.text == "(" {
            let items: Vec<Type> = match st {
                Type::Tuple(items) => items.clone(),
                _ => vec![unknown(""); subs.len()],
            };
            if let Type::Tuple(it) = st {
                if it.len() != subs.len() {
                    self.typ("E-TYP-040", format!("tuple pattern of {} against {}", subs.len(), st.show()), head, &st.show(), &format!("{} elements", subs.len()), "pattern", "LANG-06-01");
                }
            } else if known(st) {
                self.typ("E-TYP-040", format!("tuple pattern against {}", st.show()), head, "a tuple", &st.show(), "pattern", "LANG-06-01");
            }
            for (p, t) in subs.iter().zip(items.iter()) {
                self.pattern(*p, t, env, moving);
            }
            return (BTreeSet::new(), subs.iter().all(|p| self.irrefutable(*p)));
        }
        if head.text == "{" {
            let fields: BTreeMap<String, Type> = match st {
                Type::Record(fs) => fs.iter().cloned().collect(),
                _ => BTreeMap::new(),
            };
            if known(st) && !matches!(st, Type::Record(_)) {
                self.typ("E-TYP-040", format!("record pattern against {}", st.show()), head, "an anonymous record", &st.show(), "pattern", "LANG-06-01");
            }
            let keys: Vec<&Token> = ts.iter().filter(|t| t.kind == Kind::Lower).copied().collect();
            for (k, p) in keys.iter().zip(subs.iter()) {
                let t = fields.get(&k.text).cloned().unwrap_or_else(|| unknown(""));
                self.pattern(*p, &t, env, moving);
            }
            return (BTreeSet::new(), subs.iter().all(|p| self.irrefutable(*p)));
        }
        if head.kind == Kind::Upper {
            let d = self.decl_of(st);
            if let Some(d) = &d {
                if d.kind == "enum" {
                    let payload = match d.variants.get(&head.text) {
                        Some(p) => p.clone(),
                        None => {
                            self.rec("E-NAM-001", format!("{} has no variant `{}`", d.name, head.text), "LANG-09-05", head, Self::nam(&head.text, d.variants.sorted_keys()));
                            return (BTreeSet::new(), false);
                        }
                    };
                    let mm = self.recv_subst(st, d);
                    let fields: Vec<(String, Type)> = payload.iter().map(|(k, v)| (k.clone(), self.subst(v, &mm))).collect();
                    let covs = self.pattern_fields(pat, &fields, env, moving, head, &format!("{}.{}", d.name, head.text), false);
                    // D-79: a variant is covered by irrefutable payload patterns, or, for a one-field payload, by refutable
                    // ones that cover the field's cases together (`Flag(true)` and `Flag(false)`)
                    if self.field_patterns(pat).iter().all(|fp| self.irrefutable(*fp)) {
                        return (one(&head.text), false);
                    }
                    if payload.len() == 1 && covs.len() == 1 {
                        let (cov, catch) = &covs[0];
                        return (if *catch { one(&head.text) } else { cov.iter().map(|c| format!("{}/{c}", head.text)).collect() }, false);
                    }
                    return (BTreeSet::new(), false);
                }
                if d.kind == "class" {
                    if d.name != head.text {
                        self.typ("E-TYP-040", format!("pattern {} against {}", head.text, st.show()), head, &st.show(), &head.text, "pattern", "LANG-06-01");
                        return (BTreeSet::new(), false);
                    }
                    let mat = |arm: &str| Json::obj([("missing_variants", Json::Arr(vec![])), ("arm_set", Json::str(arm))]);
                    if !moving {
                        self.rec("E-MAT-002", format!("destructuring class {} needs `match move` / `let … = move` (9.8)", d.name), "LANG-09-06", head, mat(&d.name));
                    }
                    if d.has_cleanup {
                        self.rec("E-MAT-002", format!("class {} has a cleanup and cannot be destructured", d.name), "LANG-09-06", head, mat(&d.name));
                    }
                    let mm = self.recv_subst(st, d);
                    let fields: Vec<(String, Type)> = d.fields.iter().map(|(k, v)| (k.clone(), self.subst(&v.0, &mm))).collect();
                    self.pattern_fields(pat, &fields, env, moving, head, &d.name.clone(), true);
                    // D-79: a class destructure always matches only when every field pattern is irrefutable
                    let always = self.field_patterns(pat).iter().all(|fp| self.irrefutable(*fp));
                    return (BTreeSet::new(), always);
                }
            }
            if known(st) {
                self.typ("E-TYP-040", format!("pattern {} against {}", head.text, st.show()), head, &st.show(), &head.text, "pattern", "LANG-06-01");
            } else {
                for p in pat.find_all("pattern") {
                    if !p.same(pat) {
                        for b in self.names_in(p) {
                            env_set(env, &b, unknown("pattern binding"), false, "pattern");
                        }
                    }
                }
            }
            return (BTreeSet::new(), false);
        }
        (BTreeSet::new(), false)
    }

    /// The pattern of each field of a variant or class pattern.
    pub fn field_patterns(&self, pat: N<'a>) -> Vec<N<'a>> {
        match pat.find("patternFields") {
            Some(pfs) => pfs.child_nodes("patternField").into_iter().filter_map(|pf| pf.find("pattern")).collect(),
            None => vec![],
        }
    }

    /// Bind each field pattern; return each typed field pattern's (covered cases, is catch-all) in order (D-79).
    #[allow(clippy::too_many_arguments)]
    pub fn pattern_fields(&mut self, pat: N<'a>, fields: &[(String, Type)], env: &Env, moving: bool, tok: &Token, label: &str, require_all: bool) -> Vec<(BTreeSet<String>, bool)> {
        let mut covs: Vec<(BTreeSet<String>, bool)> = vec![];
        let pfs = match pat.find("patternFields") {
            Some(pf) => pf.find_all("patternField"),
            None => vec![],
        };
        let names_: Vec<String> = fields.iter().map(|(k, _)| k.clone()).collect();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for (i, pf) in pfs.iter().enumerate() {
            let ts = pf.direct_tokens();
            let sub = pf.find("pattern");
            let fname;
            if !ts.is_empty() && ts[0].kind == Kind::Lower && ts.len() > 1 && ts[1].text == ":" {
                fname = ts[0].text.clone();
                if !names_.contains(&fname) {
                    self.rec("E-NAM-001", format!("{label} has no field `{fname}`"), "LANG-09-05", ts[0], Self::nam(&fname, names_.clone()));
                    continue;
                }
            } else {
                if require_all {
                    self.typ("E-TYP-130", format!("class destructure of {label} names every field"), ftok(*pf), "field: pattern", "positional", label, "LANG-09-06");
                }
                if i >= names_.len() {
                    let s = if names_.len() != 1 { "s" } else { "" };
                    self.typ("E-TYP-130", format!("{label} has {} payload field{s}", names_.len()), ftok(*pf), &names_.len().to_string(), &(i + 1).to_string(), label, "LANG-09-05");
                    continue;
                }
                fname = names_[i].clone();
            }
            seen.insert(fname.clone());
            let ft = fields.iter().find(|(k, _)| *k == fname).map(|(_, t)| t.clone()).unwrap();
            if let Some(sub) = sub {
                covs.push(self.pattern(sub, &ft, env, moving));
            }
        }
        if require_all {
            let missing: Vec<String> = names_.iter().filter(|f| !seen.contains(*f)).cloned().collect();
            if !missing.is_empty() {
                let s = if missing.len() > 1 { "s" } else { "" };
                self.typ("E-TYP-050", format!("destructure of {label} lacks field{s} {}", missing.join(", ")), tok, &missing.join(", "), "absent", label, "LANG-09-06");
            }
        }
        covs
    }

    pub fn names_in(&self, pat: N<'a>) -> Vec<String> {
        let mut out = Vec::new();
        let ts = pat.direct_tokens();
        let words: Vec<&str> = ts.iter().map(|t| t.text.as_str()).collect();
        if let Some(i) = words.iter().position(|w| *w == "as") {
            out.push(ts[i + 1].text.clone());
        }
        if words.first() == Some(&"otherwise") && ts.len() >= 3 {
            out.push(ts[2].text.clone());
        }
        if ts.len() == 1 && ts[0].kind == Kind::Lower && ts[0].text != "_" {
            out.push(ts[0].text.clone());
        }
        for sub in pat.find_all("pattern") {
            if !sub.same(pat) {
                out.extend(self.names_in(sub));
            }
        }
        out
    }

    pub fn if_expr(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let mut results: Vec<(Type, Token)> = Vec::new();
        let entry = self.facts.clone();
        let ends = self.if_expr_paths(node, env, expected, &mut results);
        // D-76: each branch runs from its condition's state, and the expression's value comes from one of them; the state
        // after is the join of the branches that complete, never the last branch walked
        self.facts = if ends.is_empty() { entry } else { Facts::join_all(&ends) };
        self.unify_branches(&results, "if-expression branch", expected)
    }

    /// Walk an if-expression's branches, each from the state its condition gives; return the end states of those that
    /// complete (D-76).
    fn if_expr_paths(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>, results: &mut Vec<(Type, Token)>) -> Vec<Facts> {
        let mut ends: Vec<Facts> = vec![];
        let mut nxt = self.facts.clone(); // the state the next block starts from
        let mut other = self.facts.clone(); // the state when every condition so far was false
        let mut has_else = false;
        for c in node.kids() {
            if c.text_is("else") {
                has_else = true;
                continue;
            }
            match c.name() {
                Some("expr") => {
                    let cond = c.node().unwrap();
                    self.facts = other.clone();
                    let (yes, no, _) = self.condition_states(cond, env);
                    nxt = yes;
                    other = no;
                }
                Some("block") => {
                    let b = c.node().unwrap();
                    self.facts = nxt.clone();
                    self.ctx.push("ifexpr".into());
                    self.arm_types.push(vec![]);
                    self.exit_frames.push((self.depth, vec![]));
                    self.branch_expected.push(expected.cloned());
                    self.walk_block(b, env);
                    self.branch_expected.pop();
                    let gives = self.arm_types.pop().unwrap();
                    let gave = self.exit_frames.pop().map(|f| f.1).unwrap_or_default();
                    self.ctx.pop();
                    let falls = !self.terminates(b); // D-76: an early `give` does not stop the branch falling through
                    if falls {
                        self.typ("E-TYP-110", "if-expression branch does not end in `give`".into(), ftok(b), "give value", "no give", "if expression", "LANG-05-05");
                    }
                    results.extend(gives);
                    ends.extend(gave); // D-76: the state at every `give`, an early one included
                    if falls {
                        ends.push(self.facts.clone());
                    }
                    nxt = other.clone();
                }
                Some("ifExpr") => {
                    let n = c.node().unwrap();
                    self.facts = other.clone();
                    let mut sub: Vec<(Type, Token)> = Vec::new();
                    ends.extend(self.if_expr_paths(n, env, expected, &mut sub));
                    let t = self.unify_branches(&sub, "if-expression branch", expected);
                    results.push((t, ftok(n).clone()));
                }
                _ => {}
            }
        }
        if !has_else {
            ends.push(other);
        }
        ends
    }

    pub fn condition(&mut self, expr: N<'a>, env: &Env) {
        let t0 = self.infer(Some(expr), env, None);
        let t = strip(&t0).clone();
        if !known(&t) {
            return;
        }
        if base_name(&t) != Some("Bool") || t.is_optional() {
            let tk = ftok(expr);
            let span = expr.tokens().iter().take(6).map(|x| x.text.clone()).collect::<Vec<_>>().join(" ");
            self.typ("E-TYP-010", format!("condition has type {}, not Bool", t.show()), tk, "Bool", &t.show(), &span, "LANG-05-03");
        }
    }
}
