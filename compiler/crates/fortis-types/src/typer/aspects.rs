//! Aspects (11.8), observation (13), pool launches (12.5), unsupported shapes (C.2) and defer (12.1).

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use fortis_diag::Json;
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{C, N};

use super::*;
use crate::decls::{tok_of, Decl, DeclChecker, DeclRef, Sig};
use crate::effects::Summary;
use crate::intervals::Facts;
use crate::owners::{root_of, Owners};
use crate::types::*;

pub type AspectArg<'a> = (Option<String>, Option<N<'a>>, bool, Env);

impl<'a> Typer<'a> {
    pub fn asp_err(&mut self, msg: String, tok: &Token, expected: &str, actual: &str, span: &str) {
        self.typ("E-TYP-150", msg, tok, expected, actual, span, "LANG-11-09");
    }

    pub fn aspect_args(&self, al: Option<N<'a>>, env: &Env) -> Vec<AspectArg<'a>> {
        let mut out = Vec::new();
        let al = match al {
            Some(a) => a,
            None => return out,
        };
        for a in al.child_nodes("arg") {
            let ats = a.direct_tokens();
            let e = a.find("expr");
            if ats.first().map_or(false, |t| t.text == "...") {
                out.push((Some(ats[1].text.clone()), None, true, env.clone()));
            } else if ats.len() > 1 && ats[1].text == ":" {
                out.push((Some(ats[0].text.clone()), e, false, env.clone()));
            } else {
                out.push((None, e, false, env.clone()));
            }
        }
        out
    }

    pub fn resolve_aspect(&mut self, d: &DeclRef<'a>, args: Vec<AspectArg<'a>>, tok: &Token, seen: &BTreeSet<String>) -> (Option<DeclRef<'a>>, BTreeMap<String, (Option<N<'a>>, Env)>) {
        let a = d.aspect.as_ref().unwrap();
        let params = a.params.clone();
        let names_: Vec<String> = params.iter().map(|p| p.0.clone()).collect();
        let mut bound: BTreeMap<String, (Option<N<'a>>, Env)> = BTreeMap::new();
        let mut extra: Vec<AspectArg<'a>> = Vec::new();
        let mut positional = false;
        for (lbl, e, rest, env) in args {
            if rest {
                self.asp_err(format!("`...{}` forwards arguments only inside a specialisation `aspect x(...) = base(...)`", lbl.clone().unwrap_or_default()), tok, "named arguments", "...rest", &d.name);
                continue;
            }
            match lbl {
                None => {
                    if positional || params.is_empty() {
                        let t = e.map(ftok).cloned().unwrap_or_else(|| tok.clone());
                        self.asp_err(format!("@{}: only the first declared parameter may be passed positionally", d.name), &t, &format!("a named argument of {:?}", names_), "positional", &d.name);
                        continue;
                    }
                    bound.insert(names_[0].clone(), (e, env));
                    positional = true;
                }
                Some(l) if names_.contains(&l) => {
                    bound.insert(l, (e, env));
                }
                Some(l) if a.rest => extra.push((Some(l), e, false, env)),
                Some(l) => {
                    let t = e.map(ftok).cloned().unwrap_or_else(|| tok.clone());
                    self.asp_err(format!("@{} has no parameter `{l}`", d.name), &t, &format!("one of {:?}", names_), &l, &d.name);
                }
            }
        }
        for (pn, _, has_default) in &params {
            if !bound.contains_key(pn) && !has_default {
                self.asp_err(format!("@{}: argument `{pn}` is required", d.name), tok, pn, "absent", &d.name);
            }
        }
        let level = env_new(None);
        for (pn, pt, _) in &params {
            env_set(&level, pn, pt.clone(), false, "let");
        }
        let (bname, bal, btok) = match &a.base {
            None => {
                if let Some(x) = extra.first() {
                    let l = x.0.clone().unwrap_or_default();
                    self.asp_err(format!("@{} has no parameter `{l}`", d.name), tok, &format!("one of {:?}", names_), &l, &d.name);
                }
                return (Some(d.clone()), bound);
            }
            Some(b) => b.clone(),
        };
        let bd = a.base_decl.borrow().clone().or_else(|| self.dc.table.get(&bname).cloned());
        let bd = match bd {
            Some(bd) if bd.kind == "aspect" && !seen.contains(&bname) => bd,
            _ => {
                self.asp_err(format!("aspect {} specialises `{bname}`, which is not a declared aspect", d.name), &btok, "an aspect", &bname, &d.name);
                return (None, BTreeMap::new());
            }
        };
        let mut fargs = Vec::new();
        for (lbl, e, rest, _) in self.aspect_args(bal, &level) {
            if rest {
                fargs.extend(extra.iter().cloned());
            } else {
                fargs.push((lbl, e, false, level.clone()));
            }
        }
        self.level_values.push(bound);
        let mut seen2 = seen.clone();
        seen2.insert(d.name.clone());
        let r = self.resolve_aspect(&bd, fargs, tok, &seen2);
        self.level_values.pop();
        r
    }

    pub fn type_aspect_arg(&mut self, pn: &str, pt: &Type, e: Option<N<'a>>, env: &Env, aspect: &str) {
        let e = match e {
            Some(e) => e,
            None => return,
        };
        let ts = e.tokens();
        if let Type::Named(n) = strip(pt) {
            if n.name == "ObservationSchema" {
                let d = if ts.len() == 1 && ts[0].kind == Kind::Upper { self.dc.table.get(&ts[0].text).cloned() } else { None };
                if let Some(d) = &d {
                    if d.kind == "unknown" {
                        self.schema_unknown = true;
                        return;
                    }
                }
                match d {
                    Some(d) if d.kind == "observation" => self.schema = Some(d),
                    _ => {
                        let text = ts.iter().map(|t| t.text.clone()).collect::<Vec<_>>().join(" ");
                        self.asp_err(format!("@{aspect}: `{pn}` names an observation schema; `{text}` is not one"), ts[0], "an observation declaration", &ts[0].text, pn);
                    }
                }
                return;
            }
        }
        if ts.len() == 1 && ts[0].kind == Kind::Lower && env_lookup(env, &ts[0].text).is_some() {
            return;
        }
        if ts.len() == 1 && ts[0].kind == Kind::Upper && self.dc.table.get(&ts[0].text).is_some() {
            return;
        }
        if let Some(call) = self.nonconstant_call(e) {
            self.asp_err(
                format!("@{aspect}: the argument `{pn}` makes a call (`{}`); aspect arguments are compile-time constants, evaluated by no code (11.8)", call.text),
                &call,
                "a compile-time constant",
                "a call",
                pn,
            );
            return;
        }
        self.last_closure = None;
        let t = self.infer(Some(e), env, Some(pt));
        if self.last_closure.as_ref().map_or(false, |c| !c.captures.is_empty()) {
            self.asp_err(format!("@{aspect}: the `fn` argument `{pn}` captures; aspect arguments are compile-time constants"), ts[0], "a capture-free fn literal", "a closure with captures", pn);
        }
        if let Type::Named(s) = strip(pt) {
            if known(&t) && s.args.is_empty() && s.name != "VariantMap" && !matches!(pt, Type::Optional(_) | Type::Fn { .. } | Type::Dynamic(_)) {
                self.expect(&t, pt, ts[0], &format!("aspect argument `{pn}`"));
            }
        }
    }

    pub fn hook(&mut self, blk: N<'a>, env: &Env, ret: Type, in_try: bool, what: &str, owned: Option<&str>, tok: Option<&Token>) -> Summary {
        let saved = (std::mem::take(&mut self.own), std::mem::take(&mut self.facts), self.ret_type.clone(), self.in_try, std::mem::take(&mut self.ctx), self.depth, std::mem::take(&mut self.eff));
        self.ret_type = Some(ret.clone());
        self.in_try = in_try;
        self.ctx = vec!["function".into(), "hook".into()];
        self.depth = 0;
        crate::journal::enter(what);
        if let Some(o) = owned {
            let t = tok.cloned().unwrap_or_else(|| ftok(blk).clone());
            self.own.open_obligation(o, t, 0);
        }
        self.walk_block(blk, env);
        if ret != void() && !self.terminates(blk) {
            self.typ("E-TYP-110", format!("the {what} must end every path in `return` or `fail`"), ftok(blk), &ret.show(), "fall-through", what, "LANG-11-09");
        } else if owned.is_some() && !self.terminates(blk) {
            self.report_open(0, &format!("at the end of the {what}"));
        }
        crate::journal::leave();
        let out = std::mem::take(&mut self.eff);
        self.own = saved.0;
        self.facts = saved.1;
        self.ret_type = saved.2;
        self.in_try = saved.3;
        self.ctx = saved.4;
        self.depth = saved.5;
        self.eff = saved.6;
        self.eff.imports(&out, None, &[]);
        for (k, v) in &out.sites {
            self.eff.sites.entry(k.clone()).or_insert_with(|| v.clone());
        }
        self.eff.footprint.extend(out.footprint.iter().cloned());
        out
    }

    pub fn apply_aspects(&mut self, sig: &Sig<'a>, head_tok: &Token, label: &str) {
        let uses = self.current_aspects.clone();
        if uses.is_empty() {
            return;
        }
        let target_try = self.expanded_mods(sig).contains("try");
        let mut chain: Vec<(DeclRef<'a>, BTreeMap<String, (Option<N<'a>>, Env)>, Token)> = Vec::new();
        for u in uses {
            let uts = u.direct_tokens();
            let nm = uts[1].clone();
            let d = match self.dc.table.get(&nm.text) {
                Some(d) if d.kind == "aspect" => d.clone(),
                _ => {
                    self.asp_err(format!("`@{}` is not a declared aspect", nm.text), &nm, "an aspect", &nm.text, &nm.text);
                    continue;
                }
            };
            let args = self.aspect_args(u.find("argList"), &env_new(None));
            let (base, bound) = self.resolve_aspect(&d, args, &nm, &BTreeSet::new());
            let base = match base {
                Some(b) => b,
                None => continue,
            };
            let a = base.aspect.as_ref().unwrap();
            for (pn, pt, _) in a.params.clone() {
                if let Some((e, env)) = bound.get(&pn).cloned() {
                    self.type_aspect_arg(&pn, &pt, e, &env, &nm.text);
                }
            }
            for (n2, ty, mode) in a.needs.clone() {
                let matches: Vec<(String, String)> = sig.params.iter().filter(|(_, pt, _)| base_name(strip(pt)) == base_name(&ty)).map(|(pn, _, pm)| (pn.clone(), pm.clone())).collect();
                if matches.is_empty() {
                    self.asp_err(format!("@{} needs a parameter of type {} ({n2}); {label} has none", nm.text, ty.show()), &nm, &ty.show(), "no such parameter", &n2);
                } else if matches.len() > 1 {
                    let names = matches.iter().map(|m| m.0.clone()).collect::<Vec<_>>();
                    self.asp_err(format!("@{} needs exactly one parameter of type {}; {label} has {}", nm.text, ty.show(), names.join(" and ")), &nm, &format!("one {} parameter", ty.show()), &names.join(", "), &n2);
                } else if mode == "editable" && matches[0].1.trim_start_matches('.') != "editable" {
                    let actual = if matches[0].1.is_empty() { "read loan".to_string() } else { matches[0].1.clone() };
                    self.asp_err(format!("@{} needs `{n2}: editable {}`; parameter `{}` is not editable", nm.text, ty.show(), matches[0].0), &nm, &format!("editable {}", ty.show()), &actual, &n2);
                }
            }
            for c in a.needs_contracts.clone() {
                if !sig.contracts.contains(&c) {
                    self.asp_err(format!("@{} needs contract {c} on its target; {label} does not carry it", nm.text), &nm, &c, "absent", &c);
                }
            }
            chain.push((base.clone(), bound, nm));
        }
        if chain.is_empty() {
            return;
        }
        let hook_env = |me: &Self, base: &Decl<'a>| -> Env {
            let e = me.params_env(sig, &env_new(None));
            let a = base.aspect.as_ref().unwrap();
            let mut m = BTreeMap::new();
            m.insert("R".to_string(), sig.ret.clone());
            for (pn, pt, _) in &a.params {
                env_set(&e, pn, me.subst(pt, &m), false, "let");
            }
            for (n2, ty, _) in &a.needs {
                if let Some((_, pt, pm)) = sig.params.iter().find(|(_, pt, _)| base_name(strip(pt)) == base_name(ty)) {
                    let k = match pm.trim_start_matches('.') {
                        "editable" => "editable",
                        "owned" => "owned",
                        _ => "param",
                    };
                    env_set(&e, n2, pt.clone(), false, k);
                }
            }
            e
        };
        let mut handler_above = false;
        let (mut before_names, mut after_names, mut handler_names) = (vec![], vec![], vec![]);
        for (base, _, nm) in &chain {
            let a = base.aspect.as_ref().unwrap();
            if a.on_failure.is_some() {
                handler_above = true;
            }
            for blk in a.before.clone() {
                let e = hook_env(self, base);
                self.hook(blk, &e, void(), target_try || handler_above, &format!("before hook of @{}", nm.text), None, None);
                before_names.push(nm.text.clone());
            }
        }
        let handler_at_or_above: Vec<bool> = (0..chain.len()).map(|i| chain[..=i].iter().any(|c| c.0.aspect.as_ref().unwrap().on_failure.is_some())).collect();
        for i in (0..chain.len()).rev() {
            let (base, _, nm) = chain[i].clone();
            let a = base.aspect.as_ref().unwrap();
            if let Some((rn, _, blk)) = a.after.clone() {
                let e = hook_env(self, &base);
                env_set(&e, &rn, sig.ret.clone(), false, "param");
                if let Some(blk) = blk {
                    let out = self.hook(blk, &e, void(), target_try || handler_at_or_above[i], &format!("after hook of @{}", nm.text), None, None);
                    if out.has("fail") && self.is_linear(&sig.ret) {
                        let site = out.sites.get("fail").cloned().unwrap_or_else(|| nm.clone());
                        self.obl_err(
                            "E-OBL-008",
                            format!("the after hook of @{} may fail, but the result type {} is linear: a failing `after` would leave the result with no consumer (11.8)", nm.text, sig.ret.show()),
                            &site,
                            &format!("{}:{}", nm.line, nm.col),
                            &rn,
                            "LANG-11-09",
                        );
                    }
                }
                after_names.push(nm.text.clone());
            }
        }
        for i in (0..chain.len()).rev() {
            let (base, _, nm) = chain[i].clone();
            let a = base.aspect.as_ref().unwrap();
            if let Some((fname, _, blk, ftok_)) = a.on_failure.clone() {
                let e = hook_env(self, &base);
                env_set(&e, &fname, named("Failure"), false, "owned");
                if let Some(blk) = blk {
                    self.hook(blk, &e, sig.ret.clone(), true, &format!("failure handler of @{}", nm.text), Some(&fname), Some(&ftok_));
                }
                handler_names.push(nm.text.clone());
            }
        }
        if !target_try {
            self.eff.remove("fail");
        }
        self.elaborations.push(format!("{label}: before {:?} -> body -> after {:?}; on failure {:?} (innermost first)", before_names, after_names, handler_names));
        let _ = head_tok;
    }

    pub fn check_aspect_decl(&mut self, d: &DeclRef<'a>) {
        let a = d.aspect.as_ref().unwrap();
        if a.rest && a.base.is_none() {
            self.asp_err(format!("aspect {} declares `...rest` but is not a specialisation", d.name), &tok_of(d), "aspect x(...) = base(...)", "a full declaration with ...rest", &d.name);
        }
        if let Some((bname, _, btok)) = &a.base {
            let bd = a.base_decl.borrow().clone().or_else(|| self.dc.table.get(bname).cloned());
            if !bd.map_or(false, |b| b.kind == "aspect") {
                self.asp_err(format!("aspect {} specialises `{bname}`, which is not a declared aspect", d.name), btok, "an aspect", bname, &d.name);
            }
        }
    }

    // ---- observation (13) -------------------------------------------------------------
    pub fn obs_err(&mut self, code: &str, msg: String, tok: &Token, fld: &str, exit_path: &str, rule: &str) {
        self.rec(code, msg, rule, tok, Json::obj([("frame_field", Json::str(fld)), ("exit_path", Json::str(exit_path))]));
    }

    pub fn pure_snapshot(&mut self, e: Option<N<'a>>, env: &Env, expected: Option<&Type>, what: &str, tok: &Token, fld: &str) -> Type {
        let before = self.eff.effect_set();
        let t = self.infer(e, env, expected);
        let new: BTreeSet<String> = self.eff.effect_set().difference(&before).cloned().collect();
        let bad: Vec<String> = new.into_iter().filter(|x| ["io", "mutate", "nondet", "suspend", "fail", "block"].contains(&x.as_str())).collect();
        if !bad.is_empty() {
            self.obs_err("E-OBS-012", format!("the {what} expression has the {} effect; it must be pure, infallible and non-suspending (13)", bad.join(", ")), tok, fld, "statement", "LANG-13-01");
        }
        t
    }

    pub fn capture_stmt(&mut self, inner: N<'a>, env: &Env) {
        let ts = inner.direct_tokens();
        let fld = ts[1].clone();
        let e = inner.find("expr");
        let schema = match self.schema.clone() {
            None => {
                if !self.schema_unknown {
                    self.obs_err("E-OBS-012", format!("`capture {}` needs an observation schema attached with @observe(Schema) (13)", fld.text), &fld, &fld.text, "statement", "LANG-13-01");
                }
                self.pure_snapshot(e, env, None, "capture", &fld, &fld.text);
                return;
            }
            Some(s) => s,
        };
        let (mut ft, _) = match schema.obs_fields.get(&fld.text) {
            Some(v) => v.clone(),
            None => {
                self.obs_err("E-OBS-012", format!("`{}` is not a field of observation schema {}", fld.text, schema.name), &fld, &fld.text, "statement", "LANG-13-01");
                self.infer(e, env, None);
                return;
            }
        };
        if let Type::Refined { base, .. } = &ft {
            if base.name == "String" {
                ft = Type::Named(base.clone());
            }
        }
        if self.ctx.iter().any(|c| c == "loop") {
            self.obs_err("E-OBS-011", format!("`capture {}` inside a loop body; a repeated measurement is `record` (13)", fld.text), &fld, &fld.text, "loop body", "LANG-13-02");
        } else if self.own.captured.contains(&fld.text) {
            let first = inner.direct_tokens()[0].clone();
            let last = inner.tokens().into_iter().filter(|x| x.kind != Kind::Nl).last().unwrap().clone();
            let corr = if first.line == last.line {
                let f = self.dc.file.clone();
                Some(crate::decls::correction("remove the second capture", vec![crate::decls::edit(&f, first.line, first.col, crate::decls::tok_end(&last), "")]))
            } else {
                None
            };
            self.rec_c(
                "E-OBS-010",
                format!("`{}` is captured a second time on this path", fld.text),
                "LANG-13-02",
                &fld,
                Json::obj([("frame_field", Json::str(&fld.text)), ("exit_path", Json::str("statement"))]),
                corr,
            );
        }
        let t = self.pure_snapshot(e, env, Some(&ft), "capture", &fld, &fld.text);
        if known(&t) && known(&ft) {
            self.expect(&t, &ft, ftok(e.unwrap()), &format!("capture {}", fld.text));
        }
        self.own.captured.insert(&fld.text, "all".into());
        crate::journal::observe("capture", &fld.text, fld.line, vec![]);
    }

    pub fn record_stmt(&mut self, inner: N<'a>, env: &Env) {
        let ts = inner.direct_tokens();
        let nm = ts[1].clone();
        let al = inner.find("argList");
        let d = match self.dc.table.get(&nm.text) {
            Some(d) if d.kind == "observation" => d.clone(),
            _ => {
                self.obs_err("E-OBS-012", format!("`record {}` names no observation schema", nm.text), &nm, &nm.text, "statement", "LANG-13-01");
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                return;
            }
        };
        let given: Vec<String> = self.aspect_args(al, env).into_iter().filter_map(|a| a.0).collect();
        crate::journal::observe("record", &d.name, nm.line, given);
        for (lbl, e, _, _) in self.aspect_args(al, env) {
            let fd = lbl.as_ref().and_then(|l| d.obs_fields.get(l)).cloned();
            match (lbl, fd) {
                (Some(l), Some((ft, _))) => {
                    let et = e.map(ftok).cloned().unwrap_or_else(|| nm.clone());
                    let t = self.pure_snapshot(e, env, Some(&ft), "record", &et, &l);
                    if known(&t) && known(&ft) {
                        self.expect(&t, &ft, &et, &format!("record field {l}"));
                    }
                }
                (lbl, _) => {
                    let t = e.map(ftok).cloned().unwrap_or_else(|| nm.clone());
                    let l = lbl.clone().unwrap_or_else(|| "positional".into());
                    self.obs_err("E-OBS-012", format!("`record {}`: `{l}` is not a field of the schema", nm.text), &t, &lbl.unwrap_or_default(), "statement", "LANG-13-01");
                    if e.is_some() {
                        self.infer(e, env, None);
                    }
                }
            }
        }
    }

    pub fn obs_exit(&mut self, kind: &str, tok: &Token, where_: &str) {
        let schema = match &self.schema {
            Some(s) => s.clone(),
            None => return,
        };
        if self.ctx.iter().any(|c| c == "lambda") {
            return;
        }
        let required: Vec<String> = schema.obs_fields.iter().filter(|(_, (_, req))| req == "all" || req == kind).map(|(f, _)| f.clone()).collect();
        crate::journal::observe(&format!("exit {kind}"), where_, tok.line, required);
        for (fld, (_, req)) in schema.obs_fields.iter() {
            if req == "all" || req == kind {
                if self.own.captured.get(fld).map(|s| s.as_str()) != Some("all") {
                    self.obs_err("E-OBS-001", format!("required field `{fld}` ({req}) is not captured on every path before {where_}"), tok, fld, where_, "LANG-13-02");
                }
            }
        }
    }

    pub fn asy_err(&mut self, msg: String, tok: &Token, statement: &str) {
        self.rec("E-ASY-003", msg, "LANG-12-05", tok, Json::obj([("offending_statement", Json::str(statement)), ("rule_reference", Json::str("12.7"))]));
    }

    pub fn check_pool_launch(&mut self, m: &str, al: Option<N<'a>>, env: &Env, _tok: &Token) {
        let args = al.map(|a| a.child_nodes("arg")).unwrap_or_default();
        if args.is_empty() {
            return;
        }
        let e = match args[0].find("expr") {
            Some(e) => e,
            None => return,
        };
        let lam = e.find("lambda");
        if m == "launchBlocking" || lam.is_some() {
            let lam = match lam {
                Some(l) => l,
                None => {
                    self.asy_err("launchBlocking takes a synchronous `fn once` closure (12.5)".into(), ftok(e), m);
                    return;
                }
            };
            if lam.words().contains(&"mut") {
                self.asy_err("a `fn mut` closure is not Shareable and cannot cross a pool launch (12.7)".into(), ftok(lam), m);
            }
            if let Some(cl) = lam.child_node("captureList") {
                for cap in cl.child_nodes("capture") {
                    let ck = cap.kids();
                    if let C::T(_) = ck[0] {
                        continue;
                    }
                    let mtok = ck[0].tokens()[0].clone();
                    let marker = mtok.text.clone();
                    let root = ck[1].tokens()[0].text.clone();
                    let b = env_lookup(env, &root);
                    if marker == "alias" || marker == "editable" {
                        self.asy_err(format!("the closure captures a loan of `{root}` (`{marker}`): loans never cross a pool launch (12.5)"), &mtok, m);
                    } else if let Some(b) = b {
                        if known(strip(&b.ty)) && !self.dc.is_shareable(strip(&b.ty)) {
                            self.asy_err(format!("the closure captures `{root}` of type {}, which is not Shareable (12.7)", strip(&b.ty).show()), &mtok, m);
                        }
                    }
                }
            }
            return;
        }
        let inner_al = e.find("argList");
        for a in inner_al.map(|x| x.child_nodes("arg")).unwrap_or_default() {
            let ae = match a.find("expr") {
                Some(x) => x,
                None => continue,
            };
            let mk = ae.find("markerExpr");
            let pl = self.path_of_expr(Some(ae));
            if let Some(mk) = mk {
                let mt = mk.tokens()[0].text.clone();
                if mt == "alias" || mt == "editable" {
                    self.asy_err(format!("`{mt} {}` is a loan: loans never cross a pool launch (12.5)", pl.clone().unwrap_or("None".into())), ftok(ae), m);
                    continue;
                }
            }
            if let Some(pl) = pl {
                let b = env_lookup(env, &root_of(&pl));
                let t = if let Some(place) = ae.find("place") {
                    strip(&self.declared_type(place, env)).clone()
                } else if b.is_some() && !pl.contains('.') {
                    strip(&b.as_ref().unwrap().ty).clone()
                } else {
                    continue;
                };
                if b.as_ref().map_or(false, |b| b.kind == "param" || b.kind == "editable") && mk.is_none() && self.is_bearing(&t) {
                    self.asy_err(format!("`{pl}` is a loan parameter: loans never cross a pool launch (12.5)"), ftok(ae), m);
                } else if mk.is_none() && self.is_bearing(&t) {
                    self.asy_err(format!("`{pl}` is passed by loan: a pool task may not borrow; write `copy {pl}` or `move {pl}` of a Shareable value (12.5)"), ftok(ae), m);
                } else if known(&t) && !self.dc.is_shareable(&t) {
                    self.asy_err(format!("`{pl}` has type {}, which is not Shareable (12.7)", t.show()), ftok(ae), m);
                }
            }
        }
    }

    pub fn task_typed(&mut self, e: N<'a>, env: &Env) -> bool {
        let saved = self.nrecords();
        let own = self.own.copy();
        let t = strip(&self.infer(Some(e), env, None)).clone();
        self.truncate_records(saved);
        self.own = own;
        t.is_named("Task")
    }

    pub fn shape_err(&mut self, msg: &str, tok: &Token, shape: &str, alternative: &str) {
        self.own_err("E-OWN-090", format!("{msg}; alternative: {alternative} (C.2)"), tok, shape, "unsupported shape", "LANG-07-09", "");
    }

    pub fn check_shared_shape(&mut self, args_types: &[(Type, N<'a>)], what: &str) {
        for (t, e) in args_types {
            let s = strip(t);
            if t.is_loan() || matches!(s, Type::Named(n) if ["StringView", "BytesView", "ListView"].contains(&n.name.as_str())) {
                self.shape_err(&format!("a loan is stored in {what}"), ftok(*e), &format!("loan in {what}"), "own the value");
            } else if matches!(s, Type::Fn { kind, .. } if kind == "fn mut") {
                self.shape_err(&format!("a `fn mut` closure escapes into {what}"), ftok(*e), &format!("fn mut in {what}"), "a Mutex around the state and a `fn` closure");
            } else {
                let d = if known(s) { self.decl_of(s) } else { None };
                if let Some(d) = d {
                    if d.lifetime.is_some() {
                        self.shape_err(
                            &format!("a value of {}, which carries a loan through its lifetime parameter, is stored in {what}", d.name),
                            ftok(*e),
                            &format!("loan-carrying value in {what}"),
                            "own the value",
                        );
                    }
                }
            }
        }
    }

    pub fn defer_stmt(&mut self, inner: N<'a>, env: &Env) {
        let blk = inner.find("block").unwrap();
        let roots: BTreeSet<String> = blk.tokens().iter().filter(|t| t.kind == Kind::Lower && env_lookup(env, &t.text).is_some()).map(|t| t.text.clone()).collect();
        let tok = inner.direct_tokens()[0].clone();
        for r in &roots {
            let b = env_lookup(env, r);
            let h = self.own.holders.get(r).cloned();
            if let (Some(b), Some(h)) = (b, h) {
                if b.depth == self.depth && h.borrow().holder.as_deref() == Some(r.as_str()) {
                    self.own_err(
                        "E-OWN-052",
                        format!("`{r}` holds a loan that ends when this block exits, before its deferred block runs; the defer may not use it (12.1)"),
                        &tok,
                        r,
                        "loan ended before the defer",
                        "LANG-07-04",
                        "",
                    );
                }
            }
        }
        crate::journal::cleanup("defer", "", tok.line, vec![]);
        self.deferred.push((self.depth, roots, tok));
        self.defer_bodies.push((self.depth, blk));
        // D-75: the body runs at every exit of the enclosing block, after any of the statements that follow. It is checked
        // from the registration state without the facts of every place those statements (later defers included) assign
        // or loan exclusively, and its own effects do not happen here: they happen at the block's exit.
        let rest = self.block_rest.clone();
        let saved_facts = self.facts.clone();
        for st in rest {
            for p in self.assigned_places(st) {
                self.inval_log.push(p.clone());
                self.facts.invalidate(&p);
            }
        }
        self.defer_depth += 1;
        let saved_try = self.in_try;
        self.in_try = true;
        self.walk_block(blk, env);
        self.in_try = saved_try;
        self.defer_depth -= 1;
        self.facts = saved_facts;
    }

    pub fn fresh_activation(&mut self) {
        self.facts = Facts::default();
        self.own = Owners::default();
        self.depth = 0;
        self.eff = Summary::default();
        self.reported = BTreeSet::new();
        self.schema = None;
        self.caps_calls = 0;
        self.discharges = vec![];
    }

    #[allow(dead_code)]
    fn _unused(&self) -> Option<Rc<DeclChecker<'a>>> {
        None
    }
}
