//! Parse-tree shapes of the v0.0.2 reading release (register X-345) that name resolution, the checker and the
//! backends must all recognise the same way: a port of conformance/tools/reading.py.
//!
//! - the entry `main`'s parameters, supplied by type (Language Reference 4.6): [`param_findings`], [`entry_param_kinds`];
//! - the log statements `log.info(m)`, `log.warn(m)`, `log.error(m)` (Language Reference 13): [`log_call`];
//! - the call an expression makes, for `@ignorable` statements (5.5) and the implied `or fail` of a test body (14):
//!   [`call_name_token`], [`postfix_call_token`].

use fortis_diag::Json;
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{C, N};

/// The capability types of Language Reference 11.6, defined by the core and imported from fortis.host.
pub const CAPABILITY_TYPES: &[&str] = &["FileSystem", "Network", "Clock", "Random", "Environment", "Process", "Supervisor", "Console", "SecretAccess", "RawSql"];
/// The parameter type that receives the command-line arguments.
pub const ARGS: &str = "List[String]";
/// The levels of a log statement (13).
pub const LOG_LEVELS: &[&str] = &["info", "warn", "error"];
/// The entry forms, as a finding's candidate.
pub const ENTRY_FORM: &str = "[export] [async] [try] [recovers] func main(...) Void, each parameter List[String] or a capability type";

/// The field of `Capabilities` that holds the capability `ty` (fortis.host).
pub fn capability_field(ty: &str) -> &'static str {
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

fn nf(name: &str, candidates: Vec<String>) -> Json {
    Json::obj([("name", Json::str(name)), ("candidates", Json::strs(candidates))])
}

/// (code, message, token, fields) for the parameters of the entry `main` whose `funcSignature` is `sig` (4.6).
///
/// Each parameter is `name: T` with no mode, default or `...`, where T is the prelude's `List[String]` or a capability
/// type of fortis.host, each type at most once: `E-NAM-005` for any other parameter, `E-NAM-006` for a type already
/// taken. `origin_of(n)` says where the module's name `n` comes from: "prelude", "module", an import path such as
/// "fortis.host", "alias:<path>" for a module alias, or None when nothing binds it (the unresolved name is reported
/// by name resolution, not here). A `List[String]` whose `List` or `String` is not the prelude's keeps v0.0.1's
/// finding, `E-NAM-001` (D-51.2).
pub fn param_findings<'a>(sig: N<'a>, origin_of: &dyn Fn(&str) -> Option<String>) -> Vec<(&'static str, String, &'a Token, Json)> {
    let mut out = Vec::new();
    let Some(pl) = sig.find("paramList") else { return out };
    let mut seen: Vec<(String, String)> = Vec::new();
    let candidates: Vec<String> = std::iter::once(ARGS.to_string()).chain(CAPABILITY_TYPES.iter().map(|s| s.to_string())).collect();
    for p in pl.child_nodes("param") {
        let ts = p.direct_tokens();
        let Some(tok) = ts.first().copied().or_else(|| p.first_token()) else { continue };
        let pname = tok.text.clone();
        let pt = p.find("paramType");
        let mode = pt.map_or(false, |pt| pt.direct_tokens().iter().any(|t| t.text == "editable" || t.text == "owned"));
        if p.find("selfParam").is_some() || p.find("restParam").is_some() || ts.iter().any(|t| t.text == "...") || p.find("constExpr").is_some() || mode || pt.is_none() {
            out.push(("E-NAM-005", format!("main's parameter `{pname}` must be written `name: T`, with no mode, default or `...`: the runtime supplies it by its type (4.6)"), tok, nf(&pname, candidates.clone())));
            continue;
        }
        let Some(ty) = pt.and_then(|pt| pt.find("type")) else { continue };
        let tts = ty.tokens();
        let words: Vec<&str> = tts.iter().map(|t| t.text.as_str()).collect();
        let text: String = words.concat();
        let mut kind: Option<String> = None;
        if words == ["List", "[", "String", "]"] {
            let shadowed = ["List", "String"].into_iter().find(|n| !matches!(origin_of(n).as_deref(), None | Some("prelude")));
            if let Some(s) = shadowed {
                out.push(("E-NAM-001", format!("main's `{s}` is not the prelude {s}, so the signature is not one of the forms of 4.6"), tok, nf("main", vec![ENTRY_FORM.into()])));
                continue;
            }
            kind = Some(ARGS.into());
        } else if tts.len() == 1 && tts[0].kind == Kind::Upper {
            match origin_of(&tts[0].text) {
                None => continue,
                Some(o) if o == "fortis.host" && CAPABILITY_TYPES.contains(&tts[0].text.as_str()) => kind = Some(tts[0].text.clone()),
                _ => {}
            }
        } else if tts.len() == 3 && tts[0].kind == Kind::Lower && words[1] == "." && tts[2].kind == Kind::Upper {
            match origin_of(&tts[0].text) {
                None => continue,
                Some(o) if o == "alias:fortis.host" && CAPABILITY_TYPES.contains(&tts[2].text.as_str()) => kind = Some(tts[2].text.clone()),
                _ => {}
            }
        }
        let Some(kind) = kind else {
            out.push((
                "E-NAM-005",
                format!("main's parameter `{pname}` has type {text}, which the runtime cannot supply: a parameter of the entry `main` is List[String] or a capability of fortis.host ({}) (4.6)", CAPABILITY_TYPES.join(", ")),
                tok,
                nf(&pname, candidates.clone()),
            ));
            continue;
        };
        if let Some((_, first)) = seen.iter().find(|(k, _)| *k == kind) {
            out.push(("E-NAM-006", format!("main takes two parameters of type {kind}, `{first}` and `{pname}`: the runtime supplies each type once (4.6)"), tok, nf(&pname, vec![first.clone()])));
            continue;
        }
        seen.push((kind, pname));
    }
    out
}

