//! Expressions, calls and members (Language Reference 5.8, 6, 7.5, 8): the expression half of checker/typer.py.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use fortis_diag::Json;
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{C, N};
use fortis_syntax::{parse_from, Parsed};

use super::*;
use crate::decls::{core_origin_of, named_of_decl, DeclChecker, DeclRef, Sig};
use crate::effects::{Summary, PURE_FORBIDS};
use crate::intervals::{self as iv, Facts};
use crate::literals::{assignable, closure_conflict, concrete, contains_dynamic_fn, declared_vs_lambda, dynamic_covers, interp, is_interp, is_strlit, literal_type, negate, CLOSURE_WHY};
use crate::owners::{root_of, Owners};
use crate::types::*;

thread_local! {
    static HOLES_AT: RefCell<HashMap<(usize, usize, String, usize, usize), Option<&'static Parsed>>> = RefCell::new(HashMap::new());
}

/// A concrete nominal type as a key, peeling a refinement to its base. None when an argument is not nominal
/// (a type parameter, a loan, an unknown), so those are not reported as a constructor mismatch.
fn nominal_key(t: &Type) -> Option<String> {
    match strip(t) {
        Type::Refined { base, .. } => nominal_key(&base.as_type()),
        Type::Named(n) => {
            let mut s = n.name.clone();
            for a in &n.args {
                s.push('<');
                s.push_str(&nominal_key(a)?);
                s.push('>');
            }
            Some(s)
        }
        _ => None,
    }
}

/// The parse of one interpolation hole at its place in the file: its tokens carry their file positions, so
/// two holes with the same text are distinct trees (node types, closure labels and failure lines are per
/// occurrence). `line`/`col` locate the hole's first character (the lexer records them in `Token::interps`).
pub fn hole_tree_at<'t>(tk: &'t Token, k: usize, src: &str) -> Option<&'t Parsed> {
    let (line, col) = hole_position(tk, k, src);
    // a lexed string token owns a slot per hole: the parse lives as long as the token (defect D-32: the cache below
    // leaked a tree for every hole of every check, so a long-running process grew without bound)
    if let (Some(slot), Some((s, _, _))) = (tk.holes.0.get(k), tk.interps.get(k)) {
        if s == src {
            return slot.get_or_init(|| parse_hole(src, line, col).map(Box::new)).as_deref();
        }
    }
    // a token without slots (synthesised): the old cache, which keeps its trees for the process's life
    HOLES_AT.with(|h| {
        let mut h = h.borrow_mut();
        // the token's identity separates files checked in one process; text and position guard against reuse
        let key = (tk as *const Token as usize, k, src.to_string(), line, col);
        if let Some(p) = h.get(&key) {
            return *p;
        }
        let p = parse_hole(src, line, col).map(|p| &*Box::leak(Box::new(p)));
        h.insert(key, p);
        p
    })
}

/// A hole's parse, its tokens moved to the hole's place in the file.
fn parse_hole(src: &str, line: usize, col: usize) -> Option<Parsed> {
    parse_from(src, "expr").ok().map(|mut p| {
        for t in &mut p.toks {
            if t.line == 1 {
                t.col += col - 1;
                for x in &mut t.interps {
                    if x.1 == 1 {
                        x.2 += col - 1;
                    }
                    x.1 += line - 1;
                }
            }
            t.line += line - 1;
        }
        p
    })
}

/// The position of the `k`th hole of a string token (its first character), or the token's own position.
pub fn hole_position(tk: &Token, k: usize, src: &str) -> (usize, usize) {
    match tk.interps.get(k) {
        Some((s, l, c)) if s == src => (*l, *c),
        _ => (tk.line, tk.col),
    }
}


/// The source of each `${…}` hole of a string token, in order: the lexer's record, which skips strings nested in a hole
/// (D-161), else (a synthesised token) a scan of its text.
pub fn hole_sources(tk: &Token) -> Vec<String> {
    if tk.interps.is_empty() {
        holes_of(&tk.text)
    } else {
        tk.interps.iter().map(|(s, _, _)| s.clone()).collect()
    }
}

