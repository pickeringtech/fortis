//! Expressions, calls and members (Language Reference 5.8, 6, 7.5, 8): the expression half of checker/typer.py.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use fortis_diag::Json;
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{C, N};
use fortis_syntax::{parse_from, Parsed};

use super::*;

#[cfg(test)]
#[path = "test_loan_origins.rs"]
mod test_loan_origins;
use crate::conversions::{self as cv, Conversion};
use crate::decls::{core_origin_of, named_of_decl, Convert, DeclChecker, DeclRef, Sig};
use crate::effects::{Summary, PURE_FORBIDS};
use crate::intervals::{self as iv, Facts};
use crate::literals::{assignable, closure_conflict, concrete, contains_dynamic_fn, declared_vs_lambda, dynamic_covers, interp, is_interp, is_strlit, literal_type, negate, CLOSURE_WHY};
use crate::owners::{root_of, Owners};
use crate::types::*;

thread_local! {
    static HOLES_AT: RefCell<HashMap<(usize, usize, String, usize, usize), Option<&'static Parsed>>> = RefCell::new(HashMap::new());
}

/// What `x as T` resolves to for a pair of known types (6.13, X-475): shared by the typer's judgement and the IR
/// builder's lowering, so both name the same row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// a row of the built-in table
    Builtin(Conversion),
    /// a base type into a refinement of it: the spelled `validate` (6.3), fallible
    Refine,
    /// nothing to convert, with the sections that say so (E-TYP-181)
    Identity(&'static str),
    /// no conversion for the pair (E-TYP-180)
    None,
}

/// The declared conversion a pair resolves to through the members of its types (6.13, X-551).
pub enum MemberConv<'a> {
    /// the pair has no declared conversion
    None,
    /// one member converts the pair: the declaring type and the member
    One(DeclRef<'a>, Convert<'a>),
    /// the source declares `convert to` and the target declares `convert from`: an `as` would have to choose
    /// (E-TYP-182)
    Two,
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
/// A lambda's body: its `funcBody`, or, for an expression body written without a return type (8.2, X-345), the lambda
/// itself, whose first `expr` is that body (its parameters, captures and modifiers hold none), so `body` reads both
/// forms alike.
pub fn lambda_body<'a>(node: N<'a>) -> Option<N<'a>> {
    node.child_node("funcBody").or_else(|| node.child_node("lambdaBlock")).or_else(|| node.direct_tokens().iter().any(|t| t.text == "=>").then_some(node))
}

/// The operators with a checked `Int` operation (6.4, X-345).
pub const CHECKED_OPS: &[&str] = &["+", "-", "*", "/", "%", "<<"];

/// The methods a `T?` receiver has without a null test: the prelude's `Optional` class holds them (6.8, X-479).
pub const OPTIONAL_HELPERS: &[&str] = &["map"];

/// The operators of a checked region (6.4, X-345): the integer arithmetic an outcome operator's left operand is made
/// of, through `+ - * / % << >>`, unary `-` and parentheses, and nothing else (not into a call's arguments, an index, a
/// lambda or another operator). Empty when the left operand is not arithmetic. Positions are (line, column).
pub fn region_operators(c: C) -> BTreeSet<(usize, usize)> {
    fn walk(c: C, out: &mut BTreeSet<(usize, usize)>) {
        let Some(n) = c.node() else { return };
        let kids = n.kids();
        let nm = n.name();
        if matches!(nm, "additiveExpr" | "multiplicativeExpr" | "shiftExpr") && kids.len() > 1 {
            for k in kids {
                match k.tok() {
                    Some(t) => {
                        out.insert((t.line, t.col));
                    }
                    None => walk(k, out),
                }
            }
            return;
        }
        if nm == "unaryExpr" && kids.len() == 2 && kids[0].tok().map_or(false, |t| t.text == "-") {
            let t = kids[0].tok().unwrap();
            out.insert((t.line, t.col));
            walk(kids[1], out);
            return;
        }
        if nm == "primaryExpr" && kids.len() == 3 && kids[0].tok().map_or(false, |t| t.text == "(") {
            walk(kids[1], out);
            return;
        }
        const WRAPPERS: &[&str] = &[
            "expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr", "asExpr", "unaryExpr", "postfixExpr",
        ];
        if WRAPPERS.contains(&nm) && kids.len() == 1 {
            walk(kids[0], out);
        }
    }
    let mut out = BTreeSet::new();
    walk(c, &mut out);
    out
}

/// The name of an argument written as a bare local name (`wanted`), which a call reads as `wanted: wanted` where a
/// positional argument cannot stand: after a named argument, and in a class's generated constructor (5.8, X-345).
pub fn pun_name<'a>(arg: N<'a>) -> Option<&'a Token> {
    let ks = arg.kids();
    if ks.len() != 1 {
        return None;
    }
    let e = ks[0].node().filter(|n| n.name() == "expr" || n.name() == "constExpr")?;
    let ts = e.tokens();
    (ts.len() == 1 && ts[0].kind == Kind::Lower).then(|| ts[0])
}