/// (parameter name, [`ARGS`] or a capability type) of an entry `main` that name resolution accepted, in order.
pub fn entry_param_kinds(sig: N) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(pl) = sig.find("paramList") else { return out };
    for p in pl.child_nodes("param") {
        let ts = p.direct_tokens();
        let Some(first) = ts.first() else { continue };
        let words: Vec<String> = p.find("paramType").and_then(|pt| pt.find("type")).map(|t| t.tokens().iter().map(|t| t.text.clone()).collect()).unwrap_or_default();
        let kind = if words.first().map(|w| w.as_str()) == Some("List") { ARGS.to_string() } else { words.last().cloned().unwrap_or_default() };
        out.push((first.text.clone(), kind));
    }
    out
}

/// Descend through rules with exactly one child until `rule`; None if any level has more than one child.
fn single<'a>(node: N<'a>, rule: &str) -> Option<N<'a>> {
    let mut n = node;
    while n.name() != rule {
        if n.nkids() != 1 {
            return None;
        }
        n = n.kid(0)?.node()?;
    }
    Some(n)
}

/// (the `log` token, the level token, the argList node) when `expr` (an expr, exprStmt or statement node) has the
/// shape of a log statement, `log.LEVEL(args)`; whether `log` names a binding is the caller's to decide (13). LEVEL is
/// returned whatever it is, so the caller can reject another level.
pub fn log_call<'a>(expr: N<'a>) -> Option<(&'a Token, &'a Token, Option<N<'a>>)> {
    let mut e = expr;
    if e.name() == "statement" {
        if e.nkids() != 1 {
            return None;
        }
        e = e.kid(0)?.node()?;
        if e.name() != "exprStmt" {
            return None;
        }
    }
    let pf = single(e, "postfixExpr")?;
    let kids = pf.kids();
    if kids.len() != 2 || !kids[0].is("primaryExpr") || !kids[1].is("postfix") {
        return None;
    }
    let prim = kids[0].node()?;
    let pk = prim.kids();
    let log = match pk.as_slice() {
        [C::T(t)] if t.kind == Kind::Lower && t.text == "log" => *t,
        _ => return None,
    };
    let post = kids[1].node()?;
    let mc = match post.kids().as_slice() {
        [C::N(m)] if m.name() == "methodCall" => *m,
        _ => return None,
    };
    let level = mc.find("memberName")?.tokens().first().copied()?;
    if mc.direct_tokens().first().map(|t| t.text.as_str()) != Some(".") {
        return None;
    }
    Some((log, level, mc.find("argList")))
}

/// The name token of the call an expression makes, when the whole expression is one call (an `@ignorable` statement,
/// 5.5): `f(a)`, `x.m(a)`, `Type.m(a)`, `a.b().m()` and the marked `editable x.m(a)`; None otherwise.
pub fn call_name_token<'a>(expr: N<'a>) -> Option<&'a Token> {
    let mut n = expr;
    while n.name() != "markerExpr" && n.name() != "postfixExpr" {
        if n.nkids() != 1 {
            return None;
        }
        n = n.kid(0)?.node()?;
    }
    let kids = n.kids();
    if n.name() == "markerExpr" {
        let posts: Vec<N> = kids.iter().skip(2).filter_map(|k| k.node()).filter(|k| k.name() == "postfix").collect();
        if !posts.is_empty() {
            return postfix_call_name(&posts, None);
        }
        match kids.get(2) {
            Some(C::N(m)) if m.name() == "methodCall" => return m.find("memberName").and_then(|mn| mn.tokens().first().copied()),
            Some(C::T(t)) if t.text == "(" => {
                let pl = n.find("place")?;
                let members = pl.child_nodes("memberName");
                return match members.last() {
                    Some(m) => m.tokens().first().copied(),
                    None => pl.direct_tokens().first().copied(),
                };
            }
            _ => return None,
        }
    }
    let prim = kids.first()?.node()?;
    let posts: Vec<N> = kids.iter().skip(1).filter_map(|k| k.node()).collect();
    if posts.is_empty() {
        return primary_call_token(prim);
    }
    postfix_call_name(&posts, Some(prim))
}