/// The source of each `${…}` hole of a string literal's text, in order (braces only; see `hole_sources`).
pub fn holes_of(text: &str) -> Vec<String> {
    let cs: Vec<char> = text.chars().collect();
    let body: Vec<char> = cs[1..cs.len() - 1].to_vec();
    let (mut i, mut out) = (0usize, Vec::new());
    while i < body.len() {
        if body[i] == '\\' {
            i += 2;
            continue;
        }
        if body[i] == '$' && i + 1 < body.len() && body[i + 1] == '{' {
            let (mut depth, mut j) = (1i32, i + 2);
            while j < body.len() && depth > 0 {
                if body[j] == '{' {
                    depth += 1;
                } else if body[j] == '}' {
                    depth -= 1;
                }
                j += 1;
            }
            out.push(body[i + 2..j.max(i + 3) - 1].iter().collect());
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

type Param = (String, Type, String);

/// Whether a parameter's default makes a new value at each call (anything but a literal string, 5.8; D-31).
fn default_is_temporary(decl: N, pname: &str) -> bool {
    for p in decl.find_all("param") {
        let first = p.direct_tokens().into_iter().find(|t| t.kind == Kind::Lower);
        if first.map_or(false, |t| t.text == pname) {
            return match p.find("constExpr") {
                None => false,
                Some(d) => {
                    // `null` and a literal string lend nothing that expires, grouped or not
                    let dts: Vec<&Token> = d.tokens().into_iter().filter(|t| t.text != "(" && t.text != ")").collect();
                    !(dts.len() == 1 && (dts[0].text == "null" || (dts[0].kind == Kind::Str && !dts[0].text.contains("${"))))
                }
            };
        }
    }
    false
}

/// The source of a node with canonical spacing (as the CFG renders statements).
pub fn source_text(c: C) -> String {
    let toks = c.tokens();
    let mut s = toks.iter().filter(|t| t.kind != Kind::Nl).map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ");
    for (a, b) in [(" ( ", "("), ("( ", "("), (" )", ")"), (" ,", ","), (" .", "."), (". ", "."), (" [", "["), ("[ ", "["), (" ]", "]")] {
        s = s.replace(a, b);
    }
    s
}

/// The expected type with every present-injection optional removed (6.8).
fn peel_optional(t: &Type) -> &Type {
    let mut cur = strip(t);
    while let Type::Optional(inner) = cur {
        cur = strip(inner);
    }
    cur
}

impl<'a> Typer<'a> {
    /// A core literal names the standard type. `10ms` is `Duration` of `fortis.time`, the origin a signature parsed
    /// against the prelude's copy carries. An unstamped name would be a different type.
    pub(crate) fn stamp_literal(&self, t: Type) -> Type {
        if let Type::Named(n) = &t {
            if n.origin.is_empty() && n.package.is_empty() {
                let origin = self.dc.core_module(&n.name);
                if !origin.is_empty() {
                    return Type::Named(NamedT { name: n.name.clone(), args: n.args.clone(), package: String::new(), origin });
                }
            }
        }
        t
    }

    // ---- calls (5.8) --------------------------------------------------------------------
    pub fn check_call(&mut self, sig: &Sig<'a>, al: Option<N<'a>>, env: &Env, m: &mut BTreeMap<String, Type>, where_: &str, tok: &Token) -> Type {
        let expected = self.pending_call_expected.take();
        self.call_depth += 1;
        let t = self.check_call_at(sig, al, env, m, where_, tok, expected.as_ref());
        self.call_depth -= 1;
        t
    }

    fn check_call_at(&mut self, sig: &Sig<'a>, al: Option<N<'a>>, env: &Env, m: &mut BTreeMap<String, Type>, where_: &str, tok: &Token, expected: Option<&Type>) -> Type {
        let (call_decl, call_label) = (self.call_decl.clone(), self.call_label.clone());
        let args = Self::arg_nodes(al);
        let has_lambda = args.iter().any(|a| self.is_lambda(Self::arg_expr(*a)));
        if let Some(expected) = expected {
            // the expected result fixes type arguments before the arguments are typed (6.10). A function binding
            // is dropped only when a lambda is pending, so the lambda can pin the closure (8.4); `seed_dynamic_fn`
            // then puts a `dynamic fn` back.
            self.seed_expected_result(&sig.ret, expected, m, has_lambda);
            self.seed_dynamic_fn(&sig.ret, expected, m);
        }
        let params = sig.params.clone();
        let mut bound: BTreeSet<String> = BTreeSet::new();
        let mut seen_named = false;
        let mut pairs: Vec<(Param, Option<N<'a>>)> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let ts = a.direct_tokens();
            let e = Self::arg_expr(*a);
            if ts.first().map_or(false, |t| t.text == "...") {
                continue;
            }
            let p: Param;
            if !ts.is_empty() && ts[0].kind == Kind::Lower && ts.len() > 1 && ts[1].text == ":" {
                seen_named = true;
                match params.iter().find(|p| p.0 == ts[0].text) {
                    None => {
                        let names = params.iter().map(|p| p.0.clone()).collect::<Vec<_>>().join(", ");
                        self.typ("E-TYP-130", format!("{where_} has no parameter `{}`", ts[0].text), ts[0], &names, &ts[0].text, &ts[0].text, "LANG-05-09");
                        self.infer(e, env, None);
                        continue;
                    }
                    Some(pp) => {
                        if bound.contains(&pp.0) {
                            self.typ("E-TYP-130", format!("parameter `{}` of {where_} is bound twice", pp.0), ts[0], "each parameter once", &pp.0, &pp.0, "LANG-05-09");
                            continue;
                        }
                        p = pp.clone();
                    }
                }
            } else {
                if seen_named {
                    self.typ("E-TYP-130", format!("positional argument after a named argument in {where_}"), ftok(*a), "named arguments last", "positional", where_, "LANG-05-09");
                    self.infer(e, env, None);
                    continue;
                }
                if !params.is_empty() && params.last().unwrap().2.starts_with("...") && i + 1 >= params.len() {
                    p = params.last().unwrap().clone();
                } else if i < params.len() {
                    p = params[i].clone();
                } else {
                    let s = if params.len() != 1 { "s" } else { "" };
                    self.typ(
                        "E-TYP-130",
                        format!("{where_} takes {} argument{s}, {} given", params.len(), args.len()),
                        ftok(*a),
                        &format!("{} arguments", params.len()),
                        &args.len().to_string(),
                        where_,
                        "LANG-05-09",
                    );
                    self.infer(e, env, None);
                    continue;
                }
            }
            if !p.2.starts_with("...") {
                bound.insert(p.0.clone());
            }
            pairs.push((p, e));
        }
        let mut arg_types: Vec<(Param, Type, Option<N<'a>>)> = Vec::new();
        let mut deferred: Vec<(Param, Option<N<'a>>)> = Vec::new();
        let saved_res = std::mem::take(&mut self.own.reservations);
        self.res_stack.push(saved_res.clone());
        let recv = self.receiver.take();
        if let Some((rp, rm)) = &recv {
            self.reserve(rp, if rm != "alias" { rm } else { "alias" }, tok);
        }
        let origin = if let Type::Loan { origin, .. } = &sig.ret { Some(origin.clone()) } else { None };
        let mut temps: Vec<Token> = Vec::new();
        // A pending lambda suppresses an empty-function pin only for a type parameter it can
        // constrain. `foo(move id, fn ...)` passes the lambda to a concrete `fn`; `id` still fixes `T` (8.4).
        let pending_names: BTreeSet<String> = pairs
            .iter()
            .filter(|(_, e)| self.is_lambda(*e))
            .flat_map(|(p, _)| Self::tparams_of(&p.1))
            .collect();
        for (p, e) in pairs {
            if self.is_lambda(e) {
                deferred.push((p, e));
                continue;
            }
            let pmode = if origin.as_deref() == Some(p.0.as_str()) {
                "from".to_string()
            } else if p.1.is_loan() {
                "loan".to_string()
            } else {
                p.2.clone()
            };
            self.arg_mode = Some(pmode.clone());
            let want = self.subst(&p.1, m);
            let t = self.infer(e, env, Some(&want));
            self.arg_mode = None;
            if let Some(e) = e {
                self.check_arg_marker(e, &p.0, &pmode, &t, ftok(e), env);
                if sig.has_mod("async") && (pmode.is_empty() || pmode == "loan") && self.lends_temporary(e, &t) {
                    temps.push(ftok(e).clone());
                }
            }
            let shares = Self::tparams_of(&p.1).iter().any(|tp| pending_names.contains(tp) && !m.contains_key(tp));
            self.unify_pin(&p.1, &t, m, !shares);
            arg_types.push((p, t, e));
        }
        // an omitted parameter's default that is evaluated afresh (not a literal string) is a temporary too (D-31)
        if sig.has_mod("async") {
            if let Some(node) = sig.node {
                for (pn, pt, pm) in &sig.params {
                    if !bound.contains(pn) && pm.is_empty() && self.is_bearing(pt) && default_is_temporary(node, pn) {
                        temps.push(tok.clone());
                    }
                }
            }
        }
        let depth = self.call_depth;
        self.carried_temps.extend(temps.into_iter().map(|tk| (tk, depth)));
        for (p, e) in deferred {
            let want = self.subst(&p.1, m);
            let t = self.infer(e, env, Some(&want));
            self.unify(&p.1, &t, m);
            arg_types.push((p, t, e));
        }
        self.activate(tok, Self::result_carries(sig), env);
        self.res_stack.pop();
        self.own.reservations = saved_res;
        self.call_decl = call_decl.clone();
        self.call_label = call_label.clone();
        let callee = self.callee_summary(call_decl.as_ref(), sig, &call_label);
        let arg_places: Vec<(String, String)> = arg_types
            .iter()
            .filter(|(p, _, e)| self.path_of_expr(*e).is_some() && p.2.trim_start_matches('.') == "editable")
            .map(|(p, _, e)| (p.0.clone(), self.path_of_expr(*e).unwrap()))
            .collect();
        let rp = recv.as_ref().map(|r| r.0.clone()).or_else(|| self.coll_path.clone());
        self.import_call(&callee, tok, rp, &arg_places, env, sig.has_mod("async"));
        if self.call_label == "method Shared.create" || self.call_label == "method Mutex.create" {
            let what = self.call_label.split('.').next().unwrap().split_whitespace().last().unwrap().to_string();
            let at: Vec<(Type, N<'a>)> = arg_types.iter().filter_map(|(_, t, e)| e.map(|e| (t.clone(), e))).collect();
            self.check_shared_shape(&at, &what);
        }
        for (p, t, e) in &arg_types {
            let want = self.subst(&p.1, m);
            let e = match e {
                Some(e) => *e,
                None => continue,
            };
            if !has_free(&want) {
                // a declared function against a lambda-pinned parameter is the same clash as two lambdas (8.4).
                // `expect` would accept it: an empty identity is not a `closure_conflict`. One record, not both.
                if declared_vs_lambda(t, &want) {
                    let shown = t.show();
                    let ws = want.show();
                    let what = format!("argument `{}`", p.0);
                    self.typ("E-TYP-040", format!("{what} has type {shown}, expected {ws}: {CLOSURE_WHY}"), ftok(e), &ws, &shown, &what, "LANG-08-04");
                } else {
                    self.expect(t, &want, ftok(e), &format!("argument `{}`", p.0));
                }
            }
            if self.is_linear(t) && self.path_of_expr(Some(e)).is_none() && !p.2.trim_start_matches('.').starts_with("owned") && e.find("markerExpr").is_none() && assignable(t, &want).0 != Some(false) {
                let ft = ftok(e);
                self.obl_err(
                    "E-OBL-001",
                    format!("the linear value passed to `{}` is only loaned for the call and dropped afterwards; the parameter must be `owned`", p.0),
                    ft,
                    &format!("{}:{}", ft.line, ft.col),
                    &format!("argument {}", p.0),
                    "LANG-10-01",
                );
            }
        }
        // the callee's `where` bounds, and a generic owner's, hold for the type arguments the call fixes (6.10):
        // `MutexGuard[T].get` needs a Copyable T (D-171)
        let mut bounds = std::mem::take(&mut self.pending_owner_where);
        bounds.extend(sig.where_.iter().cloned());
        for (tp, iface) in bounds {
            let a = match m.get(&tp) {
                Some(a) => strip(a).clone(),
                None => continue,
            };
            let want = match self.subst(&Type::Named(iface.clone()), m) {
                Type::Named(w) => w,
                _ => iface.clone(),
            };
            if self.bound_holds(&a, &want) == Some(false) {
                let ws = Type::Named(want.clone()).show();
                self.typ("E-TYP-090", format!("{where_} needs `{tp}` to implement {ws}; {} does not (6.10)", a.show()), tok, &ws, &a.show(), where_, "LANG-06-10");
            }
        }
        for p in &params {
            if !bound.contains(&p.0) && !p.2.starts_with("...") && !Self::has_default(sig, &p.0) {
                self.typ("E-TYP-050", format!("{where_} needs argument `{}`", p.0), tok, &p.0, "absent", where_, "LANG-05-09");
            }
        }
        let unknown_args = arg_types.iter().any(|(_, t, _)| !known(&strip(t)));
        self.result_of_call(sig, m, unknown_args)
    }

    /// A constant expression's arithmetic (grammar constExpr: terms and operators in one flat list): `*`, `/` and `%`
    /// bind tighter than `+` and `-`, so each product is typed first, then the sum; every term is typed (D-149).
    pub fn const_arith(&mut self, kids: &[C<'a>], env: &Env) -> Type {
        let mut groups: Vec<Vec<C<'a>>> = vec![vec![kids[0]]];
        let mut ops: Vec<Token> = vec![];
        let mut i = 1;
        while i + 1 < kids.len() {
            let op = ctok(kids[i]).clone();
            if op.text == "+" || op.text == "-" {
                ops.push(op);
                groups.push(vec![kids[i + 1]]);
            } else {
                let g = groups.last_mut().unwrap();
                g.push(kids[i]);
                g.push(kids[i + 1]);
            }
            i += 2;
        }
        let mut total: Option<Type> = None;
        for (gi, g) in groups.iter().enumerate() {
            let mut t = self.infer_c(g[0], env, None);
            let mut j = 1;
            while j + 1 < g.len() {
                let r = self.infer_c(g[j + 1], env, None);
                let op = ctok(g[j]).clone();
                t = self.arith(&op, &t, &r);
                j += 2;
            }
            total = Some(match total {
                None => t,
                Some(acc) => {
                    let op = ops[gi - 1].clone();
                    self.arith(&op, &acc, &t)
                }
            });
        }
        total.unwrap()
    }

    /// The type of a constant at a use: a known declared type, else the initialiser's type stored for this check,
    /// else (in the declaring module) the initialiser inferred now (D-54.4).
    fn const_use_type(&mut self, module: &str, name: &str, alias: Option<&Type>) -> Type {
        if let Some(a) = alias {
            if known(a) {
                return a.clone();
            }
        }
        if let Some(t) = self.inferred_const(module, name) {
            return t;
        }
        if module == self.module {
            if let Some(t) = self.const_value_type(name) {
                return t;
            }
        }
        alias.cloned().unwrap_or_else(|| unknown("const"))
    }

    /// Inside a constant expression a constant stands for its value (4.2: constants are built at load, before any code
    /// runs), so arithmetic over it is proved on the value: the type its initialiser gives, when that fits the declared
    /// type. Its initialiser's own records are reported where it is declared (D-149).
    pub fn const_value_type(&mut self, nm: &str) -> Option<Type> {
        let d = self.dc.table.get(nm).cloned()?;
        if d.kind != "const" || d.module != self.module || self.const_stack.contains(nm) {
            return None;
        }
        let node = d.node?;
        let ce = node.child_node("constExpr")?;
        self.const_stack.insert(nm.to_string());
        let before = self.nrecords();
        let t = self.infer(Some(ce), &env_new(None), None);
        self.dc.records.truncate(before);
        self.const_stack.remove(nm);
        if !known(strip(&t)) {
            return None;
        }
        if node.child_node("type").is_some() {
            let want = d.alias.clone()?;
            if crate::literals::assignable(&t, &want).0 != Some(true) {
                return None;
            }
        } else {
            // an unannotated constant's type is its initialiser's (4.2; D-54.4)
            self.note_inferred_const(&d.module, nm, &t);
        }
        Some(t)
    }

    /// A constant term `Type.member(args)` that is not a constant expression (4.2): only a constructor, an enum variant,
    /// a `Type.of` constructor of the `fortis` package and a derived `Type.default()` are (D-149).
    pub fn nonconstant_term(&mut self, node: N<'a>) -> Option<(Token, String)> {
        let m = node.child_node("memberName")?;
        let tr = node.child_node("typeRef")?;
        if !node.direct_tokens().iter().any(|t| t.text == "(") {
            return None;
        }
        let mt = m.tokens()[0].clone();
        let tt = self.dc.type_of(Some(tr));
        let d = self.decl_of(&tt);
        let ok = match mt.text.as_str() {
            "of" => d.map_or(true, |d| d.kind == "unknown" || d.module.starts_with("fortis")),
            "default" => d.map_or(true, |d| d.kind == "unknown" || d.derives.iter().any(|(n, _)| n == "Default")),
            _ => false,
        };
        if ok {
            return None;
        }
        let what = format!("{}.{}", tr.tokens().iter().map(|t| t.text.as_str()).collect::<String>(), mt.text);
        Some((mt, what))
    }

    /// The first call in an expression outside its `fn` literals, or None: aspect arguments are compile-time constants
    /// (11.8), so `@caps(Host.capabilities())` runs nothing (D-149).
    pub fn nonconstant_call(&self, node: N<'a>) -> Option<Token> {
        match node.name() {
            "lambda" => return None,
            "methodCall" => return Some(node.child_node("memberName").map(|m| m.tokens()[0].clone()).unwrap_or_else(|| node.tokens()[0].clone())),
            "postfix" if node.direct_tokens().first().map_or(false, |t| t.text == "(") => return Some(node.direct_tokens()[0].clone()),
            "primaryExpr" if node.child_node("memberName").is_some() && node.direct_tokens().iter().any(|t| t.text == "(") => {
                return Some(node.child_node("memberName").unwrap().tokens()[0].clone())
            }
            _ => {}
        }
        for c in node.kids().into_iter().filter_map(|c| c.node()) {
            if let Some(t) = self.nonconstant_call(c) {
                return Some(t);
            }
        }
        None
    }

    /// A constant, a default or a class constant is typed outside every body: a fresh activation under a label of its
    /// own, whose effects belong to no function's summary (D-149).
    pub fn decl_context(&mut self, label: String) {
        self.fresh_activation();
        self.self_type = None;
        self.self_mode = "self".into();
        self.ret_type = None;
        self.in_try = false;
        self.ctx.clear();
        self.loop_labels.clear();
        self.current_label = label;
    }

    /// Each parameter default (5.8) is typed where it is declared, against the parameter's type, under a label of its
    /// own: outside every body, so it cannot create the capabilities either (D-149).
    pub fn check_param_defaults(&mut self, sig: &Sig<'a>, label: &str) {
        let node = match sig.node {
            Some(n) => n,
            None => return,
        };
        let types: Vec<(String, Type)> = sig.params.iter().map(|(n, t, _)| (n.clone(), t.clone())).collect();
        self.check_defaults(node.find_all("param"), &types, label);
    }

    pub fn check_defaults(&mut self, params: Vec<N<'a>>, types: &[(String, Type)], label: &str) {
        for p in params {
            let ce = match p.child_node("constExpr") {
                Some(c) => c,
                None => continue,
            };
            let ts: Vec<&Token> = p.direct_tokens().into_iter().filter(|t| t.text != "...").collect();
            let pname = match ts.first() {
                Some(t) => t.text.clone(),
                None => continue,
            };
            let want = match types.iter().find(|(n, _)| *n == pname) {
                Some((_, t)) => t.clone(),
                None => continue,
            };
            self.decl_context(format!("default of parameter `{pname}` of {label}"));
            let fixed = known(strip(&want)) && !has_free(&want);
            let t = self.infer(Some(ce), &env_new(None), if fixed { Some(&want) } else { None });
            if fixed {
                self.expect(&t, &want, ftok(ce), &format!("default of `{pname}`"));
            }
        }
    }

    pub fn has_default(sig: &Sig<'a>, pname: &str) -> bool {
        let node = match sig.node {
            Some(n) => n,
            None => return false,
        };
        for p in node.find_all("param") {
            let ts: Vec<&Token> = p.direct_tokens().into_iter().filter(|t| t.text != "...").collect();
            if ts.first().map_or(false, |t| t.text == pname) {
                return p.find("constExpr").is_some();
            }
        }
        false
    }

    /// A lambda value, including one wrapped in parentheses (8.4).
    ///
    /// `(fn ...)(1)` is a call: a postfix on the group is not a lambda value. A bare lambda's
    /// first token is a lambda keyword; parentheses hide that token, so a group with an empty
    /// postfix is peeled and the inner expression is judged again.
    pub fn is_lambda(&self, e: Option<N<'a>>) -> bool {
        let mut node = e;
        while let Some(n) = node {
            if n.name() == "lambda" {
                return true;
            }
            if n.name() == "postfixExpr" {
                if n.child_node("postfix").is_some() {
                    return false;
                }
                node = n.child_node("primaryExpr");
                continue;
            }
            if n.name() == "primaryExpr" {
                if n.direct_tokens().first().map(|t| t.text.as_str()) == Some("(") {
                    node = n.child_node("expr");
                    continue;
                }
                return n.child_node("lambda").is_some();
            }
            let kids: Vec<N<'a>> = n.kids().into_iter().filter_map(|c| c.node()).collect();
            let has_op = n.direct_tokens().iter().any(|t| !matches!(t.kind, Kind::Nl | Kind::Newline));
            if kids.len() == 1 && !has_op {
                node = Some(kids[0]);
                continue;
            }
            return n.find("lambda").is_some()
                && n.first_token().map_or(false, |t| ["fn", "pure", "noalloc", "async", "try"].contains(&t.text.as_str()));
        }
        false
    }

    /// Bind type parameters from the expected result (LR 6.10), then drop what must not stick.
    ///
    /// A binding that still names an unsolved type parameter (the caller's, not this body's) is dropped.
    /// A binding to a function type is dropped only when a lambda argument is pending (8.4).
    ///
    /// A result that is itself a type parameter is left unbound. The arguments fix it, and the caller
    /// checks that result against the expected type. Binding `T` to `print`'s `StringView` would reject
    /// `same(text)` for a Copyable `String` (6.6, 6.10). `List[T]` against `List[Int]` still binds `T`.
    fn seed_expected_result(&self, ret: &Type, expected: &Type, m: &mut BTreeMap<String, Type>, drop_functions: bool) {
        let pat = self.subst(ret, m);
        if matches!(strip(&pat), Type::TypeParam(_)) {
            return;
        }
        let before: BTreeSet<String> = m.keys().cloned().collect();
        self.unify(&pat, strip(expected), m);
        let keys: Vec<String> = m.keys().cloned().collect();
        for k in keys {
            if before.contains(&k) {
                continue;
            }
            let v = match m.get(&k) {
                Some(v) => v.clone(),
                None => continue,
            };
            if !Self::tparams_of(&v).iter().all(|p| self.tparams_in_scope.contains(p)) {
                m.remove(&k);
                continue;
            }
            if drop_functions {
                let mut bound = &v;
                let drop_it = loop {
                    match bound {
                        Type::Optional(inner) | Type::Loan { inner, .. } => bound = inner,
                        Type::Fn { .. } => break true,
                        _ => break false,
                    }
                };
                if drop_it {
                    m.remove(&k);
                }
            }
        }
    }

    /// Bind an unbound type parameter where the expected type already says `dynamic fn` (8.4).
    ///
    /// A non-dynamic expected type is left alone, so the first lambda still pins the parameter and a
    /// second, different lambda conflicts with it. An expected stack is not used: `List.of(1, 2).count()`
    /// must not take its element type from the chain's result. A bare type parameter is bound to the
    /// complete expected type, optional wrappers included, so `List[(dynamic fn)?]` keeps the `?`.
    /// Optional injection is then peeled one layer at a time (6.8): `List[dynamic fn]?` and
    /// `(List[dynamic fn]?)?` still seed the list.
    fn seed_dynamic_fn(&self, pattern: &Type, actual: &Type, m: &mut BTreeMap<String, Type>) {
        let pattern = strip(pattern);
        let actual = strip(actual);
        // a bare type parameter takes the complete expected type, optional wrappers included
        if let Type::TypeParam(n) = pattern {
            if !m.contains_key(n) && contains_dynamic_fn(actual) {
                let mut a = concrete(strip(actual));
                if let Type::Refined { base, kind, .. } = &a {
                    if kind == "exact" {
                        a = Type::Named(base.clone());
                    }
                }
                m.insert(n.clone(), a);
            }
            return;
        }
        // present injection, one layer at a time, after the type parameter has kept its own optional
        if let Type::Optional(p) = pattern {
            if !matches!(actual, Type::Optional(_)) {
                self.seed_dynamic_fn(p, actual, m);
                return;
            }
        }
        if let Type::Optional(a) = actual {
            if !matches!(pattern, Type::Optional(_)) {
                self.seed_dynamic_fn(pattern, a, m);
                return;
            }
        }
        match (pattern, actual) {
            (Type::Named(p), Type::Named(a)) if p.name == a.name => {
                for (pa, aa) in p.args.iter().zip(a.args.iter()) {
                    self.seed_dynamic_fn(pa, aa, m);
                }
            }
            (Type::Optional(p), Type::Optional(a)) => self.seed_dynamic_fn(p, a, m),
            (Type::Loan { inner, .. }, _) => self.seed_dynamic_fn(inner, actual, m),
            (Type::Fn { params: pp, ret: pr, .. }, Type::Fn { params: ap, ret: ar, .. }) => {
                for (x, y) in pp.iter().zip(ap.iter()) {
                    self.seed_dynamic_fn(x, y, m);
                }
                self.seed_dynamic_fn(pr, ar, m);
            }
            (Type::Tuple(pi), Type::Tuple(ai)) => {
                for (x, y) in pi.iter().zip(ai.iter()) {
                    self.seed_dynamic_fn(x, y, m);
                }
            }
            (Type::Record(pf), Type::Record(af)) => {
                let am: BTreeMap<&str, &Type> = af.iter().map(|(k, t)| (k.as_str(), t)).collect();
                for (k, ft) in pf {
                    if let Some(at) = am.get(k.as_str()) {
                        self.seed_dynamic_fn(ft, at, m);
                    }
                }
            }
            _ => {}
        }
    }

    /// Narrow a declared function type to the one lambda that initialises it (8.4).
    ///
    /// The declared signature stays; only an empty identity is filled in. `dynamic fn` is the boxed
    /// position and is not narrowed, so a later lambda is still assignable there.
    pub fn with_closure(&self, want: &Type, have: &Type) -> Type {
        if let Type::Loan { inner, mode, origin } = want {
            return Type::Loan { inner: Box::new(self.with_closure(inner, strip(have))), mode: mode.clone(), origin: origin.clone() };
        }
        let ws = strip(want);
        let hs = strip(have);
        if let Type::Optional(inner) = ws {
            let inner_have = match hs {
                Type::Optional(i) => i.as_ref(),
                _ => hs,
            };
            return Type::Optional(Box::new(self.with_closure(inner, inner_have)));
        }
        if let (Type::Fn { params, ret, kind, modifiers, origin, dynamic, identity }, Type::Fn { identity: hid, dynamic: hd, .. }) = (ws, hs) {
            if !*dynamic && identity.is_empty() && !hid.is_empty() && !*hd {
                return Type::Fn {
                    params: params.clone(),
                    ret: ret.clone(),
                    kind: kind.clone(),
                    modifiers: modifiers.clone(),
                    origin: origin.clone(),
                    dynamic: false,
                    identity: hid.clone(),
                };
            }
        }
        if let (Type::Named(w), Type::Named(h)) = (ws, hs) {
            if w.name == h.name && w.args.len() == h.args.len() {
                let args = w.args.iter().zip(h.args.iter()).map(|(a, b)| self.with_closure(a, b)).collect();
                // the declared type's origin stays. `named_pkg` would drop it, so a later use of the binding
                // (`sum = Money.add(sum, …)` in a loop, which rewrites the binding) would name a different Money.
                return Type::Named(NamedT { name: w.name.clone(), args, package: w.package.clone(), origin: w.origin.clone() });
            }
        }
        if let (Type::Tuple(wi), Type::Tuple(hi)) = (ws, hs) {
            if wi.len() == hi.len() {
                return Type::Tuple(wi.iter().zip(hi.iter()).map(|(a, b)| self.with_closure(a, b)).collect());
            }
        }
        if let (Type::Record(wf), Type::Record(hf)) = (ws, hs) {
            let hm: BTreeMap<&str, &Type> = hf.iter().map(|(k, t)| (k.as_str(), t)).collect();
            return Type::Record(wf.iter().map(|(k, ft)| (k.clone(), hm.get(k.as_str()).map(|ht| self.with_closure(ft, ht)).unwrap_or_else(|| ft.clone()))).collect());
        }
        want.clone()
    }

    fn field_pin_key(decl: &crate::decls::Decl<'a>, field: &str, path: &str) -> (String, String, String, String) {
        (decl.type_identity(), decl.name.clone(), field.to_string(), path.to_string())
    }

    /// `slot` with each empty closure identity replaced by the lambda this field already holds (8.4).
    fn overlay_pins(&self, decl: &crate::decls::Decl<'a>, field: &str, slot: &Type, path: &str) -> Type {
        let slot = strip(slot);
        if let Type::Optional(inner) = slot {
            return Type::Optional(Box::new(self.overlay_pins(decl, field, inner, path)));
        }
        if let Type::Loan { inner, mode, origin } = slot {
            return Type::Loan { inner: Box::new(self.overlay_pins(decl, field, inner, path)), mode: mode.clone(), origin: origin.clone() };
        }
        if let Type::Fn { dynamic, identity, params, ret, kind, modifiers, origin } = slot {
            if *dynamic || !identity.is_empty() {
                return slot.clone();
            }
            let key = Self::field_pin_key(decl, field, path);
            let ident = self.dc.sess.field_closures.borrow().get(&key).cloned();
            if let Some(ident) = ident {
                return Type::Fn { params: params.clone(), ret: ret.clone(), kind: kind.clone(), modifiers: modifiers.clone(), origin: origin.clone(), dynamic: false, identity: ident };
            }
            return slot.clone();
        }
        match slot {
            Type::Named(n) => {
                let args = n.args.iter().enumerate().map(|(i, a)| self.overlay_pins(decl, field, a, &format!("{path}/{i}"))).collect();
                Type::Named(NamedT { name: n.name.clone(), args, package: n.package.clone(), origin: n.origin.clone() })
            }
            Type::Tuple(xs) => Type::Tuple(xs.iter().enumerate().map(|(i, a)| self.overlay_pins(decl, field, a, &format!("{path}/{i}"))).collect()),
            Type::Record(fs) => Type::Record(fs.iter().map(|(k, ft)| (k.clone(), self.overlay_pins(decl, field, ft, &format!("{path}/{k}")))).collect()),
            _ => slot.clone(),
        }
    }

    /// A field whose type contains a closure holds one lambda at that position for the whole check (8.4).
    pub fn note_field_closure(&mut self, decl: &crate::decls::Decl<'a>, field: &str, field_ty: &Type, value_ty: &Type, tok: &Token) {
        self.pin_field_closure(decl, field, field_ty, value_ty, tok, "");
    }

    /// Pin each non-dynamic closure inside `slot` to the lambda in `value` (8.4).
    ///
    /// Optional injection, a loan, and the elements of a collection, tuple or record are the same
    /// position as their inner closure. The first identity wins. A second, different lambda is rejected.
    /// `dynamic fn` is not pinned: it is the boxed heterogeneous position.
    fn pin_field_closure(&mut self, decl: &crate::decls::Decl<'a>, field: &str, slot: &Type, value: &Type, tok: &Token, path: &str) {
        let slot_s = strip(slot);
        let value_s = strip(value);
        if let Type::Optional(inner) = slot_s {
            let v = match value_s {
                Type::Optional(i) => i.as_ref(),
                other => other,
            };
            self.pin_field_closure(decl, field, inner, v, tok, path);
            return;
        }
        if let Type::Loan { inner, .. } = slot_s {
            let v = match value_s {
                Type::Loan { inner: i, .. } => i.as_ref(),
                other => other,
            };
            self.pin_field_closure(decl, field, inner, v, tok, path);
            return;
        }
        if let Type::Fn { dynamic, .. } = slot_s {
            if *dynamic {
                return;
            }
            let Type::Fn { identity, dynamic: vdyn, .. } = value_s else { return };
            if identity.is_empty() || *vdyn {
                return;
            }
            let key = Self::field_pin_key(decl, field, path);
            let prev = self.dc.sess.field_closures.borrow().get(&key).cloned();
            match prev {
                None => {
                    self.dc.sess.field_closures.borrow_mut().insert(key, identity.clone());
                }
                Some(prev) if prev != *identity => {
                    let shown = value.show();
                    let expected = slot_s.show();
                    self.typ("E-TYP-040", format!("field `{field}` has type {shown}, expected {expected}: {CLOSURE_WHY}"), tok, &expected, &shown, field, "LANG-08-04");
                }
                _ => {}
            }
            return;
        }
        match (slot_s, value_s) {
            (Type::Named(s), Type::Named(v)) if s.name == v.name => {
                for (i, (a, b)) in s.args.iter().zip(v.args.iter()).enumerate() {
                    self.pin_field_closure(decl, field, a, b, tok, &format!("{path}/{i}"));
                }
            }
            (Type::Tuple(ss), Type::Tuple(vs)) => {
                for (i, (a, b)) in ss.iter().zip(vs.iter()).enumerate() {
                    self.pin_field_closure(decl, field, a, b, tok, &format!("{path}/{i}"));
                }
            }
            (Type::Record(sf), Type::Record(vf)) => {
                let vm: BTreeMap<&str, &Type> = vf.iter().map(|(k, t)| (k.as_str(), t)).collect();
                for (k, ft) in sf {
                    if let Some(vt) = vm.get(k.as_str()) {
                        self.pin_field_closure(decl, field, ft, vt, tok, &format!("{path}/{k}"));
                    }
                }
            }
            _ => {}
        }
    }

    /// `p.field = fn …`: the field's class, not the binding, owns the one closure type (8.4).
    pub fn note_field_assign(&mut self, pl: N<'a>, value_ty: &Type, tok: &Token, env: &Env) {
        let members: Vec<Token> = pl.tokens().into_iter().filter(|t| t.kind == Kind::Lower).cloned().collect();
        if members.len() < 2 {
            return;
        }
        let field = members.last().unwrap().text.clone();
        let mut owner = if members[0].text == "self" { self.self_type.clone().unwrap_or_else(|| unknown("self")) } else { env_get(env, &members[0].text).unwrap_or_else(|| unknown("field owner")) };
        owner = strip(&owner).clone();
        let mut decl = self.decl_of(&owner);
        for mid in &members[1..members.len() - 1] {
            let Some(d) = decl.clone() else { return };
            if !d.fields.contains(&mid.text) {
                return;
            }
            let mm = if matches!(owner, Type::Named(_)) { self.recv_subst(&owner, &d) } else { BTreeMap::new() };
            let ft = d.fields.get(&mid.text).unwrap().0.clone();
            owner = strip(&self.subst(&ft, &mm)).clone();
            decl = self.decl_of(&owner);
        }
        let Some(d) = decl else { return };
        if !d.fields.contains(&field) {
            return;
        }
        let mm = if matches!(owner, Type::Named(_)) { self.recv_subst(&owner, &d) } else { BTreeMap::new() };
        let ft = self.subst(&d.fields.get(&field).unwrap().0, &mm);
        self.note_field_closure(&d, &field, &ft, value_ty, tok);
    }

    // ---- assignability ------------------------------------------------------------------
    pub fn expect(&mut self, have: &Type, want: &Type, tok: &Token, what: &str) {
        // a class value made an owned `dynamic I` is boxed: the creation allocates (Appendix C.3; X-117)
        if matches!(want, Type::Dynamic(_)) && matches!(strip(have), Type::Named(_) | Type::Refined { .. }) {
            self.eff.add("alloc", "proved", Some(tok));
        }
        let (ok, why) = assignable(have, want);
        let hs = strip(have).clone();
        let ws = strip(want).clone();
        if ok.is_none() {
            if let Type::Refined { .. } = &ws {
                if self.is_int(&ws) && self.is_int(&hs) {
                    let hi_ = self.interval_of(&hs);
                    let wi = self.interval_of(&ws);
                    if let (Some(hi_), Some(wi)) = (hi_, wi) {
                        if wi.0 <= hi_.0 && hi_.1 <= wi.1 {
                            return;
                        }
                        let status = iv::classify(hi_, wi);
                        self.ref_(
                            "E-REF-001",
                            format!("{what} has interval {}, which is not within {} ({status}); validate it or widen the declared type", Self::show_iv(Some(hi_)), ws.show()),
                            tok,
                            status,
                            &ws.show(),
                            &Self::show_iv(Some(hi_)),
                            "",
                        );
                        return;
                    }
                }
            }
            if let Type::Refined { base: wb, lo: wlo, hi: whi, kind: wk } = &ws {
                if wk == "length" || wk == "scalars" {
                    if let Type::Refined { base: hb, lo: hlo, hi: hhi, kind: hk } = &hs {
                        let base_ok = hb == wb || is_strlit(&hs);
                        let lo_ok = wlo.is_none() || (hlo.is_some() && hlo.unwrap() >= wlo.unwrap());
                        let hi_ok = whi.is_none() || (hhi.is_some() && hhi.unwrap() <= whi.unwrap());
                        if hk == wk && base_ok && lo_ok && hi_ok {
                            return;
                        }
                    }
                    self.ref_("E-REF-001", format!("{what} is not proved to satisfy {}: the domain has no {wk} facts (unknown); validate it", ws.show()), tok, "unknown", &ws.show(), "?", "");
                    return;
                }
            }
        }
        if ok == Some(false) {
            let shown = if is_strlit(have) { "a string literal".to_string() } else { have.show() };
            if let Type::Refined { lo, hi, kind, .. } = have {
                if kind == "exact" && why.contains("interval") {
                    let (l, h) = (lo.unwrap(), hi.unwrap());
                    self.ref_("E-REF-001", format!("literal {l} does not fit {}: {why}", want.show()), tok, "violated", &want.show(), &format!("[{l}, {h}]"), "");
                    return;
                }
            }
            let rule = if closure_conflict(have, want) {
                "LANG-08-04"
            } else if strip(have).is_optional() || strip(want).is_optional() {
                "LANG-06-08"
            } else {
                "LANG-06-01"
            };
            self.typ("E-TYP-040", format!("{what} has type {shown}, expected {}: {why}", want.show()), tok, &want.show(), &shown, what, rule);
        }
    }

    pub fn unify_branches(&mut self, types: &[(Type, Token)], what: &str, expected: Option<&Type>) -> Type {
        let mut result: Option<Type> = None;
        for (t, tok) in types {
            if !known(t) || *t == never() {
                continue;
            }
            let t = if is_interp(t) { string() } else { t.clone() };
            let r = match &result {
                None => {
                    result = Some(t);
                    continue;
                }
                Some(r) => r.clone(),
            };
            if t == r {
                continue;
            }
            if is_strlit(&r) && (t == string() || t == string_view() || is_strlit(&t)) {
                if !is_strlit(&t) {
                    result = Some(t);
                }
                continue;
            }
            if is_strlit(&t) && (r == string() || r == string_view()) {
                continue;
            }
            if t == null_t() && !strip(&r).is_optional() {
                result = Some(optional(r));
                continue;
            }
            if r == null_t() && !strip(&t).is_optional() {
                result = Some(optional(t));
                continue;
            }
            if let Type::Optional(ti) = strip(&t) {
                if !strip(&r).is_optional() && assignable(&r, ti).0 == Some(true) {
                    result = Some(t.clone());
                    continue;
                }
            }
            let (ok, _) = assignable(&t, &r);
            if ok == Some(true) {
                continue;
            }
            let (ok2, _) = assignable(&r, &t);
            if ok2 == Some(true) {
                result = Some(t);
                continue;
            }
            if ok == Some(false) {
                if closure_conflict(&t, &r) && dynamic_covers(expected, &t) && dynamic_covers(expected, &r) {
                    continue; // `dynamic fn` is the one position two lambdas may join (8.4)
                }
                let rule = if closure_conflict(&t, &r) { "LANG-08-04" } else { "LANG-06-01" };
                self.typ("E-TYP-040", format!("{what} yields {} where the other branches yield {}", t.show(), r.show()), tok, &r.show(), &t.show(), what, rule);
            }
        }
        result.unwrap_or_else(|| unknown("branches"))
    }

    // ---- expressions ------------------------------------------------------------------
    pub fn infer_c(&mut self, c: C<'a>, env: &Env, expected: Option<&Type>) -> Type {
        match c {
            C::T(_) => unknown(""),
            C::N(n) => self.infer(Some(n), env, expected),
        }
    }

    pub fn infer(&mut self, node: Option<N<'a>>, env: &Env, expected: Option<&Type>) -> Type {
        let t = self.infer_raw(node, env, expected);
        // a call whose arguments do not fix its type arguments, with no expected type to fix them (6.10): a proved
        // inference failure, repaired by annotating the receiving binding
        if let (Type::Unknown(why), None, Some(n)) = (&t, expected, node) {
            if why == "generic result" && !self.branch_target {
                // each enclosing expression node sees the same result; the record is kept once per line and message
                let tok = ftok(n);
                self.typ("E-TYP-160", "the type arguments of this call are not fixed by its arguments; annotate the receiving binding (6.10)".to_string(), tok, "an annotated binding", "uninferred type arguments", &tok.text, "LANG-06-11");
            }
        }
        if let (Some(nt_), Some(n)) = (self.node_types.as_mut(), node) {
            nt_.insert(n.id(), t.clone());
        }
        t
    }

    fn infer_raw(&mut self, node: Option<N<'a>>, env: &Env, expected: Option<&Type>) -> Type {
        let node = match node {
            Some(n) => n,
            None => return unknown(""),
        };
        let n = node.name();
        let kids = node.kids();
        match n {
            "constExpr" if kids.len() > 1 => return self.const_arith(&kids, env),
            "constExpr" | "expr" | "exprStmt" => return self.infer_c(kids[0], env, expected),
            "constTerm" => return self.const_term(node, env, expected),
            "outcomeExpr" => return self.outcome(node, env, expected),
            "coalesceExpr" => {
                if kids.len() == 1 {
                    return self.infer_c(kids[0], env, expected);
                }
                let left = self.infer_c(kids[0], env, None);
                let after_left = self.facts.clone();
                let skip_own = self.own.copy(); // the fallback runs only when the left is null (5.7)
                let exp_r = if let Type::Optional(i) = strip(&left) { Some((**i).clone()) } else { None };
                let right = self.infer_c(kids[2], env, exp_r.as_ref());
                self.facts = after_left.join(&self.facts); // D-76: the fallback runs only when the left is null
                self.join_conditional(&skip_own);
                let ls = strip(&left).clone();
                if known(&ls) && !ls.is_optional() {
                    self.typ("E-TYP-040", format!("`??` needs an optional left operand, got {}", ls.show()), ctok(kids[0]), "T?", &ls.show(), "??", "LANG-06-08");
                    return unknown("");
                }
                if let Type::Optional(i) = &ls {
                    self.expect(&right, i, ctok(kids[2]), "`??` fallback");
                    return (**i).clone();
                }
                return unknown("");
            }
            "orExpr" | "andExpr" => {
                if kids.len() == 1 {
                    return self.infer_c(kids[0], env, expected);
                }
                let mut cur = self.facts.clone();
                // the state after each operand: the expression may end there (D-73)
                let mut exits: Vec<Facts> = vec![];
                let opt = kids[1].tok().unwrap().text.clone();
                let mut first = true;
                for k in kids.iter().step_by(2) {
                    let pre = cur.clone();
                    self.facts = cur.clone();
                    // the first operand always runs; each later one is skipped when an earlier operand decides (5.7)
                    let skip_own = if first { None } else { Some(self.own.copy()) };
                    let t = self.infer_c(*k, env, None);
                    cur = self.facts.clone();
                    exits.push(cur.clone());
                    if known(strip(&t)) && base_name(strip(&t)) != Some("Bool") {
                        self.typ("E-TYP-040", format!("`{opt}` operand has type {}, expected Bool", t.show()), ctok(*k), "Bool", &t.show(), &opt, "LANG-06-01");
                    }
                    if n == "andExpr" {
                        let before = self.nrecords();
                        self.facts = pre; // narrowing re-reads the operand from its own start (D-73)
                        cur = self.narrow(k.node().unwrap(), env).0;
                        self.truncate_records(before);
                    }
                    if let Some(skip) = &skip_own {
                        self.join_conditional(skip); // a consume on the skipped operand is one path only (7.4, 10.1)
                    }
                    first = false;
                }
                // D-73: the operands' effects stay (an `editable` argument in the right operand invalidates its place);
                // the state after the expression is the join of the states where it can end
                self.facts = Facts::join_all(&exits);
                return boolean();
            }
            "bitOrExpr" | "bitXorExpr" | "bitAndExpr" | "shiftExpr" | "additiveExpr" | "multiplicativeExpr" => {
                if kids.len() == 1 {
                    return self.infer_c(kids[0], env, expected);
                }
                let mut t = self.infer_c(kids[0], env, None);
                let mut i = 1;
                while i < kids.len() {
                    let r = self.infer_c(kids[i + 1], env, None);
                    let op = kids[i].tok().unwrap().clone();
                    let before = self.nrecords();
                    let base_is_int = base_name(strip(&t)) == Some("Int");
                    t = self.arith(&op, &t, &r);
                    if kids.len() == 3 && base_is_int && self.nrecords() > before && self.dc.records.last().map_or(false, |x| x.code == "E-REF-002") {
                        if let Some(c) = self.checked_alternative(node, &op) {
                            self.dc.records.last_mut().unwrap().correction = Some(c);
                        }
                    }
                    i += 2;
                }
                return t;
            }
            "equalityExpr" | "relationalExpr" => {
                if kids.len() == 1 {
                    return self.infer_c(kids[0], env, expected);
                }
                let l = self.infer_c(kids[0], env, None);
                let r = self.infer_c(kids[2], env, None);
                let op = kids[1].tok().unwrap().clone();
                self.compare(&op, &l, &r);
                return boolean();
            }
            "rangeExpr" => {
                for k in kids.iter().step_by(2) {
                    let t = self.infer_c(*k, env, None);
                    if known(strip(&t)) && int_bounds(strip(&t)).is_none() {
                        self.typ("E-TYP-040", format!("range bound has type {}, expected an integer", t.show()), ctok(*k), "Int", &t.show(), "range", "LANG-06-01");
                    }
                }
                return unknown("range");
            }
            "unaryExpr" => {
                if let C::T(optok) = kids[0] {
                    let op = optok.text.clone();
                    let t = self.infer_c(kids[1], env, if op == "-" { expected } else { None });
                    let s = strip(&t).clone();
                    if op == "!" {
                        if known(&s) && base_name(&s) != Some("Bool") {
                            self.typ("E-TYP-040", format!("`!` operand has type {}, expected Bool", t.show()), ctok(kids[1]), "Bool", &t.show(), "!", "LANG-06-01");
                        }
                        return boolean();
                    }
                    if op == "-" {
                        return self.negated_type(optok, kids[1], &t);
                    }
                    if op == "await" {
                        return self.await_(optok, kids[1], env, &t, &s);
                    }
                }
                return self.infer_c(kids[0], env, expected);
            }
            "markerExpr" => return self.marker(node, env),
            "postfixExpr" => return self.postfix_chain(node, env, expected),
            "primaryExpr" => return self.primary(node, env, expected),
            "literal" => {
                let tk = node.tokens()[0];
                if tk.kind == Kind::Str && tk.text.contains("${") {
                    self.check_interpolation(tk, env);
                    self.eff.add("alloc", "proved", Some(tk));
                }
                return self.stamp_literal(literal_type(tk));
            }
            "place" => return self.place_type(node, env),
            "lambda" => return self.lambda_type(node, env, expected),
            "matchExpr" => return self.match_(node, env, expected, false),
            "ifExpr" => return self.if_expr(node, env, expected),
            "block" => {
                self.walk_block(node, env);
                return void();
            }
            "tupleLit" => {
                let es = node.child_nodes("expr");
                let exp = expected.map(peel_optional);
                let mut items = Vec::new();
                for (i, e) in es.into_iter().enumerate() {
                    let child = match exp {
                        Some(Type::Tuple(ts)) if i < ts.len() => Some(&ts[i]),
                        _ => None,
                    };
                    items.push(self.infer(Some(e), env, child));
                    self.settle_element(); // a move in this element is consumed before the next one (D-159)
                }
                return Type::Tuple(items);
            }
            "recordLit" => {
                let mut ks: Vec<&Token> = kids.iter().filter(|k| k.is("ident")).map(|k| ctok(*k)).collect();
                if ks.is_empty() {
                    ks = kids.iter().filter_map(|k| k.tok()).filter(|t| matches!(t.kind, Kind::Lower | Kind::Upper | Kind::Kw)).collect();
                }
                let es = node.child_nodes("expr");
                if ks.iter().any(|k| k.kind == Kind::Upper) {
                    for e in es {
                        self.infer(Some(e), env, None);
                        self.settle_element();
                    }
                    return unknown("variant map");
                }
                let exp = expected.map(peel_optional);
                let em: BTreeMap<&str, &Type> = match exp {
                    Some(Type::Record(fs)) => fs.iter().map(|(k, t)| (k.as_str(), t)).collect(),
                    _ => BTreeMap::new(),
                };
                let mut fields = Vec::new();
                for (k, e) in ks.iter().zip(es.iter()) {
                    let child = em.get(k.text.as_str()).copied();
                    let t = self.infer(Some(*e), env, child);
                    self.settle_element();
                    fields.push((k.text.clone(), t));
                }
                return Type::Record(fields);
            }
            "argList" => {
                for a in node.find_all("expr") {
                    self.infer(Some(a), env, None);
                }
                return void();
            }
            _ => {}
        }
        for c in kids {
            if let C::N(k) = c {
                self.infer(Some(k), env, None);
            }
        }
        unknown(n)
    }

    fn await_(&mut self, optok: &Token, operand: C<'a>, env: &Env, t: &Type, s: &Type) -> Type {
        if self.defer_depth > 0 {
            self.def_err("E-DEF-001", "`await` inside a deferred block: `defer await` is not in v1 (12.1)".into(), optok, "await", "LANG-12-01");
        }
        for (gname, gdepth, gtok) in self.guards.clone() {
            if gdepth <= self.depth && env_lookup(env, &gname).is_some() && !self.own.is_moved(&gname) {
                self.rec(
                    "E-ASY-002",
                    format!("the MutexGuard `{gname}` (line {}) is live across this await: an exclusive guard may not be held across a suspension (12.3)", gtok.line),
                    "LANG-12-06",
                    optok,
                    Json::obj([("offending_statement", Json::str("await")), ("rule_reference", Json::str("12.3"))]),
                );
            }
        }
        self.obs_exit("cancellation", optok, &format!("the await at line {} (a cancellation point)", optok.line));
        // the awaited value is consumed by the await, not live across it (X-123)
        let awaited = self.path_of_expr(operand.node());
        self.unwind_point(optok, "await", env, awaited.as_deref());
        self.eff.add("suspend", "proved", Some(optok));
        // the future awaited here lends its temporaries no further, unless what it gives can still carry them (a
        // future, a task, a loan); another future's stay (D-31)
        let onward = match strip(t) {
            Type::Named(n) if (n.name == "Future" || n.name == "Task") && !n.args.is_empty() => self.carries_onward(&n.args[0]),
            _ => false,
        };
        let ots: Vec<Token> = if onward { vec![] } else { match operand {
            C::N(n) => n.tokens().into_iter().cloned().collect(),
            C::T(t) => vec![t.clone()],
        } };
        if let (Some(a), Some(b)) = (ots.first(), ots.last()) {
            let (lo, hi) = ((a.line, a.col), (b.line, b.col));
            self.carried_temps.retain(|(tk, _)| !(lo <= (tk.line, tk.col) && (tk.line, tk.col) <= hi));
        }
        let keep_future = matches!(strip(t), Type::Named(n) if n.name == "Future" && !n.args.is_empty() && self.loan_carrying(&n.args[0]));
        for l in self.carried.clone() {
            if self.own.has_loan(&l) && !keep_future {
                self.own.remove_loan(&l);
            }
        }
        let own = &self.own;
        self.carried.retain(|l| own.has_loan(l));
        if let Type::Named(n) = s {
            if (n.name == "Future" || n.name == "Task") && !n.args.is_empty() {
                let ap = self.path_of_expr(operand.node());
                if let Some(ap) = &ap {
                    // awaiting consumes the future (12.3) or the task handle (12.5): the place must own it, and is moved
                    // (a loan or a Shared owns nothing to consume; a second await is a use after move; D-33)
                    let root = root_of(ap);
                    let lent = self.own.holders.get(&root).map(|h| h.borrow().owner.clone()).filter(|o| root_of(o) != root);
                    match lent {
                        Some(owner) => self.own_err(
                            "E-OWN-001",
                            format!("`await {ap}`: `{root}` holds a loan of `{owner}` and owns no future to consume (12.3)"),
                            ctok(operand),
                            ap,
                            "await of a loan",
                            "LANG-07-01",
                            "",
                        ),
                        None => self.use_place(Some(ap), ctok(operand), "move", env),
                    }
                    self.own.move_(ap);
                }
                self.consume(ap, ctok(operand).line, "awaited");
                return n.args[0].clone();
            }
        }
        if known(s) {
            self.typ("E-TYP-040", format!("`await` on {}, which is not a Future or a Task", t.show()), ctok(operand), "Future[T] or Task[T]", &t.show(), "await", "LANG-06-01");
        }
        unknown("await")
    }

    /// `a + b` as `Int.add(a, b) or fail` in a `try` function (Programme 7.1: the explicit checked alternative).
    fn checked_alternative(&self, node: N<'a>, op: &Token) -> Option<Json> {
        if !self.in_try || self.ctx.iter().any(|c| c == "lambda") {
            return None;
        }
        let f = match op.text.as_str() {
            "+" => "add",
            "-" => "sub",
            "*" => "mul",
            _ => return None,
        };
        let kids = node.kids();
        let toks: Vec<&Token> = node.tokens().into_iter().filter(|t| t.kind != Kind::Nl).collect();
        let (first, last) = (toks.first()?, toks.last()?);
        if first.line != last.line {
            return None;
        }
        let l = crate::typer::expr::source_text(kids[0]);
        let r = crate::typer::expr::source_text(kids[2]);
        let file = self.dc.file.clone();
        Some(crate::decls::correction("use the checked operation", vec![crate::decls::edit(&file, first.line, first.col, crate::decls::tok_end(last), &format!("Int.{f}({l}, {r}) or fail"))]))
    }

    pub fn arith(&mut self, op: &Token, l: &Type, r: &Type) -> Type {
        let ls = strip(l).clone();
        let rs = strip(r).clone();
        if !known(&ls) || !known(&rs) {
            return unknown("arithmetic");
        }
        let lb = base_name(&ls).map(|s| s.to_string());
        let rb = base_name(&rs).map(|s| s.to_string());
        if let (Some(lb), Some(rb)) = (&lb, &rb) {
            if is_int_type(lb) && is_int_type(rb) {
                if lb != rb {
                    self.typ("E-TYP-040", format!("`{}` on {lb} and {rb}: distinct integer types do not mix", op.text), op, lb, rb, &op.text, "LANG-06-01");
                    return unknown("");
                }
                let (a, b) = (self.interval_of(&ls), self.interval_of(&rs));
                return self.int_arith(op, &nt(lb), a, b);
            }
            if ["+", "-", "*", "/"].contains(&op.text.as_str()) && lb == rb && (lb == "Float" || lb == "Float32") {
                return named(lb);
            }
        }
        let lbs = lb.clone().unwrap_or_default();
        let rbs = rb.clone().unwrap_or_default();
        let hint = if lbs == "Decimal" || rbs == "Decimal" {
            " (use Decimal.add and friends)"
        } else if lbs == "Duration" || rbs == "Duration" {
            " (use Duration.add and friends)"
        } else if lbs == "String" || lbs == "StringView" {
            " (text is built with text.Builder)"
        } else {
            ""
        };
        self.typ("E-TYP-040", format!("`{}` is not defined on {} and {}{hint}", op.text, ls.show(), rs.show()), op, "integer or float operands", &format!("{}, {}", ls.show(), rs.show()), &op.text, "LANG-06-01");
        unknown("")
    }

    pub fn int_arith(&mut self, op: &Token, base: &NamedT, a: Option<iv::Interval>, b: Option<iv::Interval>) -> Type {
        let (a, b) = match (a, b) {
            (Some(a), Some(b)) => (a, b),
            _ => return Type::Named(base.clone()),
        };
        let full = int_types(&base.name).unwrap();
        let width: Int = match base.name.as_str() {
            "Int8" | "UInt8" | "Byte" => 8,
            "Int16" | "UInt16" => 16,
            "Int32" | "UInt32" => 32,
            _ => 64,
        };
        let o = op.text.as_str();
        if ["&", "|", "^"].contains(&o) {
            return Type::Named(base.clone());
        }
        let checked = match o {
            "+" => "Int.add",
            "-" => "Int.sub",
            "*" => "Int.mul",
            "/" => "Int.div",
            "%" => "Int.rem",
            "<<" => "Int.shiftLeft",
            _ => "a checked operation",
        };
        let r = iv::arith(o, a, b, width);
        let r = match r {
            None => {
                let why = if o == "/" || o == "%" { "the divisor may be zero".to_string() } else { format!("the shift amount is not proved within [0, {}]", width - 1) };
                let status = if (o == "/" || o == "%") && b == (0, 0) { "violated" } else { "unproved" };
                let req = if o == "/" || o == "%" { "divisor excluding 0".to_string() } else { format!("[0, {}]", width - 1) };
                self.ref_(
                    "E-REF-002",
                    format!("`{o}` is not proved safe: {why} (operands {}, {}); use {checked}", Self::show_iv(Some(a)), Self::show_iv(Some(b))),
                    op,
                    status,
                    &req,
                    &Self::show_iv(Some(b)),
                    "",
                );
                return Type::Named(base.clone());
            }
            Some(r) => r,
        };
        if o == "/" && a.0 <= full.0 && b.0 <= -1 && -1 <= b.1 {
            self.ref_(
                "E-REF-002",
                format!("`/` is not proved safe: MIN / -1 cannot be excluded (operands {}, {}); use {checked}", Self::show_iv(Some(a)), Self::show_iv(Some(b))),
                op,
                "unproved",
                &Self::show_iv(Some(full)),
                &Self::show_iv(Some(a)),
                "",
            );
            return Type::Named(base.clone());
        }
        if r.0 < full.0 || r.1 > full.1 {
            let status = if r.1 < full.0 || r.0 > full.1 { "violated" } else { "unproved" };
            self.ref_(
                "E-REF-002",
                format!("`{o}` may overflow {}: operands {} and {} give {}; use {checked}", base.name, Self::show_iv(Some(a)), Self::show_iv(Some(b)), Self::show_iv(Some(r))),
                op,
                status,
                &Self::show_iv(Some(full)),
                &Self::show_iv(Some(r)),
                "",
            );
            return Type::Named(base.clone());
        }
        refined(base.clone(), Some(r.0), Some(r.1), "range")
    }

    pub fn compare(&mut self, op: &Token, l: &Type, r: &Type) {
        let ls = concrete(strip(l));
        let rs = concrete(strip(r));
        if !known(&ls) || !known(&rs) {
            return;
        }
        if op.text == "==" || op.text == "!=" {
            if (ls.is_optional() && rs == null_t()) || (rs.is_optional() && ls == null_t()) {
                return;
            }
            let a = ls.opt_inner().cloned().unwrap_or_else(|| ls.clone());
            let b = rs.opt_inner().cloned().unwrap_or_else(|| rs.clone());
            if ["Float", "Float32"].contains(&base_name(&a).unwrap_or("")) && base_name(&b) == base_name(&a) {
                return;
            }
            let (ok, _) = assignable(&a, &b);
            let (ok2, _) = assignable(&b, &a);
            if ok == Some(false) && ok2 == Some(false) {
                let rule = if ls.is_optional() != rs.is_optional() { "LANG-06-08" } else { "LANG-06-01" };
                self.typ("E-TYP-040", format!("`{}` compares {} with {}", op.text, ls.show(), rs.show()), op, &ls.show(), &rs.show(), &op.text, rule);
                return;
            }
            if !self.dc.implements(&a, "Equatable") {
                self.typ("E-TYP-040", format!("`{}` needs Equatable; {} does not implement it", op.text, a.show()), op, "Equatable", &a.show(), &op.text, "LANG-06-01");
            }
            return;
        }
        let (a, b) = (ls, rs);
        if ["Float", "Float32"].contains(&base_name(&a).unwrap_or("")) && base_name(&b) == base_name(&a) {
            return;
        }
        let (ok, _) = assignable(&a, &b);
        let (ok2, _) = assignable(&b, &a);
        if ok == Some(false) && ok2 == Some(false) {
            self.typ("E-TYP-040", format!("`{}` compares {} with {}", op.text, a.show(), b.show()), op, &a.show(), &b.show(), &op.text, "LANG-06-01");
            return;
        }
        if !self.dc.implements(&a, "Comparable") {
            self.typ("E-TYP-040", format!("`{}` needs Comparable; {} does not implement it", op.text, a.show()), op, "Comparable", &a.show(), &op.text, "LANG-06-01");
        }
    }

    pub fn outcome(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let kids = node.kids();
        let words: Vec<String> = kids.iter().filter_map(|k| k.tok()).map(|t| t.text.clone()).collect();
        let left = self.infer_c(kids[0], env, if kids.len() == 1 { expected } else { None });
        if kids.len() == 1 {
            return left;
        }
        let ls = strip(&left).clone();
        let inner = match &ls {
            Type::Named(n) if n.name == "Outcome" && !n.args.is_empty() => n.args[0].clone(),
            _ => {
                if known(&ls) {
                    self.typ("E-TYP-040", format!("`{}` needs an Outcome left operand, got {}", words[..words.len().min(2)].join(" "), ls.show()), ctok(kids[0]), "Outcome[T]", &ls.show(), &words[0], "LANG-06-08");
                    return unknown("");
                }
                unknown("outcome")
            }
        };
        let lp = self.path_of_expr(kids[0].node());
        self.consume(lp, ctok(kids[0]).line, "handled");
        let or_tok = kids.iter().filter_map(|k| k.tok()).find(|t| t.text == "or").cloned();
        if words.iter().any(|w| w == "fail") {
            if self.defer_depth > 0 {
                self.def_err("E-DEF-001", "`or fail` inside a deferred block: there is no outcome to propagate to (12.1)".into(), or_tok.as_ref().unwrap(), "or fail", "LANG-12-01");
            }
            self.eff.add("fail", "proved", Some(ctok(kids[0])));
            let ortok = or_tok.unwrap();
            self.journal_exit("or fail", ortok.line);
            self.unwind_point(&ortok, "or fail", env, None);
            self.obs_exit("failure", &ortok, &format!("the `or fail` at line {}", ortok.line));
            if !self.in_try {
                let t0 = ctok(kids[0]);
                let c = if self.ctx.iter().any(|c| c == "lambda") { None } else { self.insert_modifier("make the function `try`", "try", &["recovers", "func"]) };
                self.rec_c(
                    "E-ERR-001",
                    "`or fail` outside a `try` function".into(),
                    "LANG-10-01",
                    &ortok,
                    Json::obj([("obligation_origin", Json::str(format!("{}:{}", t0.line, t0.col))), ("unconsumed_path", Json::str("or fail"))]),
                    c,
                );
            }
            return inner;
        }
        let mut target = inner.clone();
        if let Some(es) = expected.map(|e| strip(e).clone()) {
            if known(&es) && known(&inner) && assignable(&inner, &es).0 == Some(true) {
                target = es;
            }
        }
        let after_left = self.facts.clone(); // the success path's state (D-76)
        let skip_own = self.own.copy(); // the handler runs only on failure (10.2); the outcome itself is already consumed
        if words.iter().any(|w| w == "recover") {
            let last = *kids.last().unwrap();
            self.eff.add("recovers", "proved", Some(ctok(last)));
            let r = self.infer_c(last, env, Some(&target));
            self.facts = after_left.join(&self.facts); // D-76: the replacement runs only on failure
            self.join_conditional(&skip_own);
            if known(&target) {
                if r == null_t() && !strip(&target).is_optional() {
                    return optional(target);
                }
                self.expect(&r, &target, ctok(last), "`or recover` value");
            }
            return target;
        }
        let ident = kids.iter().filter_map(|k| k.tok()).find(|t| t.kind == Kind::Lower).unwrap().clone();
        let e = env_new(Some(env));
        env_set(&e, &ident.text, named("Failure"), false, "catch");
        self.ctx.push("catch".into());
        self.arm_types.push(vec![]);
        self.exit_frames.push((self.depth, vec![]));
        self.branch_expected.push(Some(target.clone()));
        let blk = node.find("block").unwrap();
        let d = self.depth + 1;
        self.own.open_obligation(&ident.text, ident.clone(), d);
        self.catch_bound.push(ident.text.clone());
        if !self.terminates(blk) {
            let ct = kids.iter().filter_map(|k| k.tok()).find(|t| t.text == "catch").unwrap().clone();
            self.obl_err("E-OBL-002", "a catch block must end every path in `recover`, `return`, `fail` or a Never call".into(), &ct, &format!("{}:{}", ident.line, ident.col), &ident.text, "LANG-10-01");
        }
        self.walk_block(blk, &e);
        let recovered = self.exit_frames.pop().map(|f| f.1).unwrap_or_default();
        // A handler that terminates without `recover` leaves the function; it does not continue (10.2).
        let handler_returns = self.terminates(blk) && recovered.is_empty();
        // D-76: the handler runs only on failure, and its value leaves at each `recover`
        let mut paths = vec![after_left];
        paths.extend(recovered);
        if !self.terminates(blk) {
            paths.push(self.facts.clone());
        }
        self.facts = Facts::join_all(&paths);
        self.branch_expected.pop();
        self.catch_bound.pop();
        self.reported.remove(&ident.text);
        self.own.close_obligation(&ident.text, ident.line, "catch block");
        self.own.reinit(&ident.text);
        // A handler that leaves the function (`return`, `fail`) does not continue into this expression,
        // so its consume is not a path of the value (10.2). A `recover` does continue, and so does a
        // handler that fails to terminate. The catch binding itself was closed above.
        if handler_returns {
            self.own = skip_own;
        } else {
            self.join_conditional(&skip_own);
        }
        let rec = self.arm_types.pop().unwrap();
        self.ctx.pop();
        if known(&target) && rec.iter().any(|(t, _)| *t == null_t()) && !strip(&target).is_optional() {
            target = optional(target);
        }
        for (t, tok) in &rec {
            if known(&target) {
                self.expect(t, &target, tok, "`recover` value");
            }
        }
        target
    }

    pub fn marker(&mut self, node: N<'a>, env: &Env) -> Type {
        let kids = node.kids();
        let pl = node.find("place").unwrap();
        self.in_marker = true;
        let mut t = self.place_type(pl, env);
        self.in_marker = false;
        let mtok = kids[0].tokens()[0].clone();
        let mk = mtok.text.clone();
        let members: Vec<N<'a>> = pl.child_nodes("memberName");
        let call_follows = kids.len() > 2 && kids[2].text_is("(") && !members.is_empty();
        let path = if !call_follows {
            self.path_of_place(pl)
        } else {
            let mut parts = vec![pl.direct_tokens()[0].text.clone()];
            for k in &members[..members.len() - 1] {
                parts.push(k.tokens()[0].text.clone());
            }
            parts.join(".")
        };
        self.use_place(Some(&path), &mtok, if mk == "copy" { "copy" } else { &mk }, env);
        if mk == "copy" {
            let base = if !call_follows { strip(&self.declared_type(pl, env)).clone() } else { unknown("") };
            if known(&base) && !self.dc.is_trivial(&base) {
                self.eff.add("alloc", "proved", Some(&mtok));
            }
            if known(&base) && !self.dc.is_copyable(&base) {
                let rule = if matches!(&base, Type::Named(n) if n.name == "Shared" || n.name == "Weak") { "LANG-07-08" } else { "LANG-07-01" };
                self.own_err("E-OWN-002", format!("`copy {path}`: {} is not Copyable (6.11)", base.show()), &mtok, &path, "copy of a non-copyable place", rule, "");
            }
        }
        if kids.len() > 2 {
            self.receiver = Some((path.clone(), mk.clone()));
        } else if let Some(mode) = self.arg_mode.clone() {
            let mut ok = self.use_ok;
            if mk == "move" || mk == "copy" {
                if mode == "editable" {
                    self.own_err("E-OWN-001", format!("an `editable` parameter takes `editable {path}`, not `{mk}`"), &mtok, &path, "wrong marker", "LANG-07-01", "");
                    ok = false;
                } else if mode == "loan" {
                    self.own_err("E-OWN-001", format!("a stored loan takes `alias {path}` or `editable {path}`, not `{mk}`"), &mtok, &path, "wrong marker", "LANG-07-06", "");
                    ok = false;
                }
                if mk == "move" && ok {
                    self.reserve(&path, "move", &mtok);
                }
            } else if mk == "editable" {
                if mode == "owned" {
                    self.own_err("E-OWN-001", format!("an `owned` parameter takes `move {path}` or `copy {path}`, not `editable`"), &mtok, &path, "wrong marker", "LANG-07-01", "");
                    ok = false;
                }
                if ok {
                    self.reserve(&path, "editable", &mtok);
                }
            } else {
                if mode == "owned" {
                    self.own_err("E-OWN-001", format!("an `owned` parameter takes `move {path}` or `copy {path}`, not `alias`"), &mtok, &path, "wrong marker", "LANG-07-01", "");
                    ok = false;
                }
                if ok {
                    self.reserve(&path, "alias", &mtok);
                }
            }
        } else {
            self.pending = Some((path.clone(), mk.clone(), mtok.clone()));
        }
        // the call's receiver, narrowed where the marker stands, captured before the loan drops that fact (6.8)
        let mut narrowed_recv: Option<Type> = None;
        if mk == "editable" {
            let saved = std::mem::take(&mut self.facts);
            let mut decl_t = self.root_type(pl.direct_tokens()[0], env);
            let upto = if call_follows { members.len() - 1 } else { members.len() };
            for k in &members[..upto] {
                decl_t = self.member_type(&decl_t, k.tokens()[0]);
            }
            self.facts = saved;
            let decl_t = strip(&decl_t).clone();
            if let Type::Refined { kind, .. } = &decl_t {
                if kind == "length" {
                    self.ref_("E-REF-003", format!("`{path}` has a length refinement and may not be loaned editable"), &mtok, "violated", &decl_t.show(), "", "");
                }
            }
            if call_follows {
                narrowed_recv = Some(self.fact_type(Some(&path), decl_t));
            }
            self.inval_log.push(path.clone());
            self.facts.invalidate(&path);
        }
        if kids.len() == 2 {
            if (mk == "alias" || mk == "editable") && known(&t) && !t.is_loan() {
                return Type::Loan { inner: Box::new(t), mode: mk, origin: pl.direct_tokens()[0].text.clone() };
            }
            return t;
        }
        let mut rest: Vec<C<'a>> = kids[2..].to_vec();
        if rest[0].is("methodCall") {
            t = self.method_call(&t, rest[0].node().unwrap(), env);
            rest = rest[1..].to_vec();
        } else if rest[0].text_is("(") {
            let al = kids.iter().filter_map(|k| k.node()).find(|k| k.name() == "argList");
            if !members.is_empty() {
                let prefix_t = if let Some(n) = narrowed_recv.take() {
                    n // captured before `editable` invalidated the receiver (6.8)
                } else {
                    let mut prefix_t = self.root_type(pl.direct_tokens()[0], env);
                    for k in &members[..members.len() - 1] {
                        prefix_t = self.member_type(&prefix_t, k.tokens()[0]);
                    }
                    self.fact_type(Some(&path), prefix_t) // a receiver narrowed where the marker stands keeps its narrowing (6.8)
                };
                // the place node carries the receiver type the call was checked at, so lowering sees a narrowed `T` (6.8)
                if let Some(nt_) = self.node_types.as_mut() {
                    nt_.insert(pl.id(), prefix_t.clone());
                }
                let last = members.last().unwrap().tokens()[0].clone();
                let msg = format!("`{}`", last.text);
                self.dc.records.retain(|r| !(r.code == "E-NAM-001" && r.line == last.line && r.message.contains(&msg)));
                t = self.method_call_named(&prefix_t, &last, al, env);
            } else if let Type::Fn { kind, .. } = strip(&t).clone() {
                let ft = strip(&t).clone();
                if kind == "fn mut" && mk != "editable" {
                    self.own_err("E-OWN-001", format!("a `fn mut` closure is called `editable {path}(…)`, not `{mk}`"), &mtok, &path, "wrong call marker", "LANG-08-02", "");
                } else if kind == "fn once" && mk != "move" {
                    self.own_err("E-OWN-001", format!("a `fn once` closure is called `move {path}(…)`: it is consumed by the call"), &mtok, &path, "wrong call marker", "LANG-08-02", "");
                }
                let info = self.closures.get(&root_of(&path)).cloned();
                for (cp, cm) in info.as_ref().map(|i| i.captures.clone()).unwrap_or_default() {
                    if cm == "editable" {
                        self.inval_log.push(cp.clone());
                        self.facts.invalidate(&cp);
                    }
                }
                t = self.call_fn(&ft, al, env, "closure", Some(&mtok), info.and_then(|i| i.eff));
            } else {
                self.receiver = None;
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                t = unknown("call");
            }
            rest = rest.into_iter().filter(|r| r.is("postfix")).collect();
        }
        self.receiver = None;
        self.marker_call_field(pl, &mk, &mtok, &rest, env);
        for pf in rest {
            if pf.is("postfix") {
                let pfn = pf.node().unwrap();
                self.temp_field_siblings(&t, pfn);
                t = self.apply_postfix(&t, pfn, env);
            }
        }
        t
    }

    pub fn call_fn(&mut self, ft: &Type, al: Option<N<'a>>, env: &Env, where_: &str, tok: Option<&Token>, known_eff: Option<Summary>) -> Type {
        let (params, ret, modifiers) = match ft {
            Type::Fn { params, ret, modifiers, .. } => (params.clone(), (**ret).clone(), modifiers.clone()),
            _ => return unknown(""),
        };
        let sig = Sig::simple(where_, params.iter().enumerate().map(|(i, p)| (format!("p{i}"), p.clone(), String::new())).collect(), ret, modifiers);
        self.call_decl = None;
        self.call_label = "closure".into();
        let s = known_eff.unwrap_or_else(|| Self::summary_from_modifiers(&sig));
        self.summaries.insert("closure".into(), s);
        let t = match tok {
            Some(t) => t.clone(),
            None => al.and_then(|a| a.first_token()).cloned().unwrap_or_else(|| synth("(", 1, 1)),
        };
        let mut m = BTreeMap::new();
        self.check_call(&sig, al, env, &mut m, where_, &t)
    }

    pub fn place_type(&mut self, pl: N<'a>, env: &Env) -> Type {
        let ts = pl.direct_tokens();
        let mut t = self.root_type(ts[0], env);
        for k in pl.kids().into_iter().skip(1) {
            match k {
                C::N(n) if n.name() == "memberName" => t = self.member_type(&t, n.tokens()[0]),
                C::T(tk) if tk.kind == Kind::Int => {
                    let s = strip(&t).clone();
                    t = match &s {
                        Type::Named(n) if (n.name == "Array" || n.name == "FixedList") && !n.args.is_empty() => n.args[0].clone(),
                        _ => unknown("element"),
                    };
                }
                _ => {}
            }
        }
        let p = self.path_of_place(pl);
        self.fact_type(Some(&p), t)
    }

    pub fn root_type(&mut self, tok: &Token, env: &Env) -> Type {
        if tok.text == "self" {
            return self.self_type.clone().unwrap_or_else(|| unknown("self"));
        }
        if let Some(b) = env_lookup(env, &tok.text) {
            return b.ty;
        }
        if let Some(d) = self.dc.table.get(&tok.text).cloned() {
            if d.kind == "const" {
                if let Some(a) = &d.alias {
                    if known(a) {
                        return a.clone();
                    }
                }
                if let Some(t) = self.inferred_const(&d.module, &d.name) {
                    return t;
                }
                if let Some(t) = self.const_value_type(&d.name) {
                    return t;
                }
                if let Some(a) = &d.alias {
                    return a.clone();
                }
            }
            if d.kind == "func" {
                if let Some(sig) = &d.sig {
                    self.entry_main_ref(&d, tok);
                    if !sig.where_.is_empty() {
                        self.typ("E-TYP-040", format!("`{}` has `where` bounds, which hold at each call (6.10): it is called, never taken as a value; to pass it, wrap the call in a `fn` literal", d.name), tok, &format!("a call of {}", d.name), "a function value", &d.name, "LANG-06-01");
                    }
                    let r = self.result_of(sig, &BTreeMap::new());
                    return Type::Fn { params: sig.params.iter().map(|p| p.1.clone()).collect(), ret: Box::new(r), kind: "fn".into(), modifiers: sig.modifiers.clone(), origin: String::new(), dynamic: false, identity: String::new() };
                }
            }
        }
        unknown("untyped binding")
    }

    pub fn member_type(&mut self, t: &Type, mtok: &Token) -> Type {
        self.member_type_as(t, mtok, false)
    }

    /// `as_value`: a postfix `.member` used as a value, where a method is refused (4.5); a marked call's callee place
    /// (`editable xs.push(10)`) is looked up the same way and is not one
    pub fn member_type_as(&mut self, t: &Type, mtok: &Token, as_value: bool) -> Type {
        let mut s = strip(t).clone();
        if let Type::Named(n) = &s {
            if n.name == "Shared" && !n.args.is_empty() {
                s = strip(&n.args[0]).clone();
            }
        }
        if let Type::Tuple(_) = s {
            return unknown("tuple member");
        }
        if let Type::Record(fs) = &s {
            for (k, ft) in fs {
                if *k == mtok.text {
                    return ft.clone();
                }
            }
            self.rec("E-NAM-001", format!("record has no field `{}`", mtok.text), "LANG-04-05", mtok, Self::nam(&mtok.text, fs.iter().map(|(k, _)| k.clone()).collect()));
            return unknown("");
        }
        let d = match self.decl_of(&s) {
            Some(d) => d,
            None => return unknown("member of unknown"),
        };
        let m = &mtok.text;
        if let Some((ft, _, _)) = d.fields.get(m) {
            let ft = ft.clone();
            self.check_member(&d, m, mtok, "field");
            let rs = self.recv_subst(&s, &d);
            return self.subst(&ft, &rs);
        }
        if d.methods.contains(m) && !as_value {
            return unknown("method value");
        }
        if d.methods.contains(m) {
            // a method is called, never taken as a value (4.5: `value.field` or `value.method(...)`): a value would skip its
            // signature and its `where` bounds (D-171)
            self.typ(
                "E-TYP-040",
                format!("`{m}` is a method of {}: it is called (`.{m}(…)`), never taken as a value; to pass it, wrap the call in a `fn` literal (4.5)", d.name),
                mtok,
                &format!("a call of {}.{m}", d.name),
                "a method value",
                m,
                "LANG-06-01",
            );
            return unknown("method value");
        }
        if (d.kind == "class" || d.kind == "enum") && !d.constants.contains(m) {
            let mut c = d.fields.sorted_keys();
            c.extend(d.methods.sorted_keys());
            self.rec("E-NAM-001", format!("{} has no member `{m}`", d.name), "LANG-04-05", mtok, Self::nam(m, c));
        }
        unknown("member")
    }

    pub fn method_call(&mut self, recv: &Type, mc: N<'a>, env: &Env) -> Type {
        let mtok = mc.find("memberName").unwrap().tokens()[0];
        self.method_call_named(recv, mtok, mc.find("argList"), env)
    }

    pub fn method_call_named(&mut self, recv: &Type, mtok: &Token, al: Option<N<'a>>, env: &Env) -> Type {
        // A marked receiver (`editable p.m(...)`) belongs to this call only: it is handed back on the path that
        // reaches check_call and dropped on every other exit, so it cannot attach to a later call (7.6; X-114).
        let marked = self.receiver.take();
        let mut s = concrete(strip(recv));
        if let Type::Refined { base, .. } = &s {
            s = Type::Named(base.clone());
        }
        if let Type::Named(n) = &s {
            if n.name == "Shared" && !n.args.is_empty() && mtok.text != "retain" && mtok.text != "downgrade" {
                s = strip(&n.args[0]).clone();
            }
        }
        // a method call reads its receiver as T only where a null test, an `&&` conjunct or a diverging guard narrowed
        // the place (6.8); `||` and function calls do not narrow (6.3), so an unnarrowed T? receiver is rejected (D-51.6)
        if let Type::Optional(i) = &s {
            if !matches!(**i, Type::Unknown(_)) {
                self.typ("E-TYP-040", format!("method `{}` of an optional {}: narrow it with a null test, or match on it", mtok.text, s.show()), mtok, &i.show(), &s.show(), &mtok.text, "LANG-06-08");
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                return unknown("");
            }
        }
        // a `dynamic I` receiver's methods are I's (X-124: they were looked up after an early unknown return)
        let d = match &s {
            Type::Dynamic(i) => self.dc.resolve_named(i, None),
            _ => self.decl_of(&s),
        };
        if d.is_none() || !known(&s) {
            if let Some(al) = al {
                self.infer(Some(al), env, None);
            }
            return unknown("method on unknown");
        }
        let d = d.unwrap();
        let m = mtok.text.clone();
        let mut sig = d.methods.get(&m).cloned();
        if sig.is_none() && d.kind == "mixin" {
            sig = d.methods.get(&format!("requires:{m}")).cloned();
        }
        if sig.is_none() {
            for mx in &d.mixins {
                if let Some(md) = self.dc.resolve_named(mx, None) {
                    if let Some(ms) = md.methods.get(&m) {
                        sig = Some(ms.clone());
                    }
                }
            }
            for iface in &d.implements {
                if sig.is_none() {
                    if let Some(idecl) = self.dc.resolve_named(iface, None) {
                        if let Some(ms) = idecl.methods.get(&m) {
                            sig = Some(ms.clone());
                        }
                    }
                }
            }
            if sig.is_none() && m == "to" {
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                return unknown("to[T]");
            }
            if sig.is_none() && (m == "toText" || m == "toBytes") {
                if !self.dc.implements(&s, "Encodable") {
                    self.typ("E-TYP-090", format!("`{m}` needs {} to be Encodable", s.show()), mtok, "Encodable", &s.show(), &m, "LANG-06-10");
                }
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                self.eff.add("alloc", "proved", Some(mtok));
                return if m == "toText" { string() } else { named("Bytes") };
            }
            if sig.is_none() && ["equals", "compare", "hash", "display", "encode"].contains(&m.as_str()) && (self.dc.implements(&s, "Equatable") || self.dc.implements(&s, "Encodable") || self.dc.implements(&s, "Displayable")) {
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                return match m.as_str() {
                    "equals" => boolean(),
                    "compare" => named("Ordering"),
                    _ => void(),
                };
            }
        }
        let sig = match sig {
            Some(s) => s,
            None => {
                if m == "cleanup" && d.has_cleanup {
                    self.typ("E-TYP-040", format!("`cleanup` of {} runs at destruction and cannot be called directly (9.4)", d.name), mtok, "destruction", "direct call", &m, "LANG-06-01");
                } else if d.fields.contains(&m) {
                    self.typ("E-TYP-040", format!("`{m}` is a field of {}, not a method", d.name), mtok, "a method", "field", &m, "LANG-06-01");
                } else {
                    self.rec("E-NAM-001", format!("{} has no method `{m}`", d.name), "LANG-04-05", mtok, Self::nam(&m, d.methods.sorted_keys()));
                }
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                return unknown("");
            }
        };
        if sig.receiver.is_none() {
            self.typ("E-TYP-040", format!("`{m}` is an associated function of {}: call it as {}.{m}(…)", d.name, d.name), mtok, &format!("{}.{m}", d.name), "method call", &m, "LANG-06-01");
        }
        self.check_member(&d, &m, mtok, "method");
        if matches!(strip(recv), Type::Dynamic(_)) && sig.modifiers.iter().any(|x| x == "async") {
            self.eff.add("alloc", "proved", Some(mtok)); // an async call through `dynamic` boxes its future (C.3; X-122)
        }
        let mut mm = self.recv_subst(&s, &d);
        let rp = self.coll_path.clone();
        let receiver_place = rp.is_some();
        self.call_decl = Some(d.clone());
        self.call_label = self.method_label(&d, &m);
        if d.name == "Scope" && (m == "launchParallel" || m == "launchBlocking") {
            self.check_pool_launch(&m, al, env, mtok);
        }
        self.receiver = marked;
        if self.receiver.is_none() && rp.is_some() && self.is_bearing(recv) && sig.receiver.is_some() {
            let rp = rp.unwrap();
            match sig.receiver.as_deref() {
                Some("editable self") => self.own_err("E-OWN-001", format!("`{m}` takes `editable self`: write `editable {rp}.{m}(…)`"), mtok, &rp, "editable receiver marker required", "LANG-07-01", ""),
                Some("owned self") => self.own_err("E-OWN-001", format!("`{m}` takes `owned self`: write `move {rp}.{m}(…)`"), mtok, &rp, "move receiver marker required", "LANG-07-01", ""),
                _ => self.receiver = Some((rp, "alias".into())),
            }
        } else if let (Some((rpath, rmk)), Some(srecv)) = (self.receiver.clone(), sig.receiver.clone()) {
            if srecv == "editable self" && rmk != "editable" {
                self.own_err("E-OWN-001", format!("`{m}` takes `editable self`: the receiver needs `editable`, not `{rmk}`"), mtok, &rpath, "wrong receiver marker", "LANG-07-01", "");
            } else if srecv == "owned self" && rmk != "move" {
                self.own_err("E-OWN-001", format!("`{m}` takes `owned self`: the receiver needs `move`, not `{rmk}`"), mtok, &rpath, "wrong receiver marker", "LANG-07-01", "");
            } else if srecv == "self" && rmk == "move" {
                self.own_err("E-OWN-001", format!("`{m}` takes a read loan of self: `move` is not permitted on the receiver"), mtok, &rpath, "wrong receiver marker", "LANG-07-01", "");
            }
        }
        // a temporary receiver of an async read method is lent to the future (12.3; D-31)
        let temp_receiver = match &self.receiver {
            None => !receiver_place,
            Some((_, mk)) => mk == "copy",
        };
        if sig.has_mod("async") && sig.receiver.as_deref() == Some("self") && self.is_bearing(recv) && temp_receiver {
            self.carried_temps.push((mtok.clone(), self.call_depth + 1));
        }
        let where_ = format!("{}.{m}", d.name);
        self.pending_owner_where = d.where_.clone();
        if self.result_expected.is_some() {
            self.pending_call_expected = self.result_expected.clone(); // copied here, after every early return (8.4)
        }
        let t = self.check_call(&sig, al, env, &mut mm, &where_, mtok);
        if d.name == "Scope" && (m == "launch" || m == "launchParallel") {
            // a future launched directly owns its call's own temporaries (its arguments and receiver): they live as
            // long as the task; a nested future's are not the launched call's (12.3, X-251)
            if let Some(al) = al {
                let ats = al.tokens();
                if let (Some(a), Some(b)) = (ats.first(), ats.last()) {
                    let (lo, hi) = ((a.line, a.col), (b.line, b.col));
                    let launched = self.call_depth + 2;
                    self.carried_temps.retain(|(tk, d)| !(*d == launched && lo <= (tk.line, tk.col) && (tk.line, tk.col) <= hi));
                }
            }
        }
        t
    }

    /// `move make().a` marks the function name. A function is not a place, and neither is its result (7.2).
    fn marker_call_field(&mut self, pl: N<'a>, mk: &str, mtok: &Token, rest: &[C<'a>], env: &Env) {
        let has_field = rest.iter().any(|c| c.node().map(|n| Self::taken_field(n).is_some()).unwrap_or(false));
        if !has_field {
            return;
        }
        let root = pl.direct_tokens()[0].text.clone();
        if root == "self" || self.binding_kind(&root, env).is_some() {
            return;
        }
        let path = self.path_of_place(pl);
        let func = self.dc.table.get(&path).or_else(|| self.dc.table.get(&root)).map(|d| d.kind == "func").unwrap_or(false);
        if !func {
            return;
        }
        self.own_err("E-OWN-001", format!("`{mk} {root}` applies a marker to `{root}`, which is not a place; a call result is not a place either (7.2)"), mtok, &root, "marker on a non-place", "LANG-07-01", "");
    }

    /// The field a `.` or `?.` postfix projects, and the token that names it.
    fn taken_field(pf: N<'a>) -> Option<(String, &'a Token)> {
        let kids = pf.kids();
        if kids.len() < 2 || !(kids[0].text_is(".") || kids[0].text_is("?.")) {
            return None;
        }
        let second = kids[1];
        if second.is("memberName") {
            let tok = second.node()?.tokens().into_iter().next()?;
            return Some((tok.text.clone(), tok));
        }
        if let C::T(st) = second {
            if st.kind == Kind::Int {
                return Some((st.text.clone(), st));
            }
        }
        None
    }

    /// Fields of a class, a record or a tuple, in declaration order. A field type is substituted.
    fn aggregate_fields(&self, recv: &Type) -> Vec<(String, Type)> {
        let s = strip(recv);
        match s {
            Type::Tuple(items) => items.iter().enumerate().map(|(i, t)| (i.to_string(), t.clone())).collect(),
            Type::Record(fs) => fs.clone(),
            _ => {
                let Some(d) = self.decl_of(s) else { return vec![] };
                if d.kind != "class" {
                    return vec![];
                }
                let mm = self.recv_subst(s, &d);
                d.fields.iter().map(|(f, (ft, _, _))| (f.clone(), self.subst(ft, &mm))).collect()
            }
        }
    }

    /// Projecting a field from a value that is not a place drops the other fields at the end of the statement
    /// (7.9). A linear one has no consumer (10.2). The projected field itself is the expression's result.
    fn temp_field_siblings(&mut self, recv: &Type, pf: N<'a>) {
        let Some((taken, tok)) = Self::taken_field(pf) else { return };
        // `?.` projects a field of the value inside the optional. One layer, as the member lookup does.
        let base = if pf.kids().first().is_some_and(|c| c.text_is("?.")) {
            let Some(inner) = strip(recv).opt_inner() else { return };
            inner
        } else {
            recv
        };
        for (fname, ft) in self.aggregate_fields(base) {
            if fname == taken || !self.is_linear(&ft) {
                continue;
            }
            self.obl_err("E-OBL-007", format!("projecting `{taken}` from this temporary drops field `{fname}` ({}), which is linear", ft.show()), tok, &fname, &fname, "LANG-10-02");
        }
    }

    pub fn apply_postfix(&mut self, t: &Type, pf: N<'a>, env: &Env) -> Type {
        let kids = pf.kids();
        if pf.name() == "methodCall" {
            return self.method_call(t, pf, env);
        }
        if let Some(C::N(k0)) = kids.first() {
            if k0.name() == "methodCall" {
                return self.method_call(t, *k0, env);
            }
        }
        let first = kids[0];
        if first.text_is(".") || first.text_is("?.") {
            let second = kids[1];
            if second.is("memberName") {
                let mt = second.node().unwrap().tokens()[0];
                let s = strip(t).clone();
                if first.text_is("?.") {
                    if known(&s) && !s.is_optional() {
                        self.typ("E-TYP-040", format!("`?.` on {}, which is not optional", s.show()), mt, "T?", &s.show(), "?.", "LANG-06-08");
                        return unknown("");
                    }
                    let base = s.opt_inner().cloned().unwrap_or_else(|| s.clone());
                    let r = self.member_type_as(&base, mt, true);
                    return if r.is_optional() || !known(&r) { r } else { optional(r) };
                }
                if let Type::Optional(i) = &s {
                    self.typ("E-TYP-040", format!("member `{}` of an optional {}: use `?.` or match on it", mt.text, s.show()), mt, &i.show(), &s.show(), &mt.text, "LANG-06-08");
                    return unknown("");
                }
                return self.member_type_as(&s, mt, true);
            }
            if let C::T(st) = second {
                if st.kind == Kind::Int {
                    let s = strip(t).clone();
                    if let Type::Tuple(items) = &s {
                        let i: usize = st.text.parse().unwrap_or(usize::MAX);
                        if i < items.len() {
                            return items[i].clone();
                        }
                        self.typ("E-TYP-040", format!("tuple {} has no field .{i}", s.show()), st, &format!(".0 to .{}", items.len() as i64 - 1), &format!(".{i}"), &format!(".{i}"), "LANG-06-01");
                        return unknown("");
                    }
                    if known(&s) {
                        self.typ("E-TYP-040", format!(".{} on {}, which is not a tuple", st.text, s.show()), st, "a tuple", &s.show(), &format!(".{}", st.text), "LANG-06-01");
                    }
                    return unknown("tuple field");
                }
            }
        }
        if first.text_is("(") {
            let ft = first.tok().unwrap().clone();
            let al = pf.find("argList");
            let s = strip(t).clone();
            if let Type::Fn { kind, .. } = &s {
                if let Some(cp) = self.coll_path.clone() {
                    if kind == "fn mut" {
                        self.own_err("E-OWN-001", format!("a `fn mut` closure is called `editable {cp}(…)`"), &ft, &cp, "editable call marker required", "LANG-08-02", "");
                    } else if kind == "fn once" {
                        self.own_err("E-OWN-001", format!("a `fn once` closure is called `move {cp}(…)` and consumed"), &ft, &cp, "move call marker required", "LANG-08-02", "");
                    }
                }
                return self.call_fn(&s, al, env, "closure", Some(&ft), None);
            }
            if let Some(al) = al {
                self.infer(Some(al), env, None);
            }
            if known(&s) {
                self.typ("E-TYP-040", format!("call of a value of type {}", s.show()), &ft, "a function type", &s.show(), "call", "LANG-06-01");
            }
            return unknown("call");
        }
        if first.text_is("[") {
            let ft = first.tok().unwrap().clone();
            let e = pf.find("expr").unwrap();
            let cp = self.coll_path.clone();
            let it = self.infer(Some(e), env, None);
            self.coll_path = cp;
            let s = strip(t).clone();
            if known(strip(&it)) && int_bounds(strip(&it)).is_none() {
                self.typ("E-TYP-040", format!("index has type {}, expected an integer", it.show()), ftok(e), "Int", &it.show(), "index", "LANG-06-01");
            }
            if let Type::Named(n) = &s {
                if ["List", "ListView", "Array", "FixedList"].contains(&n.name.as_str()) && !n.args.is_empty() {
                    self.index_proof(t, n, &it, e, &ft);
                    return n.args[0].clone();
                }
                if ["Map", "OrderedMap", "FixedMap"].contains(&n.name.as_str()) && n.args.len() >= 2 {
                    return optional(n.args[1].clone());
                }
            }
            if known(&s) {
                self.typ("E-TYP-040", format!("indexing a value of type {}", s.show()), &ft, "a list, array or map", &s.show(), "index", "LANG-06-01");
            }
            return unknown("index");
        }
        unknown("postfix")
    }

    pub fn index_proof(&mut self, coll_t: &Type, s: &NamedT, it: &Type, e: N<'a>, tok: &Token) {
        let ii = match self.interval_of(it) {
            Some(i) => i,
            None => return,
        };
        let ip = self.path_of_expr(Some(e));
        if ii.0 < 0 {
            self.ref_(
                "E-REF-002",
                format!("index interval {} is not proved non-negative; test `i >= 0` or use get/at", Self::show_iv(Some(ii))),
                tok,
                if ii.1 >= 0 { "unproved" } else { "violated" },
                "[0, length)",
                &Self::show_iv(Some(ii)),
                "",
            );
            return;
        }
        let mut upper: Option<Int> = None;
        if (s.name == "Array" || s.name == "FixedList") && s.args.len() > 1 {
            if let Type::Named(a) = &s.args[1] {
                if let Some(n) = a.name.strip_prefix('#') {
                    upper = n.parse().ok();
                }
            }
        }
        if let Type::Refined { kind, hi: Some(h), .. } = coll_t {
            if kind == "length" {
                upper = Some(*h);
            }
        }
        if let Some(u) = upper {
            if ii.1 < u {
                return;
            }
            self.ref_("E-REF-002", format!("index interval {} is not below the length {u}", Self::show_iv(Some(ii))), tok, if ii.0 < u { "unproved" } else { "violated" }, &format!("[0, {u})"), &Self::show_iv(Some(ii)), "");
            return;
        }
        let cp = self.coll_path.clone();
        if let (Some(ip), Some(cp)) = (&ip, &cp) {
            if self.facts.below.get(ip) == Some(cp) {
                return;
            }
        }
        self.ref_(
            "E-REF-002",
            format!(
                "index is not proved below the length: no `{} < {}.count()` fact is in force; use get/at or test the bound",
                ip.clone().unwrap_or("i".into()),
                cp.clone().unwrap_or("list".into())
            ),
            tok,
            "unproved",
            "[0, length)",
            &Self::show_iv(Some(ii)),
            "",
        );
    }

    pub fn postfix_chain(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let saved_result = self.result_expected.clone();
        let t = self.postfix_chain_at(node, env, expected);
        self.result_expected = saved_result;
        t
    }

    fn postfix_chain_at(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let kids = node.kids();
        let prim = kids[0].node().unwrap();
        let pk = prim.kids();
        if pk.len() == 1 && kids.len() > 1 {
            if let C::T(t0) = pk[0] {
                if t0.kind == Kind::Lower && self.aliases.contains_key(&t0.text) && env_lookup(env, &t0.text).is_none() {
                    if let Some(mn) = kids[1].node().and_then(|k| k.find("memberName")) {
                        let mtok = mn.tokens()[0];
                        let d = self.dc.table.get(&format!("{}.{}", t0.text, mtok.text)).cloned();
                        let exported = || {
                            let prefix = format!("{}.", t0.text);
                            let mut cands: Vec<String> = self.dc.table.decls.iter().filter(|(k, v)| k.starts_with(&prefix) && v.exported).map(|(k, _)| k.splitn(2, '.').nth(1).unwrap().to_string()).collect();
                            cands.sort();
                            cands
                        };
                        match &d {
                            Some(d) if !d.exported && d.kind != "unknown" => {
                                let cands = exported();
                                self.rec("E-NAM-004", format!("{} does not export `{}`", self.aliases[&t0.text], mtok.text), "LANG-04-05", mtok, Self::nam(&mtok.text, cands));
                            }
                            // a member the module does not declare (D-36), reported as the same name in a type position
                            // is (fortis-names); a module the corpus does not contain has no declarations to judge against
                            None if self.dc.library().modules.contains_key(&self.aliases[&t0.text]) => {
                                let cands = exported();
                                self.rec("E-NAM-001", format!("{} declares no {}", self.aliases[&t0.text], mtok.text), "LANG-04-03", mtok, Self::nam(&mtok.text, cands));
                            }
                            _ => {}
                        }
                        let mut t = unknown("module member");
                        if let Some(d) = &d {
                            self.entry_main_ref(d, mtok);
                        }
                        let k1 = kids[1].node().unwrap();
                        let al = k1.find("argList");
                        let is_call = k1.name() == "postfix" && k1.kid(0).map_or(false, |c| c.is("methodCall"));
                        match &d {
                            Some(d) if d.kind == "func" && d.sig.is_some() && is_call => {
                                let sig = d.sig.clone().unwrap();
                                // every call names its callee (SP-12): an alias call never inherits an earlier call's
                                self.call_decl = Some(d.clone());
                                self.call_label = self.function_label(d);
                                if kids.len() == 2 {
                                    self.pending_call_expected = expected.cloned(); // the call is the chain's result
                                }
                                t = self.check_call(&sig, al, env, &mut BTreeMap::new(), &format!("{}.{}", t0.text, mtok.text), mtok);
                            }
                            Some(d) if d.kind == "const" => t = self.const_use_type(&d.module, &d.name, d.alias.as_ref()),
                            _ => {
                                if let Some(al) = al {
                                    self.infer(Some(al), env, None);
                                }
                            }
                        }
                        if let Some(nt_) = self.node_types.as_mut() {
                            nt_.insert(k1.id(), t.clone());
                        }
                        for pf in &kids[2..] {
                            let pfn = pf.node().unwrap();
                            self.temp_field_siblings(&t, pfn);
                            t = self.apply_postfix(&t, pfn, env);
                            if let Some(nt_) = self.node_types.as_mut() {
                                nt_.insert(pfn.id(), t.clone());
                            }
                        }
                        return t;
                    }
                }
            }
        }
        if pk.len() == 1 && kids.len() > 1 {
            if let C::T(t0) = pk[0] {
                if t0.kind == Kind::Lower && env_lookup(env, &t0.text).is_none() {
                    let fd = self.dc.table.get(&t0.text).cloned();
                    let first_pf = kids[1].node().map(|k| k.kids()).unwrap_or_default();
                    if let Some(fd) = fd {
                        if fd.kind == "func" && fd.sig.is_some() && !first_pf.is_empty() && first_pf[0].text_is("(") {
                            self.entry_main_ref(&fd, t0);
                            self.call_decl = None;
                            self.call_label = self.function_label(&fd);
                            let sig = fd.sig.clone().unwrap();
                            if kids.len() == 2 {
                                self.pending_call_expected = expected.cloned(); // the call is the chain's result
                            }
                            let mut t = self.check_call(&sig, kids[1].node().unwrap().find("argList"), env, &mut BTreeMap::new(), &t0.text, t0);
                            if let Some(nt_) = self.node_types.as_mut() {
                                nt_.insert(kids[1].node().unwrap().id(), t.clone());
                            }
                            for pf in &kids[2..] {
                                let pfn = pf.node().unwrap();
                                self.temp_field_siblings(&t, pfn);
                                t = self.apply_postfix(&t, pfn, env);
                                if let Some(nt_) = self.node_types.as_mut() {
                                    nt_.insert(pfn.id(), t.clone());
                                }
                            }
                            return t;
                        }
                    }
                }
            }
        }
        let mut t = self.primary(prim, env, if kids.len() == 1 { expected } else { None });
        if let Some(nt_) = self.node_types.as_mut() {
            nt_.insert(prim.id(), t.clone());
        }
        let mut path = self.path_of_expr(Some(prim));
        let posts = &kids[1..];
        for (i, pf) in posts.iter().enumerate() {
            // only the last call's result is the chain's result; an earlier `List.of` must not see it (8.4)
            self.result_expected = if i + 1 == posts.len() { expected.cloned() } else { None };
            let pfn = pf.node().unwrap();
            self.coll_path = path.clone();
            let is_mc = pfn.name() == "methodCall" || pfn.kid(0).map_or(false, |c| c.is("methodCall"));
            if path.is_some() && !self.in_marker && is_mc {
                self.use_place(path.as_deref(), ftok(prim), "read", env);
            }
            if path.is_none() {
                self.temp_field_siblings(&t, pfn);
            }
            t = self.apply_postfix(&t, pfn, env);
            let k = pfn.kids();
            if path.is_some() && !k.is_empty() && k[0].text_is(".") && k.len() > 1 && k[1].is("memberName") {
                path = Some(format!("{}.{}", path.unwrap(), k[1].node().unwrap().tokens()[0].text));
                if !is_mc {
                    t = self.fact_type(path.as_deref(), t); // `d.p.read()` sees `d.p` as the null test narrowed it (6.8)
                }
            } else {
                path = None;
            }
            if let Some(nt_) = self.node_types.as_mut() {
                nt_.insert(pfn.id(), t.clone());
            }
        }
        self.coll_path = None;
        let full = self.path_of_expr(Some(node));
        if full.is_some() && !self.in_marker {
            self.use_place(full.as_deref(), ftok(node), "read", env);
        }
        self.fact_type(full.as_deref(), t)
    }

    /// `-e` (5.6): a numeric operand, its exact value or interval negated.
    pub fn negated_type(&mut self, optok: &Token, operand: C<'a>, t: &Type) -> Type {
        let s = strip(t).clone();
        if known(&s) && int_bounds(&s).is_none() && !["Float", "Float32", "Decimal", "Duration"].contains(&base_name(&s).unwrap_or("")) {
            self.typ("E-TYP-040", format!("unary `-` on {}", t.show()), ctok(operand), "a numeric type", &t.show(), "-", "LANG-06-01");
        }
        if let Type::Refined { kind, .. } = &s {
            if kind == "exact" {
                return negate(&s);
            }
        }
        if let Some(i) = self.interval_of(&s) {
            let base = match &s {
                Type::Refined { base, .. } => base.clone(),
                Type::Named(n) => n.clone(),
                _ => return t.clone(),
            };
            let st = synth("-", optok.line, optok.col);
            return self.int_arith(&st, &base, Some((0, 0)), Some(i));
        }
        t.clone()
    }

    /// The arguments of an argument list: `arg` in an expression, `constArg` in a constant expression (D-149).
    pub fn arg_nodes(al: Option<N<'a>>) -> Vec<N<'a>> {
        al.map(|a| a.kids().into_iter().filter_map(|k| k.node()).filter(|k| k.name() == "arg" || k.name() == "constArg").collect()).unwrap_or_default()
    }

    /// An argument's expression: `expr`, or a constant argument's `constExpr`.
    pub fn arg_expr(a: N<'a>) -> Option<N<'a>> {
        if a.name() == "arg" {
            a.find("expr")
        } else {
            a.child_node("constExpr")
        }
    }

    /// A term of a constant expression (grammar constTerm: a constant, a field or parameter default), typed like the
    /// expression it spells; before, every one was left untyped (D-149).
    pub fn const_term(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let kids = node.kids();
        match kids[0] {
            C::T(first) => {
                match first.text.as_str() {
                    "null" => return null_t(),
                    "(" => return self.infer(node.child_node("constExpr"), env, expected),
                    "-" => {
                        let t = self.infer_c(kids[1], env, expected);
                        return self.negated_type(first, kids[1], &t);
                    }
                    "{" => {
                        let mut ks: Vec<&Token> = kids.iter().filter(|k| k.is("ident")).map(|k| ctok(*k)).collect();
                        if ks.is_empty() {
                            ks = kids.iter().filter_map(|k| k.tok()).filter(|t| matches!(t.kind, Kind::Lower | Kind::Upper | Kind::Kw)).collect();
                        }
                        let es = node.child_nodes("constExpr");
                        if ks.iter().any(|k| k.kind == Kind::Upper) {
                            for e in es {
                                self.infer(Some(e), env, None);
                            }
                            return unknown("variant map");
                        }
                        let mut fields = Vec::new();
                        for (k, e) in ks.iter().zip(es.iter()) {
                            let t = self.infer(Some(*e), env, None);
                            fields.push((k.text.clone(), t));
                        }
                        return Type::Record(fields);
                    }
                    _ => {}
                }
                if first.kind == Kind::Lower {
                    let ms: Vec<&Token> = kids.iter().filter_map(|k| k.node()).filter(|k| k.name() == "memberName").map(|k| k.tokens()[0]).collect();
                    if ms.is_empty() {
                        if let Some(value) = self.const_value_type(&first.text) {
                            return value;
                        }
                        let rt = self.root_type(first, env);
                        return self.fact_type(Some(&first.text), rt);
                    }
                    if self.aliases.contains_key(&first.text) && ms.len() == 1 {
                        if let Some(d) = self.dc.table.get(&format!("{}.{}", first.text, ms[0].text)).cloned() {
                            self.entry_main_ref(&d, ms[0]);
                            if d.kind == "const" {
                                return self.const_use_type(&d.module, &d.name, d.alias.as_ref());
                            }
                        }
                    }
                    return unknown("constant path");
                }
                unknown("constant term")
            }
            C::N(n) if n.name() == "literal" => self.infer(Some(n), env, expected),
            C::N(n) if n.name() == "typeRef" => {
                if let Some((tok, what)) = self.nonconstant_term(node) {
                    self.typ("E-TYP-040", format!("`{what}(…)` is not a constant expression (4.2): a constant, a field default or a parameter default calls only constructors, enum variants, the `fortis` package's `Type.of` constructors and a derived `Type.default()`"), &tok, "a constant expression", &format!("a call of {what}"), &what, "LANG-04-02");
                    return unknown("not constant");
                }
                self.type_primary(node, env, expected)
            }
            _ => unknown("constant term"),
        }
    }

    pub fn primary(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let kids = node.kids();
        match kids[0] {
            C::T(first) => {
                if first.text == "self" {
                    return self.self_type.clone().unwrap_or_else(|| unknown("self"));
                }
                if first.kind == Kind::Lower {
                    if self.aliases.contains_key(&first.text) && env_lookup(env, &first.text).is_none() {
                        // a module is not a value: its members are named as `alias.member`, where they are checked
                        // (4.5; D-36: a grouped or bound alias hid its members from that check)
                        let prefix = format!("{}.", first.text);
                        let mut cands: Vec<String> = self.dc.table.decls.iter().filter(|(k, v)| k.starts_with(&prefix) && v.exported).map(|(k, _)| k.splitn(2, '.').nth(1).unwrap().to_string()).collect();
                        cands.sort();
                        let msg = format!("`{}` names the module {}, which is not a value: name a member as `{}.member`", first.text, self.aliases[&first.text], first.text);
                        self.rec("E-NAM-001", msg, "LANG-04-03", first, Self::nam(&first.text, cands));
                        return unknown("module");
                    }
                    let rt = self.root_type(first, env);
                    return self.fact_type(Some(&first.text), rt);
                }
                if first.text == "null" {
                    return null_t();
                }
                if first.text == "present" {
                    let t = self.infer(node.find("expr"), env, None);
                    return optional(t);
                }
                if first.text == "fail" {
                    if self.defer_depth > 0 {
                        self.def_err("E-DEF-001", "`fail` inside a deferred block: a defer consumes its own failures (or recover, catch … recover, report) (12.1)".into(), first, "fail", "LANG-12-01");
                    }
                    let fe = node.find("expr");
                    self.infer(fe, env, None);
                    let fp = self.path_of_expr(fe);
                    if let Some(fp) = &fp {
                        self.use_place(Some(fp), first, "move", env);
                        self.own.move_(fp);
                    }
                    self.consume(fp, first.line, "failed");
                    if !self.in_try {
                        let c = if self.ctx.iter().any(|c| c == "lambda") { None } else { self.insert_modifier("make the function `try`", "try", &["recovers", "func"]) };
                        self.rec_c("E-ERR-001", "`fail` outside a `try` function".into(), "LANG-10-01", first, Json::obj([("obligation_origin", Json::str(format!("{}:{}", first.line, first.col))), ("unconsumed_path", Json::str("fail"))]), c);
                    }
                    self.eff.add("fail", "proved", Some(first));
                    self.journal_exit("fail", first.line);
                    self.unwind_point(first, "fail", env, None);
                    self.obs_exit("failure", first, &format!("the fail at line {}", first.line));
                    return never();
                }
                if first.text == "(" {
                    return self.infer(node.find("expr"), env, expected);
                }
                unknown("primary token")
            }
            C::N(f) => match f.name() {
                "literal" => self.infer(Some(f), env, None),
                "typeRef" => self.type_primary(node, env, expected),
                "lambda" | "matchExpr" | "ifExpr" | "tupleLit" | "recordLit" => self.infer(Some(f), env, expected),
                _ => unknown("primary"),
            },
        }
    }

    /// The enums in scope that declare `vname` as a variant, reachable bare: the table's entries under their own
    /// name (the module's, braced imports', the prelude's). An alias import's enum sits under its `alias.name`
    /// key and does not make the bare name ambiguous (the resolver's `enum_variants` agrees).
    fn variant_owners(&self, vname: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .dc
            .table
            .decls
            .iter()
            .filter(|(k, d)| k.as_str() == d.name.as_str() && d.kind == "enum" && d.variants.contains(vname))
            .map(|(_, d)| d.name.clone())
            .collect();
        out.sort();
        out
    }

    /// The constructor a variant name denotes where a function type is expected (LR 9.7): the payload fields are
    /// positional parameters and the result is that enum. `pure`, so it meets a `pure fn` position.
    fn variant_constructor(&self, d: &DeclRef<'a>, vname: &str, ret: &Type) -> Type {
        let payload = d.variants.get(vname).cloned().unwrap_or_default();
        let mm = self.recv_subst(ret, d);
        let params: Vec<Type> = payload.iter().map(|(_, t)| self.subst(t, &mm)).collect();
        // a variant constructor is a declared function, not a lambda: empty identity, not `dynamic` (8.4)
        Type::Fn { params, ret: Box::new(ret.clone()), kind: "fn".into(), modifiers: vec!["pure".into()], origin: String::new(), dynamic: false, identity: String::new() }
    }

    /// The result of `Enum.Variant` where a function type is expected (LR 9.7). A reference that already carries
    /// type arguments keeps them. When the expected result is this enum, that result is used so its arguments flow
    /// (`Level.Low` where `fn() Level[Int]` is expected). Otherwise the enum itself, and the expected function is
    /// checked against this constructor.
    /// Explicit type arguments that are a different concrete instantiation from the expected result (LR 9.7).
    fn note_ctor_mismatch(&mut self, have: &Type, want: &Type, tok: &Token) {
        let (Type::Named(hs), Type::Named(ws)) = (strip(have), strip(want)) else { return };
        if hs.name != ws.name || hs.args.len() != ws.args.len() || hs.args.is_empty() {
            return;
        }
        let hk: Option<Vec<String>> = hs.args.iter().map(nominal_key).collect();
        let wk: Option<Vec<String>> = ws.args.iter().map(nominal_key).collect();
        if hk.is_none() || wk.is_none() || hk == wk {
            return;
        }
        self.typ(
            "E-TYP-040",
            format!("variant constructor has type {}, expected {}: {} is not {}", hs.show(), ws.show(), hs.show(), ws.show()),
            tok,
            &ws.show(),
            &hs.show(),
            "variant constructor",
            "LANG-06-01",
        );
    }

    fn variant_ctor_result(&self, d: &DeclRef<'a>, tref: &Type, expected_fn: &Type) -> Type {
        // `named_of_decl` keeps a core enum's module, so `money.Currency` is not this package's `Currency` (D-142).
        let Type::Fn { ret, .. } = expected_fn else {
            return named_of_decl(d, vec![]);
        };
        if let Type::Named(tn) = tref {
            if !tn.args.is_empty() || d.type_params.is_empty() {
                return named_of_decl(d, tn.args.clone());
            }
        }
        if let Some(ret_d) = self.decl_of(ret) {
            if ret_d.kind == "enum" && ret_d.name == d.name && ret_d.type_identity() == d.type_identity() && core_origin_of(&ret_d) == core_origin_of(d) {
                return (**ret).clone();
            }
        }
        if d.type_params.is_empty() {
            return named_of_decl(d, vec![]);
        }
        named_of_decl(d, d.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect())
    }

    pub fn type_primary(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let kids = node.kids();
        let tr = kids[0].node().unwrap();
        let al = if node.name() != "constTerm" { node.find("argList") } else { node.child_node("constArgList") };
        let called = kids.iter().any(|k| k.text_is("("));
        let mut member: Option<Token> = None;
        for k in &kids[1..] {
            match k {
                C::N(n) if n.name() == "memberName" => member = Some(n.tokens()[0].clone()),
                C::T(t) if t.kind == Kind::Upper => member = Some((*t).clone()),
                _ => {}
            }
        }
        let t = self.dc.type_of_ref(tr);
        let mut d = self.decl_of(&t);
        let rts = tr.direct_tokens();
        if rts[0].kind == Kind::Lower && rts.len() > 2 {
            // `alias.Type` names the declaration its module alias reaches (4.3), whatever a type of that name elsewhere
            // is: a core type and a single-file program's own type share the empty identity (D-140)
            if let Some(dq) = self.dc.table.get(&format!("{}.{}", rts[0].text, rts[2].text)) {
                if dq.kind != "unknown" {
                    d = Some(dq.clone());
                }
            }
        }
        let tr_tok = tr.tokens().into_iter().find(|x| x.kind == Kind::Upper).unwrap().clone();
        if d.is_none() && member.is_none() {
            if let Type::Named(tn) = &t {
                let es = expected.map(|e| strip(e).clone());
                let ed = es.as_ref().and_then(|e| self.decl_of(e));
                if let Some(ed) = ed {
                    if ed.kind == "enum" && ed.variants.contains(&tn.name) {
                        self.variant_args(&ed, &tn.name, al, env, &tr_tok);
                        return es.unwrap();
                    }
                }
                // a variant name where a function type is expected denotes that enum's constructor (LR 9.7). The
                // result type picks the enum, as an expected enum picks the value form. One owner is unambiguous,
                // so the name denotes that enum's constructor and the expected function is checked against it.
                // Several owners otherwise stay ambiguous (D-253)
                if let Some(Type::Fn { ret, .. }) = &es {
                    if !called {
                        if let Some(ret_d) = self.decl_of(ret) {
                            if ret_d.kind == "enum" && ret_d.variants.contains(&tn.name) {
                                return self.variant_constructor(&ret_d, &tn.name, ret);
                            }
                        }
                        let owners = self.variant_owners(&tn.name);
                        if owners.len() == 1 {
                            if let Some(only) = self.dc.table.get(&owners[0]).cloned() {
                                if only.kind == "enum" && only.variants.contains(&tn.name) {
                                    let own = named_of_decl(&only, only.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect());
                                    return self.variant_constructor(&only, &tn.name, &own);
                                }
                            }
                        }
                    }
                }
                let owners = self.variant_owners(&tn.name);
                if owners.len() > 1 {
                    // no expected type picks the enum and several in scope declare the name: the unprefixed form
                    // is ambiguous (LR 9.7 lets it out only where the expected type is that enum, or the name is
                    // unambiguous; D-253)
                    let listed = if owners.len() == 2 { owners.join(" and ") } else { format!("{} and {}", owners[..owners.len() - 1].join(", "), owners[owners.len() - 1]) };
                    let forms = owners.iter().map(|o| format!("`{o}.{}`", tn.name)).collect::<Vec<_>>().join(" or ");
                    let cands = owners.iter().map(|o| format!("{o}.{}", tn.name)).collect();
                    self.rec("E-NAM-001", format!("`{}` is ambiguous: {listed} declare it as a variant; write {forms}", tn.name), "LANG-04-05", &tr_tok, Self::nam(&tn.name, cands));
                }
            }
        }
        let d = match d {
            Some(d) => d,
            None => {
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                return unknown("type primary");
            }
        };
        if let Some(mb) = &member {
            if (mb.text == "fromText" || mb.text == "fromBytes") && called {
                if !self.dc.implements(&t, "Decodable") {
                    self.typ("E-TYP-090", format!("`{}` needs {} to be Decodable", mb.text, t.show()), mb, "Decodable", &t.show(), &mb.text, "LANG-06-10");
                }
                let inp = if mb.text == "fromText" { string_view() } else { named("BytesView") };
                let sig = Sig::simple(&mb.text, vec![("input".into(), inp, String::new())], t.clone(), vec!["try".into()]);
                self.call_decl = None;
                self.call_label = format!("codec {}", mb.text);
                self.eff.add("alloc", "proved", Some(mb));
                self.pending_call_expected = expected.cloned();
                return self.check_call(&sig, al, env, &mut BTreeMap::new(), &format!("{}.{}", t.show(), mb.text), mb);
            }
            if d.kind == "type" && mb.text == "validate" {
                let repr_t = match &d.alias {
                    Some(Type::Refined { base, .. }) => Type::Named(base.clone()),
                    Some(a) => a.clone(),
                    None => unknown(""),
                };
                let sig = Sig::simple("validate", vec![("value".into(), repr_t, String::new())], named_of_decl(&d, vec![]), vec!["try".into()]);
                self.call_decl = Some(d.clone());
                self.call_label = format!("validate {}", d.name);
                self.pending_call_expected = expected.cloned();
                return self.check_call(&sig, al, env, &mut BTreeMap::new(), &format!("{}.validate", d.name), mb);
            }
        }
        let member = match member {
            None => {
                if d.kind == "class" && called {
                    return self.construct(&d, &t, al, env, &tr_tok);
                }
                if d.kind == "type" && called {
                    if let Some(al) = al {
                        self.infer(Some(al), env, None);
                    }
                    return unknown("newtype call");
                }
                if d.kind == "error" && called {
                    for a in Self::arg_nodes(al) {
                        let ats = a.tokens();
                        let lbl = if ats.len() > 1 && ats[1].text == ":" { Some(ats[0].text.clone()) } else { None };
                        let e = Self::arg_expr(a);
                        let want = lbl.as_ref().and_then(|l| d.payload.get(l)).cloned();
                        let at = if e.is_some() { self.infer(e, env, want.as_ref()) } else { unknown("arg") };
                        if let Some(w) = &want {
                            if known(&at) && known(w) {
                                self.expect(&at, w, ats[0], &format!("payload field `{}` of {}", lbl.clone().unwrap(), d.name));
                            }
                        }
                        let ft_ = want.unwrap_or(at);
                        if known(strip(&ft_)) {
                            if let Type::Named(bn) = base_type(&ft_) {
                                if ["String", "Bytes", "List", "Map", "Set"].contains(&bn.name.as_str()) {
                                    self.eff.add("alloc", "proved", Some(ats[0]));
                                }
                            }
                        }
                    }
                    return named_of_decl(&d, vec![]);
                }
                if !called && !(d.name == "Void" && d.module == "fortis.prelude") {
                    // a type is not a value: there is no first-class type, so `let h = Host` names nothing a program can
                    // use (4.5's qualified forms are `Type.associated` and `Enum.Variant`; D-141). `Void` alone is also its
                    // one value (6.2)
                    self.typ(
                        "E-TYP-040",
                        format!("type `{}` is used as a value: name a value, a constructor call, `{}.associated(…)` or a variant", d.name, d.name),
                        &tr_tok,
                        "a value",
                        &format!("type {}", d.name),
                        &d.name,
                        "LANG-06-01",
                    );
                }
                return unknown("type value");
            }
            Some(m) => m,
        };
        let m = member.text.clone();
        if let Type::Refined { base, .. } = &t {
            if m == "validate" {
                let mode = if !self.is_int(&base.as_type()) { "owned" } else { "" };
                let sig = Sig::simple("validate", vec![("value".into(), base.as_type(), mode.into())], t.clone(), vec!["try".into()]);
                if called {
                    self.call_decl = None;
                    self.call_label = format!("validate {}", t.show());
                    self.pending_call_expected = expected.cloned();
                    return self.check_call(&sig, al, env, &mut BTreeMap::new(), &format!("{}.validate", t.show()), &member);
                }
                return Type::Fn { params: vec![base.as_type()], ret: Box::new(named_args("Outcome", vec![t.clone()])), kind: "fn".into(), modifiers: vec!["try".into()], origin: String::new(), dynamic: false, identity: String::new() };
            }
        }
        if d.kind == "enum" && d.variants.contains(&m) {
            // `Enum.Variant` where a function type is expected denotes the constructor (LR 9.7), as the bare name
            // does. Called, or where the expected type is not a function, it stays the enum value (D-54.5).
            if !called {
                if let Some(es) = expected.map(|e| strip(e).clone()) {
                    if let Type::Fn { ret: want, .. } = &es {
                        let ret = self.variant_ctor_result(&d, &t, &es);
                        // `Level[Int].High` is not `fn(Int) Level[String]`. Assignability leaves two concrete
                        // instantiations unknown, so the let would otherwise be accepted.
                        self.note_ctor_mismatch(&ret, want, &member);
                        return self.variant_constructor(&d, &m, &ret);
                    }
                }
            }
            self.variant_args(&d, &m, al, env, &member);
            return if d.type_params.is_empty() { named_of_decl(&d, vec![]) } else { unknown("generic enum") };
        }
        if let Some(sig) = d.methods.get(&m).cloned() {
            self.check_member(&d, &m, &member, "associated function");
            if sig.receiver.is_some() {
                self.typ("E-TYP-040", format!("`{m}` is a method of {}: call it on a value", d.name), &member, "a value receiver", &format!("{}.{m}", d.name), &m, "LANG-06-01");
            }
            let mut mm: BTreeMap<String, Type> = BTreeMap::new();
            mm.insert("Self".into(), named_of_decl(&d, vec![]));
            if iv::CHECKED_RESULT_INTERVAL && is_int_type(&d.name) && ["add", "sub", "mul", "div", "rem"].contains(&m.as_str()) && al.is_some() && called {
                let args = Self::arg_nodes(al);
                if args.len() == 2 {
                    let a_t = self.infer(Self::arg_expr(args[0]), env, None);
                    let b_t = self.infer(Self::arg_expr(args[1]), env, None);
                    let (ai, bi) = (self.interval_of(&a_t), self.interval_of(&b_t));
                    let full = int_types(&d.name).unwrap();
                    let op = match m.as_str() {
                        "add" => "+",
                        "sub" => "-",
                        "mul" => "*",
                        "div" => "/",
                        _ => "%",
                    };
                    if let (Some(ai), Some(bi)) = (ai, bi) {
                        if let Some(r) = iv::arith(op, ai, bi, 64) {
                            if let Some(c) = iv::meet(r, full) {
                                return named_args("Outcome", vec![refined(nt(&d.name), Some(c.0), Some(c.1), "range")]);
                            }
                        }
                    }
                }
            }
            if !d.type_params.is_empty() {
                if let Type::Named(tn) = &t {
                    for (p, a) in d.type_params.iter().zip(tn.args.iter()) {
                        mm.insert(p.clone(), a.clone());
                    }
                }
            }
            if Self::is_host(&d) && m == "capabilities" {
                self.capabilities_use(&member, called);
            }
            if !called {
                if !sig.where_.is_empty() {
                    self.typ("E-TYP-040", format!("`{}.{m}` has `where` bounds, which hold at each call (6.10): it is called, never taken as a value; to pass it, wrap the call in a `fn` literal", d.name), &member, &format!("a call of {}.{m}", d.name), "a function value", &m, "LANG-06-01");
                }
                let r = self.result_of(&sig, &mm);
                return Type::Fn { params: sig.params.iter().map(|p| p.1.clone()).collect(), ret: Box::new(r), kind: "fn".into(), modifiers: sig.modifiers.clone(), origin: String::new(), dynamic: false, identity: String::new() };
            }
            self.call_decl = Some(d.clone());
            self.call_label = self.method_label(&d, &m);
            self.pending_owner_where = d.where_.clone();
            // the expected result of a generic associated call fills its type parameters before the arguments are
            // typed (LR 6.10). `List.of(Red)`, where the call is expected `List[Light]`, then sees `Red` as `Light`,
            // so 9.7's unprefixed form is the variant that type picks (D-253). A binding that still names an unsolved
            // type parameter (the caller's, not this body's) is dropped: the arguments infer it. `List.of` passed to
            // `Outcome.collect` is expected `List[Outcome[T]]` while T is collect's parameter. Several owners and no
            // expected result stay ambiguous.
            //
            // A binding to a function type is dropped as well, dynamic or not, with optional and loan wrappers peeled
            // (8.4). `List[fn(Int) Int]` would otherwise fix the element before either argument, so
            // `List.of(lamA, lamB)` would accept the second lambda; left unbound, the first lambda pins the parameter
            // and the second conflicts. `dynamic fn` is not installed here either. `seed_dynamic_fn` binds that one,
            // and it peels present-injection optionals this unification does not (`(List[dynamic fn]?)?`,
            // `List[(dynamic fn)?]`). An enum, or any other non-function expected type, still binds.
            if let Some(want) = expected {
                let pat = self.subst(&sig.ret, &mm);
                // A bare result parameter is fixed by the arguments (see seed_expected_result).
                if !matches!(strip(&pat), Type::TypeParam(_)) {
                    let before: BTreeSet<String> = mm.keys().cloned().collect();
                    self.unify(&pat, want, &mut mm);
                    mm.retain(|k, v| {
                        if before.contains(k) {
                            return true;
                        }
                        if !Self::tparams_of(v).iter().all(|p| self.tparams_in_scope.contains(p)) {
                            return false;
                        }
                        let mut bound = v;
                        loop {
                            match bound {
                                Type::Optional(inner) | Type::Loan { inner, .. } => bound = inner,
                                Type::Fn { .. } => return false,
                                _ => return true,
                            }
                        }
                    });
                }
            }
            self.pending_call_expected = expected.cloned();
            return self.check_call(&sig, al, env, &mut mm, &format!("{}.{m}", d.name), &member);
        }
        if m == "default" && d.derives.iter().any(|(n, _)| n == "Default") {
            // `derive Default` generates the associated `default()` from each field's initialiser (6.10, 9.2; D-170)
            let self_t = if matches!(t, Type::Named(_)) { t.clone() } else { named_of_decl(&d, vec![]) };
            if !called {
                return Type::Fn { params: vec![], ret: Box::new(self_t), kind: "fn".into(), modifiers: vec!["pure".into()], origin: String::new(), dynamic: false, identity: String::new() };
            }
            if d.fields.iter().any(|(_, (ft, _, _))| !self.dc.is_trivial(ft)) {
                // a field that owns heap storage is created (C.3)
                self.eff.add("alloc", "proved", Some(&member));
            }
            self.call_decl = None;
            self.call_label = format!("derived {}.default", d.name);
            let sig = Sig::simple("default", vec![], self_t, vec!["pure".into()]);
            self.pending_call_expected = expected.cloned();
            return self.check_call(&sig, al, env, &mut BTreeMap::new(), &format!("{}.default", d.name), &member);
        }
        if let Some(c) = d.constants.get(&m) {
            return c.clone();
        }
        if d.kind == "enum" {
            self.rec("E-NAM-001", format!("{} has no variant `{m}`", d.name), "LANG-04-05", &member, Self::nam(&m, d.variants.sorted_keys()));
        } else {
            let mut c: Vec<String> = d.methods.iter().filter(|(_, s)| s.receiver.is_none()).map(|(k, _)| k.clone()).collect();
            c.sort();
            c.extend(d.constants.sorted_keys());
            self.rec("E-NAM-001", format!("{} has no associated member `{m}`", d.name), "LANG-04-05", &member, Self::nam(&m, c));
        }
        if let Some(al) = al {
            self.infer(Some(al), env, None);
        }
        unknown("associated")
    }

    pub fn variant_args(&mut self, d: &DeclRef<'a>, vname: &str, al: Option<N<'a>>, env: &Env, tok: &Token) {
        let payload = d.variants.get(vname).cloned().unwrap_or_default();
        let args = Self::arg_nodes(al);
        if payload.is_empty() && !args.is_empty() {
            self.typ("E-TYP-130", format!("variant {}.{vname} has no payload", d.name), tok, "no arguments", &args.len().to_string(), vname, "LANG-05-09");
        }
        let sig = Sig::simple(vname, payload.iter().map(|(f, t)| (f.clone(), t.clone(), String::new())).collect(), named_of_decl(&d, vec![]), vec![]);
        if !payload.is_empty() && al.is_some() {
            self.call_decl = Some(d.clone());
            self.call_label = format!("variant {}.{vname}", d.name);
            self.check_call(&sig, al, env, &mut BTreeMap::new(), &format!("{}.{vname}", d.name), tok);
        }
    }

    pub fn construct(&mut self, d: &DeclRef<'a>, t: &Type, al: Option<N<'a>>, env: &Env, tok: &Token) -> Type {
        if self.foreign(d) {
            let hidden: Vec<String> = d.fields.iter().filter(|(_, (_, exp, _))| !exp).map(|(f, _)| f.clone()).collect();
            if !hidden.is_empty() {
                let s = if hidden.len() > 1 { "s" } else { "" };
                let list = hidden.iter().map(|h| format!("`{h}`")).collect::<Vec<_>>().join(", ");
                self.rec("E-NAM-004", format!("constructor of {} is not callable here: field{s} {list} not exported ({} is declared in {})", d.name, d.name, d.module), "LANG-09-02", tok, Self::nam(&d.name, hidden));
            }
        }
        let mut given: BTreeMap<String, Token> = BTreeMap::new();
        let mm = match t {
            Type::Named(n) if !n.args.is_empty() => self.recv_subst(t, d),
            _ => {
                let mut m = BTreeMap::new();
                m.insert("Self".to_string(), t.clone());
                m
            }
        };
        for a in Self::arg_nodes(al) {
            let ts = a.direct_tokens();
            let e = Self::arg_expr(a);
            if ts.first().map_or(false, |t| t.text == "...") {
                let pl = a.find("place");
                if let Some(pl) = pl {
                    let p = self.path_of_place(pl);
                    let mode = if ts[1].text == "move" { "move" } else { "copy" };
                    self.use_place(Some(&p), ftok(a), mode, env);
                    if ts[1].text == "move" {
                        self.own.move_(&p);
                    }
                }
                let st = match pl {
                    Some(pl) => strip(&self.place_type(pl, env)).clone(),
                    None => unknown(""),
                };
                if ts[1].text == "copy" && known(&st) && !self.dc.is_trivial(&st) {
                    self.eff.add("alloc", "proved", Some(ftok(a))); // `...copy` of a heap-owning value deep-copies it (6.11; X-116)
                }
                let sd = self.decl_of(&st);
                let (src_fields, src_decl): (Option<Vec<(String, Type)>>, Option<DeclRef>) = if let Type::Record(fs) = &st {
                    (Some(fs.clone()), None)
                } else if let Some(sd) = sd {
                    (Some(sd.fields.iter().map(|(k, v)| (k.clone(), v.0.clone())).collect()), Some(sd))
                } else {
                    (None, None)
                };
                let src_fields = match src_fields {
                    Some(s) => s,
                    None => return named_of_decl(&d, vec![]),
                };
                let src_mm: BTreeMap<String, Type> = match &src_decl {
                    Some(src) if matches!(st, Type::Named(_)) => self.recv_subst(&st, src),
                    _ => BTreeMap::new(),
                };
                for (f, ft) in src_fields {
                    if d.fields.contains(&f) {
                        if given.contains_key(&f) {
                            self.typ("E-TYP-130", format!("field `{f}` of {} is given by the spread and explicitly", d.name), ftok(a), "each field once", &f, &f, "LANG-09-02");
                        }
                        let declared = self.subst(&d.fields.get(&f).unwrap().0, &mm);
                        // a class field's declared type has no identity; the session pin is the lambda it holds (8.4)
                        let value_ty = if let Some(src) = &src_decl {
                            self.overlay_pins(src, &f, &self.subst(&ft, &src_mm), "")
                        } else {
                            ft.clone()
                        };
                        if !has_free(&declared) {
                            self.expect(&value_ty, &declared, ftok(a), &format!("field `{f}`"));
                        }
                        self.note_field_closure(d, &f, &declared, &value_ty, ftok(a));
                        given.insert(f, ftok(a).clone());
                    } else if ts[1].text == "move" && self.is_linear(&ft) {
                        let t0 = ftok(a);
                        self.obl_err("E-OBL-007", format!("`...move` drops field `{f}` ({}), which is linear and has no counterpart in {}", ft.show(), d.name), t0, &format!("{}:{}", t0.line, t0.col), &f, "LANG-10-02");
                    }
                }
                continue;
            }
            if !(!ts.is_empty() && ts[0].kind == Kind::Lower && ts.len() > 1 && ts[1].text == ":") {
                self.typ("E-TYP-130", format!("constructor arguments of {} are named (field: value)", d.name), ftok(a), "field: value", "positional", &d.name, "LANG-09-02");
                self.infer(e, env, None);
                continue;
            }
            let f = ts[0].text.clone();
            let fdecl = match d.fields.get(&f) {
                Some(x) => x.clone(),
                None => {
                    let names = d.fields.keys().cloned().collect::<Vec<_>>().join(", ");
                    self.typ("E-TYP-130", format!("{} has no field `{f}`", d.name), ts[0], &names, &f, &f, "LANG-09-02");
                    self.infer(e, env, None);
                    continue;
                }
            };
            if given.contains_key(&f) {
                self.typ("E-TYP-130", format!("field `{f}` of {} is given twice", d.name), ts[0], "each field once", &f, &f, "LANG-09-02");
            }
            given.insert(f.clone(), ts[0].clone());
            let ft = self.subst(&fdecl.0, &mm);
            let fmode = if ft.is_loan() { "loan" } else { "owned" };
            self.arg_mode = Some(fmode.into());
            let et = self.infer(e, env, Some(&ft));
            self.arg_mode = None;
            if let Some(e) = e {
                self.check_arg_marker(e, &f, fmode, &et, ftok(e), env);
                if !has_free(&ft) {
                    self.expect(&et, &ft, ftok(e), &format!("field `{f}`"));
                }
                self.note_field_closure(d, &f, &ft, &et, ftok(e));
            }
        }
        let missing: Vec<String> = d.fields.iter().filter(|(f, (_, _, hd))| !given.contains_key(*f) && !hd).map(|(f, _)| f.clone()).collect();
        if !missing.is_empty() {
            let s = if missing.len() > 1 { "s" } else { "" };
            let list = missing.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
            self.typ("E-TYP-050", format!("constructor of {} lacks field{s} {list}", d.name), tok, &missing.join(", "), "absent", &d.name, "LANG-09-02");
        }
        self.activate(tok, d.lifetime.is_some(), env);
        match t {
            Type::Named(n) if !n.args.is_empty() => t.clone(),
            _ => named_of_decl(&d, vec![]),
        }
    }

    pub fn lambda_type(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let e = env_new(Some(env));
        let exp_fn = expected.map(|x| strip(x).clone()).filter(|x| matches!(x, Type::Fn { .. }));
        let mut params = Vec::new();
        // the lambda's own parts, not a nested lambda's (D-34)
        if let Some(lp) = node.child_node("lambdaParams") {
            for (i, p) in lp.find_all("lambdaParam").into_iter().enumerate() {
                let ts = p.direct_tokens();
                let t = if let Some(pt) = p.find("paramType") {
                    self.dc.type_of(pt.find("type"))
                } else if let Some(Type::Fn { params: ep, .. }) = &exp_fn {
                    ep.get(i).cloned().unwrap_or_else(|| unknown("inferred lambda parameter"))
                } else {
                    unknown("inferred lambda parameter")
                };
                params.push(t.clone());
                env_set(&e, &ts[0].text, t, false, "param");
            }
        }
        let words: Vec<String> = node.kids().iter().filter_map(|k| k.tok()).map(|t| t.text.clone()).collect();
        let kind = if words.iter().any(|w| w == "once") {
            "fn once"
        } else if words.iter().any(|w| w == "mut") {
            "fn mut"
        } else {
            "fn"
        }
        .to_string();
        let mut captures: Vec<(String, String)> = Vec::new();
        let mut cap_loans = Vec::new();
        let mut linear_capture = false;
        let ltok = ftok(node).clone();
        if let Some(cl) = node.child_node("captureList") {
            for cap in cl.find_all("capture") {
                let ctok_ = ftok(cap).clone();
                let root_name = cap.find("place").map_or(ctok_.text.clone(), |pl| pl.direct_tokens()[0].text.clone());
                if root_name != "self" && env_lookup(env, &root_name).is_none() {
                    // not a local or parameter: the resolver rejects the entry (E-FUN-011), and the body names it directly
                    continue;
                }
                if let Some(pl) = cap.find("place") {
                    let mk = ctok_.text.clone();
                    let path = self.path_of_place(pl);
                    self.in_marker = true;
                    let ct = self.place_type(pl, env);
                    self.in_marker = false;
                    self.use_place(Some(&path), &ctok_, if mk == "copy" { "copy" } else { &mk }, env);
                    if mk == "copy" {
                        let dt = strip(&self.declared_type(pl, env)).clone();
                        if known(&dt) && !self.dc.is_copyable(&dt) {
                            self.own_err("E-OWN-002", format!("`copy {path}` in a capture list: {} is not Copyable", strip(&ct).show()), &ctok_, &path, "copy of a non-copyable place", "LANG-07-01", "");
                        }
                    }
                    captures.push((path.clone(), mk.clone()));
                    let root_tok = pl.direct_tokens()[0];
                    let saved_f = std::mem::take(&mut self.facts);
                    let root_t = self.root_type(root_tok, env);
                    self.facts = saved_f;
                    let prev = env_local(&e, &root_tok.text);
                    let mutable = ["editable", "move", "copy"].contains(&mk.as_str()) || prev.as_ref().map_or(false, |p| p.mutable);
                    let kind_s = match &prev {
                        None => format!("capture-{mk}"),
                        Some(p) if p.kind == "capture-alias" => format!("capture-{mk}"),
                        Some(p) => p.kind.clone(),
                    };
                    env_set(&e, &root_tok.text, root_t, mutable, &kind_s);
                } else {
                    let ct_t = env_get(env, &ctok_.text).unwrap_or_else(|| unknown(""));
                    if known(strip(&ct_t)) && !self.dc.is_trivial(strip(&ct_t)) {
                        self.own_err("E-OWN-001", format!("bare capture `{}`: only a trivial value may be listed without a marker (8.3)", ctok_.text), &ctok_, &ctok_.text, "marker required in a capture list", "LANG-08-02", "");
                    }
                    captures.push((ctok_.text.clone(), "copy".into()));
                    env_set(&e, &ctok_.text, ct_t, false, "capture-copy");
                }
            }
            for i in 0..captures.len() {
                for j in i + 1..captures.len() {
                    let (pa, ma) = &captures[i];
                    let (pb, mb) = &captures[j];
                    if (ma == "editable" || mb == "editable") && root_of(&self.own.owner_path(pa)) == root_of(&self.own.owner_path(pb)) {
                        let (pa, ma, pb, mb) = (pa.clone(), ma.clone(), pb.clone(), mb.clone());
                        self.own_err("E-OWN-010", format!("captures `{ma} {pa}` and `{mb} {pb}` loan one owner and one of them is exclusive"), &ltok, &pa, &format!("{mb} {pb}"), "LANG-08-02", "");
                    }
                }
            }
            linear_capture = false;
            for (path, mk) in captures.clone() {
                if (mk == "alias" || mk == "editable") && self.use_ok {
                    let d = self.depth;
                    cap_loans.push(self.own.add_loan(&path, &mk, None, d, ltok.line));
                } else if mk == "move" {
                    if self.own.obligations.contains(&path) {
                        linear_capture = true;
                    }
                    self.own.move_(&path);
                }
            }
            if linear_capture && kind != "fn once" {
                self.obl_err("E-OBL-006", format!("this `{kind}` lambda captures a linear value by move: a closure owning a linear value must be `fn once`"), &ltok, &format!("{}:{}", ltok.line, ltok.col), "closure", "LANG-10-02");
            }
        }
        let rt = match node.child_node("type") {
            Some(t) => self.dc.type_of(Some(t)),
            None => unknown("lambda result"),
        };
        let written: Vec<String> = node.child_node("modifiers").map(|m| m.tokens().iter().map(|t| t.text.clone()).collect()).unwrap_or_default();
        let is_contract = |m: &String| m.chars().next().map_or(false, |c| c.is_ascii_uppercase());
        let lam_contracts: Vec<String> = written.iter().filter(|m| is_contract(m)).cloned().collect();
        let mut modifiers: Vec<String> = written.iter().filter(|m| !is_contract(m)).cloned().collect();
        {
            // a contract on a lambda binds it as its modifiers do (11.5): `P fn` with `contract P { pure }` is pure (D-145)
            let mut probe = Sig::simple("lambda", vec![], void(), modifiers.clone());
            probe.contracts = lam_contracts.clone();
            let extra: Vec<String> = self.expanded_mods(&probe).into_iter().filter(|m| !modifiers.contains(m)).collect();
            modifiers.extend(extra);
        }
        {
            let ltok = node.tokens()[0].clone();
            for part in [node.child_node("lambdaParams"), node.child_node("type")].into_iter().flatten() {
                for (q, n, qtok) in self.dc.shadowed_qualified_interfaces(part) {
                    self.typ("E-TYP-040", format!("`{q}` is the interface {n}, which no value has: write `dynamic {q}`, `Shared[dynamic {q}]` or a type parameter bounded by it (9.5)"), &qtok, &format!("dynamic {q}"), &n, "lambda", "LANG-06-01");
                }
            }
            let mut sig_types: Vec<Type> = params.clone();
            sig_types.push(rt.clone());
            for t in &sig_types {
                for i in self.dc.bare_interfaces(t) {
                    self.typ(
                        "E-TYP-040",
                        format!("a parameter or the result of this lambda has the interface type {i}, which no value has: write `dynamic {i}`, `Shared[dynamic {i}]` or a type parameter bounded by {i} (9.5)"),
                        &ltok,
                        &format!("dynamic {i}"),
                        &i,
                        "lambda",
                        "LANG-06-01",
                    );
                }
            }
            if modifiers.iter().any(|m| m == "pure") {
                for t in params.clone() {
                    if self.is_authority(&t) {
                        let name = base_name(Self::unshared(&t)).unwrap_or("").to_string();
                        self.eff_err("E-EFF-001", format!("lambda is declared `pure` but takes the capability {name}: a pure function receives no authority (11.5)"), &ltok, "pure", "io", "lambda", "LANG-11-06");
                    }
                }
            }
        }
        let body = node.child_node("funcBody");
        if let Some(body) = body {
            let saved_ret = self.ret_type.replace(rt.clone());
            let saved_try = std::mem::replace(&mut self.in_try, modifiers.iter().any(|m| m == "try"));
            let saved_ctx = std::mem::replace(&mut self.ctx, vec!["lambda".into()]);
            let saved_labels = std::mem::take(&mut self.loop_labels);
            let saved_facts = std::mem::take(&mut self.facts);
            let saved_own = std::mem::take(&mut self.own);
            let saved_depth = std::mem::replace(&mut self.depth, 0);
            let saved_arg = self.arg_mode.take();
            let saved_pending = self.pending.take();
            let saved_carried = std::mem::take(&mut self.carried);
            let saved_temps = std::mem::take(&mut self.carried_temps);
            let saved_res = std::mem::take(&mut self.res_stack);
            self.lambda_kind.push(kind.clone());
            let lt = node.direct_tokens();
            let fn_tok = lt.iter().find(|t| t.text == "fn").map(|t| (*t).clone()).unwrap_or_else(|| ltok.clone());
            let mut_tok = lt.iter().find(|t| t.text == "mut").map(|t| (*t).clone());
            self.lambda_toks.push((fn_tok, mut_tok));
            // the lambda body is a separate activation: its exits and defers are its own, and the places it writes are its
            // own (its invalidations are dropped from the log when it ends, D-74)
            let saved_exits = std::mem::take(&mut self.exit_frames);
            let saved_breaks = std::mem::take(&mut self.break_frames);
            let saved_defers = std::mem::take(&mut self.defer_bodies);
            let saved_rest = std::mem::take(&mut self.block_rest);
            let log_mark = self.inval_log.len();
            let outer_eff = std::mem::take(&mut self.eff);
            self.body(Some(body), &e, &rt, &ltok, "lambda");
            self.exit_frames = saved_exits;
            self.break_frames = saved_breaks;
            self.defer_bodies = saved_defers;
            self.block_rest = saved_rest;
            self.inval_log.truncate(log_mark);
            if matches!(strip(&rt), Type::Fn { .. }) {
                // natively, a closure's function result is boxed: a Rust closure cannot name an `impl Fn` result
                // (Appendix C.1), so returning one allocates (C.3; D-34)
                self.eff.add("alloc", "proved", Some(&ltok));
            }
            let mut lam_eff = std::mem::replace(&mut self.eff, outer_eff);
            self.lambda_kind.pop();
            self.lambda_toks.pop();
            if self.final_pass {
                // the reference mutates the one summary object the closure keeps: its callers import these authorities
                lam_eff.authorities = self.eff.authorities.clone();
                lam_eff.authorities.insert("enclosing".into());
                let outer = std::mem::replace(&mut self.eff, lam_eff);
                let lam_names: Vec<String> = node.child_node("lambdaParams").map(|lp| lp.child_nodes("lambdaParam").iter().map(|p| p.direct_tokens()[0].text.clone()).collect()).unwrap_or_default();
                let lparams: Vec<(String, Type, String)> = lam_names.into_iter().zip(params.iter().cloned()).map(|(n, t)| (n, t, String::new())).collect();
                let mut lsig = Sig::simple("lambda", lparams, rt.clone(), modifiers.clone());
                lsig.contracts = lam_contracts.clone();
                self.check_claims(&lsig, &ltok, "lambda");
                lam_eff = std::mem::replace(&mut self.eff, outer);
            }
            self.last_closure_eff = Some(lam_eff);
            self.ret_type = saved_ret;
            self.in_try = saved_try;
            self.ctx = saved_ctx;
            self.loop_labels = saved_labels;
            self.facts = saved_facts;
            self.own = saved_own;
            self.depth = saved_depth;
            self.arg_mode = saved_arg;
            self.pending = saved_pending;
            self.carried = saved_carried;
            self.carried_temps = saved_temps;
            self.res_stack = saved_res;
        }
        self.carried.extend(cap_loans);
        let info = ClosureInfo { kind: kind.clone(), captures, linear: linear_capture, eff: self.last_closure_eff.clone() };
        self.last_closure = Some(info.clone());
        self.closure_info.insert(node.id(), info);
        if body.is_some() {
            if let Some(le) = &self.last_closure_eff {
                if !modifiers.iter().any(|m| m == "pure") && !le.effects.iter().any(|(k, _)| PURE_FORBIDS.contains(&k.as_str())) {
                    modifiers.push("pure".into());
                }
            }
        }
        let identity = node.direct_tokens().iter().find(|t| t.text == "fn").map(|t| format!("{}:{}:{}", self.module, t.line, t.col)).unwrap_or_default();
        Type::Fn { params, ret: Box::new(rt), kind, modifiers, origin: String::new(), dynamic: false, identity }
    }

    pub fn check_interpolation(&mut self, tk: &'a Token, env: &Env) {
        for (k, src) in hole_sources(tk).into_iter().enumerate() {
            let parsed = match hole_tree_at(tk, k, &src) {
                Some(p) => p,
                None => continue,
            };
            let et = N::new(&parsed.root, &parsed.toks);
            let before = self.nrecords();
            let effects_before = self.eff.effect_set();
            let t0 = self.infer(Some(et), env, None);
            let t = concrete(strip(&t0));
            for r in &mut self.dc.records[before..] {
                r.line = tk.line;
                r.column = tk.col;
            }
            let bad: Vec<String> = self.eff.effect_set().difference(&effects_before).filter(|e| *e == "fail" || *e == "suspend").cloned().collect();
            if !bad.is_empty() {
                self.rec(
                    "E-TYP-040",
                    format!("interpolated `{src}` may {}: an interpolated expression is evaluated once, in order, and is neither fallible nor suspending (3.6)", bad.join(" and ")),
                    "LANG-03-06",
                    tk,
                    DeclChecker::typ_fields("an infallible, non-suspending expression", &bad.join(", "), &format!("${{{src}}}")),
                );
            }
            if known(&t) && !self.dc.implements(&t, "Displayable") {
                self.rec(
                    "E-TYP-040",
                    format!("interpolated `{src}` has type {}, which is not Displayable", t.show()),
                    "LANG-03-05",
                    tk,
                    DeclChecker::typ_fields("Displayable", &t.show(), &format!("${{{src}}}")),
                );
            }
        }
    }

    #[allow(dead_code)]
    fn _types_used(_f: Facts, _o: Owners) {}
}

#[allow(dead_code)]
fn _interp_used() -> Type {
    interp()
}