/// The value of a named argument written `x: x`, unmarked: what a pun would write (5.8, X-345).
pub fn pun_value<'a>(arg: N<'a>) -> Option<&'a Token> {
    let ts = arg.direct_tokens();
    if ts.len() < 2 || ts[0].kind != Kind::Lower || ts[1].text != ":" {
        return None;
    }
    let e = arg.child_node("expr").or_else(|| arg.child_node("constExpr"))?;
    let vt = e.tokens();
    (vt.len() == 1 && vt[0].kind == Kind::Lower && vt[0].text == ts[0].text).then(|| vt[0])
}

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
    /// The enum a comparison's left operand has (an optional one's present type), which the right operand is expected
    /// to have, so a bare variant there is read as that enum's (9.7, X-345); None for any other type.
    pub(crate) fn enum_operand(&self, t: &Type) -> Option<Type> {
        let mut s = strip(t).clone();
        if let Type::Optional(i) = &s {
            s = strip(i).clone();
        }
        if !known(&s) {
            return None;
        }
        match self.decl_of(&s) {
            Some(d) if d.kind == "enum" => Some(s),
            _ => None,
        }
    }

    /// The type an unannotated binding takes from its initialiser (5.1): a computed integer interval or a literal's
    /// exact value widens to its base type, which stays a fact; an inferred lambda result takes the same (8.2, X-345).
    pub(crate) fn widened(&self, t: &Type) -> Type {
        let c = concrete(t);
        if let Type::Refined { base, kind, .. } = &c {
            if (kind == "exact" || kind == "range") && self.is_int(&c) {
                return Type::Named(base.clone());
            }
        }
        c
    }

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

    /// A callback parameter whose result is an outcome (`tryMap`, `tryEach`, `tryFold`, 6.7) given a function that returns
    /// something else: the mismatch is named, with the outcome the callback must return, rather than left to an unfixed
    /// type argument (`E-TYP-160`; D-536). The argument then types as unknown, so nothing else is said of it.
    fn outcome_callback(&mut self, p: &Param, t: Type, e: Option<N<'a>>, m: &mut BTreeMap<String, Type>, where_: &str) -> Type {
        let (want_ret, have_ret) = match (strip(&p.1), strip(&t)) {
            (Type::Fn { ret: w, .. }, Type::Fn { ret: h, .. }) => ((**w).clone(), strip(h).clone()),
            _ => return t,
        };
        let is_outcome = |x: &Type| matches!(strip(x), Type::Named(n) if (n.package.is_empty() && n.name == "Outcome"));
        if !is_outcome(&want_ret) || !known(&have_ret) || is_outcome(&have_ret) || have_ret == never() {
            return t;
        }
        let e = match e {
            Some(e) => e,
            None => return t,
        };
        let displays = diagnostic_types(&[&self.subst(&want_ret, m), &have_ret]);
        let shown = &displays[0];
        self.typ(
            "E-TYP-040",
            format!("argument `{}` of {where_} must return an outcome, {shown}; the callback returns {} (a `try` method stops at the first failure, 6.7)", p.0, displays[1]),
            ftok(e),
            &shown,
            &displays[1],
            &format!("argument `{}`", p.0),
            "LANG-06-16",
        );
        for tp in Self::tparams_of(&want_ret) {
            m.entry(tp).or_insert_with(|| unknown("callback"));
        }
        unknown("callback")
    }

    // ---- calls (5.8) --------------------------------------------------------------------
    pub fn check_call(&mut self, sig: &Sig<'a>, al: Option<N<'a>>, env: &Env, m: &mut BTreeMap<String, Type>, where_: &str, tok: &Token) -> Type {
        let saved_optional = self.optional_arguments;
        let expected = self.pending_call_expected.take();
        let mut receiver_borrows = std::mem::take(&mut self.temporary_borrows);
        self.call_depth += 1;
        let t = self.check_call_at(sig, al, env, m, where_, tok, expected.as_ref());
        self.optional_arguments = saved_optional;
        self.call_depth -= 1;
        if Self::lends_from_self(sig) {
            receiver_borrows.append(&mut self.temporary_borrows);
            self.temporary_borrows = receiver_borrows;
        }
        // the callee at its name token, for `@ignorable` statements (5.5) and a test body's implied `or fail` (14)
        self.call_sigs.insert((tok.line, tok.col), (sig.has_mod("try"), sig.ignorable.is_some()));
        t
    }

    fn check_call_at(&mut self, sig: &Sig<'a>, al: Option<N<'a>>, env: &Env, m: &mut BTreeMap<String, Type>, where_: &str, tok: &Token, expected: Option<&Type>) -> Type {
        let optional = std::mem::take(&mut self.optional_arguments);
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
        // fixed before the arguments by the receiver or the expected type: the arguments are checked against them
        let fixed: BTreeSet<String> = m.keys().cloned().collect();
        let params = sig.params.clone();
        let mut bound: BTreeSet<String> = BTreeSet::new();
        let mut seen_named = false;
        let mut out_named = false;
        let mut unknown_named = false;
        let mut pairs: Vec<(Param, Option<N<'a>>)> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let ts = a.direct_tokens();
            let e = Self::arg_expr(*a);
            if ts.first().map_or(false, |t| t.text == "...") {
                continue;
            }
            // the formatter's pun (5.8, X-345): `x: x` is written `x` after a named argument, or where `x` is the
            // parameter in that position and every argument before it is positional
            if ts.len() > 1 && ts[0].kind == Kind::Lower && ts[1].text == ":" {
                let positional_same = !out_named && params.get(i).map_or(false, |p| p.0 == ts[0].text && !p.2.starts_with("..."));
                if pun_value(*a).is_some() && (out_named || positional_same) {
                    if self.final_pass {
                        let v = pun_value(*a).unwrap();
                        self.migrations.push(crate::migrate::Edit { from: (ts[0].line, ts[0].col), to: (v.line, v.col), text: String::new(), rule: "R9" });
                    }
                } else {
                    out_named = true;
                }
            } else if seen_named {
                out_named = true;
            }
            let p: Param;
            // `wanted` after a named argument is `wanted: wanted` (5.8, X-345)
            let pun = if seen_named { pun_name(*a) } else { None };
            let ts: Vec<&Token> = match pun {
                Some(pt) => vec![pt, pt],
                None => ts,
            };
            if !ts.is_empty() && ts[0].kind == Kind::Lower && ts.len() > 1 && (ts[1].text == ":" || pun.is_some()) {
                seen_named = true;
                match params.iter().find(|p| p.0 == ts[0].text) {
                    None => {
                        let names = params.iter().map(|p| p.0.clone()).collect::<Vec<_>>().join(", ");
                        let why = if pun.is_some() { format!(" (`{0}` after a named argument stands for `{0}: {0}`)", ts[0].text) } else { String::new() };
                        self.typ("E-TYP-130", format!("{where_} has no parameter `{}`{why}", ts[0].text), ts[0], &names, &ts[0].text, &ts[0].text, "LANG-05-09");
                        self.infer(e, env, None);
                        unknown_named = true; // the misnamed argument is the record; the parameter it leaves unbound is not a second one
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
        // the reservations each argument made, by parameter (D-348)
        let mut spans: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        if let Some((rp, rm)) = &recv {
            self.reserve(rp, if rm != "alias" { rm } else { "alias" }, tok);
            spans.insert("self".into(), (0, self.own.reservations.len()));
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
            let n0 = self.own.reservations.len();
            let saved_inferred_target = self.inferred_conversion_target;
            self.inferred_conversion_target = !Self::tparams_of(&p.1).is_empty();
            let previous_borrows = self.temporary_borrows.len();
            if optional {
                if let Some(e) = e { self.optional_scopes.push((self.own.obligations.items.iter().cloned().collect(), ftok(e).clone())); }
            }
            let t = self.infer(e, env, Some(&want));
            let mut result_origins = Vec::new();
            Self::nested_origins(&sig.ret, true, &mut result_origins);
            if origin.as_deref() != Some(p.0.as_str()) && !result_origins.iter().any(|(o, _)| o == &p.0) && !sig.has_mod("async") {
                self.temporary_borrows.truncate(previous_borrows);
            }
            self.inferred_conversion_target = saved_inferred_target;
            // String.from accepts an owned String through its read-only StringView parameter. It copies that
            // String; `as String` would be identity, so the formatter retains the copy explicitly (X-553).
            if self.final_pass && sig.name == "from" && call_decl.as_ref().map_or(false, |d| d.name == "String" && crate::decls::is_standard(&d.module)) {
                let mut source = strip(&t);
                while let Type::Loan { inner, .. } = source { source = strip(inner); }
                if self.decl_of(source).map_or(false, |d| d.name == "String" && crate::decls::is_standard(&d.module)) {
                    let kind = if self.path_of_expr(e).is_some() { "string-copy" } else if t.is_loan() { "string-borrowed" } else { "string-owned" };
                    self.legacy_conversions.insert((tok.line, tok.col), crate::migrate::LegacyConversion { target: Some(string()), kind });
                }
            }
            self.arg_mode = None;
            if let Some(e) = e {
                self.check_arg_marker(e, &p.0, &pmode, &t, ftok(e), env);
                if sig.has_mod("async") && (pmode.is_empty() || pmode == "loan") && self.lends_temporary(e, &t) {
                    temps.push(ftok(e).clone());
                }
            }
            if optional && e.is_some() { self.optional_scopes.pop(); }
            spans.insert(p.0.clone(), (n0, self.own.reservations.len()));
            let shares = Self::tparams_of(&p.1).iter().any(|tp| pending_names.contains(tp) && !m.contains_key(tp));
            self.join_pin(&p.1, &t, m, &fixed);
            self.unify_pin(&p.1, &t, m, !shares);
            let t = self.outcome_callback(&p, t, e, m, where_);
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
        let carries = Self::result_carries(sig);
        for (p, e) in deferred {
            let mut result_origins = vec![];
            Self::nested_origins(&sig.ret, true, &mut result_origins);
            if result_origins.iter().any(|(o, _)| o == &p.0) && p.2.trim_start_matches('.') != "owned" {
                if let Some(e) = e {
                    self.own_err("E-OWN-051", format!("the result borrows from callback `{}`, a temporary that ends with the call: bind the closure first (8.3)", p.0), ftok(e), &p.0, "loan of a temporary callback", "LANG-07-06", "");
                }
            }
            let want = self.subst(&p.1, m);
            let n_carried = self.carried.len();
            let previous_infer = self.infer_capture_node;
            self.infer_capture_node = if p.2.trim_start_matches('.') != "owned" && matches!(strip(&want), Type::Fn { .. }) { e.and_then(|n| n.find("lambda")).map(|n| n.id()) } else { None };
            if optional {
                if let Some(e) = e { self.optional_scopes.push((self.own.obligations.items.iter().cloned().collect(), ftok(e).clone())); }
            }
            let t = self.infer(e, env, Some(&want));
            self.infer_capture_node = previous_infer;
            if optional && e.is_some() { self.optional_scopes.pop(); }
            if !carries && p.2.trim_start_matches('.') != "owned" {
                // a lambda lent to a plain callback parameter: the callee holds a read loan of the closure for the call, so
                // the loans its captures hold end with the call, as the call's own do (8.3)
                for l in self.carried.split_off(n_carried) {
                    self.own.remove_loan(&l);
                }
            }
            self.unify(&p.1, &t, m);
            let t = self.outcome_callback(&p, t, e, m, where_);
            arg_types.push((p, t, e));
        }
        let mut keep: BTreeMap<usize, String> = BTreeMap::new();
        if !carries {
            let mut origins = Vec::new();
            Self::nested_origins(&sig.ret, true, &mut origins);
            for (o, mode) in origins {
                let (lo, hi) = spans.get(&o).copied().unwrap_or((0, 0));
                for i in lo..hi {
                    keep.insert(i, mode.clone());
                }
            }
        }
        self.activate(tok, carries, env, &keep);
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
                    let names = diagnostic_types(&[t, &want]);
                    let (shown, ws) = (&names[0], &names[1]);
                    let what = format!("argument `{}`", p.0);
                    self.typ("E-TYP-040", format!("{what} has type {shown}, expected {ws}: {CLOSURE_WHY}"), ftok(e), &ws, &shown, &what, "LANG-08-04");
                } else {
                    self.expect(t, &want, ftok(e), &format!("argument `{}`", p.0));
                }
            }
            if self.is_linear(t) && self.path_of_expr(Some(e)).is_none() && p.2 != "payload" && !p.2.trim_start_matches('.').starts_with("owned") && e.find("markerExpr").is_none() && assignable(t, &want).0 != Some(false) {
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
                let displays = diagnostic_types(&[&Type::Named(want.clone()), &a]);
                self.typ("E-TYP-090", format!("{where_} needs `{tp}` to implement {}; {} does not (6.10)", displays[0], displays[1]), tok, &displays[0], &displays[1], where_, "LANG-06-10");
            }
        }
        // Prelude signatures with a dropping/short-circuit path cannot discharge a linear payload (10.7).
        let limited = if call_decl.as_ref().map_or(false, |d| d.module == "fortis.prelude") {
            match where_ { "Outcome.map" | "Outcome.collect" => Some("T"), _ => None }
        } else { None };
        if let Some(tp) = limited {
            if let Some(a) = m.get(tp) {
                if self.is_linear(a) {
                    self.obl_err("E-OBL-003", format!("{where_} cannot be instantiated with the linear type {} for `{tp}`: its signature may drop that value; use a consuming loop and handle every item (10.7)", a.show()), tok, &format!("{}:{}", tok.line, tok.col), tp, "LANG-10-02");
                }
            }
        }
        // a callee holding one of its own type parameters by value (an `owned` argument that is the parameter or a
        // structure whose linearity depends on it) cannot be instantiated at a linear type unless it declares that
        // parameter `T: linear` (6.12, 10.7; X-519); an owner's type parameters are judged on the owner's own
        // declaration. A by-value result or callback result also holds the parameter, even inside Outcome (D-596).
        for tp in &sig.type_params {
            if sig.linear.contains(tp) {
                continue;
            }
            let a = match m.get(tp) {
                Some(a) => strip(a).clone(),
                None => continue,
            };
            if !self.is_linear(&a) {
                continue;
            }
            let holds = self.holds_by_value(&sig.ret, tp, true) || sig.params.iter().any(|p|
                (p.2.trim_start_matches('.').starts_with("owned") && self.holds_by_value(&p.1, tp, false)) ||
                matches!(strip(&p.1), Type::Fn { ret, .. } if self.holds_by_value(ret, tp, true)));
            if holds {
                self.obl_err("E-OBL-003", format!("{where_} cannot be instantiated with the linear type {} for `{tp}`: it holds `{tp}` by value and does not declare `{tp}: linear` (6.12)", a.show()), tok, &format!("{}:{}", tok.line, tok.col), tp, "LANG-10-02");
            }
        }
        for p in &params {
            if !bound.contains(&p.0) && !p.2.starts_with("...") && !Self::has_default(sig, &p.0) && !unknown_named {
                self.typ("E-TYP-050", format!("{where_} needs argument `{}`", p.0), tok, &p.0, "absent", where_, "LANG-05-09");
            }
        }
        let unknown_args = arg_types.iter().any(|(_, t, _)| !known(&strip(t)));
        if !unknown_args && !has_free(&self.subst(&sig.ret, m)) {
            // a type parameter the result does not name is fixed by the arguments alone: `null` fixes nothing, so
            // `label(null, …)` leaves `T` unfixed (6.10, X-513); one the result names is reported where the result is used
            if let Some(tp) = sig.type_params.iter().find(|tp| !m.contains_key(*tp)) {
                self.typ("E-TYP-160", format!("type argument `{tp}` of {where_} is fixed by none of the arguments (`null` fixes nothing): pass a value of a typed optional (6.10)"), tok, "an argument that fixes it", "an unfixed type argument", &tok.text, "LANG-06-11");
            }
        }
        for (p, t, e) in &arg_types {
            let Some(e) = *e else { continue };
            let want = self.subst(&p.1, m);
            let core = call_decl.as_ref().map_or(false, |d| crate::decls::is_standard(&d.module));
            if call_decl.as_ref().map_or(false, |d| matches!(d.kind.as_str(), "enum" | "type") && !self.dc.view_newtype(&named_of_decl(d, vec![]))) && !core {
                self.check_stored_type(&self.with_closure(&want, t), ftok(e), true);
            }
            if p.2.trim_start_matches('.') == "owned" && !core {
                let mut callables = Vec::new();
                Self::closure_values(t, &want, &mut callables);
                for (ho, capture_origins, wo) in callables {
                    let mut roots: Vec<String> = capture_origins.iter().map(|(o, _)| o.clone()).collect();
                    if !ho.is_empty() && roots.is_empty() {
                        let path = self.path_of_expr(Some(e));
                        roots = self.own.loans.iter().filter_map(|l| {
                            let l = l.borrow();
                            if path.as_ref().map_or(false, |p| l.holder.as_ref() == Some(&root_of(p))) { Some(l.owner.clone()) } else { None }
                        }).collect();
                        if roots.is_empty() { roots.push(ho.to_string()); }
                    }
                    if roots.iter().any(|r| r != "static") {
                        if wo.is_empty() {
                            self.own_err("E-OWN-053", format!("owned callback `{}` holds captured loans but its parameter type names no region (8.3)", p.0), ftok(e), &p.0, "loan-carrying callback without an origin", "LANG-07-06", "");
                        } else {
                            let source = arg_types.iter().find(|(ap, _, _)| ap.0 == wo).and_then(|(_, _, ae)| self.path_of_expr(*ae))
                                .or_else(|| if wo == "self" { recv.as_ref().map(|r| r.0.clone()) } else { None });
                            let allowed = source.as_ref().map(|p| root_of(&self.own.owner_path(p))).unwrap_or_else(|| wo.to_string());
                            if roots.iter().any(|r| r != "static" && root_of(r) != allowed) {
                                self.own_err("E-OWN-050", format!("owned callback `{}` borrows outside its declared region `{wo}` (8.3)", p.0), ftok(e), &p.0, "callback capture from a different origin", "LANG-07-06", "");
                            }
                        }
                    }
                }
            }
        }
        let mut storage = m.clone();
        for (p, t, _) in &arg_types {
            if matches!(p.2.trim_start_matches('.'), "owned" | "payload") {
                self.storage_bind(&p.1, t, &mut storage);
            } else if let (Type::Fn { ret: pr, .. }, Type::Fn { ret: ar, .. }) = (&p.1, strip(t)) {
                self.storage_bind(pr, ar, &mut storage);
            }
        }
        let storage_result = self.subst(&sig.ret, &storage);
        self.check_stored_type(&storage_result, tok, false);
        if let Some(d) = call_decl.as_ref().filter(|d| d.kind == "class" || d.kind == "enum") {
            self.check_stored_type(&named_of_decl(d, d.type_params.iter().map(|p| storage.get(p).cloned().unwrap_or_else(|| Type::TypeParam(p.clone()))).collect()), tok, false);
        }
        self.check_storage_call(sig, &storage, call_decl.as_ref(), where_, tok);
        let built = self.widened_args(&sig.ret, m, &fixed);
        let result = self.result_of_call(sig, built.as_ref().unwrap_or(m), unknown_args);
        self.check_stored_type(&result, tok, false);
        result
    }

    /// The type arguments the result is built with (6.10, X-513): one that only the arguments fixed and that the result
    /// names inside a type argument (`List.of`'s `List[T]`) takes an integer argument's base type, as an unannotated
    /// binding does (5.1), the interval staying a fact of the argument, so `List.of(i)` with `i: Int` known to be 4 is a
    /// `List[Int]` and not a `List[Int<4..4>]` that type arguments' invariance (6.3) would set apart from `List[Int]`. A
    /// result that is the parameter itself (`same(x)`) keeps the argument's interval, the value passing through. None
    /// when nothing widens.
    fn widened_args(&self, ret: &Type, m: &BTreeMap<String, Type>, fixed: &BTreeSet<String>) -> Option<BTreeMap<String, Type>> {
        let inner: Vec<String> = Self::nested_tparams(ret).into_iter().filter(|tp| !fixed.contains(tp)).collect();
        let narrowed = |t: &Type| matches!(strip(t), Type::Refined { .. }) && self.is_int(strip(t));
        if !inner.iter().any(|tp| m.get(tp).map_or(false, |t| narrowed(t))) {
            return None;
        }
        let mut out = m.clone();
        for tp in &inner {
            if let Some(t) = out.get(tp).cloned() {
                if narrowed(&t) {
                    out.insert(tp.clone(), self.widened(strip(&t)));
                }
            }
        }
        Some(out)
    }

    /// The type parameters a type names inside a type argument (`T` in `List[T]` and `Box[T]?`, not in `T?`).
    fn nested_tparams(t: &Type) -> BTreeSet<String> {
        match t {
            Type::Named(n) => n.args.iter().flat_map(|a| Self::tparams_of(a)).collect(),
            Type::Optional(i) => Self::nested_tparams(i),
            Type::Loan { inner, .. } => Self::nested_tparams(inner),
            Type::Tuple(xs) => xs.iter().flat_map(|x| Self::nested_tparams(x)).collect(),
            _ => BTreeSet::new(),
        }
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
                // the default is the parameter's own `= constExpr`, never a bound inside its refined type (D-62)
                return p.child_node("constExpr").is_some();
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
    /// Arguments that disagree are joined to their common type (6.10, X-513): a type parameter an earlier argument fixed
    /// is widened to a later argument's type when that contains it, or to the hull of two integer refinements of one
    /// base (`List.of(small, big)`); a parameter the receiver or the expected type fixed stays, and every argument is
    /// checked against it. Anything else is left for the argument check to report.
    fn join_pin(&self, pt: &Type, t: &Type, m: &mut BTreeMap<String, Type>, fixed: &BTreeSet<String>) {
        let mut s = strip(pt);
        if let Type::Optional(i) = s {
            s = i;
        }
        let Type::TypeParam(n) = s else { return };
        if !m.contains_key(n) || fixed.contains(n) || !known(strip(t)) {
            return;
        }
        let mut a = crate::literals::concrete(strip(t)).clone();
        if let (Type::Optional(i), Type::Optional(_)) = (&a, strip(pt)) {
            a = (**i).clone();
        }
        let exact = a.clone(); // a literal joins by its value, the hull of [9, 9] and Int<0..3> being Int<0..9>
        if let Type::Refined { base, kind, .. } = &a {
            if kind == "exact" {
                a = Type::Named(base.clone());
            }
        }
        let cur = m[n].clone();
        if a == never() || !known(&a) || assignable(&exact, &cur).0 == Some(true) {
            return;
        }
        if assignable(&cur, &a).0 == Some(true) {
            m.insert(n.clone(), a);
            return;
        }
        if let (Some(cb), Some(ab)) = (int_bounds(&cur), int_bounds(&exact)) {
            if core_name(&cur) == core_name(&a) {
                let base = match &cur {
                    Type::Refined { base, .. } => base.clone(),
                    Type::Named(b) => b.clone(),
                    _ => return,
                };
                let (lo, hi) = (cb.0.min(ab.0), cb.1.max(ab.1));
                let full = int_types(&base.name).unwrap_or((lo, hi));
                m.insert(n.clone(), if (lo, hi) == full { Type::Named(base) } else { Type::Refined { base, lo: Some(lo), hi: Some(hi), kind: "range".into() } });
            }
        }
    }

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
            (Type::Named(p), Type::Named(a)) if (p.name == a.name && p.package == a.package && p.origin == a.origin) => {
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

    /// Callable values in an owned argument, including aggregate holders (8.3).
    pub(super) fn closure_values<'t>(have: &'t Type, want: &'t Type, out: &mut Vec<(&'t str, &'t [(String, String)], &'t str)>) {
        let (have, want) = (strip(have), strip(want));
        match (have, want) {
            (h, Type::Optional(w)) => Self::closure_values(h.opt_inner().unwrap_or(h), w, out),
            (Type::Fn { origin: ho, capture_origins, .. }, Type::Fn { origin: wo, .. }) => out.push((ho, capture_origins, wo)),
            (Type::Named(h), Type::Named(w)) => for (h, w) in h.args.iter().zip(&w.args) { Self::closure_values(h, w, out); },
            (Type::Tuple(h), Type::Tuple(w)) => for (h, w) in h.iter().zip(w) { Self::closure_values(h, w, out); },
            (Type::Record(h), Type::Record(w)) => for (k, w) in w {
                if let Some((_, h)) = h.iter().find(|(n, _)| n == k) { Self::closure_values(h, w, out); }
            },
            _ => {}
        }
    }

    /// Narrow a declared function type to the one lambda that initialises it (8.4).
    ///
    /// The declared signature stays; an empty identity is filled in except for `dynamic fn`.
    /// Capture provenance is always retained, including at a dynamic or already fixed position.
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
        if let (Type::Fn { params, ret, kind, modifiers, origin, dynamic, identity, param_modes, capture_origins }, Type::Fn { identity: hid, dynamic: hd, origin: ho, capture_origins: hc, .. }) = (ws, hs) {
            let mut origins = capture_origins.clone();
            origins.extend(hc.iter().cloned());
            if !ho.is_empty() && ho != origin { origins.push((ho.clone(), "alias".into())); }
            origins.sort();
            origins.dedup();
            return Type::Fn {
                params: params.clone(), ret: ret.clone(), kind: kind.clone(), modifiers: modifiers.clone(),
                origin: origin.clone(), dynamic: *dynamic,
                identity: if !*dynamic && identity.is_empty() && !*hd { hid.clone() } else { identity.clone() },
                param_modes: param_modes.clone(), capture_origins: origins,
            };
        }
        if let (Type::Named(w), Type::Named(h)) = (ws, hs) {
            if (w.name == h.name && w.package == h.package && w.origin == h.origin) && w.args.len() == h.args.len() {
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
        if let Type::Fn { capture_origins, dynamic, identity, params, ret, kind, modifiers, origin, param_modes } = slot {
            if *dynamic || !identity.is_empty() {
                return slot.clone();
            }
            let key = Self::field_pin_key(decl, field, path);
            let ident = self.dc.sess.field_closures.borrow().get(&key).cloned();
            if let Some(ident) = ident {
                return Type::Fn { capture_origins: capture_origins.clone(), params: params.clone(), ret: ret.clone(), kind: kind.clone(), modifiers: modifiers.clone(), origin: origin.clone(), dynamic: false, identity: ident, param_modes: param_modes.clone() };
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
        self.check_stored_type(&self.with_closure(field_ty, value_ty), tok, true);
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
                    let names = diagnostic_types(&[value, slot_s]);
                    let (shown, expected) = (&names[0], &names[1]);
                    self.typ("E-TYP-040", format!("field `{field}` has type {shown}, expected {expected}: {CLOSURE_WHY}"), tok, &expected, &shown, field, "LANG-08-04");
                }
                _ => {}
            }
            return;
        }
        match (slot_s, value_s) {
            (Type::Named(s), Type::Named(v)) if (s.name == v.name && s.package == v.package && s.origin == v.origin) => {
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
    /// A value converts to `dynamic I` only from a type that implements `I` with the written arguments, from a type
    /// parameter whose bound grants `I`, or from a `dynamic J` where `J` is `I` or requires it (9.5); E-TYP-040
    /// otherwise. True when refused.
    pub fn dynamic_conversion_refused(&mut self, have: &Type, want: &Type, tok: &Token, what: &str) -> bool {
        let Type::Dynamic(wi) = strip(want).clone() else { return false };
        let hs = strip(have).clone();
        match &hs {
            Type::Dynamic(hi) => {
                if self.grants(hi, &wi, &[]) {
                    return false;
                }
                let shown = diagnostic_types(&[have, want, &hi.as_type(), &wi.as_type()]);
                self.typ("E-TYP-040", format!("{what} has type {}, expected {}: {} is not {} and does not require it (9.5)", shown[0], shown[1], shown[2], shown[3]), tok, &shown[1], &shown[0], what, "LANG-09-07");
                true
            }
            Type::Named(_) | Type::Refined { .. } | Type::TypeParam(_) => {
                let a = if crate::literals::is_strlit(&hs) { string() } else { hs.clone() };
                if self.bound_holds(&a, &wi) == Some(false) {
                    let names = diagnostic_types(&[&hs, want, &wi.as_type()]);
                    let shown = if crate::literals::is_strlit(&hs) { "a string literal" } else { &names[0] };
                    self.typ("E-TYP-040", format!("{what} has type {shown}, expected {}: {shown} does not implement {} (9.5)", names[1], names[2]), tok, &names[1], shown, what, "LANG-09-07");
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    pub fn expect(&mut self, have: &Type, want: &Type, tok: &Token, what: &str) {
        self.check_static_source(have, want, tok, what);
        self.expect_type(have, want, tok, what);
    }

    pub fn expect_return(&mut self, have: &Type, want: &Type, tok: &Token, what: &str) {
        // Returned value origins use check_returned_loans' expression and region evidence. Callable signatures
        // cannot strengthen a result's origin or weaken a static parameter's precondition.
        self.check_static_signatures(have, want, tok, what);
        self.expect_type(have, want, tok, what);
    }

    fn expect_type(&mut self, have: &Type, want: &Type, tok: &Token, what: &str) {
        self.dc.check_linear_instantiation(want, &self.linear_in_scope, tok);
        // a class value, or a bounded `T`'s, made an owned `dynamic I` is boxed: the creation allocates (Appendix C.3;
        // X-117; D-88)
        if matches!(want, Type::Dynamic(_)) && matches!(strip(have), Type::Named(_) | Type::Refined { .. } | Type::TypeParam(_)) {
            self.eff.add("alloc", "proved", Some(tok));
        }
        if self.dynamic_conversion_refused(have, want, tok, what) {
            return;
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
            if let Some((ha, wa)) = self.nested_unproved(&hs, &ws) {
                self.ref_(
                    "E-REF-001",
                    format!("{what} has type {}, whose type argument {} is not proved within {} (unproved): a refinement inside a type argument is proved like the type itself (6.3); validate each element", hs.show(), ha.show(), wa.show()),
                    tok,
                    "unproved",
                    &wa.show(),
                    "?",
                    "",
                );
                return;
            }
        }
        if ok == Some(false) {
            let shown_types = diagnostic_types(&[have, want]);
            let shown = if is_strlit(have) { "a string literal".to_string() } else { shown_types[0].clone() };
            let wanted = &shown_types[1];
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
            self.typ("E-TYP-040", format!("{what} has type {shown}, expected {wanted}: {why}"), tok, wanted, &shown, what, rule);
        }
    }

    /// A refined type argument of `want` that the matching argument of `have` is not proved to satisfy (6.3, X-514;
    /// D-465): the pair, or none. A refinement inside a type argument is proved like the type itself, and a value no
    /// literal built is never proved element by element.
    fn nested_unproved(&self, have: &Type, want: &Type) -> Option<(Type, Type)> {
        let (hs, ws) = (strip(have), strip(want));
        match (hs, ws) {
            (hs, Type::Refined { kind, .. }) if matches!(kind.as_str(), "range" | "scalars" | "length") && known(hs) && assignable(hs, ws).0.is_none() => Some((hs.clone(), ws.clone())),
            (Type::Optional(a), Type::Optional(b)) => self.nested_unproved(a, b),
            (a, Type::Optional(b)) => self.nested_unproved(a, b),
            (Type::Record(xs), Type::Record(ys)) => xs.iter().find_map(|(k, x)| ys.iter().find(|(l, _)| k == l).and_then(|(_, y)| self.nested_unproved(x, y))),
            (Type::Tuple(xs), Type::Tuple(ys)) => xs.iter().zip(ys.iter()).find_map(|(a, b)| self.nested_unproved(a, b)),
            (Type::Named(h), Type::Named(w)) if (h.name == w.name && h.package == w.package && h.origin == w.origin) && h.args.len() == w.args.len() => {
                for (a, b) in h.args.iter().zip(w.args.iter()) {
                    let (a2, b2) = (strip(a), strip(b));
                    if matches!(b2, Type::Refined { kind, .. } if kind == "range" || kind == "scalars" || kind == "length") && known(a2) && assignable(a2, b2).0.is_none() {
                        return Some((a2.clone(), b2.clone()));
                    }
                    if let Some(r) = self.nested_unproved(a, b) {
                        return Some(r);
                    }
                }
                None
            }
            _ => None,
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
            // LR 6.3/6.10: branch values join by the hull of their intervals.
            if let (Some(rb), Some(tb)) = (int_bounds(&r), int_bounds(&t)) {
                if core_name(&r) == core_name(&t) {
                    let base = match &r {
                        Type::Refined { base, .. } => base.clone(),
                        Type::Named(base) => base.clone(),
                        _ => unreachable!(),
                    };
                    let (lo, hi) = (rb.0.min(tb.0), rb.1.max(tb.1));
                    result = Some(if Some((lo, hi)) == int_types(&base.name) { Type::Named(base) } else {
                        Type::Refined { base, lo: Some(lo), hi: Some(hi), kind: "range".into() }
                    });
                    continue;
                }
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
                let names = diagnostic_types(&[&t, &r]);
                self.typ("E-TYP-040", format!("{what} yields {} where the other branches yield {}", names[0], names[1]), tok, &names[1], &names[0], what, rule);
            }
        }
        if let Some(r) = result.as_mut() {
            for (t, _) in types { *r = self.with_closure(r, t); }
        }
        if let Some(r) = result.as_ref() {
            let target = expected.filter(|t| known(t) && !has_free(t)).unwrap_or(r);
            for (t, tok) in types { self.check_static_source(t, target, tok, "branch result"); }
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
        let incoming = self.carried.clone();
        let mut saved_borrows = std::mem::take(&mut self.temporary_borrows);
        // Only function values consult this context. Keep its saved state small across recursive inference.
        let storage_expected = expected.filter(|t| matches!(strip(t), Type::Fn { .. })).map(|t| Box::new(t.clone()));
        let saved_storage_expected = std::mem::replace(&mut self.storage_expected, storage_expected);
        let ops = node.map(|n| self.test_region(n)).unwrap_or_default();
        let mut t = if ops.is_empty() {
            self.infer_raw(node, env, expected)
        } else {
            // unproved `Int` arithmetic in a test body is a checked region with an implied `or fail` (14, 6.4; X-345)
            let first = *ops.iter().next().unwrap();
            self.regions.push((ops, Vec::new(), false));
            let t = self.infer_raw(node, env, expected);
            let (_, checked, _) = self.regions.pop().unwrap();
            if let Some(op) = checked.first() {
                self.eff.add("fail", "proved", Some(op));
                self.unwind_point(op, "or fail", env, None);
                if self.defer_depth > 0 {
                    self.def_err("E-DEF-001", "failing arithmetic inside a deferred block: there is no outcome to propagate to (12.1)".into(), op, "or fail", "LANG-12-01");
                }
                self.implied_regions.insert(first);
            }
            t
        };
        if let Some(n) = node {
            if self.test_body && (n.name() == "postfixExpr" || n.name() == "markerExpr") && !self.consumed_ops.contains(&n.id()) {
                t = self.implied_or_fail(n, t, env);
            }
        }
        // a call whose arguments do not fix its type arguments, with no expected type to fix them (6.10): a proved
        // inference failure, repaired by annotating the receiving binding
        if let (Type::Unknown(why), None, Some(n)) = (&t, expected, node) {
            if why == "generic result" && !self.branch_target {
                // each enclosing expression node sees the same result; the record is kept once per line and message
                let tok = ftok(n);
                self.typ("E-TYP-160", "the type arguments of this call are not fixed by its arguments; annotate the receiving binding (6.10)".to_string(), tok, "an annotated binding", "uninferred type arguments", &tok.text, "LANG-06-11");
            }
        }
        if let Some(n) = node {
            self.dc.check_linear_instantiation(&t, &self.linear_in_scope, ftok(n));
            self.dc.check_keys(&t, ftok(n), None, true);
            self.check_stored_type(&t, ftok(n), false);
        }
        if let (Some(nt_), Some(n)) = (self.node_types.as_mut(), node) {
            nt_.insert(n.id(), t.clone());
        }
        if let Some(n) = node {
            if self.temporary_value(Some(n), Some(&t)) { self.temporary_values.insert(n.id()); }
        }
        if self.borrows_value(&t) { saved_borrows.append(&mut self.temporary_borrows); }
        self.temporary_borrows = saved_borrows;
        self.storage_expected = saved_storage_expected;
        if known(&t) && !has_free(&t) && !self.carries_onward(&t) {
            // End only this expression's carried operand loans (D-709, D-720).
            let expired: Vec<_> = self.carried.iter().filter(|l| !incoming.iter().any(|old| Rc::ptr_eq(l, old))).cloned().collect();
            self.own.loans.retain(|l| !expired.iter().any(|old| Rc::ptr_eq(l, old)));
            self.carried.retain(|l| incoming.iter().any(|old| Rc::ptr_eq(l, old)));
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
                return self.coalesce_expr(node, &kids, env);
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
                    if known(strip(&t)) && core_name(strip(&t)) != Some("Bool") {
                        let names = diagnostic_types(&[&t, &boolean()]);
                        self.typ("E-TYP-040", format!("`{opt}` operand has type {}, expected {}", names[0], names[1]), ctok(*k), &names[1], &names[0], &opt, "LANG-06-01");
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
                    let base_is_int = core_name(strip(&t)) == Some("Int");
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
                // `s == Error`: the right operand is expected to have the left operand's enum (9.7, X-345)
                let want = self.enum_operand(&l);
                let r = self.infer_c(kids[2], env, want.as_ref());
                let op = kids[1].tok().unwrap().clone();
                self.compare(&op, &l, &r);
                return boolean();
            }
            "rangeExpr" => {
                for k in kids.iter().step_by(2) {
                    let t = self.infer_c(*k, env, None);
                    if known(strip(&t)) && int_bounds(strip(&t)).is_none() {
                        let displays = diagnostic_types(&[&t, &named("Int")]);
                        self.typ("E-TYP-040", format!("range bound has type {}, expected an integer", displays[0]), ctok(*k), &displays[1], &displays[0], "range", "LANG-06-01");
                    }
                }
                return unknown("range");
            }
            "asExpr" => {
                if kids.len() == 1 {
                    return self.infer_c(kids[0], env, expected);
                }
                return self.conversion(node, env);
            }
            "unaryExpr" => {
                if let C::T(optok) = kids[0] {
                    let op = optok.text.clone();
                    let t = self.infer_c(kids[1], env, if op == "-" { expected } else { None });
                    let s = strip(&t).clone();
                    if op == "!" {
                        if known(&s) && core_name(&s) != Some("Bool") {
                            let names = diagnostic_types(&[&t, &boolean()]);
                            self.typ("E-TYP-040", format!("`!` operand has type {}, expected {}", names[0], names[1]), ctok(kids[1]), &names[1], &names[0], "!", "LANG-06-01");
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
            "tupleLit" => return self.tuple_lit(node, env, expected),
            "recordLit" => return self.record_lit(node, &kids, env, expected),
            "argList" => {
                // the arguments of a call the checker could not resolve: every expression in them is typed with no
                // context, which proves nothing about what it expects (D-304)
                self.unresolved_args += 1;
                for a in node.find_all("expr") {
                    self.infer(Some(a), env, None);
                }
                self.unresolved_args -= 1;
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

    /// `a ?? b` (6.8, 7.5, 7.9). Its own function, so that `infer_raw`'s frame stays small enough for a source nested
    /// to the quota (E-LIM-002).
    #[inline(never)]
    fn coalesce_expr(&mut self, node: N<'a>, kids: &[C<'a>], env: &Env) -> Type {
        let left = self.infer_c(kids[0], env, None);
        self.settle_element(); // `move p ?? …` moves `p` on every path; `copy p` is done (7.5; D-357)
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
        let Type::Optional(i) = &ls else { return unknown("") };
        self.expect(&right, i, ctok(kids[2]), "`??` fallback");
        let res = self.coalesce_result(i, &right, kids[0].node(), kids[2].node(), env);
        let lp = self.path_of_expr(kids[0].node());
        let left_n = kids[0].node().unwrap();
        let bare_owned = lp.is_some() && left_n.find("markerExpr").is_none() && self.is_bearing(i) && !i.is_loan() && self.lent_operand(Some(left_n), env).is_none();
        if let (true, Some(lp)) = (bare_owned, &lp) {
            if !res.is_loan() {
                // the left operand owns its value and the result is owned: in a position that takes ownership
                // (`let x = p ?? move d`) the result and `p` would own one value each, so the marker rule of
                // `let x = p` applies there (7.5; D-357); lent for a call or an interpolation, the value is a
                // read loan of `p`, as `p` is
                self.coalesced_places.insert(node.id(), lp.clone());
            } else if self.use_ok {
                // the result lends on the fallback path, so it is a loan of `p` on the other (7.3)
                let d = self.depth;
                let l = self.own.add_loan(lp, "alias", None, d, ctok(kids[0]).line);
                l.borrow_mut().nested = true;
                self.carried.push(l);
            }
        }
        // a fallback of the expression's own beside a lent left side: whatever receives the value owns the
        // fallback, and a loan drawn from it is a loan of that owner (7.9; D-366, D-369)
        let owned = self.owns_value(kids[2].node(), &right, env) || self.has_owned_part(kids[2].node());
        self.note_owned_part(node, &res, owned);
        res
    }

    #[inline(never)]
    fn tuple_lit(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let es = node.child_nodes("expr");
        let exp = expected.map(peel_optional);
        let mut items = Vec::new();
        let mut owned = false;
        for (i, e) in es.into_iter().enumerate() {
            let child = match exp {
                Some(Type::Tuple(ts)) if i < ts.len() => Some(&ts[i]),
                _ => None,
            };
            let t = self.infer(Some(e), env, child);
            owned = owned || self.owns_value(Some(e), &t, env) || self.has_owned_part(Some(e));
            items.push(t);
            self.settle_element(); // a move in this element is consumed before the next one (D-159)
        }
        let tt = Type::Tuple(items);
        self.note_owned_part(node, &tt, owned); // a tuple owns the parts beside its loans (7.9; D-366)
        tt
    }

    #[inline(never)]
    fn record_lit(&mut self, node: N<'a>, kids: &[C<'a>], env: &Env, expected: Option<&Type>) -> Type {
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
        let mut owned = false;
        for (k, e) in ks.iter().zip(es.iter()) {
            let child = em.get(k.text.as_str()).copied();
            let t = self.infer(Some(*e), env, child);
            owned = owned || self.owns_value(Some(*e), &t, env) || self.has_owned_part(Some(*e));
            self.settle_element();
            fields.push((k.text.clone(), t));
        }
        let rt = Type::Record(fields);
        self.note_owned_part(node, &rt, owned); // a record owns the fields beside its loans (7.9; D-366)
        rt
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
            Type::Named(n) if ((n.package.is_empty() && n.name == "Future") || (n.package.is_empty() && n.name == "Task")) && !n.args.is_empty() => self.carries_onward(&n.args[0]),
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
        let keep_future = matches!(strip(t), Type::Named(n) if (n.package.is_empty() && n.name == "Future") && !n.args.is_empty() && self.loan_carrying(&n.args[0]));
        for l in self.carried.clone() {
            if self.own.has_loan(&l) && !keep_future {
                self.own.remove_loan(&l);
            }
        }
        let own = &self.own;
        self.carried.retain(|l| own.has_loan(l));
        if let Type::Named(n) = s {
            if ((n.package.is_empty() && n.name == "Future") || (n.package.is_empty() && n.name == "Task")) && !n.args.is_empty() {
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
        let lb = core_name(&ls).map(|s| s.to_string());
        let rb = core_name(&rs).map(|s| s.to_string());
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
        let names = diagnostic_types(&[&ls, &rs]);
        self.typ("E-TYP-040", format!("`{}` is not defined on {} and {}{hint}", op.text, names[0], names[1]), op, "integer or float operands", &format!("{}, {}", names[0], names[1]), &op.text, "LANG-06-01");
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
        let is_int = base.name == "Int" || base.name == "Int64";
        let in_region = self.regions.last().map_or(false, |rg| rg.0.contains(&(op.line, op.col)));
        if in_region && is_int && CHECKED_OPS.contains(&o) && !Self::proved(o, full, a, b, r) {
            // the checked operation (6.4, X-345): it fails as `Int.add` and its family fail, and its success value has
            // the operation's interval clipped to the type's range, as theirs does (6.3)
            self.regions.last_mut().unwrap().1.push(op.clone());
            self.checked_ops.insert((op.line, op.col));
            return match r.and_then(|r| iv::meet(r, full)) {
                Some(c) => refined(base.clone(), Some(c.0), Some(c.1), "range"),
                None => Type::Named(base.clone()),
            };
        }
        let checked_hint;
        let checked = if is_int && CHECKED_OPS.contains(&o) {
            let form = if o == "-" && a == (0, 0) { "-x".to_string() } else { format!("a {o} b") };
            checked_hint = format!("{checked}, or write the operator as the checked operation: `{form} or fail` (or `or recover`, `catch`)");
            checked_hint.as_str()
        } else if !is_int {
            // the sized integer types have no checked family (6.4): only a proof makes the operator safe
            checked_hint = format!("a refined type or a test that proves it ({} has no checked operation)", base.name);
            checked_hint.as_str()
        } else {
            checked_hint = format!("a refined type or a test that proves it (`{o}` has no checked operation)");
            checked_hint.as_str()
        };
        if in_region && !Self::proved(o, full, a, b, r) {
            self.regions.last_mut().unwrap().2 = true; // rejected below: the region's handler adds no record of its own (6.4)
        }
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

    /// `Int.add(a, b) or fail` and its family, whose operator would be unproved, written as the checked operator
    /// `a + b or fail` (6.4, X-345): a rewrite for the formatter. One line only, so no comment is inside. Under `or fail`,
    /// an operand that is itself such a call under `or fail` joins the region: both propagate the same failure.
    fn note_operator_form(&mut self, left: C<'a>, or_fail: bool) {
        let Some(prim) = Self::unwrap_to(left, "primaryExpr") else { return };
        let Some((text, _)) = self.operator_form(prim, or_fail) else { return };
        let toks = prim.tokens();
        let (first, last) = (toks[0], *toks.last().unwrap());
        self.migrations.push(crate::migrate::Edit { from: (first.line, first.col), to: (last.line, crate::decls::tok_end(last)), text, rule: "R14" });
    }

    /// The node a chain of single-child wrappers leads to, when it is a `name` node.
    fn unwrap_to(c: C<'a>, name: &str) -> Option<N<'a>> {
        let mut c = c;
        while let Some(n) = c.node() {
            if n.name() == name {
                return Some(n);
            }
            let ks = n.kids();
            if ks.len() != 1 {
                return None;
            }
            c = ks[0];
        }
        None
    }

    /// The operator form of a checked call `Int.add(a, b)` whose operator would be unproved, with its precedence.
    fn operator_form(&self, prim: N<'a>, or_fail: bool) -> Option<(String, u8)> {
        if self.int_calls.get(&prim.id()) != Some(&true) {
            return None;
        }
        let m = prim.child_node("memberName")?.tokens()[0].text.clone();
        let op = match m.as_str() {
            "add" => "+",
            "sub" => "-",
            "mul" => "*",
            "div" => "/",
            _ => "%",
        };
        let args = Self::arg_nodes(prim.find("argList"));
        let toks = prim.tokens();
        if args.len() != 2 || toks[0].line != toks.last().unwrap().line {
            return None;
        }
        let prec = |o: &str| -> u8 {
            match o {
                "*" | "/" | "%" => 4,
                "+" | "-" => 5,
                _ => 6,
            }
        };
        // an operand keeps its own grouping: parentheses where its top operator binds looser than `op` (or as loose,
        // on the right, since the operators associate to the left)
        let operand = |x: N<'a>, right: bool| -> String {
            let mut top: u8 = 0;
            let mut text = source_text(C::N(x));
            let mut cur = C::N(x);
            loop {
                let Some(n) = cur.node() else { break };
                let ks = n.kids();
                if ks.len() == 1 {
                    cur = ks[0];
                    continue;
                }
                top = match n.name() {
                    "multiplicativeExpr" | "additiveExpr" | "shiftExpr" => ks.get(1).and_then(|k| k.tok()).map_or(9, |t| prec(&t.text)),
                    "unaryExpr" if ks[0].tok().map_or(false, |t| t.text == "-") => 2,
                    "outcomeExpr" if or_fail && ks.len() == 3 && ks[1].tok().map_or(false, |t| t.text == "or") && ks[2].tok().map_or(false, |t| t.text == "fail") => {
                        match Self::unwrap_to(ks[0], "primaryExpr").and_then(|p| self.operator_form(p, true)) {
                            Some((inner, p)) => {
                                text = inner;
                                p
                            }
                            None => 9,
                        }
                    }
                    "postfixExpr" | "primaryExpr" | "markerExpr" | "unaryExpr" | "literal" => 0,
                    _ => 9,
                };
                break;
            }
            if top > prec(op) || (right && top == prec(op)) {
                format!("({text})")
            } else {
                text
            }
        };
        let (a, b) = (Self::arg_expr(args[0])?, Self::arg_expr(args[1])?);
        Some((format!("{} {op} {}", operand(a, false), operand(b, true)), prec(op)))
    }

    /// Whether the interval rules prove an integer operation safe (6.4): no division by zero, no out-of-range shift, no
    /// `MIN / -1` and no overflow.
    fn proved(o: &str, full: iv::Interval, a: iv::Interval, b: iv::Interval, r: Option<iv::Interval>) -> bool {
        let Some(r) = r else { return false };
        if o == "/" && a.0 <= full.0 && b.0 <= -1 && -1 <= b.1 {
            return false;
        }
        full.0 <= r.0 && r.1 <= full.1
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
            if ["Float", "Float32"].contains(&core_name(&a).unwrap_or("")) && core_name(&b) == core_name(&a) {
                return;
            }
            let (ok, _) = assignable(&a, &b);
            let (ok2, _) = assignable(&b, &a);
            if ok == Some(false) && ok2 == Some(false) {
                let rule = if ls.is_optional() != rs.is_optional() { "LANG-06-08" } else { "LANG-06-01" };
                let names = diagnostic_types(&[&ls, &rs]);
                self.typ("E-TYP-040", format!("`{}` compares {} with {}", op.text, names[0], names[1]), op, &names[0], &names[1], &op.text, rule);
                return;
            }
            if !self.dc.implements(&a, "Equatable") {
                let displays = diagnostic_types(&[&a, &named("Equatable")]);
                self.typ("E-TYP-040", format!("`{}` needs {expected}; {} does not implement it", op.text, displays[0], expected = displays[1]), op, &displays[1], &displays[0], &op.text, "LANG-06-01");
            }
            return;
        }
        let (a, b) = (ls, rs);
        if ["Float", "Float32"].contains(&core_name(&a).unwrap_or("")) && core_name(&b) == core_name(&a) {
            return;
        }
        let (ok, _) = assignable(&a, &b);
        let (ok2, _) = assignable(&b, &a);
        if ok == Some(false) && ok2 == Some(false) {
            let names = diagnostic_types(&[&a, &b]);
            self.typ("E-TYP-040", format!("`{}` compares {} with {}", op.text, names[0], names[1]), op, &names[0], &names[1], &op.text, "LANG-06-01");
            return;
        }
        if !self.dc.implements(&a, "Comparable") {
            let displays = diagnostic_types(&[&a, &named("Comparable")]);
            self.typ("E-TYP-040", format!("`{}` needs {expected}; {} does not implement it", op.text, displays[0], expected = displays[1]), op, &displays[1], &displays[0], &op.text, "LANG-06-01");
        }
    }

    pub fn outcome(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        let kids = node.kids();
        let words: Vec<String> = kids.iter().filter_map(|k| k.tok()).map(|t| t.text.clone()).collect();
        if kids.len() > 1 {
            self.mark_consumed(kids[0].node()); // `or` and `catch` consume the outcome themselves (14)
        }
        // the left operand of an outcome operator, when it is integer arithmetic, is a checked region: an `Int` operator
        // in it that the interval rules cannot prove safe is the checked operation, and the region fails as it fails
        // (6.4, X-345)
        let ops = if kids.len() > 1 { region_operators(kids[0]) } else { BTreeSet::new() };
        let is_region = !ops.is_empty();
        if is_region {
            self.regions.push((ops, Vec::new(), false));
        }
        let mut left = self.infer_c(kids[0], env, if kids.len() == 1 { expected } else { None });
        let region = if is_region { self.regions.pop() } else { None };
        if kids.len() == 1 {
            return left;
        }
        if self.final_pass && !is_region {
            let or_fail = words.len() >= 2 && words[0] == "or" && words[1] == "fail";
            self.note_operator_form(kids[0], or_fail);
        }
        if region.as_ref().map_or(false, |r| !r.1.is_empty()) {
            left = named_args("Outcome", vec![left]);
        }
        let ls = strip(&left).clone();
        let inner = match &ls {
            Type::Named(n) if (n.package.is_empty() && n.name == "Outcome") && !n.args.is_empty() => n.args[0].clone(),
            _ => {
                if known(&ls) {
                    if region.as_ref().is_some_and(|r| r.2) {
                        return unknown(""); // an operator the region could not check is E-REF-002 already (6.4)
                    }
                    let w = words[..words.len().min(2)].join(" ");
                    let msg = if region.is_some() {
                        format!("`{w}` needs an Outcome left operand: this arithmetic is proved safe ({}) and cannot fail, so drop `{w}`", ls.show())
                    } else {
                        format!("`{w}` needs an Outcome left operand, got {}", ls.show())
                    };
                    self.typ("E-TYP-040", msg, ctok(kids[0]), "Outcome[T]", &ls.show(), &words[0], "LANG-06-08");
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
            // an `or recover` whose value side lends and whose replacement is the expression's own (7.9; D-366, D-369)
            let owned = self.owns_value(last.node(), &r, env) || self.has_owned_part(last.node()) || self.has_owned_part(kids[0].node());
            self.note_owned_part(node, &target, owned);
            if known(&target) {
                if r == null_t() && !strip(&target).is_optional() {
                    return optional(target);
                }
                self.expect(&r, &target, ctok(last), "`or recover` value");
                self.loan_into_owned(&r, Some(&target), ctok(last), "the `or recover` value");
            }
            return target;
        }
        let ident = kids.iter().filter_map(|k| k.tok()).find(|t| t.kind == Kind::Lower).unwrap().clone();
        let e = env_new(Some(env));
        env_set(&e, &ident.text, named("Failure"), false, "catch");
        self.ctx.push("catch".into());
        self.arm_types.push(vec![]);
        self.arm_loans.push(vec![]);
        self.arm_owned.push(false);
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
        let handed = self.arm_loans.pop().unwrap_or_default();
        let owned = self.arm_owned.pop().unwrap_or(false);
        self.ctx.pop();
        if known(&target) && rec.iter().any(|(t, _)| *t == null_t()) && !strip(&target).is_optional() {
            target = optional(target);
        }
        for (t, tok) in &rec {
            if known(&target) {
                self.expect(t, &target, tok, "`recover` value");
                self.loan_into_owned(t, Some(&target), tok, "the `recover` value");
            }
        }
        self.take_handed(handed); // a recovered loan goes to the expression's consumer (7.3; D-355)
        let owned = owned || self.has_owned_part(kids[0].node());
        self.note_owned_part(node, &target, owned);
        target
    }

    // ---- conversions (6.13, X-475) ---------------------------------------------------------------------------

    /// `x as T` (6.13, X-475): the operand converted by the one conversion declared for the pair (its type, `T`):
    /// a row of the built-in table (`crate::conversions`), or a base type into a refinement of it, the old
    /// `validate` (6.3). A total conversion has type `T`; a fallible one `Outcome[T]`, handled on the line like any
    /// outcome. A pair with no conversion is `E-TYP-180`; the identity, and a refinement to its base or a wider
    /// refinement (subtyping, 6.3), is `E-TYP-181`. The chain reads left to right. The operand is passed as to a
    /// call with one read parameter (5.7): a bare ownership-bearing place is loaned for the conversion, `move p`
    /// moves it in, and the loan ends when the conversion returns.
    pub fn conversion(&mut self, node: N<'a>, env: &Env) -> Type {
        let kids = node.kids();
        let operand = kids[0];
        let as_tok = kids[1].tok().unwrap();
        let incoming_loans = self.carried.len();
        let saved_res = std::mem::take(&mut self.own.reservations);
        self.res_stack.push(saved_res.clone());
        self.pending = None;
        // Table fields are typed by use (14). A newtype wrap has exactly one representation source, so it
        // supplies that expected base just as the retired validator's parameter does (D-507).
        let row_source = self.path_of_expr(operand.node()).filter(|path| self.row_name.as_ref().map_or(false, |row| {
            path.strip_prefix(&format!("{row}.")).map_or(false, |field| !field.is_empty() && !field.contains('.'))
        })).and_then(|_| kids.get(2).and_then(|k| k.node())).and_then(|tr| {
            let target = self.dc.type_of_ref(tr);
            self.dc.resolve_type(&target, None).filter(|d| d.kind == "type").and_then(|d| d.alias.clone())
        }).map(|rep| match rep { Type::Refined { base, .. } => base.as_type(), rep => rep });
        let mut t = self.infer_c(operand, env, row_source.as_ref());
        if !known(&t) {
            if let Some(source) = row_source { t = source; }
        }
        if let Some((path, mk, mtok)) = self.pending.take() {
            if mk == "move" {
                self.reserve(&path, "move", &mtok); // moved into the conversion, as into a read parameter (7.5)
            } else if mk == "editable" {
                self.own_err("E-OWN-001", format!("a conversion reads `{path}`: write `{path}` or `alias {path}`, not `editable`"), &mtok, &path, "wrong marker", "LANG-07-01", "");
            }
        } else if let Some(n) = operand.node() {
            if let Some(path) = self.path_of_expr(Some(n)) {
                if n.find("markerExpr").is_none() && self.is_bearing(&t) {
                    self.reserve(&path, "alias", as_tok); // the implicit read loan of a call argument (7.5)
                }
            }
        }
        self.activate(as_tok, false, env, &BTreeMap::new());
        self.res_stack.pop();
        self.own.reservations = saved_res;
        // the operand the first `as` of the chain converts, for a declared member that consumes it (6.13, X-551)
        let mk_node = operand.node().and_then(|n| n.find("markerExpr"));
        let ctx = (operand.node().and_then(|n| self.path_of_expr(Some(n))), mk_node.map(|m| m.tokens()[0].text.clone()));
        let mut i = 1;
        while i < kids.len() {
            let dst = match kids[i + 1].node() {
                Some(tr) => self.dc.type_of_ref(tr),
                None => unknown("conversion target"),
            };
            t = self.convert_once(&t, &dst, kids[i].tok().unwrap(), Some(env), if i == 1 { Some(ctx.clone()) } else { None });
            let result = match &t { Type::Named(n) if n.package.is_empty() && n.name == "Outcome" && !n.args.is_empty() => &n.args[0], _ => &t };
            if known(result) && !self.loan_carrying_or_cursor(result) && !self.holds_loan(result) {
                self.carried.truncate(incoming_loans); // D-419: keep enclosing receiver/argument loans.
            }
            i += 2;
        }
        t
    }

    /// The declared conversion for the pair (`s`, `d`): a `convert to d` member of the source's type, or a
    /// `convert from s` member of the target's (6.13, X-551). A member without `export` resolves only inside the
    /// module that declares it, so the pair has no conversion elsewhere. A refined source is a value of its base
    /// (6.3): the member converts the base's values, as a built-in row does.
    pub fn member_conversion(&self, s: &Type, d: &Type) -> MemberConv<'a> {
        let src = match s {
            Type::Refined { base, .. } => Type::Named(base.clone()),
            _ => s.clone(),
        };
        let mut found: Vec<(DeclRef<'a>, Convert<'a>)> = Vec::new();
        for (t, direction, other) in [(&src, "to", d), (d, "from", &src)] {
            if !matches!(t, Type::Named(_)) {
                continue;
            }
            let Some(decl) = self.decl_of(t) else { continue };
            if decl.kind != "class" && decl.kind != "enum" {
                continue;
            }
            for cvm in self.dc.instantiated_conversions(&decl, t) {
                if cvm.direction == direction && cvm.other == *other && (cvm.exported || decl.module == self.module) {
                    found.push((decl.clone(), cvm.clone()));
                }
            }
        }
        match found.len() {
            0 => MemberConv::None,
            1 => {
                let (decl, cvm) = found.pop().unwrap();
                MemberConv::One(decl, cvm)
            }
            _ => MemberConv::Two,
        }
    }

    pub fn newtype_conversion(&self, src: &Type, dst: &Type) -> Option<(DeclRef<'a>, Type, bool, bool)> {
        self.dc.newtype_conversion(src, dst)
    }

    /// One `as T` of a chain: the conversion for (type of the operand, `T`) and its result type. The IR builder
    /// resolves the same pair again (`resolve_conversion`, `member_conversion`) to name the backends' row or
    /// member. `operand` is the (path, marker) of the chain's operand, for the first `as`: a declared member
    /// written `owned` consumes it, and the use site needs `move` (or `copy`).
    pub fn convert_once(&mut self, src: &Type, dst: &Type, as_tok: &Token, env: Option<&Env>, operand: Option<(Option<String>, Option<String>)>) -> Type {
        self.dc.check_keys(dst, as_tok, None, false);
        let mut s = strip(src).clone();
        if let Type::Loan { inner, .. } = &s {
            s = strip(inner).clone();
        }
        let d = strip(dst).clone();
        if let Some((decl, rep, wrapping, proved)) = self.newtype_conversion(&s, &d) {
            if !wrapping && decl.module != self.module {
                self.rec("E-NAM-004", format!("representation of {} is not visible outside its declaring module {}", decl.name, decl.module), "LANG-06-14", as_tok, Self::nam(&decl.name, vec![]));
                return unknown("private representation");
            }
            let rep_base = if let Type::Refined { base, .. } = &rep { base.as_type() } else { rep.clone() };
            if wrapping && is_strlit(&s) && rep_base == string() {
                self.eff.add("alloc", "proved", Some(as_tok)); // X-17's literal-to-owned-String representation
            }
            if self.is_bearing(&rep) {
                let (path, marker) = operand.clone().unwrap_or((None, None));
                if matches!(src, Type::Loan { .. }) || (path.is_some() && matches!(marker.as_deref(), None | Some("alias" | "editable"))) {
                    let shown = path.as_deref().unwrap_or("the operand");
                    self.own_err("E-OWN-001", format!("a newtype conversion takes ownership of its representation: write `move {shown}` or `copy {shown}`"), as_tok, shown, "move or copy required", "LANG-07-01", "");
                }
            }
            if wrapping && !proved && self.is_linear(&rep) {
                self.obl_err("E-OBL-001", format!("an unproved conversion to {} would discard its linear representation on failure: prove the refinement before wrapping", d.show()), as_tok, &format!("{}:{}", as_tok.line, as_tok.col), "linear representation", "LANG-10-01");
            }
            let view = self.dc.view_newtype(if wrapping { &d } else { &s });
            let mut result = if wrapping { d } else { rep };
            if view {
                let path = operand.as_ref().and_then(|(p, _)| p.as_deref());
                if let Some(p) = path {
                    let loan = self.own.add_loan(p, "alias", None, self.depth, as_tok.line);
                    self.carried.push(loan);
                }
                let origin = if let Type::Loan { origin, .. } = src { origin.clone() } else { path.map(root_of).unwrap_or_else(|| "value".into()) };
                result = Type::Loan { inner: Box::new(result), mode: "alias".into(), origin };
            }
            return if wrapping && !proved { named_args("Outcome", vec![result]) } else { result };
        }
        if is_strlit(&s) {
            s = string_view(); // a string literal is a `StringView from static` (6.6)
        }
        if !known(&s) || !known(&d) {
            return unknown("conversion");
        }
        // the rows are keyed by the built-in types, not by their spellings: a user's class named `Int` is itself,
        // and converts by declaration alone (D-516)
        let core_pair = match (cv::base_of(&s), cv::base_of(&d)) {
            (Some(_), Some(_)) => self.dc.is_core_type(&s) && self.dc.is_core_type(&d),
            _ => false,
        };
        match Self::resolve_conversion(&s, &d, core_pair) {
            Resolved::Identity(why) => {
                self.typ("E-TYP-181", format!("`as {}` converts nothing: the operand already has type {} ({why})", d.show(), s.show()), as_tok, "a different type", &s.show(), "as", "LANG-06-14");
                d
            }
            Resolved::None => {
                match self.member_conversion(&s, &d) {
                    MemberConv::Two => {
                        self.typ("E-TYP-182", format!("two conversions are declared from {} to {}: the source declares `convert to` and the target declares `convert from`: one conversion per pair (6.13)", s.show(), d.show()), as_tok, "one conversion", "two", "as", "LANG-06-15");
                        return unknown("two conversions");
                    }
                    MemberConv::One(decl, cvm) => {
                        let text_copy = cvm.direction == "from" && cvm.owned && cvm.sig.params[0].1 == string() && self.dc.is_core_type(&s) && cv::base_of(&s) == Some("StringView");
                        if text_copy { self.eff.add("alloc", "proved", Some(as_tok)); }
                        if cvm.owned && !text_copy {
                            if let Some((Some(p), omk)) = &operand {
                                // `editable` is refused for every conversion operand where the marker is read
                                // (7.5), so only a bare place and a loan marker reach here
                                if omk.is_none() && self.is_bearing(&s) {
                                    self.own_err("E-OWN-001", format!("`as {}` consumes its source ({} declares it `owned`): write `move {p} as {}` or `copy {p} as {}`", d.show(), decl.name, d.show(), d.show()), as_tok, p, "move or copy required", "LANG-07-01", "");
                                } else if omk.as_deref() == Some("alias") {
                                    self.own_err("E-OWN-001", format!("`as {}` consumes its source: `alias {p}` is a loan; write `move {p} as {}` or `copy {p} as {}`", d.show(), d.show(), d.show()), as_tok, p, "wrong marker", "LANG-07-01", "");
                                }
                            }
                        }
                        if let Some(env) = env {
                            // the member's effects reach the use site, as a callee's reach its caller (11.3)
                            let label = self.method_label(&decl, &cvm.sig.name);
                            let summary = self.callee_summary(Some(&decl), &cvm.sig, &label);
                            self.import_call(&summary, as_tok, None, &[], env, false);
                        }
                        return if cvm.fallible { named_args("Outcome", vec![d.clone()]) } else { d.clone() };
                    }
                    MemberConv::None => {}
                }
                let hint = cv::rounding_hint(cv::base_of(&s).unwrap_or(""), cv::base_of(&d).unwrap_or(""));
                let why = hint.map(|h| format!(": {h}")).unwrap_or_default();
                let names = diagnostic_types(&[&s, &d]);
                self.typ("E-TYP-180", format!("no conversion from {} to {}{why} (6.13)", names[0], names[1]), as_tok, &format!("a conversion from {} to {}", names[0], names[1]), "none", "as", "LANG-06-14");
                unknown("no conversion")
            }
            Resolved::Refine => {
                let result = if self.loan_carrying(&d) {
                    let path = operand.as_ref().and_then(|(p, _)| p.as_deref());
                    if let Some(p) = path {
                        let loan = self.own.add_loan(p, "alias", None, self.depth, as_tok.line);
                        self.carried.push(loan);
                    }
                    let origin = if let Type::Loan { origin, .. } = src { origin.clone() } else { path.map(root_of).unwrap_or_else(|| "value".into()) };
                    Type::Loan { inner: Box::new(d), mode: "alias".into(), origin }
                } else { d };
                named_args("Outcome", vec![result])
            },
            Resolved::Builtin(c) => {
                if c.allocates() {
                    self.eff.add("alloc", "proved", Some(as_tok)); // a new String or Bytes (6.6)
                }
                if c.fallible {
                    named_args("Outcome", vec![d])
                } else {
                    d
                }
            }
        }
    }

    /// The conversion for the pair (`s`, `d`), both stripped and known: a built-in row, a base type into a
    /// refinement of it (`Refine`, the old `validate`), the identity (nothing to convert, with the section that
    /// says so), or none. `core_pair` says whether both sides are core built-in types here: the rows are keyed by
    /// the built-in types, not by their spellings, so a user's class named `Int` is itself and converts by
    /// declaration alone (D-516).
    pub fn resolve_conversion(s: &Type, d: &Type, core_pair: bool) -> Resolved {
        if let Type::Refined { base: dbase, lo: dlo, hi: dhi, kind: dkind } = d {
            let s_base = match s {
                Type::Refined { base, .. } => Type::Named(base.clone()),
                _ => s.clone(),
            };
            if s_base == Type::Named(dbase.clone()) {
                // a refinement into a refinement whose range contains it is a subtype already (6.3)
                if let Type::Refined { lo: slo, hi: shi, kind: skind, .. } = s {
                    let within = skind == dkind && dlo.map_or(true, |l| slo.map_or(false, |x| x >= l)) && dhi.map_or(true, |h| shi.map_or(false, |x| x <= h));
                    if within {
                        return Resolved::Identity("6.3, 6.13");
                    }
                }
                return Resolved::Refine;
            }
            return Resolved::None;
        }
        if let Type::Refined { base, .. } = s {
            if Type::Named(base.clone()) == *d {
                return Resolved::Identity("6.3, 6.13");
            }
        }
        let (Some(sname), Some(dname)) = (cv::base_of(s), cv::base_of(d)) else { return Resolved::None };
        if s == d || (core_pair && cv::same_builtin(sname, dname)) {
            return Resolved::Identity("6.13");
        }
        if !core_pair {
            return Resolved::None;
        }
        match cv::resolve(sname, dname) {
            Some(c) => Resolved::Builtin(c),
            None => Resolved::None,
        }
    }

    pub fn marker(&mut self, node: N<'a>, env: &Env) -> Type {
        let kids = node.kids();
        let pl = node.find("place").unwrap();
        self.in_marker = true;
        let mut t = self.place_type_value(pl, env, false, kids.len() == 2);
        self.in_marker = false;
        let mtok = kids[0].tokens()[0].clone();
        let mk = mtok.text.clone();
        let members: Vec<N<'a>> = pl.child_nodes("memberName");
        let call_follows = kids.len() > 2 && kids[2].text_is("(") && !members.is_empty();
        // 5.6: a marker takes a place, optionally followed by a call on it; a function's name is not a place, so a
        // marker written on a call of a function (`alias first(alias xs)`) marks nothing: the call is written plain
        // and the markers sit on its arguments (D-382). The call is then checked as the plain call it is.
        let root_tok = pl.direct_tokens()[0].clone();
        let func_call = kids.len() > 2
            && kids[2].text_is("(")
            && members.is_empty()
            && root_tok.text != "self"
            && env_lookup(env, &root_tok.text).is_none()
            && self.dc.table.get(&root_tok.text).map_or(false, |d| d.kind == "func");
        if func_call {
            self.own_err(
                "E-OWN-001",
                format!("`{mk} {}(…)`: a marker takes a place (5.6), and `{}` is a function, not a place: write `{}(…)` and put the markers on its arguments", root_tok.text, root_tok.text, root_tok.text),
                &mtok,
                &root_tok.text,
                "marker on a call",
                "LANG-07-01",
                "",
            );
        }
        let path = if !call_follows {
            self.path_of_place(pl)
        } else {
            let mut parts = vec![pl.direct_tokens()[0].text.clone()];
            for k in &members[..members.len() - 1] {
                parts.push(k.tokens()[0].text.clone());
            }
            parts.join(".")
        };
        if !func_call {
            self.use_place(Some(&path), &mtok, if mk == "copy" { "copy" } else { &mk }, env);
        }
        if mk == "copy" {
            // `copy v` of a binding that holds a loan copies the value it lends, narrowed or not (7.5; D-341)
            let declared = if !call_follows { self.declared_type(pl, env) } else { unknown("") };
            let base = Self::lent(&declared).unwrap_or_else(|| strip(&declared).clone());
            if known(&base) && !self.dc.is_trivial(&base) {
                self.eff.add("alloc", "proved", Some(&mtok));
            }
            // `copy x` of a `T` needs `T implements Copyable` (6.10, X-513; D-352)
            if (known(&base) || matches!(base, Type::TypeParam(_))) && !self.dc.is_copyable(&base) {
                let rule = if matches!(&base, Type::Named(n) if (n.package.is_empty() && n.name == "Shared") || (n.package.is_empty() && n.name == "Weak")) { "LANG-07-08" } else { "LANG-07-01" };
                self.copy_refused.insert((mtok.line, mtok.col));
                let fix = if let Type::TypeParam(n) = &base { format!(": bound it, `where {n} implements Copyable` (6.10)") } else { String::new() };
                self.own_err("E-OWN-002", format!("`copy {path}`: {} is not Copyable (6.11){fix}", base.show()), &mtok, &path, "copy of a non-copyable place", rule, "");
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
            if (mk == "alias" || mk == "editable") && (known(&t) || matches!(t, Type::TypeParam(_))) && !t.is_loan() {
                return Type::Loan { inner: Box::new(t), mode: mk, origin: pl.direct_tokens()[0].text.clone() };
            }
            if mk == "copy" {
                if let Some(owned) = Self::lent(&t) {
                    return owned; // a copy is owned, whatever the place holds (7.5; D-341)
                }
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
                if mk == "editable" && self.receiver_mode(&prefix_t, &last.text).as_deref() == Some("editable self") && self.unmarked_editable_root(&path, env) {
                    self.redundant_markers.insert((mtok.line, mtok.col)); // the declaration shows it (X-345)
                }
                t = self.method_call_named(&prefix_t, &last, al, env);
            } else if let Type::Fn { kind, .. } = strip(&t).clone() {
                let ft = strip(&t).clone();
                if kind == "fn mut" && mk == "editable" && self.unmarked_editable_root(&path, env) {
                    self.redundant_markers.insert((mtok.line, mtok.col)); // the declaration shows it (X-345)
                }
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
                if known(&t) {
                    // a marked call of a value that is no function is the plain call's E-TYP-040 (D-382)
                    let s = strip(&t).clone();
                    self.typ("E-TYP-040", format!("call of a value of type {}", s.show()), &root_tok, "a function type", &s.show(), "call", "LANG-06-01");
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
        let (params, ret, modifiers, param_modes) = match ft {
            Type::Fn { params, ret, modifiers, param_modes, .. } => (params.clone(), (**ret).clone(), modifiers.clone(), param_modes.clone()),
            _ => return unknown(""),
        };
        let sig = Sig::simple(where_, params.iter().enumerate().map(|(i, p)| (format!("p{i}"), p.clone(), param_modes.get(i).cloned().unwrap_or_default())).collect(), ret, modifiers);
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
        self.place_type_access(pl, env, false)
    }

    pub fn place_type_access(&mut self, pl: N<'a>, env: &Env, writing: bool) -> Type {
        self.place_type_value(pl, env, writing, false)
    }

    fn place_type_value(&mut self, pl: N<'a>, env: &Env, writing: bool, as_value: bool) -> Type {
        let ts = pl.direct_tokens();
        let path = self.path_of_place(pl);
        if as_value && env_lookup(env, &ts[0].text).is_none() {
            if let Some(d) = self.dc.table.get(&path).cloned().filter(|d| d.kind == "func") {
                if let Some(sig) = &d.sig {
                    self.entry_main_ref(&d, ts[0]);
                    return self.function_value(sig, &BTreeMap::new(), &d, ts[0]);
                }
            }
        }
        let mut t = self.root_type(ts[0], env);
        for k in pl.kids().into_iter().skip(1) {
            if writing && k.text_is("[") {
                let coll = strip(&t);
                if matches!(coll, Type::Refined { kind, .. } if kind == "length") {
                    self.ref_("E-REF-003", format!("{} may not be mutated through an index", coll.show()), k.tok().unwrap(), "violated", &coll.show(), "", "");
                }
            }
            match k {
                C::N(n) if n.name() == "memberName" => t = self.member_type_as(&t, n.tokens()[0], as_value),
                C::T(tk) if tk.kind == Kind::Int => {
                    let s = strip(&t).clone();
                    t = match &s {
                        Type::Named(n) if n.package.is_empty() && matches!(n.name.as_str(), "List" | "Array" | "FixedList") && !n.args.is_empty() => n.args[0].clone(),
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
                    return self.function_value(sig, &BTreeMap::new(), &d, tok);
                }
            }
        }
        unknown("untyped binding")
    }

    /// A callable expression has one complete, monomorphic signature (LR 8.1).
    pub fn function_value(&mut self, sig: &Sig<'a>, subst: &BTreeMap<String, Type>, owner: &DeclRef<'a>, tok: &Token) -> Type {
        let mut subst = subst.clone();
        let parameters: BTreeSet<_> = sig.type_params.iter().chain(&owner.type_params).cloned().collect();
        let wanted = self.storage_expected.as_deref().map(strip).cloned();
        if let Some(Type::Fn { params, ret, .. }) = wanted {
            if parameters.iter().any(|p| !subst.contains_key(p)) {
                for ((_, pt, _), actual) in sig.params.iter().zip(&params) {
                    self.unify(pt, actual, &mut subst);
                }
                self.unify(&sig.ret, &ret, &mut subst);
            }
        }
        let before = self.dc.records.len();
        self.check_storage_function(sig, &subst, owner, tok);
        if self.dc.records[before..].iter().any(|r| r.code == "E-OWN-053") { return unknown("stored function loan"); }
        // A function type stores its success result, as a lambda does. call_fn applies the modifiers once.
        let ft = Type::Fn { capture_origins: Vec::new(), params: sig.params.iter().map(|p| self.subst(&p.1, &subst)).collect(),
            ret: Box::new(self.subst(&sig.ret, &subst)), kind: "fn".into(), modifiers: sig.modifiers.clone(), origin: String::new(),
            dynamic: false, identity: String::new(), param_modes: sig.params.iter().map(|p| p.2.trim_start_matches('.').to_string()).collect() };
        let missing = parameters.iter().any(|p| !subst.contains_key(p)) || Self::tparams_of(&ft).iter().any(|p| !self.tparams_in_scope.contains(p));
        if missing || !sig.where_.is_empty() || !owner.where_.is_empty() {
            self.typ("E-TYP-040", format!("`{}` has unresolved type parameters or `where` bounds: call it directly, or wrap the call in a lambda with typed parameters", tok.text), tok,
                "a complete function type", "a generic or constrained function value", &tok.text, "LANG-06-01");
            return unknown("unsupported function value");
        }
        ft
    }

    pub fn member_type(&mut self, t: &Type, mtok: &Token) -> Type {
        self.member_type_as(t, mtok, false)
    }

    /// `as_value`: a postfix `.member` used as a value, where a method is refused (4.5); a marked call's callee place
    /// (`editable xs.push(10)`) is looked up the same way and is not one
    pub fn member_type_as(&mut self, t: &Type, mtok: &Token, as_value: bool) -> Type {
        let mut s = strip(t).clone();
        if let Type::Named(n) = &s {
            if (n.package.is_empty() && n.name == "Shared") && !n.args.is_empty() {
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
            None => {
                if as_value && matches!(s, Type::Dynamic(_) | Type::TypeParam(_)) {
                    self.typ("E-TYP-040", format!("member `{}` of {} has no value type: call it directly, or wrap the call in a lambda with the required captures", mtok.text, s.show()), mtok,
                        "a complete value type", "an unresolved member value", &mtok.text, "LANG-06-01");
                }
                return unknown("member of unknown");
            },
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
        if d.methods.contains(m) || (as_value && self.receiver_mode(t, m).is_some()) {
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

    /// The receiver of an `editable self` method, or a `fn mut` closure being called, needs no `editable` wherever
    /// `editable` could be written (7.5, X-345), whatever the receiver's type, and the call is then checked exactly as
    /// `editable p…`. On any other root it is rejected as the marked form is (D-303).
    pub fn unmarked_editable_root(&self, path: &str, env: &Env) -> bool {
        self.editable_root(path, env)
    }

    /// The receiver mode of method `m` on `recv`, found as `method_call_named` finds it, recording nothing.
    /// The one collection representation whose instance methods a newtype forwards (LR 6.9, X-581).
    pub fn collection_representation(&self, t: &Type) -> Option<(DeclRef<'a>, Type)> {
        let d = self.decl_of(t)?;
        if d.kind != "type" { return None; }
        let rep = strip(d.alias.as_ref()?).clone();
        let base = match &rep { Type::Refined { base, .. } => base.as_type(), _ => rep.clone() };
        let rd = self.decl_of(&base)?;
        if crate::decls::is_standard(&rd.module) && ["List", "Array", "ListView", "Map", "Set", "OrderedMap", "FixedList", "FixedMap", "Deque", "Queue", "SortedSet"].contains(&rd.name.as_str()) {
            Some((d, rep))
        } else { None }
    }

    pub fn receiver_mode(&self, recv: &Type, m: &str) -> Option<String> {
        let mut s = concrete(strip(recv));
        let forwarded = self.collection_representation(&s);
        if let Some((_, rep)) = &forwarded { s = rep.clone(); }
        if let Type::Refined { base, .. } = &s {
            s = Type::Named(base.clone());
        }
        if let Type::Named(n) = &s {
            if (n.package.is_empty() && n.name == "Shared") && !n.args.is_empty() && m != "retain" && m != "downgrade" {
                s = strip(&n.args[0]).clone();
            }
        }
        if matches!(s, Type::Optional(_)) || !known(&s) {
            return None;
        }
        let d = match &s {
            Type::Dynamic(i) => self.dc.resolve_named(i, None),
            _ => self.decl_of(&s),
        }?;
        let mut sig = d.methods.get(m).cloned();
        if sig.is_none() && d.kind == "mixin" {
            sig = d.methods.get(&format!("requires:{m}")).cloned();
        }
        if sig.is_none() {
            for mx in &d.mixins {
                if let Some(md) = self.dc.resolve_named(mx, None) {
                    if let Some(ms) = md.methods.get(m) {
                        sig = Some(ms.clone());
                    }
                }
            }
            for iface in &d.implements {
                if sig.is_none() {
                    if let Some(idecl) = self.dc.resolve_named(iface, None) {
                        if let Some(ms) = idecl.methods.get(m) {
                            sig = Some(ms.clone());
                        }
                    }
                }
            }
        }
        sig.and_then(|s| s.receiver)
    }

    /// An unmarked receiver or callee the declaration shows to be editable: the steps of `editable p` (`marker`).
    pub fn unmarked_editable(&mut self, path: &str, tok: &Token, env: &Env) {
        self.use_place(Some(path), tok, "editable", env);
        self.inval_log.push(path.to_string());
        self.facts.invalidate(path);
        self.receiver = Some((path.to_string(), "editable".into()));
    }

    /// `maybe.map(f)` on a `T?` receiver (6.8, X-479): a call of the prelude's `Optional.map(value, f)` whose first
    /// argument is the receiver, so `T` is fixed by the receiver and the rest of the call is checked as written.
    fn optional_helper(&mut self, inner: &Type, recv: &Type, marked: Option<(String, String)>, mtok: &Token, al: Option<N<'a>>, env: &Env) -> Option<Type> {
        let d = self.dc.decl_by_module("Optional", "fortis.prelude", None)?;
        let sig = d.methods.get(&mtok.text)?.clone();
        if sig.receiver.is_some() || sig.params.is_empty() {
            return None;
        }
        let rp = self.coll_path.clone();
        self.receiver = marked;
        if self.receiver.is_none() && self.is_bearing(recv) {
            if let Some(rp) = rp {
                self.receiver = Some((rp, "alias".into())); // the receiver is lent for the call, as a method's read receiver is
            }
        }
        self.call_decl = Some(d.clone());
        self.call_label = self.method_label(&d, &mtok.text);
        let mut rest = sig;
        rest.params.remove(0);
        let mut mm = BTreeMap::new();
        // an optional loan (`(alias T from xs)?`) lends its value: the callback sees a `T`, as it does for a `T?`
        mm.insert("T".to_string(), strip(inner).clone());
        let where_ = format!("Optional.{}", mtok.text);
        Some(self.check_call(&rest, al, env, &mut mm, &where_, mtok))
    }

    pub fn method_call(&mut self, recv: &Type, mc: N<'a>, env: &Env) -> Type {
        let mtok = mc.find("memberName").unwrap().tokens()[0];
        let optional = mc.tokens()[0].text == "?.";
        let recv = if optional {
            match strip(recv) {
                Type::Optional(i) => &**i,
                s => {
                    self.typ("E-TYP-040", format!("`?.` on {}, which is not optional", s.show()), mtok, "T?", &s.show(), "?.", "LANG-06-08");
                    return unknown("");
                }
            }
        } else { recv };
        let t = self.method_call_named_optional(recv, mtok, mc.find("argList"), env, optional);
        if optional {
            if let Some(types) = self.node_types.as_mut() {
                // Preserve the instantiated call result before optional lifting for MIR, including generic U?.
                types.insert(mc.find("memberName").unwrap().id(), t.clone());
            }
        }
        if optional && known(&t) && !matches!(t, Type::Optional(_)) { Type::Optional(Box::new(t)) } else { t }
    }

    pub fn method_call_named(&mut self, recv: &Type, mtok: &Token, al: Option<N<'a>>, env: &Env) -> Type {
        self.method_call_named_optional(recv, mtok, al, env, false)
    }

    fn method_call_named_optional(&mut self, recv: &Type, mtok: &Token, al: Option<N<'a>>, env: &Env, optional: bool) -> Type {
        // A marked receiver (`editable p.m(...)`) belongs to this call only: it is handed back on the path that
        // reaches check_call and dropped on every other exit, so it cannot attach to a later call (7.6; X-114).
        let marked = self.receiver.take();
        let mut s = concrete(strip(recv));
        let forwarded = self.collection_representation(&s);
        if let Some((_, rep)) = &forwarded { s = rep.clone(); }
        if let Type::Refined { base, .. } = &s {
            s = Type::Named(base.clone());
        }
        if let Type::Named(n) = &s {
            if (n.package.is_empty() && n.name == "Shared") && !n.args.is_empty() && mtok.text != "retain" && mtok.text != "downgrade" {
                s = strip(&n.args[0]).clone();
            }
        }
        // a method call reads its receiver as T only where a null test, an `&&` conjunct or a diverging guard narrowed
        // the place (6.8); `||` and function calls do not narrow (6.3), so an unnarrowed T? receiver is rejected (D-51.6)
        if let Type::Optional(i) = &s {
            if !matches!(**i, Type::Unknown(_)) {
                if OPTIONAL_HELPERS.contains(&mtok.text.as_str()) {
                    let inner = (**i).clone();
                    if let Some(t) = self.optional_helper(&inner, recv, marked.clone(), mtok, al, env) {
                        return t;
                    }
                }
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
                let target = self.result_expected.clone().filter(known);
                if self.final_pass {
                    self.legacy_conversions.insert((mtok.line, mtok.col), crate::migrate::LegacyConversion { target: if self.inferred_conversion_target { None } else { target.clone() }, kind: "to" });
                    self.legacy_conversion_sources.insert((mtok.line, mtok.col), s.clone());
                }
                if let Some(target) = target {
                    return self.convert_once(&s, &target, mtok, Some(env), None);
                }
                return unknown("to[T]");
            }
            if sig.is_none() && (m == "toText" || m == "toBytes") {
                if !self.dc.implements(&s, "Encodable") {
                    let displays = diagnostic_types(&[&s, &named("Encodable")]);
                    self.typ("E-TYP-090", format!("`{m}` needs {} to be {expected}", displays[0], expected = displays[1]), mtok, &displays[1], &displays[0], &m, "LANG-06-10");
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
        if optional && (sig.receiver.as_deref() != Some("self") || sig.has_mod("try")) {
            self.typ("E-TYP-040", format!("`?.{m}` requires a read, non-try method: test for null, then call with its receiver mode and handle its outcome"), mtok, "a read, non-try method", sig.receiver.as_deref().unwrap_or("associated function"), &m, "LANG-06-08");
            return unknown("");
        }
        if let Some((nd, rep)) = &forwarded {
            if matches!(sig.receiver.as_deref(), Some("editable self" | "owned self")) {
                if self.foreign(nd) {
                    self.rec("E-NAM-004", format!("`{m}` of {} is visible only in its declaring module {}", nd.name, nd.module), "LANG-04-05", mtok, Self::nam(&m, vec![]));
                }
                if sig.receiver.as_deref() == Some("editable self") && matches!(rep, Type::Refined { kind, .. } if kind == "length") {
                    self.ref_("E-REF-003", format!("{} represents {}, which may not be loaned editable", nd.name, rep.show()), mtok, "violated", &rep.show(), "", "");
                }
            }
        }
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
        if marked.is_none() && rp.is_none() && matches!(sig.receiver.as_deref(), Some("self" | "editable self")) && self.is_linear(recv) {
            self.obl_err("E-OBL-001", format!("the linear receiver of `{m}` is only loaned for the call and dropped afterwards; bind it and consume it after the call"), mtok, &format!("{}:{}", mtok.line, mtok.col), "temporary receiver", "LANG-10-01");
        }
        self.call_decl = Some(d.clone());
        self.call_label = self.method_label(&d, &m);
        // `clear()` on a list of linear elements would drop them (10.7); `consumeEach` and `pop` are the consuming ways
        if d.name == "List" && m == "clear" && crate::decls::is_standard(&d.module) {
            if let Type::Named(n) = strip(&s) {
                if let Some(e) = n.args.first() {
                    if self.is_linear(e) {
                        self.obl_err("E-OBL-001", format!("`clear()` on a List of {} drops its linear elements (10.7): consume them (`consumeEach`, `pop`) instead", e.show()), mtok, &format!("{}:{}", mtok.line, mtok.col), "elements", "LANG-10-01");
                    }
                }
            }
        }
        if d.name == "Scope" && (m == "launchParallel" || m == "launchBlocking") {
            self.check_pool_launch(&m, al, env, mtok);
        }
        self.receiver = marked;
        if self.receiver.is_none() && rp.is_none() && sig.receiver.is_some() && Self::lends_from_self(&sig) && (self.temporary_receiver || (self.is_bearing(recv) && !recv.is_loan() && !self.loan_carrying_or_cursor(recv))) {
            self.temporary_borrows.push(mtok.clone());
        }
        if self.receiver.is_none() && rp.is_none() && sig.receiver.as_deref() == Some("self") && self.is_bearing(recv) && !recv.is_loan() {
            self.charge_drop(recv, mtok); // a temporary receiver is destroyed with the statement (7.9, 11.3; D-393)
        }
        if self.receiver.is_none() && rp.is_some() && sig.receiver.as_deref() == Some("editable self") {
            // reached only on a root `editable` could not be written on (X-345; D-303: whatever the receiver's type, a
            // trivial value or a read loan included): a `let` binding, a read parameter, capture or loan, a read `self`
            let rp = rp.clone().unwrap();
            self.own_err("E-OWN-001", format!("`{m}` takes `editable self`, and `{rp}` cannot change: declare it `let mut` or take it `editable`"), mtok, &rp, "editable receiver on an immutable root", "LANG-07-01", "");
        } else if self.receiver.is_none() && rp.is_some() && self.is_bearing(recv) && sig.receiver.is_some() {
            let rp = rp.unwrap();
            match sig.receiver.as_deref() {
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
        let mut sig = sig;
        if d.module == "fortis.prelude"
            && !sig.params.is_empty()
            && ((d.name == "Map" && matches!(m.as_str(), "get" | "containsKey" | "remove") && mm.get("K") == Some(&string()))
                || (d.name == "Set" && matches!(m.as_str(), "contains" | "remove") && mm.get("T") == Some(&string())))
        {
            // these only read their key: on a Map[String, V] or Set[String] the key is a StringView, so no copy makes
            // one (6.7, X-345; X-479 for `containsKey`, `remove` and the Set's)
            sig.params[0].1 = string_view();
        }
        let saved_optional = self.optional_arguments;
        self.optional_arguments |= optional;
        let t = self.check_call(&sig, al, env, &mut mm, &where_, mtok);
        self.optional_arguments = saved_optional;
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
        self.apply_postfix_context(t, pf, env, false, false)
    }

    fn apply_postfix_context(&mut self, t: &Type, pf: N<'a>, env: &Env, temporary: bool, for_receiver: bool) -> Type {
        let saved = std::mem::replace(&mut self.temporary_receiver, temporary);
        let result = self.apply_postfix_raw(t, pf, env, for_receiver);
        self.temporary_receiver = saved;
        if !self.borrows_value(&result) { self.temporary_borrows.clear(); }
        result
    }

    fn apply_postfix_raw(&mut self, t: &Type, pf: N<'a>, env: &Env, for_receiver: bool) -> Type {
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
                    let mut base = s.opt_inner().cloned().unwrap_or_else(|| s.clone());
                    let r0 = self.member_type_as(&base, mt, true);
                    if for_receiver && !base.is_loan() {
                        if let Some(path) = self.coll_path.clone() {
                            base = Type::Loan { inner: Box::new(base), mode: "alias".into(), origin: root_of(&path).into() };
                            if self.is_bearing(&r0) {
                                let loan = self.own.add_loan(&path, "alias", None, self.depth, mt.line);
                                loan.borrow_mut().nested = true;
                                self.carried.push(loan);
                            }
                        }
                    }
                    let r = self.lent_member(&base, r0);
                    return if r.is_optional() || !known(&r) { r } else { optional(r) };
                }
                if let Type::Optional(i) = &s {
                    self.typ("E-TYP-040", format!("member `{}` of an optional {}: use `?.` or match on it", mt.text, s.show()), mt, &i.show(), &s.show(), &mt.text, "LANG-06-08");
                    return unknown("");
                }
                let r0 = self.member_type_as(&s, mt, true);
                return self.lent_member(t, r0);
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
                    if kind == "fn mut" && self.unmarked_editable_root(&cp, env) {
                        // X-345: a `fn mut` held by a binding that shows it may change is called as `editable f(…)` (8.2)
                        self.unmarked_editable(&cp, &ft, env);
                        let info = self.closures.get(&root_of(&cp)).cloned();
                        for (cap, cm) in info.as_ref().map(|i| i.captures.clone()).unwrap_or_default() {
                            if cm == "editable" {
                                self.inval_log.push(cap.clone());
                                self.facts.invalidate(&cap);
                            }
                        }
                        return self.call_fn(&s, al, env, "closure", Some(&ft), info.and_then(|i| i.eff));
                    }
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
            let coll_t = strip(t);
            let s = match coll_t {
                Type::Refined { base, .. } => base.as_type(),
                _ => coll_t.clone(),
            };
            if known(strip(&it)) && int_bounds(strip(&it)).is_none() {
                let displays = diagnostic_types(&[&it, &named("Int")]);
                self.typ("E-TYP-040", format!("index has type {}, expected an integer", displays[0]), ftok(e), &displays[1], &displays[0], "index", "LANG-06-01");
            }
            if let Type::Named(n) = &s {
                if (n.package.is_empty() && ["List", "ListView", "Array", "FixedList"].contains(&n.name.as_str())) && !n.args.is_empty() {
                    self.index_proof(coll_t, n, &it, e, &ft);
                    return n.args[0].clone();
                }
                if (n.package.is_empty() && ["Map", "OrderedMap", "FixedMap"].contains(&n.name.as_str())) && n.args.len() >= 2 {
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
        let mut maximum = upper;
        if let Type::Refined { kind, lo, hi, .. } = coll_t {
            if kind == "length" {
                // Every value has at least lo elements; hi is only a maximum (LR 6.3).
                upper = *lo;
                maximum = *hi;
            }
        }
        let cp = self.coll_path.clone();
        if let (Some(ip), Some(cp)) = (&ip, &cp) {
            if self.facts.below.get(ip) == Some(cp) {
                return;
            }
        }
        if let Some(u) = upper {
            if ii.1 < u {
                return;
            }
            self.ref_("E-REF-002", format!("index interval {} is not below the length {u}", Self::show_iv(Some(ii))), tok, if maximum.map_or(false, |h| ii.0 >= h) { "violated" } else { "unproved" }, &format!("[0, {u})"), &Self::show_iv(Some(ii)), "");
            return;
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

    /// `row.f` in a `cases` body: the field is read, and where the use expects a type the row literals are checked
    /// against it (14, D-50.3).
    fn note_row_use(&mut self, prim: N<'a>, pfs: &[C<'a>], expected: Option<&Type>) {
        let Some(row) = self.row_name.clone() else { return };
        let pk = prim.kids();
        if pfs.is_empty() || pk.len() != 1 || !pk[0].text_is(&row) {
            return;
        }
        let Some(pf) = pfs[0].node() else { return };
        let k = pf.kids();
        if k.len() < 2 || !k[0].text_is(".") || !k[1].is("memberName") {
            return;
        }
        let f = k[1].node().unwrap().tokens()[0].text.clone();
        let wants = self.row_uses.entry(f).or_default();
        if let Some(e) = expected {
            if pfs.len() == 1 && known(strip(e)) && !wants.iter().any(|w| w.show() == e.show()) {
                wants.push(e.clone());
            }
        }
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
                            Some(d) if d.kind == "func" && d.sig.is_some() => {
                                t = self.function_value(d.sig.as_ref().unwrap(), &BTreeMap::new(), d, mtok);
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
                        self.coll_path = None; // a call's result is not a place (7.2): what follows has a temporary receiver
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
                            if kids.len() > 2 {
                                t = self.unwrap_call(t0, t, env, None); // a receiver in a test body (14)
                            }
                            self.coll_path = None; // a call's result is not a place (7.2)
                            let posts: Vec<N<'a>> = kids[1..].iter().filter_map(|k| k.node()).collect();
                            for (i, pf) in kids[2..].iter().enumerate() {
                                let pfn = pf.node().unwrap();
                                self.temp_field_siblings(&t, pfn);
                                t = self.apply_postfix(&t, pfn, env);
                                if let Some(nt_) = self.node_types.as_mut() {
                                    nt_.insert(pfn.id(), t.clone());
                                }
                                if i + 3 < kids.len() {
                                    if let Some(ct) = fortis_names::reading::postfix_call_token(&posts, i + 1, Some(prim)) {
                                        t = self.unwrap_call(ct, t, env, None);
                                    }
                                }
                            }
                            return t;
                        }
                    }
                }
            }
        }
        let mut t = self.primary(prim, env, if kids.len() == 1 { expected } else { None });
        self.note_row_use(prim, &kids[1..], expected);
        if let Some(nt_) = self.node_types.as_mut() {
            nt_.insert(prim.id(), t.clone());
        }
        let mut path = self.path_of_expr(Some(prim));
        let mut temporary = self.temporary_value(Some(prim), Some(&t));
        let mut member_toks: Vec<Token> = Vec::new();
        let posts = &kids[1..];
        let post_nodes: Vec<N<'a>> = posts.iter().filter_map(|k| k.node()).collect();
        // a read of the root through `?.`, an index or a call's result is a read of the root: a moved binding there is
        // E-OWN-020 as it is in a member path (7.4); a method call's receiver is checked below
        let root_read = path.is_some() && !self.in_marker && !posts.is_empty() && self.path_of_expr(Some(node)).is_none() && !post_nodes.first().map_or(false, |p| p.name() == "methodCall" || p.kid(0).map_or(false, |c| c.is("methodCall")));
        if root_read {
            self.use_place(path.as_deref(), ftok(prim), "read", env);
        }
        if !posts.is_empty() && self.test_body {
            if let Some(pt0) = fortis_names::reading::primary_call_token(prim) {
                t = self.unwrap_call(pt0, t, env, None); // `Type.m(a).n()` in a test body (14)
            }
        }
        for (i, pf) in posts.iter().enumerate() {
            if i > 0 && self.test_body {
                if let Some(ct) = fortis_names::reading::postfix_call_token(&post_nodes, i - 1, Some(prim)) {
                    t = self.unwrap_call(ct, t, env, None); // the receiver of the next postfix, in a test body (14)
                }
            }
            // only the last call's result is the chain's result; an earlier `List.of` must not see it (8.4)
            self.result_expected = if i + 1 == posts.len() { expected.cloned() } else { None };
            let pfn = pf.node().unwrap();
            self.coll_path = path.clone();
            let is_mc = pfn.name() == "methodCall" || pfn.kid(0).map_or(false, |c| c.is("methodCall"));
            if path.is_some() && !self.in_marker && is_mc {
                let p = path.clone().unwrap();
                let mc = if pfn.name() == "methodCall" { pfn } else { pfn.kid(0).and_then(|c| c.node()).unwrap() };
                let mname = mc.find("memberName").unwrap().tokens()[0].text.clone();
                if self.receiver_mode(&t, &mname).as_deref() == Some("editable self") && self.unmarked_editable_root(&p, env) {
                    // X-345: the declaration shows the receiver is editable; checked as `editable p.m(…)` (7.5), whatever
                    // the receiver's type: a trivial value or a binding holding a loan takes the same exclusive loan (D-303)
                    let saved = std::mem::take(&mut self.facts);
                    let mut decl_t = self.root_type(ftok(prim), env);
                    for mt in &member_toks {
                        decl_t = self.member_type(&decl_t, mt);
                    }
                    self.facts = saved;
                    let decl_t = strip(&decl_t).clone();
                    if let Type::Refined { kind, .. } = &decl_t {
                        if kind == "length" {
                            self.ref_("E-REF-003", format!("`{p}` has a length refinement and may not be loaned editable"), ftok(prim), "violated", &decl_t.show(), "", "");
                        }
                    }
                    self.unmarked_editable(&p, ftok(prim), env);
                } else {
                    self.use_place(path.as_deref(), ftok(prim), "read", env);
                }
            }
            if path.is_none() {
                self.temp_field_siblings(&t, pfn);
            }
            let for_receiver = posts.iter().skip(i + 1).any(|q| q.node().is_some_and(|n| n.name() == "methodCall" || n.kid(0).is_some_and(|c| c.is("methodCall"))));
            t = self.apply_postfix_context(&t, pfn, env, temporary, for_receiver);
            temporary = self.is_bearing(&t) && !self.borrows_value(&t);
            let k = pfn.kids();
            if path.is_some() && !k.is_empty() && k[0].text_is(".") && k.len() > 1 && k[1].is("memberName") {
                member_toks.push(k[1].node().unwrap().tokens()[0].clone());
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
        if known(&s) && int_bounds(&s).is_none() && !["Float", "Float32", "Decimal", "Duration"].contains(&core_name(&s).unwrap_or("")) {
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
                    let inner_e = node.find("expr");
                    let pt = optional(self.infer(inner_e, env, None));
                    let owned = self.has_owned_part(inner_e);
                    self.note_owned_part(node, &pt, owned); // an optional holds nothing of its own (D-369)
                    return pt;
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
    fn variant_constructor(&mut self, d: &DeclRef<'a>, vname: &str, ret: &Type, tok: &Token) -> Type {
        let payload = d.variants.get(vname).cloned().unwrap_or_default();
        let mm = self.recv_subst(ret, d);
        let params: Vec<Type> = payload.iter().map(|(_, t)| self.subst(t, &mm)).collect();
        // a variant constructor is a declared function, not a lambda: empty identity, not `dynamic` (8.4)
        let ft = Type::Fn { capture_origins: Vec::new(), params, ret: Box::new(ret.clone()), kind: "fn".into(), modifiers: vec!["pure".into()], origin: String::new(), dynamic: false, identity: String::new(), param_modes: vec![] };
        if d.type_params.iter().any(|p| !mm.contains_key(p)) || Self::tparams_of(&ft).iter().any(|p| !self.tparams_in_scope.contains(p)) {
            self.typ("E-TYP-040", format!("`{}.{vname}` needs a complete expected function type, including the enum's type arguments: annotate it or wrap the call in a lambda", d.name), tok,
                "a complete function type", "an unresolved variant constructor", vname, "LANG-06-01");
            return unknown("unresolved variant constructor");
        }
        ft
    }

    /// The result of `Enum.Variant` where a function type is expected (LR 9.7). A reference that already carries
    /// type arguments keeps them. When the expected result is this enum, that result is used so its arguments flow
    /// (`Level.Low` where `fn() Level[Int]` is expected). Otherwise the enum itself, and the expected function is
    /// checked against this constructor.
    /// Explicit type arguments that are a different concrete instantiation from the expected result (LR 9.7).
    fn note_ctor_mismatch(&mut self, have: &Type, want: &Type, tok: &Token) {
        let (Type::Named(hs), Type::Named(ws)) = (strip(have), strip(want)) else { return };
        if (hs.name != ws.name || hs.package != ws.package || hs.origin != ws.origin) || hs.args.len() != ws.args.len() || hs.args.is_empty() {
            return;
        }
        let hk: Option<Vec<String>> = hs.args.iter().map(nominal_key).collect();
        let wk: Option<Vec<String>> = ws.args.iter().map(nominal_key).collect();
        if hk.is_none() || wk.is_none() || hk == wk {
            return;
        }
        let names = diagnostic_types(&[&hs.as_type(), &ws.as_type()]);
        self.typ(
            "E-TYP-040",
            format!("variant constructor has type {}, expected {}: {} is not {}", names[0], names[1], names[0], names[1]),
            tok,
            &names[1],
            &names[0],
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

    /// Language 1.0 (X-180): no associated call on a type parameter; it needs run-time type arguments.
    pub fn tparam_call_err(&mut self, p: &str, member: Option<&Token>, tr_tok: &Token) {
        let what = member.map(|m| format!("{p}.{}", m.text)).unwrap_or_else(|| p.to_string());
        self.typ(
            "E-TYP-170",
            format!("`{what}` is an associated call on the type parameter `{p}`, which Language 1.0 does not support (X-180); it comes with run-time type arguments in a later release. Take a value of `{p}`, or a function that makes one, as a parameter instead"),
            tr_tok,
            "an associated call on a concrete type",
            &format!("an associated call on type parameter {p}"),
            &what,
            "LANG-06-12",
        );
    }

    /// A constant expression of a generic declaration (a field default, a class constant, a parameter default) is not
    /// typed as a body is, so an associated call on a type parameter in it is found here, as `type_primary` finds one
    /// in a body (E-TYP-170, X-180).
    pub fn const_tparams(&mut self, ce: Option<N<'a>>, scope: &[String]) {
        let Some(ce) = ce else { return };
        if scope.is_empty() {
            return;
        }
        for ct in ce.find_all("constTerm") {
            let kids = ct.kids();
            let tr = match kids.first() {
                Some(C::N(n)) if n.name() == "typeRef" => *n,
                _ => continue,
            };
            let called = kids.iter().any(|k| k.text_is("("));
            let mut member: Option<Token> = None;
            for k in &kids[1..] {
                match k {
                    C::N(n) if n.name() == "memberName" => member = Some(n.tokens()[0].clone()),
                    C::T(t) if t.kind == Kind::Upper => member = Some((*t).clone()),
                    _ => {}
                }
            }
            let Some(first) = tr.direct_tokens().first().map(|t| (*t).clone()) else { continue };
            if first.kind == Kind::Upper && scope.contains(&first.text) && (member.is_some() || called) {
                let tr_tok = tr.tokens().into_iter().find(|x| x.kind == Kind::Upper).unwrap().clone();
                self.tparam_call_err(&first.text, member.as_ref(), &tr_tok);
            }
        }
    }

    /// Each parameter default of a signature, with the declaration's type parameters in scope (X-180).
    pub fn param_default_tparams(&mut self, sig: &Sig<'a>, scope: &[String]) {
        let Some(node) = sig.node else { return };
        if scope.is_empty() {
            return;
        }
        for p in node.find_all("param") {
            self.const_tparams(p.child_node("constExpr"), scope);
        }
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
        self.dc.check_keys(&t, ftok(tr), None, false);
        self.check_stored_type(&t, ftok(tr), false);
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
        // a type parameter in scope: the body's `T` names it, whatever the table says and whatever arguments follow
        // (`T[Int].create()`); a qualified name (`mod.T`) starts with its module and is never one
        let first = tr.direct_tokens()[0];
        let tparam = match &t {
            Type::TypeParam(p) => Some(p.clone()),
            _ if first.kind == Kind::Upper && self.tparams_in_scope.contains(&first.text) => Some(first.text.clone()),
            _ => None,
        };
        if let Some(p) = &tparam {
            if member.is_some() || called {
                self.tparam_call_err(p, member.as_ref(), &tr_tok);
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                return unknown("associated call on a type parameter");
            }
        }
        // a bare variant where the expected type is that enum (9.7) whose name also reaches a declaration in scope is
        // ambiguous: the qualified form says which is meant, so a name never silently changes meaning (X-345)
        if let (Some(dd), None, Type::Named(tn), Some(ex)) = (&d, &member, &t, expected) {
            if rts[0].kind == Kind::Upper {
                let es = strip(ex).clone();
                if let Some(ed) = self.decl_of(&es) {
                    if ed.kind == "enum" && ed.variants.contains(&tn.name) && !(dd.kind == "enum" && dd.name == ed.name) {
                        let n = tn.name.clone();
                        self.typ(
                            "E-TYP-040",
                            format!("`{n}` is ambiguous: the {} `{}` is in scope and {}, the expected type, declares `{n}` as a variant; write `{}.{n}`", dd.kind, dd.name, ed.name, ed.name),
                            &tr_tok,
                            &format!("{}.{n}", ed.name),
                            &format!("{} {}", dd.kind, dd.name),
                            &n,
                            "LANG-06-01",
                        );
                        if let Some(al) = al {
                            self.infer(Some(al), env, None);
                        }
                        return es;
                    }
                }
            }
        }
        if d.is_none() && member.is_none() {
            if let Type::Named(tn) = &t {
                let es = expected.map(|e| strip(e).clone());
                let ed = es.as_ref().and_then(|e| self.decl_of(e));
                if let Some(ed) = ed {
                    if ed.kind == "enum" && ed.variants.contains(&tn.name) {
                        let m = self.variant_args(&ed, &tn.name, al, env, &tr_tok, es.as_ref());
                        let es = es.unwrap();
                        // an expected type that still names the enum's own type parameters (a payload of the generic
                        // enum, `tail: Chain[T]`) is instantiated by what this variant's payload fixes; a parameter
                        // nothing fixes leaves the value untyped, as the qualified form is (D-307)
                        let free = |t: &Type, s: &Self| Self::tparams_of(t).into_iter().any(|p| !s.tparams_in_scope.contains(&p));
                        if !free(&es, self) {
                            return es;
                        }
                        let inst = self.subst(&es, &m);
                        return if free(&inst, self) { unknown("generic enum") } else { inst };
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
                                return self.variant_constructor(&ret_d, &tn.name, ret, &tr_tok);
                            }
                        }
                        let owners = self.variant_owners(&tn.name);
                        if owners.len() == 1 {
                            if let Some(only) = self.dc.table.get(&owners[0]).cloned() {
                                if only.kind == "enum" && only.variants.contains(&tn.name) {
                                    let own = named_of_decl(&only, only.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect());
                                    return self.variant_constructor(&only, &tn.name, &own, &tr_tok);
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
                } else if owners.len() == 1 && self.unresolved_args == 0 && !matches!(es, Some(Type::Unknown(_))) && !matches!(es.as_ref().map(|e| e.opt_inner().unwrap_or(e)), Some(Type::Fn { .. })) {
                    // the bare form is read only where the expected type is the enum (9.7): with no such type the value
                    // has no type, and no backend could build it (D-304, X-345). An unknown expected type is not a proof,
                    // and where a function (or an optional one: an aspect's `fallback`) is expected the name is the
                    // variant's constructor.
                    let o = owners[0].clone();
                    self.typ(
                        "E-TYP-040",
                        format!("`{}` is a variant of {o}, but nothing here expects a {o}: the bare form is read only where the expected type is that enum; write `{o}.{}`", tn.name, tn.name),
                        &tr_tok,
                        &format!("{o}.{}", tn.name),
                        "a bare variant with no expected enum",
                        &tn.name,
                        "LANG-06-01",
                    );
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
                    let displays = diagnostic_types(&[&t, &named("Decodable")]);
                    self.typ("E-TYP-090", format!("`{}` needs {} to be {expected}", mb.text, displays[0], expected = displays[1]), mb, &displays[1], &displays[0], &mb.text, "LANG-06-10");
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
                let arg = Self::arg_nodes(al).first().and_then(|a| Self::arg_expr(*a));
                let transferred = self.path_of_expr(arg).is_some() && arg.and_then(|a| a.find("markerExpr")).map_or(false, |m| matches!(m.tokens()[0].text.as_str(), "move" | "copy"));
                let copying = self.is_bearing(&repr_t) && self.dc.is_copyable(&repr_t) && !transferred;
                let mode = if self.is_bearing(&repr_t) && !copying { "owned" } else { "" };
                if copying {
                    self.eff.add("alloc", "proved", Some(mb));
                }
                let linear_check = self.is_linear(&repr_t) && matches!(&d.alias, Some(Type::Refined { .. }));
                let result = named_of_decl(&d, vec![]);
                let result = if self.dc.view_newtype(&result) { Type::Loan { inner: Box::new(result), mode: "alias".into(), origin: "value".into() } } else { result };
                let sig = Sig::simple("validate", vec![("value".into(), repr_t, mode.into())], result, vec!["try".into()]);
                self.call_decl = Some(d.clone());
                self.call_label = format!("validate {}", d.name);
                if self.final_pass {
                    self.legacy_conversions.insert((mb.line, mb.col), crate::migrate::LegacyConversion { target: Some(t.clone()), kind: if copying { "validate-copy" } else { "validate" } });
                }
                self.pending_call_expected = expected.cloned();
                let capture = (linear_check || self.final_pass) && self.node_types.is_none();
                if capture {
                    self.node_types = Some(HashMap::new());
                }
                let result = self.check_call(&sig, al, env, &mut BTreeMap::new(), &format!("{}.validate", d.name), mb);
                if self.dc.view_newtype(&named_of_decl(&d, vec![])) {
                    if let Some(path) = self.path_of_expr(arg) {
                        let loan = self.own.add_loan(&path, "alias", None, self.depth, mb.line);
                        self.carried.push(loan);
                    }
                }
                let source = arg.and_then(|a| self.node_types.as_ref().and_then(|types| types.get(&a.id()))).cloned().unwrap_or_else(|| unknown("validation argument"));
                if self.final_pass {
                    self.legacy_conversion_sources.insert((mb.line, mb.col), source.clone());
                }
                if linear_check {
                    let proved = self.newtype_conversion(&source, &named_of_decl(&d, vec![])).map_or(false, |(_, _, _, proved)| proved);
                    if !proved {
                        self.obl_err("E-OBL-001", format!("an unproved conversion to {} would discard its linear representation on failure: prove the refinement before wrapping", d.name), mb, &format!("{}:{}", mb.line, mb.col), "linear representation", "LANG-10-01");
                    }
                }
                if capture {
                    self.node_types = None;
                }
                return result;
            }
        }
        let member = match member {
            None => {
            if called && d.name == "Void" && d.module == "fortis.prelude" {
                self.typ("E-TYP-040", "Void has no constructor: use a block lambda `fn(n) { }` or a function returning nothing (4.5)".into(), &tr_tok, "a block lambda or a function returning nothing", "Void()", "Void", "LANG-06-01");
                return void();
            }
            if d.kind == "class" && called {
                    return self.construct(&d, &t, al, env, &tr_tok, expected);
                }
                if d.kind == "type" && called {
                    return self.construct_newtype(&d, al, env, &tr_tok, expected);
                }
                if d.kind == "error" && called {
                    let mut seen_named = false;
                    for a in Self::arg_nodes(al) {
                        let ats = a.tokens();
                        let mut lbl = if ats.len() > 1 && ats[1].text == ":" { Some(ats[0].text.clone()) } else { None };
                        // the formatter's pun after a named argument (5.8, X-345)
                        if self.final_pass && seen_named && lbl.is_some() {
                            if let Some(v) = pun_value(a) {
                                self.migrations.push(crate::migrate::Edit { from: (ats[0].line, ats[0].col), to: (v.line, v.col), text: String::new(), rule: "R9" });
                            }
                        }
                        // `wanted` after a named argument is `wanted: wanted` (5.8, X-345)
                        if lbl.is_none() && seen_named {
                            lbl = pun_name(a).map(|t| t.text.clone());
                        }
                        seen_named |= lbl.is_some();
                        let e = Self::arg_expr(a);
                        let want = lbl.as_ref().and_then(|l| d.payload.get(l)).cloned();
                        let at = if e.is_some() { self.infer(e, env, want.as_ref()) } else { unknown("arg") };
                        if let Some(w) = &want {
                            if known(&at) && known(w) {
                                self.expect(&at, w, ats[0], &format!("payload field `{}` of {}", lbl.clone().unwrap(), d.name));
                                self.loan_into_owned(&at, Some(w), ats[0], &format!("payload field `{}` of {}", lbl.clone().unwrap(), d.name));
                            }
                        }
                        let ft_ = want.unwrap_or(at);
                        if known(strip(&ft_)) {
                            if let Type::Named(bn) = base_type(&ft_) {
                                if bn.package.is_empty() && ["String", "Bytes", "List", "Map", "Set"].contains(&bn.name.as_str()) {
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
                    if self.final_pass {
                        self.legacy_conversions.insert((member.line, member.col), crate::migrate::LegacyConversion { target: Some(t.clone()), kind: "validate" });
                    }
                    self.pending_call_expected = expected.cloned();
                    let capture = self.final_pass && self.node_types.is_none();
                    if capture { self.node_types = Some(HashMap::new()); }
                    let result = self.check_call(&sig, al, env, &mut BTreeMap::new(), &format!("{}.validate", t.show()), &member);
                    if self.final_pass {
                        let arg = Self::arg_nodes(al).first().and_then(|a| Self::arg_expr(*a));
                        let source = arg.and_then(|a| self.node_types.as_ref().and_then(|types| types.get(&a.id()))).cloned().unwrap_or_else(|| unknown("validation argument"));
                        self.legacy_conversion_sources.insert((member.line, member.col), source);
                    }
                    if capture { self.node_types = None; }
                    return result;
                }
                return Type::Fn { capture_origins: Vec::new(), params: vec![base.as_type()], ret: Box::new(named_args("Outcome", vec![t.clone()])), kind: "fn".into(), modifiers: vec!["try".into()], origin: String::new(), dynamic: false, identity: String::new(), param_modes: vec![] };
            }
        }
        if d.kind == "enum" && d.variants.contains(&m) && self.final_pass && node.name() != "constTerm" && rts.len() == 1 && tr.kids().len() == 1 {
            // `Severity.Error` where the expected type is Severity and no declaration is named `Error`: the formatter
            // may drop the prefix (9.7, X-345)
            let same = expected.map(|e| strip(e).clone()).and_then(|e| self.decl_of(&e)).map_or(false, |ed| ed.kind == "enum" && ed.name == d.name && ed.module == d.module);
            if same && self.dc.table.get(&m).is_none() {
                let first = tr.tokens()[0];
                self.migrations.push(crate::migrate::Edit { from: (first.line, first.col), to: (member.line, member.col), text: String::new(), rule: "R15" });
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
                        return self.variant_constructor(&d, &m, &ret, &member);
                    }
                }
                if d.variants.get(&m).map_or(false, |payload| !payload.is_empty()) {
                    self.typ("E-TYP-040", format!("`{}.{m}` needs an expected function type as a constructor value: annotate the binding, call it directly, or wrap the call in a lambda", d.name), &member,
                        "a function type", "a payload variant without arguments", &m, "LANG-06-01");
                }
            }
            let mut mm = self.variant_args(&d, &m, al, env, &member, Some(&t));
            let mut result = if matches!(&t, Type::Named(n) if !n.args.is_empty()) { t.clone() } else {
                named_of_decl(&d, d.type_params.iter().map(|p| self.widened(&mm.get(p).cloned().unwrap_or_else(|| Type::TypeParam(p.clone())))).collect())
            };
            if let Some(expected) = expected.filter(|ex| self.decl_of(strip(ex)).is_some_and(|ed| ed.type_identity() == d.type_identity() && ed.name == d.name)) {
                self.unify(&result, strip(expected), &mut mm);
                result = self.subst(&result, &mm);
            }
            return result;
        }
        if m == "from" && called {
            let legacy: Vec<_> = self.dc.instantiated_conversions(&d, &t).into_iter().filter(|c| c.legacy).collect();
            if !legacy.is_empty() && self.final_pass {
                self.legacy_conversions.insert((member.line, member.col), crate::migrate::LegacyConversion { target: Some(t.clone()), kind: "from" });
            }
            if !d.methods.contains(&m) && legacy.len() == 1 {
                let sig = &legacy[0].sig;
                self.call_decl = Some(d.clone());
                self.call_label = self.method_label(&d, &sig.name);
                self.pending_call_expected = expected.cloned();
                return self.check_call(sig, al, env, &mut BTreeMap::new(), &format!("{}.from", d.name), &member);
            }
            if !d.methods.contains(&m) && legacy.len() > 1 {
                let mut sig = Sig::simple("legacy:from", vec![("source".into(), Type::TypeParam("__FromSource".into()), String::new())], t.clone(), vec!["pure".into(), "noalloc".into()]);
                sig.type_params = vec!["__FromSource".into()];
                self.call_decl = None;
                self.call_label = "conversion argument".into();
                let mut inferred = BTreeMap::new();
                self.check_call(&sig, al, env, &mut inferred, &format!("{}.from", d.name), &member);
                let source = strip(&inferred.get("__FromSource").cloned().unwrap_or_else(|| unknown("conversion source"))).clone();
                let source = if let Type::Loan { inner, .. } = &source { strip(inner).clone() } else { source };
                if !legacy.iter().any(|c| c.other == source) {
                    let displays = diagnostic_types(&[&source, &t]);
                    self.typ("E-TYP-180", format!("no legacy From conversion from {} to {}", displays[0], displays[1]), &member, "a declared From pair", "none", "from", "LANG-06-14");
                    return unknown("no legacy conversion");
                }
                return self.convert_once(&source, &t, &member, Some(env), None);
            }
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
                        // whether `a op b` would be unproved, so `Int.add(a, b) or fail` may be written `a + b or fail`
                        // (6.4, X-345); only positional, unmarked arguments
                        let plain = args.iter().all(|x| x.direct_tokens().is_empty() && x.find("markerExpr").is_none());
                        if self.final_pass && plain && (d.name == "Int" || d.name == "Int64") {
                            self.int_calls.insert(node.id(), !Self::proved(op, full, ai, bi, iv::arith(op, ai, bi, 64)));
                        }
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
                mm.insert("Self".into(), t.clone());
                return self.function_value(&sig, &mm, &d, &member);
            }
            if self.final_pass && m == "from" && d.name == "String" && crate::decls::is_standard(&d.module) {
                self.legacy_conversions.insert((member.line, member.col), crate::migrate::LegacyConversion { target: Some(t.clone()), kind: "string" });
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
                let sig = Sig::simple("default", vec![], self_t, vec!["pure".into()]);
                return self.function_value(&sig, &self.recv_subst(&t, &d), &d, &member);
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

    /// A variant's payload arguments checked against its fields; returns the enum's type arguments they fix.
    pub fn variant_args(&mut self, d: &DeclRef<'a>, vname: &str, al: Option<N<'a>>, env: &Env, tok: &Token, expected: Option<&Type>) -> BTreeMap<String, Type> {
        let payload = d.variants.get(vname).cloned().unwrap_or_default();
        let args = Self::arg_nodes(al);
        if payload.is_empty() && !args.is_empty() {
            self.typ("E-TYP-130", format!("variant {}.{vname} has no payload", d.name), tok, "no arguments", &args.len().to_string(), vname, "LANG-05-09");
        }
        // a payload is passed as a read parameter is, but the variant owns it: a loan there needs `copy` (D-341)
        let sig = Sig::simple(vname, payload.iter().map(|(f, t)| (f.clone(), t.clone(), "payload".to_string())).collect(), named_of_decl(&d, vec![]), vec![]);
        let mut m = expected.map(|t| self.recv_subst(t, d)).unwrap_or_default();
        m.retain(|_, t| Self::tparams_of(t).iter().all(|p| self.tparams_in_scope.contains(p)));
        if !payload.is_empty() && al.is_some() {
            self.call_decl = Some(d.clone());
            self.call_label = format!("variant {}.{vname}", d.name);
            self.check_call(&sig, al, env, &mut m, &format!("{}.{vname}", d.name), tok);
        }
        m
    }

    /// `Name(x)` builds a newtype's value (6.9, X-511): checked as a call of `func Name(value: Representation) Name`, so
    /// `x` is a literal the checker proves in range, a value of the representation type, or a value whose interval the
    /// rules of 6.3 prove in range (E-REF-001 otherwise, E-TYP-040 for another type); the representation is taken
    /// `owned` where it is ownership-bearing (D-131, D-63, D-50.1).
    pub fn construct_newtype(&mut self, d: &DeclRef<'a>, al: Option<N<'a>>, env: &Env, tok: &Token, expected: Option<&Type>) -> Type {
        let repr_t = match &d.alias {
            Some(a) if known(strip(a)) => a.clone(),
            _ => {
                if let Some(al) = al {
                    self.infer(Some(al), env, None);
                }
                return named_of_decl(d, vec![]);
            }
        };
        let base = if let Type::Refined { base, .. } = &repr_t { base.as_type() } else { repr_t.clone() };
        let arg = Self::arg_nodes(al).first().and_then(|a| Self::arg_expr(*a));
        let literal: Vec<_> = arg.map(|a| a.tokens().into_iter().filter(|t| t.text != "(" && t.text != ")").collect()).unwrap_or_default();
        if base == string() && literal.len() == 1 && matches!(literal[0].kind, Kind::Str | Kind::Raw) {
            self.eff.add("alloc", "proved", Some(tok)); // D-521: an owned String, even from a literal
        }
        let mode = if self.is_bearing(&repr_t) { "owned" } else { "" };
        let result = named_of_decl(d, vec![]);
        let result = if self.dc.view_newtype(&result) { Type::Loan { inner: Box::new(result), mode: "alias".into(), origin: "value".into() } } else { result };
        let sig = Sig::simple(&d.name, vec![("value".into(), repr_t, mode.into())], result, vec!["pure".into()]);
        self.call_decl = Some(d.clone()); // every call names its callee (D-37, D-133)
        self.call_label = format!("newtype {}", d.name);
        self.pending_call_expected = expected.cloned();
        let result = self.check_call(&sig, al, env, &mut BTreeMap::new(), &d.name, tok);
        if self.dc.view_newtype(&named_of_decl(d, vec![])) {
            if let Some(path) = self.path_of_expr(arg) {
                let loan = self.own.add_loan(&path, "alias", None, self.depth, tok.line);
                self.carried.push(loan);
            }
        }
        result
    }

    pub fn construct(&mut self, d: &DeclRef<'a>, t: &Type, al: Option<N<'a>>, env: &Env, tok: &Token, expected: Option<&Type>) -> Type {
        // a generic class's constructor takes its type arguments from the written type, else from the expected type,
        // and each argument is checked against them (6.10, 9.2, X-513; D-66)
        let from_expected = match (t, expected) {
            (Type::Named(n), Some(ex)) if n.args.is_empty() && !d.type_params.is_empty() => {
                let es = strip(ex);
                let es = es.opt_inner().unwrap_or(es);
                match es {
                    Type::Named(en) if !en.args.is_empty() && self.decl_of(es).map_or(false, |ed| ed.name == d.name && ed.type_identity() == d.type_identity()) => Some(es.clone()),
                    _ => None,
                }
            }
            _ => None,
        };
        let t = from_expected.as_ref().unwrap_or(t);
        if self.foreign(d) {
            let hidden: Vec<String> = d.fields.iter().filter(|(_, (_, exp, _))| !exp).map(|(f, _)| f.clone()).collect();
            if !hidden.is_empty() {
                let s = if hidden.len() > 1 { "s" } else { "" };
                let list = hidden.iter().map(|h| format!("`{h}`")).collect::<Vec<_>>().join(", ");
                self.rec("E-NAM-004", format!("constructor of {} is not callable here: field{s} {list} not exported ({} is declared in {})", d.name, d.name, d.module), "LANG-09-02", tok, Self::nam(&d.name, hidden));
            }
        }
        let mut given: BTreeMap<String, Token> = BTreeMap::new();
        let infer_args = !matches!(t, Type::Named(n) if !n.args.is_empty());
        let mut mm = match t {
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
                        if infer_args { self.unify(&d.fields.get(&f).unwrap().0, &value_ty, &mut mm); }
                        self.note_field_closure(d, &f, &declared, &value_ty, ftok(a));
                        given.insert(f, ftok(a).clone());
                    } else if ts[1].text == "move" && self.is_linear(&ft) {
                        let t0 = ftok(a);
                        self.obl_err("E-OBL-007", format!("`...move` drops field `{f}` ({}), which is linear and has no counterpart in {}", ft.show(), d.name), t0, &format!("{}:{}", t0.line, t0.col), &f, "LANG-10-02");
                    }
                }
                continue;
            }
            // the formatter's pun: in a constructor `x: x` is always `x` (9.2, X-345)
            if self.final_pass && ts.len() > 1 && ts[0].kind == Kind::Lower && ts[1].text == ":" {
                if let Some(v) = pun_value(a) {
                    self.migrations.push(crate::migrate::Edit { from: (ts[0].line, ts[0].col), to: (v.line, v.col), text: String::new(), rule: "R9" });
                }
            }
            // `sku` is `sku: sku`: constructor arguments are named (9.2, X-345)
            let pun = pun_name(a);
            let ts: Vec<&Token> = match pun {
                Some(pt) => vec![pt, pt],
                None => ts,
            };
            if !(!ts.is_empty() && ts[0].kind == Kind::Lower && ts.len() > 1 && (ts[1].text == ":" || pun.is_some())) {
                self.typ("E-TYP-130", format!("constructor arguments of {} are named (field: value)", d.name), ftok(a), "field: value", "positional", &d.name, "LANG-09-02");
                self.infer(e, env, None);
                continue;
            }
            let f = ts[0].text.clone();
            let fdecl = match d.fields.get(&f) {
                Some(x) => x.clone(),
                None => {
                    let names = d.fields.keys().cloned().collect::<Vec<_>>().join(", ");
                    let why = if pun.is_some() { format!(" (`{f}` stands for `{f}: {f}`)") } else { String::new() };
                    self.typ("E-TYP-130", format!("{} has no field `{f}`{why}", d.name), ts[0], &names, &f, &f, "LANG-09-02");
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
            if infer_args { self.unify(&fdecl.0, &et, &mut mm); }
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
        self.activate(tok, d.lifetime.is_some(), env, &BTreeMap::new());
        match t {
            Type::Named(n) if !n.args.is_empty() => t.clone(),
            _ => named_of_decl(&d, d.type_params.iter().map(|p| self.widened(&mm.get(p).cloned().unwrap_or_else(|| Type::TypeParam(p.clone())))).collect()),
        }
    }

    /// Discover captures in an isolated typing activation, then elaborate the head for the ordinary checker
    /// and all lowerings. The written tree/tokens stay intact for formatting (LR 8.2, X-580).
    fn infer_lambda_captures(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) {
        let mut probe = Typer::new(self.dc.clone());
        probe.dc.records.clear();
        probe.aliases = self.aliases.clone();
        probe.summaries = self.summaries.clone();
        probe.self_type = self.self_type.clone();
        probe.self_mode = self.self_mode.clone();
        probe.tparams_in_scope = self.tparams_in_scope.clone();
        probe.where_in_scope = self.where_in_scope.clone();
        probe.linear_in_scope = self.linear_in_scope.clone();
        probe.capture_probe_node = Some(node.id());
        let outer = env_new(None);
        let mut frame = Some(env.clone());
        while let Some(f) = frame {
            for (n, b) in f.vars.borrow().iter() {
                if env_local(&outer, n).is_none() {
                    env_set(&outer, n, b.ty.clone(), true, "capture-probe");
                }
            }
            frame = f.parent.clone();
        }
        if let Some(t) = &self.self_type {
            env_set(&outer, "self", t.clone(), true, "capture-probe");
        }
        let fields = self.dc.sess.field_closures.borrow().clone();
        probe.lambda_type(node, &outer, expected);
        *self.dc.sess.field_closures.borrow_mut() = fields;
        // Method lookup reads a receiver's prefixes at the same source use. Its full place is the capture.
        let uses: Vec<_> = probe.capture_uses.iter().filter(|(p, _, t)| !probe.capture_uses.iter().any(|(q, _, u)|
            q.starts_with(&format!("{p}.")) && (t.line, t.col) == (u.line, u.col))).cloned().collect();
        let rank = |h: &str| match h { "move" => 2, "editable" | "write" => 1, _ => 0 };
        let mut chosen: Vec<(String, String, Token)> = vec![];
        for (p, h, t) in uses {
            if let Some((_, old, tok)) = chosen.iter_mut().find(|(q, _, _)| *q == p) {
                if rank(&h) > rank(old) { *old = h; *tok = t; }
            } else {
                chosen.push((p, h, t));
            }
        }
        if chosen.is_empty() { return; }
        let eligible = self.infer_capture_node == Some(node.id()) && !node.direct_tokens().iter().any(|t| t.text == "mut" || t.text == "once");
        if !eligible {
            for (p, _, t) in chosen {
                self.rec("E-FUN-011", format!("`{p}` is not in the lambda's capture list; only a directly loaned callback infers its captures (8.2)"), "LANG-08-03", &t,
                    Json::obj([("violation", Json::str("uncaptured name")), ("capture_span", Json::str(&p)), ("use_span", Json::str(&p))]));
            }
            return;
        }
        let mut entries = vec![];
        let mut kind = "fn";
        for (path, how, tok) in chosen {
            let mk = match how.as_str() {
                "move" => {
                    self.rec("E-FUN-010", format!("moving capture `{path}` out is not inferred: write its capture list and `fn once` (8.3)"), "LANG-08-02", &tok,
                        Json::obj([("violation", Json::str("inferred move capture")), ("capture_span", Json::str(&path)), ("use_span", Json::str(&path))]));
                    kind = "fn once";
                    "move"
                }
                "editable" | "write" => { if kind == "fn" { kind = "fn mut"; } "editable" }
                _ => {
                    let mut ct = env_get(&outer, &root_of(&path)).unwrap_or_else(|| unknown(""));
                    for part in path.split('.').skip(1) {
                        let mut mt = tok.clone(); mt.text = part.into();
                        ct = probe.member_type(&ct, &mt);
                    }
                    if !path.contains('.') && self.dc.is_trivial(strip(&ct)) { "" } else { "alias" }
                }
            };
            entries.push(if mk.is_empty() { path } else { format!("{mk} {path}") });
        }
        if kind == "fn once" { return; } // never elaborate a rejected inferred move, including later passes
        let text = format!("{kind}[{}]() => 0", entries.join(", "));
        if let Ok(mut head) = fortis_syntax::parse_from(&text, "lambda") {
            let tok = ftok(node);
            for t in &mut head.toks { t.line = tok.line; t.col = tok.col; }
            let _ = node.node.inferred_head.set(Box::new(head));
        }
    }

    pub fn lambda_type(&mut self, node: N<'a>, env: &Env, expected: Option<&Type>) -> Type {
        if node.child_node("captureList").is_none() && self.capture_probe_node != Some(node.id()) {
            self.infer_lambda_captures(node, env, expected);
        }
        if node.node.inferred_head.get().is_some() && self.infer_capture_node != Some(node.id()) && self.capture_probe_node != Some(node.id()) {
            self.rec("E-FUN-011", "this lambda must write its capture list: only a directly loaned callback infers captures (8.2)".into(), "LANG-08-03", ftok(node),
                Json::obj([("violation", Json::str("uncaptured name")), ("capture_span", Json::str("lambda")), ("use_span", Json::str("lambda"))]));
        }
        let e = env_new(Some(env));
        let exp_fn = expected.map(|x| strip(x).clone()).filter(|x| matches!(x, Type::Fn { .. }));
        let mut params = Vec::new();
        let mut param_modes = Vec::new();
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
                let expected_mode = match &exp_fn { Some(Type::Fn { param_modes, .. }) => param_modes.get(i).cloned().unwrap_or_default(), _ => String::new() };
                let mode = p.find("paramType").map(|pt| pt.direct_tokens().into_iter().find(|t| matches!(t.text.as_str(), "editable" | "owned")).map(|t| t.text.clone()).unwrap_or_default()).unwrap_or_else(|| expected_mode.clone());
                if exp_fn.is_some() && mode != expected_mode {
                    let shown = if mode.is_empty() { "read" } else { &mode };
                    let wanted = if expected_mode.is_empty() { "read" } else { &expected_mode };
                    self.typ("E-TYP-040", format!("lambda parameter `{}` has mode {shown}, expected {wanted} (8.4)", ts[0].text), ts[0], wanted, shown, "lambda parameter", "LANG-08-04");
                }
                env_set(&e, &ts[0].text, t.clone(), false, if mode.is_empty() { "param" } else { &mode });
                param_modes.push(mode);
                params.push(t);
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
                    self.use_place(Some(&ctok_.text), &ctok_, "copy", env);
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
                    if !self.res_stack.is_empty() { self.reserve(&path, &mk, &ltok); }
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
        // an expression body may leave out its return type (8.2, X-345): the callee's parameter type decides it when
        // that type names one, otherwise the body's type does, widened as an unannotated `let` widens its initialiser
        let inferred_rt = node.child_node("type").is_none();
        let mut rt = match node.child_node("type") {
            Some(t) => self.dc.type_of(Some(t)),
            None if node.child_node("lambdaBlock").is_some() => void(),
            None => match &exp_fn {
                Some(Type::Fn { ret, .. }) if known(strip(ret)) && !super::has_free(ret) => (**ret).clone(),
                _ => unknown("lambda result"),
            },
        };
        // a result declared `from` a capture lends what the capture holds: an `alias` or `editable` capture is a loan
        // the closure holds, but a `move` or `copy` capture (a bare one copies) is the closure's own value, destroyed with
        // the closure, so a loan of it would outlive its owner (7.6, 7.9; D-379)
        {
            let mut origins: Vec<String> = Vec::new();
            match &rt {
                Type::Loan { origin, .. } => origins.push(origin.clone()),
                _ => {
                    let mut nested = Vec::new();
                    Self::nested_origins(&rt, true, &mut nested);
                    origins.extend(nested.into_iter().map(|(o, _)| o));
                }
            }
            if let Some(rt_node) = node.child_node("type") {
                for (cpath, cmk) in &captures {
                    let croot = root_of(cpath);
                    if origins.contains(&croot) && (cmk == "move" || cmk == "copy") {
                        let otok = rt_node.tokens().into_iter().find(|tk| tk.text == croot).unwrap_or(rt_node.tokens()[0]).clone();
                        self.own_err(
                            "E-OWN-051",
                            format!("the result is declared `from {croot}`, a `{cmk}` capture: the closure owns its capture and destroys it when the closure ends, so a loan of it cannot be returned; return the owned value, or capture `alias {croot}` (7.6, 8.3)"),
                            &otok,
                            &croot,
                            "loan of a capture the closure owns",
                            "LANG-07-06",
                            "",
                        );
                    }
                }
            }
        }
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
                self.dc.check_keys(t, &ltok, None, false);
                for (code, rule, shown, why) in self.dc.dynamic_problems(t) {
                    self.typ(code, format!("a parameter or the result of this lambda: {why}"), &ltok, "a dynamic-eligible interface whose bounds hold", &shown, "lambda", rule);
                }
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
                        let name = core_name(Self::unshared(&t)).unwrap_or("").to_string();
                        self.eff_err("E-EFF-001", format!("lambda is declared `pure` but takes the capability {name}: a pure function receives no authority (11.5)"), &ltok, "pure", "io", "lambda", "LANG-11-06");
                    }
                }
            }
        }
        let body = lambda_body(node);
        if let Some(body) = body {
            let saved_ret = self.ret_type.replace(rt.clone());
            let saved_try = std::mem::replace(&mut self.in_try, modifiers.iter().any(|m| m == "try"));
            let saved_ctx = std::mem::replace(&mut self.ctx, vec!["lambda".into()]);
            let saved_labels = std::mem::take(&mut self.loop_labels);
            let saved_facts = std::mem::take(&mut self.facts);
            let saved_optional = std::mem::take(&mut self.optional_arguments);
            let saved_scopes = std::mem::take(&mut self.optional_scopes);
            let saved_own = std::mem::take(&mut self.own);
            let saved_depth = std::mem::replace(&mut self.depth, 0);
            let saved_arg = self.arg_mode.take();
            let saved_pending = self.pending.take();
            let saved_carried = std::mem::take(&mut self.carried);
            let saved_temps = std::mem::take(&mut self.carried_temps);
            let saved_res = std::mem::take(&mut self.res_stack);
            let saved_parts = std::mem::take(&mut self.part_owners); // the body's bindings are its own (D-369)
            self.lambda_kind.push(kind.clone());
            let lt = node.direct_tokens();
            let fn_tok = lt.iter().find(|t| t.text == "fn").map(|t| (*t).clone()).unwrap_or_else(|| ltok.clone());
            let mut_tok = lt.iter().find(|t| t.text == "mut").map(|t| (*t).clone());
            self.lambda_toks.push((fn_tok, mut_tok));
            // the lambda body is a separate activation: its exits and defers are its own, and the places it writes are its
            // own (its invalidations are dropped from the log when it ends, D-74)
            let saved_exits = std::mem::take(&mut self.exit_frames);
            let saved_breaks = std::mem::take(&mut self.break_frames);
            let saved_linear_loops = std::mem::take(&mut self.linear_loops);
            let saved_defers = std::mem::take(&mut self.defer_bodies);
            let saved_rest = std::mem::take(&mut self.block_rest);
            let log_mark = self.inval_log.len();
            let outer_eff = std::mem::take(&mut self.eff);
            for (pt, pm) in params.iter().zip(&param_modes) {
                if pm == "owned" { self.charge_drop(pt, ftok(node)); }
            }
            let outer_test = std::mem::replace(&mut self.test_body, false); // a lambda's body is its own function, not the test's (14)
            if let Some(lp) = node.child_node("lambdaParams") {
                for (i, p) in lp.find_all("lambdaParam").iter().enumerate() {
                    let pn = &p.direct_tokens()[0].text;
                    if param_modes[i] == "owned" && self.is_linear(&params[i]) {
                        self.own.open_obligation(pn, ftok(node).clone(), 0);
                    } else if param_modes[i] != "owned" && self.is_bearing(&params[i]) {
                        let mode = if param_modes[i] == "editable" { "editable" } else { "alias" };
                        let loan = self.own.add_loan(pn, mode, Some(pn), 0, ftok(node).line);
                        self.own.holders.insert(pn, loan);
                    }
                }
            }
            let body_t = self.body(Some(body), &e, &rt, &ltok, "lambda");
            self.test_body = outer_test;
            if inferred_rt && !known(strip(&rt)) {
                if let Some(bt) = body_t {
                    rt = self.widened(&bt);
                }
            }
            self.exit_frames = saved_exits;
            self.break_frames = saved_breaks;
            self.linear_loops = saved_linear_loops;
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
            self.optional_arguments = saved_optional;
            self.optional_scopes = saved_scopes;
            self.depth = saved_depth;
            self.arg_mode = saved_arg;
            self.pending = saved_pending;
            self.carried = saved_carried;
            self.carried_temps = saved_temps;
            self.res_stack = saved_res;
            self.part_owners = saved_parts;
        }
        let mut capture_origins: Vec<(String, String)> = cap_loans.iter().map(|l| {
            let l = l.borrow(); (l.owner.clone(), l.mode.clone())
        }).collect();
        for (path, mode) in &captures {
            if mode == "move" {
                if let Some(b) = env_lookup(env, &root_of(path)) {
                    Self::nested_origins(&b.ty, true, &mut capture_origins);
                    Self::captured_loans(&b.ty, &mut capture_origins);
                }
            }
        }
        capture_origins.sort();
        capture_origins.dedup();
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
        Type::Fn { capture_origins, params, ret: Box::new(rt), kind, modifiers, origin: String::new(), dynamic: false, identity, param_modes }
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

impl<'a> Typer<'a> {
    /// The operators of a checked region that a test body's arithmetic starts at `n` (14, 6.4; X-345): integer
    /// arithmetic not already inside a region (the left operand of `or` or `catch` has its own), outside lambdas.
    pub fn test_region(&self, n: N<'a>) -> BTreeSet<(usize, usize)> {
        if !self.test_body {
            return BTreeSet::new();
        }
        let kids = n.kids();
        let root = (matches!(n.name(), "additiveExpr" | "multiplicativeExpr" | "shiftExpr") && kids.len() > 1)
            || (n.name() == "unaryExpr" && kids.len() == 2 && kids[0].tok().map_or(false, |t| t.text == "-"));
        if !root {
            return BTreeSet::new();
        }
        let ops = region_operators(C::N(n));
        if ops.is_empty() || self.regions.last().map_or(false, |rg| ops.iter().any(|o| rg.0.contains(o))) {
            return BTreeSet::new();
        }
        ops
    }

    /// The single-child chain under a consuming operand, through parentheses: a call there keeps its outcome in a
    /// test body (14).
    pub fn mark_consumed(&mut self, node: Option<N<'a>>) {
        let mut n = node;
        while let Some(x) = n {
            self.consumed_ops.insert(x.id());
            let kids = x.kids();
            if x.name() == "primaryExpr" && kids.len() == 3 && kids[0].text_is("(") {
                n = kids[1].node(); // through parentheses: `(f()) or fail`
                continue;
            }
            n = if kids.len() == 1 { kids[0].node() } else { None };
        }
    }

    /// In a `test` or `property` body, a call of a fallible function that nothing consumes is that call followed by
    /// `or fail` (14, LANG-14-03; X-345): it has its success type, and a failure fails the test.
    pub fn implied_or_fail(&mut self, node: N<'a>, t: Type, env: &Env) -> Type {
        match fortis_names::reading::call_name_token(node) {
            Some(tok) => {
                let path = self.path_of_expr(Some(node));
                self.unwrap_call(tok, t, env, path)
            }
            None => t,
        }
    }

    /// The implied `or fail` of a test body's call named at `tok` (14): its success type, when the call fails.
    pub fn unwrap_call(&mut self, tok: &Token, t: Type, env: &Env, path: Option<String>) -> Type {
        let inner = match strip(&t) {
            Type::Named(n) if self.test_body && (n.package.is_empty() && n.name == "Outcome") && !n.args.is_empty() => n.args[0].clone(),
            _ => return t,
        };
        if let Some((is_try, _)) = self.call_sigs.get(&(tok.line, tok.col)) {
            if !is_try {
                return t; // a function that returns an Outcome it built is not a failing call
            }
        }
        self.consume(path, tok.line, "handled");
        self.eff.add("fail", "proved", Some(tok));
        self.unwind_point(tok, "or fail", env, None);
        if self.defer_depth > 0 {
            self.def_err("E-DEF-001", "a failing call inside a deferred block: there is no outcome to propagate to (12.1)".into(), tok, "or fail", "LANG-12-01");
        }
        self.implied_fails.insert((tok.line, tok.col));
        inner
    }

    /// `log.info(m)`, `log.warn(m)`, `log.error(m)` (13, LANG-13-04; X-345): one `StringView` message, the `io` effect
    /// like `report` and no authority. `log` is the statement only where it names nothing in scope.
    pub fn log_statement(&mut self, inner: N<'a>, env: &Env) -> bool {
        let Some((_, level, al)) = fortis_names::reading::log_call(inner) else { return false };
        if env_lookup(env, "log").is_some() || self.dc.table.get("log").is_some() {
            return false;
        }
        if !fortis_names::reading::LOG_LEVELS.contains(&level.text.as_str()) {
            return true; // the names layer reports the level (E-NAM-001)
        }
        self.eff.add("io", "proved", Some(level)); // the diagnostic plane, as `report` (10.6, 11.6)
        let args = Self::arg_nodes(al);
        let named = args.first().map_or(false, |a| a.direct_tokens().iter().any(|t| t.text == ":" || t.text == "..."));
        if args.len() != 1 || named {
            self.typ("E-TYP-040", format!("`log.{}` takes one message", level.text), level, "one StringView argument", &format!("{} arguments", args.len()), &format!("log.{}", level.text), "LANG-06-01");
            for a in args {
                self.infer(Self::arg_expr(a), env, None);
            }
            return true;
        }
        let ae = Self::arg_expr(args[0]);
        let t = self.infer(ae, env, Some(&string_view()));
        if let Some(ae) = ae {
            self.expect(&t, &string_view(), ftok(ae), &format!("the message of `log.{}`", level.text));
        }
        true
    }

    /// The statement is a direct call of a function declared `@ignorable` (5.5, the prelude's aspect): the callee's
    /// declaring module resolved the name to the core aspect (`Sig::ignorable`; D-314), whatever it names here.
    pub fn ignorable_call(&self, e: N<'a>) -> bool {
        let Some(tok) = fortis_names::reading::call_name_token(e) else { return false };
        self.call_sigs.get(&(tok.line, tok.col)).map_or(false, |c| c.1)
    }

    /// `@ignorable` on a function whose result is linear is rejected (5.5, 11.8): a linear value is never dropped.
    pub fn check_ignorable(&mut self, sig: &Sig<'a>, label: &str) {
        let Some(tok) = sig.ignorable else { return };
        let mods = self.expanded_mods(sig);
        let what = if mods.contains("try") {
            "a `try` function's result is an Outcome".to_string()
        } else if mods.contains("async") {
            "an `async` function's result is a Future".to_string()
        } else if self.is_linear(&sig.ret) {
            format!("{} is linear", strip(&sig.ret).show())
        } else {
            return;
        };
        let actual = strip(&sig.ret).show();
        self.typ("E-TYP-150", format!("@ignorable on {label}: {what}, and a linear result can never be dropped (5.5)"), tok, "a non-linear result", &actual, "ignorable", "LANG-11-09");
    }
}