/// The name token of a primary that is itself a call, `Type.m(a)` or `Type.Variant(a)`, or None.
pub fn primary_call_token<'a>(prim: N<'a>) -> Option<&'a Token> {
    let pk = prim.kids();
    if matches!(pk.first(), Some(C::N(t)) if t.name() == "typeRef") && pk.iter().any(|c| c.text_is("(")) {
        if let Some(mn) = prim.child_node("memberName") {
            return mn.tokens().first().copied();
        }
        return pk.iter().filter_map(|c| c.tok()).filter(|t| t.kind == Kind::Upper).last();
    }
    None
}

fn postfix_call_name<'a>(posts: &[N<'a>], prim: Option<N<'a>>) -> Option<&'a Token> {
    let last = posts.last()?.kids();
    match last.first() {
        Some(C::N(m)) if m.name() == "methodCall" => m.find("memberName").and_then(|mn| mn.tokens().first().copied()),
        Some(C::T(t)) if t.text == "(" => {
            if posts.len() >= 2 {
                let prev = posts[posts.len() - 2].kids();
                if prev.len() == 2 && prev[0].text_is(".") {
                    return prev[1].tokens().first().copied();
                }
                return None;
            }
            let pk = prim?.kids();
            match pk.as_slice() {
                [C::T(t)] if t.kind == Kind::Lower => Some(*t),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The name token of the call the postfix `posts[i]` makes, or None when that postfix is not a call: a call that is
/// the receiver of the next postfix (14).
pub fn postfix_call_token<'a>(posts: &[N<'a>], i: usize, prim: Option<N<'a>>) -> Option<&'a Token> {
    if i >= posts.len() {
        return None;
    }
    postfix_call_name(&posts[..=i], prim)
}

/// The name an operand is when it is just a binding (`o`, `(o)`, `move o`), or None.
fn bare_name(node: N) -> Option<String> {
    let mut n = node;
    loop {
        let k = n.kids();
        match n.name() {
            "markerExpr" => {
                if k.len() == 2 && k[0].tokens().first().map(|t| t.text.as_str()) == Some("move") {
                    let pts = k[1].tokens();
                    return if pts.len() == 1 && pts[0].kind == Kind::Lower { Some(pts[0].text.clone()) } else { None };
                }
                return None;
            }
            "place" => {
                let pts = n.tokens();
                return if pts.len() == 1 && pts[0].kind == Kind::Lower { Some(pts[0].text.clone()) } else { None };
            }
            "primaryExpr" => {
                if let [C::T(t)] = k.as_slice() {
                    return if t.kind == Kind::Lower { Some(t.text.clone()) } else { None };
                }
                if k.len() == 3 && k[0].text_is("(") {
                    n = k[1].node()?;
                    continue;
                }
                return None;
            }
            _ => {}
        }
        if k.len() != 1 {
            return None;
        }
        n = k[0].node()?;
    }
}

/// The initialisers of the `let` bindings of a test body that keep their outcome (14, X-345): a binding the body later
/// consumes as an outcome (the operand of `or` or `catch`, or a `match` scrutinee), or one annotated `Outcome[...]`,
/// binds the call's outcome, as in v0.0.1; every other binding of a fallible call binds its success value. Lambdas
/// are their own functions and are not searched.
pub fn outcome_lets<'a>(block: N<'a>) -> Vec<N<'a>> {
    fn walk<'a>(n: N<'a>, consumed: &mut Vec<String>, lets: &mut Vec<(String, N<'a>, Option<N<'a>>)>) {
        if n.name() == "lambda" {
            return;
        }
        let k = n.kids();
        match n.name() {
            "outcomeExpr" if k.len() > 1 => {
                if let Some(b) = k[0].node().and_then(bare_name) {
                    consumed.push(b);
                }
            }
            "matchExpr" => {
                if let Some(b) = k.iter().filter_map(|c| c.node()).find(|c| c.name() == "expr" || c.name() == "place").and_then(bare_name) {
                    consumed.push(b);
                }
            }
            "letStmt" if !k.iter().any(|c| c.is("pattern")) => {
                let nm = n.direct_tokens().into_iter().find(|t| t.kind == Kind::Lower);
                let init = n.child_node("expr");
                let annot = n.child_node("type");
                if let (Some(nm), Some(init)) = (nm, init) {
                    lets.push((nm.text.clone(), init, annot));
                }
            }
            _ => {}
        }
        for c in k {
            if let C::N(m) = c {
                walk(m, consumed, lets);
            }
        }
    }
    let (mut consumed, mut lets) = (Vec::new(), Vec::new());
    walk(block, &mut consumed, &mut lets);
    lets.into_iter()
        .filter(|(nm, _, annot)| consumed.contains(nm) || annot.and_then(|a| a.first_token()).map_or(false, |t| t.text == "Outcome"))
        .map(|(_, init, _)| init)
        .collect()
}
